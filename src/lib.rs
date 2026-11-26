//! What Magnetite's phone apps share with the desktop browser: joining a
//! sync chain, reading the sidebar and the archive it holds, and changing
//! them.
//!
//! A phone's changes (`edits` module) are few and each about one item. It
//! writes them itself (`chain::write_changes`): into the sidebar and archive
//! the chain holds at that moment, as their next versions. Desktops merge
//! what arrives with their own changes (magnetite, `docs/spec/sync.md`), and
//! the model keeps the fields this version does not know, so the phone
//! writes back what a newer desktop put there. While a write does not go
//! through, a desktop is asked to make the changes, as before phones wrote:
//! they are listed in this phone's entry of the `devices` collection
//! (`devices` module), with the tabs open on it, which another device can
//! take over. The phone's history (`History`) stays on the phone, and so do
//! its filter lists for content blocking (`blocking` module).
//!
//! The apps call the functions below through UniFFI; every one of them
//! blocks, so call them off the main thread. The sync code's secret is
//! handed back to the app, which keeps it in the system's key store: this
//! crate keeps the sidebar and archive it last read, and the changes not
//! written yet.

mod blocking;
mod chain;
mod devices;
/// Public for Rust callers that play a desktop (the demo server); the apps
/// use the functions below.
pub mod edits;
mod files;
mod history;
mod view;

use std::path::Path;

pub use blocking::{FilterList, ListsUpdate};
pub use devices::{Device, DeviceTab};
pub use history::{History, Visit};
pub use view::{Accent, Archived, Page, Row, RowItem, Sidebar, Space};

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

/// Write the changes kept in `dir` into the chain's sidebar and archive, and
/// tell the chain which `tabs` are open on this `device` (and the changes
/// that could not be written, for a desktop to make); learn which tabs
/// another device took over since: their ids are returned, and the app
/// closes those tabs. Writes only what changed.
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

// Changes to the sidebar and the archive. `exchange` writes them into the
// chain (`edits`); until then the sidebar kept in `dir` shows them, marked
// `pending`. Each returns that sidebar; `None` when none is kept (the device
// is in no chain). Run `exchange` after, so the chain hears of the change.

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

/// Unpin pinned tab `item`, or remove favorite `item`. It goes to the
/// archive.
#[uniffi::export]
pub fn remove(dir: String, item: String) -> Result<Option<Sidebar>, SyncError> {
    change(&dir, edits::Change::Remove { item })
}

/// Ask `server` for the chain's archive and keep it in `dir`. Returns it
/// with this phone's changes on top, newest first.
#[uniffi::export]
pub fn refresh_archive(
    server: String,
    secret: String,
    dir: String,
) -> Result<Vec<Archived>, SyncError> {
    chain::refresh_archive(&server, &secret, Path::new(&dir))
}

/// The archive last read, for a start without a connection, with this
/// phone's changes on top.
#[uniffi::export]
pub fn cached_archive(dir: String) -> Vec<Archived> {
    chain::cached_archive(Path::new(&dir))
}

/// Bring archived tab `item` back as a Today tab of the space it was closed
/// in (the first space if that one is gone). Returns the sidebar with it.
#[uniffi::export]
pub fn restore(dir: String, item: String) -> Result<Option<Sidebar>, SyncError> {
    change(&dir, edits::Change::Restore { item })
}

/// Remove archived tab `item` from the archive for good.
#[uniffi::export]
pub fn forget_archived(dir: String, item: String) -> Result<Vec<Archived>, SyncError> {
    let dir = Path::new(&dir);
    edits::propose(dir, edits::Change::Forget { item })?;
    Ok(chain::cached_archive(dir))
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
