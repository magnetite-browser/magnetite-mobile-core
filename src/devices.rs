//! The chain's `devices` collection: per device the tabs open on it, the
//! tabs of other devices it took over, and a phone's edits to the sidebar
//! (`edits`) with the ids of those a desktop applied.
//!
//! Every device changes only its own entry and writes the collection as the
//! version after the one it read; when another device wrote first, it reads
//! again and retries. So devices cannot overwrite each other, unlike with
//! the sidebar.
//!
//! Taking a tab over: device A lists a tab; device B opens it and puts the
//! tab's id into its own `taken`; A sees its tab there, closes it and stops
//! listing it; B then drops the id. A tab with `later` asks for that: the
//! device that sees it takes it by itself ("send to desktop").

use std::path::Path;

use magnetite_sync_proto::{CollectionResponse, PutCollectionRequest, SyncCode};
use serde::{Deserialize, Serialize};

use crate::SyncError;
use crate::chain::{now_ms, request};
use crate::edits;

const DEVICES: &str = "devices";
/// An entry not written for this long is of a device that is gone.
const GONE_AFTER_SECONDS: i64 = 90 * 24 * 60 * 60;
/// How often a write is tried again when another device wrote first.
const TRIES: usize = 4;

/// This device, as the others see it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct Device {
    /// Made up once per install, never the hardware's.
    pub id: String,
    /// What the user reads: "iPhone", "Pixel 9".
    pub name: String,
}

/// A tab open on a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct DeviceTab {
    pub id: String,
    pub url: String,
    pub title: String,
    /// The space it is open in, if the chain's sidebar has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    /// Sent to the other devices: whichever sees it first takes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub later: bool,
}

/// What a newer version wrote next to the fields this one knows; kept when
/// the collection is written.
type Other = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Entry {
    id: String,
    name: String,
    /// Unix seconds of the last write of this entry.
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    tabs: Vec<DeviceTab>,
    /// Ids of other devices' tabs this device took over.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    taken: Vec<String>,
    /// A phone's edits for a desktop to make to the sidebar (`edits::Edit`).
    /// Kept as they are: a kind a newer version made is no error here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    edits: Vec<serde_json::Value>,
    /// Ids of other devices' edits this device (a desktop) applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    applied: Vec<String>,
    #[serde(flatten)]
    other: Other,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Devices {
    #[serde(default)]
    devices: Vec<Entry>,
    #[serde(flatten)]
    other: Other,
}

pub fn exchange(
    server: &str,
    secret: &str,
    device: &Device,
    tabs: Vec<DeviceTab>,
    dir: &Path,
) -> Result<Vec<String>, SyncError> {
    let unreadable = |reason: &str| SyncError::Unreadable {
        reason: reason.to_string(),
    };
    let keys = SyncCode::from_base64(secret)
        .map_err(|_| SyncError::NotACode)?
        .keys();
    let id = keys.collection_id(DEVICES);
    let path = format!("/collections/{id}");
    let now = now_ms() / 1000;
    for _ in 0..TRIES {
        let reply = request(server, &keys, "GET", &path, None)?;
        let (version, mut all) = match reply.status {
            200 => {
                let remote: CollectionResponse =
                    serde_json::from_slice(&reply.body).map_err(|_| {
                        unreadable("the sync server sent an answer this version cannot read")
                    })?;
                let data = keys
                    .open(&id, remote.version, &remote.sealed())
                    .map_err(|_| {
                        unreadable("the devices cannot be decrypted with this sync code")
                    })?;
                // Not read, not written: a newer version's data stays.
                let all: Devices = serde_json::from_slice(&data)
                    .map_err(|_| unreadable("cannot read the devices; is this app up to date?"))?;
                (remote.version, all)
            }
            // No device wrote yet.
            404 => (0, Devices::default()),
            _ => return Err(reply.error()),
        };
        let taken_by_others: Vec<&String> = all
            .devices
            .iter()
            .filter(|d| d.id != device.id)
            .flat_map(|d| &d.taken)
            .collect();
        let taken: Vec<String> = tabs
            .iter()
            .filter(|t| taken_by_others.contains(&&t.id))
            .map(|t| t.id.clone())
            .collect();
        // The edits a desktop applied are no longer listed.
        let done: Vec<String> = all
            .devices
            .iter()
            .filter(|d| d.id != device.id)
            .flat_map(|d| d.applied.iter().cloned())
            .collect();
        let applied: Vec<String> = edits::pending(dir)
            .into_iter()
            .map(|e| e.id)
            .filter(|id| done.contains(id))
            .collect();
        edits::applied(dir, &applied)?;
        let edits: Vec<serde_json::Value> = edits::pending(dir)
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();

        let before = all.devices.iter().find(|d| d.id == device.id);
        let same = match before {
            Some(e) => e.name == device.name && e.tabs == tabs && e.edits == edits,
            // A device without tabs or edits needs no entry.
            None => tabs.is_empty() && edits.is_empty(),
        };
        if same {
            return Ok(taken);
        }
        let mine = Entry {
            id: device.id.clone(),
            name: device.name.clone(),
            updated_at: now,
            tabs: tabs.clone(),
            taken: before.map(|e| e.taken.clone()).unwrap_or_default(),
            edits,
            applied: Vec::new(),
            other: before.map(|e| e.other.clone()).unwrap_or_default(),
        };
        all.devices
            .retain(|d| d.id != device.id && now - d.updated_at < GONE_AFTER_SECONDS);
        all.devices.push(mine);

        let next = version + 1;
        let plain = serde_json::to_vec(&all).unwrap_or_default();
        let sealed = keys
            .seal(&id, next, &plain)
            .map_err(|_| unreadable("the devices cannot be encrypted"))?;
        let body =
            serde_json::to_vec(&PutCollectionRequest::new(version, sealed)).unwrap_or_default();
        let put = request(server, &keys, "PUT", &path, Some(body))?;
        match put.status {
            200 => return Ok(taken),
            // Another device wrote first: read again.
            409 => {}
            _ => return Err(put.error()),
        }
    }
    Err(SyncError::Server {
        reason: "other devices kept writing first; try again later".into(),
    })
}
