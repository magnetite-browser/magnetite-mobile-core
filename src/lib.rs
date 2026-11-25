//! What Magnetite's phone apps share with the desktop browser: joining a
//! sync chain and reading the sidebar it holds.
//!
//! A phone only reads the sidebar. The desktop replaces that collection as a
//! whole, without merging, so a phone that wrote it would overwrite what a
//! desktop changed since the phone's last sync. What a phone writes is its
//! own entry in the `devices` collection (`devices` module): the tabs open
//! on it, which another device can take over, and its changes to the
//! sidebar, which a desktop makes (`edits` module). The phone's history
//! (`History`) stays on the phone.
//!
//! The apps call the functions below through UniFFI; every one of them
//! blocks, so call them off the main thread. The sync code's secret is
//! handed back to the app, which keeps it in the system's key store: this
//! crate writes only the sidebar it last read.

mod chain;
mod devices;
/// Public for Rust callers that play a desktop (the demo server); the apps
/// use the functions below.
pub mod edits;
mod history;
mod view;

use std::path::Path;

pub use devices::{Device, DeviceTab};
pub use history::{History, Visit};
pub use view::{Accent, Page, Row, RowItem, Sidebar, Space};

uniffi::setup_scaffolding!();

/// Why a call failed. The apps get the kind and the message below; the text
/// is in our words, never the server's: a server cannot use the app to ask
/// for the sync code.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum SyncError {
    /// The words are not a sync code.
    #[error("these are not the 24 words of a sync code")]
    NotACode,
    /// The address cannot be a sync server's.
    #[error("{reason}")]
    Address { reason: String },
    /// The server has no chain for this code.
    #[error("no sync chain with this code on this server")]
    NoChain,
    /// The chain is there but holds no sidebar yet.
    #[error("this sync chain holds no data yet; sync the device that started it first")]
    Empty,
    /// The server cannot be reached or refused.
    #[error("{reason}")]
    Server { reason: String },
    /// The chain's data cannot be read by this version.
    #[error("{reason}")]
    Unreadable { reason: String },
    /// The sidebar could not be kept on this device.
    #[error("{reason}")]
    Storage { reason: String },
}

/// Magnetite's own sync server, preselected in the apps.
#[uniffi::export]
pub fn default_server() -> String {
    "https://sync.magnetitebrowser.org".to_string()
}

/// Check that `server` has a chain for the 24 `words`. Returns the secret to
/// keep in the system's key store and to pass to `refresh`.
#[uniffi::export]
pub fn join(server: String, words: String) -> Result<String, SyncError> {
    chain::join(&server, &words)
}

/// Ask `server` for the chain's sidebar. Returns it when it is newer than the
/// one kept in `dir`, and keeps it there; `None` when nothing changed.
#[uniffi::export]
pub fn refresh(server: String, secret: String, dir: String) -> Result<Option<Sidebar>, SyncError> {
    chain::refresh(&server, &secret, Path::new(&dir))
}

/// The sidebar last read, for a start without a connection, with this
/// phone's changes on top.
#[uniffi::export]
pub fn cached(dir: String) -> Option<Sidebar> {
    chain::cached(Path::new(&dir))
}

/// Forget the sidebar kept in `dir` and the changes not made yet (the
/// device leaves the chain).
#[uniffi::export]
pub fn forget(dir: String) {
    chain::forget(Path::new(&dir));
}

/// Tell the chain which `tabs` are open on this `device`, and the changes to
/// the sidebar kept in `dir` that no desktop made yet; learn which tabs
/// another device took over since: their ids are returned, and the app
/// closes those tabs. Writes only when the tabs or changes did.
#[uniffi::export]
pub fn exchange(
    server: String,
    secret: String,
    device: Device,
    tabs: Vec<DeviceTab>,
    dir: String,
) -> Result<Vec<String>, SyncError> {
    devices::exchange(&server, &secret, &device, tabs, Path::new(&dir))
}

// Changes to the sidebar. A desktop of the chain makes them (`edits`); until
// then the sidebar kept in `dir` shows them, marked `pending`. Each returns
// that sidebar; `None` when none is kept (the device is in no chain). Run
// `exchange` after, so the chain hears of the change.

/// Pin `url` in `space`, last in its pinned tree.
#[uniffi::export]
pub fn pin(
    dir: String,
    space: String,
    url: String,
    title: String,
) -> Result<Option<Sidebar>, SyncError> {
    let (id, url) = (edits::new_id(), web_address(&url)?);
    change(
        &dir,
        edits::Change::Pin {
            space,
            item: id,
            url,
            title,
        },
    )
}

/// Make `url` a favorite of `space`'s profile, after the others.
#[uniffi::export]
pub fn add_favorite(
    dir: String,
    space: String,
    url: String,
    title: String,
) -> Result<Option<Sidebar>, SyncError> {
    let (id, url) = (edits::new_id(), web_address(&url)?);
    change(
        &dir,
        edits::Change::Favorite {
            space,
            item: id,
            url,
            title,
        },
    )
}

/// A new space named `title`, after the others; `icon`: one of the
/// desktop's glyph names, or none for its default.
#[uniffi::export]
pub fn new_space(
    dir: String,
    title: String,
    icon: Option<String>,
) -> Result<Option<Sidebar>, SyncError> {
    let id = edits::new_id();
    change(
        &dir,
        edits::Change::NewSpace {
            item: id,
            title,
            icon,
        },
    )
}

/// Name the space, folder, pinned tab or favorite `item` `title`.
#[uniffi::export]
pub fn rename(dir: String, item: String, title: String) -> Result<Option<Sidebar>, SyncError> {
    change(&dir, edits::Change::Rename { item, title })
}

/// Unpin pinned tab `item`, or remove favorite `item`. The desktop keeps a
/// pinned tab in its archive.
#[uniffi::export]
pub fn remove(dir: String, item: String) -> Result<Option<Sidebar>, SyncError> {
    change(&dir, edits::Change::Remove { item })
}

fn change(dir: &str, change: edits::Change) -> Result<Option<Sidebar>, SyncError> {
    let dir = Path::new(dir);
    edits::propose(dir, change)?;
    Ok(chain::cached(dir))
}

/// Only web pages go into the sidebar from a phone.
fn web_address(url: &str) -> Result<String, SyncError> {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(url.to_string())
    } else {
        Err(SyncError::Address {
            reason: format!("only web pages can be added: {url}"),
        })
    }
}
