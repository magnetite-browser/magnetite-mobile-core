//! Joining and reading a chain, against the demo server.

#[path = "../examples/demo_server.rs"]
mod demo;

use std::net::TcpListener;

use magnetite_mobile_core::{
    Device, DeviceTab, RowItem, SyncError, add_favorite, cached, exchange, join, new_space, pin,
    refresh, rename,
};
use magnetite_sync_proto::SyncCode;

/// A demo server with the sidebar at `version`, and its address.
fn chain(version: u64) -> (demo::Demo, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let chain = demo::Demo::new(version);
    let serving = chain.clone();
    std::thread::spawn(move || serving.serve(&listener));
    (chain, address)
}

fn server(version: u64) -> String {
    chain(version).1
}

fn phone() -> Device {
    Device {
        id: "phone".into(),
        name: "iPhone".into(),
    }
}

fn tab(id: &str) -> DeviceTab {
    DeviceTab {
        id: id.into(),
        url: format!("https://example.org/{id}"),
        title: id.into(),
        space: None,
        later: false,
    }
}

fn dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!(
        "magnetite-mobile-core-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

#[test]
fn a_joined_device_reads_the_sidebar_once_and_keeps_it() {
    let (server, dir) = (server(3), dir("reads"));
    let words = SyncCode::from_base64(demo::DEMO_SECRET).unwrap().words();
    let secret = join(server.clone(), words).unwrap();

    let sidebar = refresh(server.clone(), secret.clone(), dir.clone())
        .unwrap()
        .unwrap();
    assert_eq!(sidebar.version, 3);
    let home = &sidebar.spaces[0];
    assert_eq!(
        (home.title.as_str(), home.favorites.len(), home.today.len()),
        ("Home", 4, 1)
    );
    assert!(matches!(&home.pinned[1].item, RowItem::Folder { title, .. } if title == "Reading"));

    // Nothing newer: no sidebar again, and the kept one is the same.
    assert_eq!(refresh(server, secret, dir.clone()).unwrap(), None);
    assert_eq!(cached(dir), Some(sidebar));
}

#[test]
fn a_server_cannot_roll_a_device_back() {
    let dir = dir("rollback");
    refresh(server(3), demo::DEMO_SECRET.into(), dir.clone()).unwrap();
    let older = refresh(server(2), demo::DEMO_SECRET.into(), dir);
    assert!(matches!(older, Err(SyncError::Server { .. })));
}

#[test]
fn another_code_cannot_read_the_chain() {
    let other = SyncCode::generate().to_base64();
    // The demo server has no collection under the other code's id.
    assert!(matches!(
        refresh(server(1), other, dir("other")),
        Err(SyncError::Empty)
    ));
}

#[test]
fn a_device_lists_its_tabs_and_leaves_the_other_entries_alone() {
    let (chain, server) = chain(1);
    let secret = demo::DEMO_SECRET.to_string();
    // Nothing open: nothing written.
    assert!(
        exchange(
            server.clone(),
            secret.clone(),
            phone(),
            vec![],
            dir("lists")
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(chain.read("devices"), None);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    chain.write(
        "devices",
        &format!(r#"{{"devices":[{{"id":"mac","name":"Mac","updated_at":{now},"tabs":[],"taken":["b"]}}]}}"#),
    );
    // The desktop took tab b: the phone hears of it and still lists both,
    // until the app closed b.
    let taken = exchange(
        server.clone(),
        secret.clone(),
        phone(),
        vec![tab("a"), tab("b")],
        dir("lists"),
    )
    .unwrap();
    assert_eq!(taken, ["b"]);
    let held: serde_json::Value = serde_json::from_str(&chain.read("devices").unwrap()).unwrap();
    let devices = held["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(
        devices[0]["taken"][0], "b",
        "the desktop's entry is as it was"
    );
    assert_eq!(devices[1]["tabs"].as_array().unwrap().len(), 2);

    // The same tabs again: no new version.
    let before = chain.read("devices");
    exchange(
        server,
        secret,
        phone(),
        vec![tab("a"), tab("b")],
        dir("lists"),
    )
    .unwrap();
    assert_eq!(chain.read("devices"), before);
}

#[test]
fn entries_of_devices_long_gone_are_dropped() {
    let (chain, server) = chain(1);
    chain.write(
        "devices",
        r#"{"devices":[{"id":"old","name":"Old phone","updated_at":1,"tabs":[]}]}"#,
    );
    exchange(
        server,
        demo::DEMO_SECRET.into(),
        phone(),
        vec![tab("a")],
        dir("gone"),
    )
    .unwrap();
    let held: serde_json::Value = serde_json::from_str(&chain.read("devices").unwrap()).unwrap();
    assert_eq!(held["devices"].as_array().unwrap().len(), 1);
    assert_eq!(held["devices"][0]["id"], "phone");
}

#[test]
fn a_change_made_on_the_phone_waits_for_a_desktop_then_is_the_chains() {
    let (chain, server) = chain(1);
    let (secret, dir) = (demo::DEMO_SECRET.to_string(), dir("changes"));
    refresh(server.clone(), secret.clone(), dir.clone()).unwrap();

    // Shown at once, marked as waiting.
    let shown = pin(
        dir.clone(),
        "space-work".into(),
        "https://docs.rs/".into(),
        "Docs".into(),
    )
    .unwrap()
    .unwrap();
    let RowItem::Page { page } = &shown.spaces[1].pinned.last().unwrap().item else {
        panic!("a pinned page")
    };
    assert!(page.pending && page.title == "Docs");
    let pinned = page.id.clone();
    add_favorite(
        dir.clone(),
        "space-work".into(),
        "https://lib.rs/".into(),
        "Lib".into(),
    )
    .unwrap();
    rename(dir.clone(), "space-home".into(), "House".into()).unwrap();
    let shown = new_space(dir.clone(), "Trip".into(), Some("plane".into()))
        .unwrap()
        .unwrap();
    assert_eq!(shown.spaces[0].title, "House");
    assert!(shown.spaces[2].pending && shown.spaces[2].title == "Trip");

    // The chain hears of them; a desktop makes them and says so.
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    assert_eq!(chain.apply_all(), 4);
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    let held: serde_json::Value = serde_json::from_str(&chain.read("devices").unwrap()).unwrap();
    let mine = held["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone");
    assert!(
        mine.is_some_and(|d| d.get("edits").is_none()),
        "listed no more"
    );

    // The new sidebar has them all, none waiting any more.
    let synced = refresh(server, secret, dir.clone()).unwrap().unwrap();
    assert_eq!(synced.spaces[0].title, "House");
    assert_eq!(synced.spaces[2].title, "Trip");
    let RowItem::Page { page } = &synced.spaces[1].pinned.last().unwrap().item else {
        panic!("a pinned page")
    };
    assert_eq!(page.id, pinned);
    assert!(!page.pending && !synced.spaces[2].pending);
    assert!(
        synced.spaces[1]
            .favorites
            .iter()
            .any(|f| f.title == "Lib" && !f.pending)
    );
    assert_eq!(cached(dir), Some(synced));
}

#[test]
fn only_web_pages_can_be_pinned() {
    let dir = dir("web");
    refresh(server(1), demo::DEMO_SECRET.into(), dir.clone()).unwrap();
    let local = pin(
        dir,
        "space-home".into(),
        "file:///etc/hosts".into(),
        "Hosts".into(),
    );
    assert!(matches!(local, Err(SyncError::Address { .. })));
}
