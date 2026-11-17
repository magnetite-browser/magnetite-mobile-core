//! Reading a sync chain's sidebar, and keeping this device's entry in its
//! `devices` collection. The requests are the desktop's
//! (`crates/app/src/sync.rs` in magnetite).

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use magnetite_core::SidebarSnapshot;
use magnetite_sync_proto::{ChainKeys, CollectionResponse, SyncCode};
use serde::{Deserialize, Serialize};

use crate::SyncError;
use crate::view::{self, Sidebar};

const SIDEBAR: &str = "sidebar";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Largest answer read: a collection is at most 16 MiB of ciphertext, as
/// JSON a bit more.
const MAX_REPLY: u64 = 32 * 1024 * 1024;

/// The sidebar last read, as kept in `sidebar.json`.
#[derive(Serialize, Deserialize)]
struct Kept {
    version: u64,
    snapshot: SidebarSnapshot,
}

fn kept_path(dir: &Path) -> PathBuf {
    dir.join("sidebar.json")
}

fn kept(dir: &Path) -> Option<Kept> {
    serde_json::from_slice(&std::fs::read(kept_path(dir)).ok()?).ok()
}

/// Written beside the file and moved over it, so a crash leaves the old one.
fn keep(dir: &Path, kept: &Kept) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let part = dir.join("sidebar.json.part");
    std::fs::write(&part, serde_json::to_vec(kept)?)?;
    std::fs::rename(part, kept_path(dir))
}

pub fn cached(dir: &Path) -> Option<Sidebar> {
    kept(dir).map(|k| view::sidebar(&k.snapshot, k.version))
}

pub fn forget(dir: &Path) {
    std::fs::remove_file(kept_path(dir)).ok();
}

/// The address requests go to (no trailing slash). The server must use
/// `https://`; plain `http://` is accepted only for this device (`localhost`,
/// `127.0.0.1`, `::1`). A path prefix is fine; user info and queries are not.
fn server_base(url: &str) -> Result<String, SyncError> {
    let url = url.trim().trim_end_matches('/');
    let bad = |reason: String| SyncError::Address { reason };
    let not_one = || bad(format!("not a sync server address: {url}"));
    let uri: ureq::http::Uri = url.parse().map_err(|_| not_one())?;
    let authority = uri.authority().ok_or_else(not_one)?;
    if authority.as_str().contains('@') {
        return Err(bad(
            "the sync server address must not contain a user name".into()
        ));
    }
    if uri.query().is_some() {
        return Err(not_one());
    }
    let loopback = matches!(authority.host(), "localhost" | "127.0.0.1" | "[::1]");
    match uri.scheme_str() {
        Some("https") => Ok(url.to_string()),
        Some("http") if loopback => Ok(url.to_string()),
        _ => Err(bad(format!(
            "the sync server needs an https:// address: {url}"
        ))),
    }
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

impl Reply {
    /// What went wrong, in our words.
    pub(crate) fn error(&self) -> SyncError {
        let reason = match self.status {
            403 => "the sync server refused: the chain holds too many collections".into(),
            413 => "this sync chain has used up its storage on the server".into(),
            401 => "the sync server did not accept this device's signature (is the clock right?)"
                .into(),
            408 => "the request took too long to reach the sync server".into(),
            410 => "this sync chain was deleted a moment ago".into(),
            429 | 503 => "the sync server is busy; try again later".into(),
            s if s >= 500 => "the sync server had an error; try again later".into(),
            s => format!("the sync server answered {s}"),
        };
        SyncError::Server { reason }
    }
}

/// Signed request to `path` below `/v2` on `server`. The signature covers
/// the server's host, so no other server can replay it.
pub(crate) fn request(
    server: &str,
    keys: &ChainKeys,
    method: &str,
    path: &str,
    body: Option<Vec<u8>>,
) -> Result<Reply, SyncError> {
    let base = server_base(server)?;
    let host = base
        .parse::<ureq::http::Uri>()
        .ok()
        .and_then(|u| u.authority().map(|a| a.as_str().to_string()))
        .unwrap_or_default();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Status codes are handled here, not turned into errors by ureq.
        .http_status_as_error(false)
        // A redirect could lead to plain http or another host.
        .max_redirects(0)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .into();
    let server_error = |reason: String| SyncError::Server { reason };
    let bytes = body.unwrap_or_default();
    let builder = ureq::http::Request::builder()
        .method(method)
        .uri(format!("{base}/v2{path}"))
        .header(
            "authorization",
            keys.authorization(method, &host, path, &bytes, now_ms()),
        )
        // The body's hash ahead of the body: the server checks the signature
        // before it reads an upload.
        .header(
            magnetite_sync_proto::CONTENT_DIGEST,
            magnetite_sync_proto::content_digest(&bytes),
        )
        .header("content-type", "application/json");
    let sent = if bytes.is_empty() {
        builder
            .body(())
            .map_err(|e| server_error(e.to_string()))
            .and_then(|r| {
                agent
                    .run(r)
                    .map_err(|e| server_error(format!("cannot reach the sync server: {e}")))
            })
    } else {
        builder
            .body(bytes)
            .map_err(|e| server_error(e.to_string()))
            .and_then(|r| {
                agent
                    .run(r)
                    .map_err(|e| server_error(format!("cannot reach the sync server: {e}")))
            })
    };
    let mut res = sent?;
    let status = res.status().as_u16();
    // ureq's own limit counts the bytes on the wire; a compressed answer can
    // unpack to far more. Count what comes out of the decoder.
    let mut body = Vec::new();
    res.body_mut()
        .with_config()
        .limit(MAX_REPLY)
        .reader()
        .take(MAX_REPLY + 1)
        .read_to_end(&mut body)
        .map_err(|e| server_error(e.to_string()))?;
    if body.len() as u64 > MAX_REPLY {
        return Err(server_error(
            "the sync server sent more data than a collection can hold".into(),
        ));
    }
    Ok(Reply { status, body })
}

fn get(server: &str, keys: &ChainKeys, path: &str) -> Result<Reply, SyncError> {
    request(server, keys, "GET", path, None)
}

pub fn join(server: &str, words: &str) -> Result<String, SyncError> {
    let code = SyncCode::from_words(words).map_err(|_| SyncError::NotACode)?;
    let reply = get(server, &code.keys(), "/chain")?;
    match reply.status {
        204 => Ok(code.to_base64()),
        // The server tells only the key holder that a chain does not exist.
        404 => Err(SyncError::NoChain),
        _ => Err(reply.error()),
    }
}

pub fn refresh(server: &str, secret: &str, dir: &Path) -> Result<Option<Sidebar>, SyncError> {
    let unreadable = |reason: &str| SyncError::Unreadable {
        reason: reason.to_string(),
    };
    let code = SyncCode::from_base64(secret).map_err(|_| SyncError::NotACode)?;
    let keys = code.keys();
    let id = keys.collection_id(SIDEBAR);
    let reply = get(server, &keys, &format!("/collections/{id}"))?;
    match reply.status {
        200 => {}
        404 => return Err(SyncError::Empty),
        _ => return Err(reply.error()),
    }
    // A parse error would quote the body, so it is not shown.
    let remote: CollectionResponse = serde_json::from_slice(&reply.body)
        .map_err(|_| unreadable("the sync server sent an answer this version cannot read"))?;
    let seen = kept(dir).map_or(0, |k| k.version);
    if remote.version < seen {
        // Versions only go up: a server cannot roll a device back.
        return Err(SyncError::Server {
            reason: format!(
                "the server has version {} but this device has {seen}; not going back",
                remote.version
            ),
        });
    }
    if remote.version == seen {
        return Ok(None);
    }
    let data = keys
        .open(&id, remote.version, &remote.sealed())
        .map_err(|_| unreadable("the sidebar cannot be decrypted with this sync code"))?;
    let snapshot: SidebarSnapshot = serde_json::from_slice(&data)
        .map_err(|_| unreadable("cannot read the sidebar; is this app up to date?"))?;
    if snapshot.spaces.is_empty() {
        return Err(unreadable("the sidebar has no spaces"));
    }
    let kept = Kept {
        version: remote.version,
        snapshot,
    };
    keep(dir, &kept).map_err(|e| SyncError::Storage {
        reason: format!("cannot keep the sidebar on this device: {e}"),
    })?;
    Ok(Some(view::sidebar(&kept.snapshot, kept.version)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_this_device() {
        assert!(
            server_base("https://sync.example/prefix/")
                .is_ok_and(|b| b == "https://sync.example/prefix")
        );
        assert!(server_base("http://localhost:8080").is_ok());
        assert!(server_base("http://sync.example").is_err());
        assert!(server_base("https://me@sync.example").is_err());
        assert!(server_base("https://sync.example/?a=b").is_err());
    }

    #[test]
    fn words_that_are_no_code_fail_before_any_request() {
        assert!(matches!(
            join("https://sync.example", "not a code"),
            Err(SyncError::NotACode)
        ));
    }
}
