//! A stand-in sync server for trying the apps: it holds one chain with a
//! sample sidebar and takes writes of other collections, in memory. It
//! checks no signatures.
//!
//! `cargo run --example demo_server` listens on 127.0.0.1:8787 (another port
//! with `DEMO_PORT=…`, to run several) and prints the 24 words to join with.
//! A simulator on this machine reaches it as `http://localhost:8787`.
//! `curl -X POST localhost:8787/demo/take` plays a desktop that takes over
//! every tab the phones list, `curl -X POST localhost:8787/demo/apply` one
//! that makes the changes the phones asked for, and
//! `curl -X POST localhost:8787/demo/phone` a phone with tabs open.
//! `http://localhost:8787/demo/ads` is a page with an image that EasyList
//! blocks (`/ad-choices.png$image`): it says whether the image loaded.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use magnetite_sync_proto::{
    ChainKeys, CollectionResponse, PutCollectionRequest, PutCollectionResponse, SyncCode,
};

/// The demo chain's code: 32 zero bytes. Known to everyone, so only ever
/// for this server.
pub const DEMO_SECRET: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

pub const SIDEBAR: &str = r#"{
  "format": "magnetite-sidebar", "version": 1,
  "profiles": [{"id": "3b1f0a52-6c1e-4f0a-9d53-0a1e7c1b2a01", "name": "Personal", "is_default": true},
               {"id": "9c2e7d14-2b7a-4c55-8e01-5d7f3a9b6c02", "name": "Work"}],
  "favorites": [
    {"profile_id": "3b1f0a52-6c1e-4f0a-9d53-0a1e7c1b2a01", "items": [
      {"id": "fav-1", "url": "https://en.wikipedia.org/", "title": "Wikipedia", "home_url": "https://en.wikipedia.org/"},
      {"id": "fav-2", "url": "https://www.openstreetmap.org/", "title": "OpenStreetMap", "home_url": "https://www.openstreetmap.org/"},
      {"id": "fav-3", "url": "https://news.ycombinator.com/", "title": "Hacker News", "home_url": "https://news.ycombinator.com/"},
      {"id": "fav-4", "url": "https://example.org/", "title": "Example", "home_url": "https://example.org/"}
    ]},
    {"profile_id": "9c2e7d14-2b7a-4c55-8e01-5d7f3a9b6c02", "items": [
      {"id": "fav-5", "url": "https://github.com/", "title": "GitHub", "home_url": "https://github.com/"}
    ]}
  ],
  "spaces": [
    {"id": "space-home", "title": "Home", "icon": "home", "accent": {"red": 0.89, "green": 0.38, "blue": 0.17},
     "profile_id": "3b1f0a52-6c1e-4f0a-9d53-0a1e7c1b2a01",
     "pinned": [
       {"kind": "tab", "id": "pin-1", "url": "https://magnetitebrowser.org/", "title": "Magnetite", "home_url": "https://magnetitebrowser.org/"},
       {"kind": "folder", "id": "dir-1", "title": "Reading", "children": [
         {"kind": "tab", "id": "pin-2", "url": "https://www.rust-lang.org/", "title": "Rust", "home_url": "https://www.rust-lang.org/"},
         {"kind": "folder", "id": "dir-2", "title": "Later", "children": [
           {"kind": "tab", "id": "pin-3", "url": "https://developer.mozilla.org/", "title": "MDN Web Docs", "home_url": "https://developer.mozilla.org/"}
         ]}
       ]}
     ],
     "today": [
       {"id": "today-1", "url": "https://example.com/", "title": "Example Domain"}
     ]},
    {"id": "space-work", "title": "Work", "icon": "briefcase", "accent": {"red": 0.2, "green": 0.5, "blue": 0.9},
     "profile_id": "9c2e7d14-2b7a-4c55-8e01-5d7f3a9b6c02",
     "pinned": [
       {"kind": "tab", "id": "pin-4", "url": "https://github.com/magnetite-browser/magnetite", "title": "magnetite", "home_url": "https://github.com/magnetite-browser/magnetite"}
     ]}
  ]
}"#;

/// The demo chain: its collections by id, as the server holds them.
#[derive(Clone)]
pub struct Demo {
    keys: Arc<ChainKeys>,
    held: Arc<Mutex<HashMap<String, CollectionResponse>>>,
}

#[allow(dead_code)]
impl Demo {
    /// A chain with the sample sidebar at `version`.
    pub fn new(version: u64) -> Self {
        let demo = Self {
            keys: Arc::new(
                SyncCode::from_base64(DEMO_SECRET)
                    .expect("demo code")
                    .keys(),
            ),
            held: Arc::default(),
        };
        demo.hold("sidebar", version, SIDEBAR);
        demo
    }

    fn hold(&self, name: &str, version: u64, plain: &str) {
        let id = self.keys.collection_id(name);
        let sealed = self
            .keys
            .seal(&id, version, plain.as_bytes())
            .expect("seal");
        self.held
            .lock()
            .unwrap()
            .insert(id, CollectionResponse::new(version, sealed));
    }

    /// What collection `name` holds, decrypted.
    pub fn read(&self, name: &str) -> Option<String> {
        let id = self.keys.collection_id(name);
        let held = self.held.lock().unwrap();
        let c = held.get(&id)?;
        let plain = self.keys.open(&id, c.version, &c.sealed()).ok()?;
        String::from_utf8(plain).ok()
    }

    /// Write `plain` as the next version of collection `name`, as another
    /// device of the chain would.
    pub fn write(&self, name: &str, plain: &str) {
        let id = self.keys.collection_id(name);
        let version = self.held.lock().unwrap().get(&id).map_or(0, |c| c.version);
        self.hold(name, version + 1, plain);
    }

    /// Play a phone with two tabs open and one sent to the desktop
    /// (`POST /demo/phone`), for trying a desktop.
    pub fn phone(&self) {
        let mut all: serde_json::Value = self
            .read("devices")
            .and_then(|plain| serde_json::from_str(&plain).ok())
            .unwrap_or_else(|| serde_json::json!({ "devices": [] }));
        let devices = all["devices"].as_array_mut().expect("devices");
        devices.retain(|d| d["id"] != "demo-phone");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        devices.push(serde_json::json!({
            "id": "demo-phone", "name": "iPhone", "updated_at": now, "tabs": [
                {"id": "phone-1", "url": "https://en.wikipedia.org/wiki/Magnetite", "title": "Magnetite - Wikipedia", "space": "space-home"},
                {"id": "phone-2", "url": "https://www.rust-lang.org/learn", "title": "Learn Rust", "space": "space-work"},
                {"id": "phone-3", "url": "https://example.org/", "title": "Example Domain", "space": "space-home", "later": true},
            ],
        }));
        self.write("devices", &all.to_string());
    }

    /// Play a desktop that takes over every tab the other devices list
    /// (`POST /demo/take`). Returns how many.
    pub fn take_all(&self) -> usize {
        let mut all: serde_json::Value = self
            .read("devices")
            .and_then(|plain| serde_json::from_str(&plain).ok())
            .unwrap_or_else(|| serde_json::json!({ "devices": [] }));
        let devices = all["devices"].as_array_mut().expect("devices");
        let mut me = desktop(devices);
        let taken: Vec<serde_json::Value> = devices
            .iter()
            .flat_map(|d| d["tabs"].as_array().cloned().unwrap_or_default())
            .map(|t| t["id"].clone())
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let count = taken.len();
        me["taken"] = taken.into();
        me["updated_at"] = now.into();
        devices.push(me);
        self.write("devices", &all.to_string());
        count
    }

    /// Play a desktop that makes the changes to the sidebar the other
    /// devices asked for, writes the sidebar and says which it made
    /// (`POST /demo/apply`). Returns how many.
    pub fn apply_all(&self) -> usize {
        use magnetite_mobile_core::edits::{Edit, apply};
        let mut all: serde_json::Value = self
            .read("devices")
            .and_then(|plain| serde_json::from_str(&plain).ok())
            .unwrap_or_else(|| serde_json::json!({ "devices": [] }));
        let devices = all["devices"].as_array_mut().expect("devices");
        let mut me = desktop(devices);
        let before: Vec<serde_json::Value> = me["applied"].as_array().cloned().unwrap_or_default();
        let edits: Vec<Edit> = devices
            .iter()
            .flat_map(|d| d["edits"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| serde_json::from_value(e).ok())
            .filter(|e: &Edit| !before.contains(&serde_json::Value::from(e.id.clone())))
            .collect();
        if edits.is_empty() {
            return 0;
        }
        let mut sidebar: magnetite_core::SidebarSnapshot =
            serde_json::from_str(&self.read("sidebar").expect("a sidebar")).expect("the sidebar");
        for edit in &edits {
            apply(&mut sidebar, &edit.change);
        }
        self.write("sidebar", &serde_json::to_string(&sidebar).expect("json"));
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let applied: Vec<serde_json::Value> = before
            .into_iter()
            .chain(edits.iter().map(|e| e.id.clone().into()))
            .collect();
        me["applied"] = applied.into();
        me["updated_at"] = now.into();
        devices.push(me);
        self.write("devices", &all.to_string());
        edits.len()
    }

    /// Answer requests on `listener` for ever.
    pub fn serve(&self, listener: &TcpListener) {
        for stream in listener.incoming().flatten() {
            self.answer(stream);
        }
    }

    fn answer(&self, mut stream: TcpStream) {
        let mut reader = BufReader::new(&stream);
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            return;
        }
        let mut length = 0;
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse().unwrap_or(0);
            }
            line.clear();
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let mut words = first.split_whitespace();
        let (method, path) = (
            words.next().unwrap_or_default(),
            words.next().unwrap_or_default(),
        );
        if let Some((kind, page)) = page(path) {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                page.len()
            );
            let _ = stream.write_all(page);
            return;
        }
        let (status, answer) = self.reply(method, path, &body);
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
            answer.len()
        );
    }

    fn reply(&self, method: &str, path: &str, body: &[u8]) -> (&'static str, String) {
        if path == "/v2/chain" {
            return ("204 No Content", String::new());
        }
        if path == "/demo/phone" {
            self.phone();
            return ("200 OK", String::new());
        }
        if path == "/demo/take" {
            return ("200 OK", format!("{}\n", self.take_all()));
        }
        if path == "/demo/apply" {
            return ("200 OK", format!("{}\n", self.apply_all()));
        }
        let Some(id) = path.strip_prefix("/v2/collections/") else {
            return ("404 Not Found", String::new());
        };
        let mut held = self.held.lock().unwrap();
        match method {
            "GET" => match held.get(id) {
                Some(c) => ("200 OK", serde_json::to_string(c).expect("json")),
                None => ("404 Not Found", String::new()),
            },
            "PUT" => {
                let Ok(put) = serde_json::from_slice::<PutCollectionRequest>(body) else {
                    return ("400 Bad Request", String::new());
                };
                if put.base_version != held.get(id).map_or(0, |c| c.version) {
                    return ("409 Conflict", String::new());
                }
                let version = put.base_version + 1;
                held.insert(
                    id.to_string(),
                    CollectionResponse::new(version, put.sealed()),
                );
                (
                    "200 OK",
                    serde_json::to_string(&PutCollectionResponse { version }).expect("json"),
                )
            }
            _ => ("405 Method Not Allowed", String::new()),
        }
    }
}

/// The pages for trying content blocking: one whose image EasyList blocks.
fn page(path: &str) -> Option<(&'static str, &'static [u8])> {
    const ADS: &str = r#"<!doctype html><meta name="viewport" content="width=device-width">
<title>Content blocking</title>
<h1>Content blocking</h1>
<p id="state">Loading the ad…</p>
<img src="/demo/ad-choices.png" alt="" width="1" height="1"
  onload="state.textContent = 'The ad loaded'" onerror="state.textContent = 'The ad was blocked'">
"#;
    /// A transparent pixel.
    const PIXEL: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5,
        0x1c, 0x0c, 0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64,
        0x60, 0x00, 0x00, 0x00, 0x06, 0x00, 0x02, 0x30, 0x81, 0xd0, 0x2f, 0x00, 0x00, 0x00, 0x00,
        0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    match path {
        "/demo/ads" => Some(("text/html; charset=utf-8", ADS.as_bytes())),
        "/demo/ad-choices.png" => Some(("image/png", PIXEL)),
        _ => None,
    }
}

/// The demo desktop's entry, taken out of `devices` to be changed and put
/// back, so what it took and what it made both stay.
fn desktop(devices: &mut Vec<serde_json::Value>) -> serde_json::Value {
    match devices.iter().position(|d| d["id"] == "demo-desktop") {
        Some(i) => devices.remove(i),
        None => serde_json::json!({ "id": "demo-desktop", "name": "Demo desktop", "tabs": [] }),
    }
}

#[allow(dead_code)]
fn main() {
    let port = std::env::var("DEMO_PORT").unwrap_or_else(|_| "8787".into());
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .unwrap_or_else(|_| panic!("port {port} is taken"));
    let code = SyncCode::from_base64(DEMO_SECRET).expect("demo code");
    println!(
        "Demo sync server on http://localhost:{port}\nSync code: {}",
        code.words()
    );
    Demo::new(1).serve(&listener);
}
