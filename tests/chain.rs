//! Joining and reading a chain, against the demo server.

#[path = "../examples/demo_server.rs"]
mod demo;

use std::net::TcpListener;

use magnetite_mobile_core::{
    Device, DeviceTab, RowItem, SyncError, add_favorite, cached, cached_archive, exchange,
    forget_archived, join, new_space, pin, refresh, refresh_archive, remove, rename, restore,
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

/// The chain's sidebar, as the server holds it.
fn held_sidebar(chain: &demo::Demo) -> serde_json::Value {
    serde_json::from_str(&chain.read("sidebar").unwrap()).unwrap()
}

/// The edits the phone lists in the chain's `devices`, for a desktop.
fn listed_edits(chain: &demo::Demo) -> usize {
    let Some(held) = chain.read("devices") else {
        return 0;
    };
    let held: serde_json::Value = serde_json::from_str(&held).unwrap();
    let devices = held["devices"].as_array().unwrap();
    let mine = devices.iter().find(|d| d["id"] == "phone");
    mine.and_then(|d| d["edits"].as_array()).map_or(0, Vec::len)
}

#[test]
fn a_change_made_on_the_phone_is_written_into_the_chain_by_the_phone() {
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

    // Meanwhile a desktop of a newer version wrote the sidebar: it opened
    // a tab and gave a space something this version does not know.
    let mut theirs = held_sidebar(&chain);
    theirs["spaces"][0]["wallpaper"] = "hills".into();
    theirs["spaces"][0]["today"].as_array_mut().unwrap().push(
        serde_json::json!({"id": "theirs", "url": "https://example.net/", "title": "Theirs"}),
    );
    chain.write("sidebar", &theirs.to_string());

    // The phone writes its changes into that sidebar, and asks no desktop.
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    assert_eq!(listed_edits(&chain), 0);
    let held = held_sidebar(&chain);
    assert_eq!(held["spaces"][0]["title"], "House");
    assert_eq!(
        held["spaces"][0]["wallpaper"], "hills",
        "kept, though unknown"
    );
    assert_eq!(held["spaces"][0]["today"][1]["id"], "theirs");
    assert_eq!(held["spaces"][2]["title"], "Trip");
    // A new space runs in the default profile.
    assert_eq!(
        held["spaces"][2]["profile_id"], held["profiles"][0]["id"],
        "{}",
        held["profiles"]
    );

    // What the phone shows is the chain's, none waiting any more.
    let synced = cached(dir.clone()).unwrap();
    assert_eq!(synced.version, 3);
    assert_eq!(synced.spaces[0].title, "House");
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
    // Nothing newer on the server than what the phone wrote.
    assert_eq!(refresh(server, secret, dir).unwrap(), None);
}

#[test]
fn a_removed_pinned_tab_goes_to_the_archive_and_can_be_brought_back() {
    let (chain, server) = chain(1);
    let (secret, dir) = (demo::DEMO_SECRET.to_string(), dir("archive"));
    refresh(server.clone(), secret.clone(), dir.clone()).unwrap();
    assert!(
        refresh_archive(server.clone(), secret.clone(), dir.clone())
            .unwrap()
            .is_empty()
    );

    // In the archive at once, before the chain has heard of it.
    remove(dir.clone(), "pin-2".into()).unwrap();
    let archived = cached_archive(dir.clone());
    assert_eq!(archived.len(), 1);
    assert_eq!(
        (archived[0].id.as_str(), archived[0].space.as_deref()),
        ("pin-2", Some("Home"))
    );
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    let held: serde_json::Value = serde_json::from_str(&chain.read("archive").unwrap()).unwrap();
    assert_eq!(held["entries"][0]["tab"]["id"], "pin-2");
    assert_eq!(held["entries"][0]["space_id"], "space-home");
    assert!(!chain.read("sidebar").unwrap().contains("pin-2"));

    // Brought back: a Today tab of its space, and out of the archive.
    let shown = restore(dir.clone(), "pin-2".into()).unwrap().unwrap();
    assert_eq!(shown.spaces[0].today[0].id, "pin-2");
    assert!(cached_archive(dir.clone()).is_empty());
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    assert_eq!(held_sidebar(&chain)["spaces"][0]["today"][0]["id"], "pin-2");
    let held: serde_json::Value = serde_json::from_str(&chain.read("archive").unwrap()).unwrap();
    assert_eq!(held["entries"].as_array().unwrap().len(), 0);

    // A tab a desktop closed is in the phone's archive; removed there for
    // good, it leaves the chain's.
    chain.write(
        "archive",
        r#"{"version":1,"entries":[{"tab":{"id":"closed","url":"https://example.net/","title":"Closed","custom_title":false,"unread":false},"space_id":"space-work","archived_at":5}]}"#,
    );
    let archived = refresh_archive(server.clone(), secret.clone(), dir.clone()).unwrap();
    assert_eq!(
        (archived[0].id.as_str(), archived[0].space.as_deref()),
        ("closed", Some("Work"))
    );
    assert!(
        forget_archived(dir.clone(), "closed".into())
            .unwrap()
            .is_empty()
    );
    exchange(server, secret, phone(), vec![], dir).unwrap();
    let held: serde_json::Value = serde_json::from_str(&chain.read("archive").unwrap()).unwrap();
    assert_eq!(held["entries"].as_array().unwrap().len(), 0);
}

#[test]
fn while_the_phone_cannot_write_the_sidebar_a_desktop_is_asked() {
    let (chain, server) = chain(1);
    let (secret, dir) = (demo::DEMO_SECRET.to_string(), dir("fallback"));
    refresh(server.clone(), secret.clone(), dir.clone()).unwrap();
    // A newer version of Magnetite wrote the sidebar in a format this one
    // must not write over.
    let mut newer = held_sidebar(&chain);
    newer["version"] = 2.into();
    chain.write("sidebar", &newer.to_string());

    pin(
        dir.clone(),
        "space-work".into(),
        "https://docs.rs/".into(),
        "Docs".into(),
    )
    .unwrap();
    exchange(server.clone(), secret.clone(), phone(), vec![], dir.clone()).unwrap();
    assert_eq!(held_sidebar(&chain), newer, "left as it is");
    assert_eq!(listed_edits(&chain), 1, "for a desktop that can");
    // Still shown on the phone, as waiting.
    let shown = cached(dir).unwrap();
    let RowItem::Page { page } = &shown.spaces[1].pinned.last().unwrap().item else {
        panic!("a pinned page")
    };
    assert!(page.pending);
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

#[test]
fn a_kind_of_change_a_newer_phone_made_does_not_stop_this_one() {
    let (chain, server) = chain(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    chain.write(
        "devices",
        &format!(
            r#"{{"devices":[{{"id":"tablet","name":"iPad","updated_at":{now},"tabs":[],"edits":[{{"id":"x","op":"paint_it_blue","item":"s"}}]}}]}}"#
        ),
    );
    exchange(
        server,
        demo::DEMO_SECRET.into(),
        phone(),
        vec![tab("a")],
        dir("newer"),
    )
    .unwrap();
    let held: serde_json::Value = serde_json::from_str(&chain.read("devices").unwrap()).unwrap();
    assert_eq!(held["devices"][0]["edits"][0]["op"], "paint_it_blue");
}
