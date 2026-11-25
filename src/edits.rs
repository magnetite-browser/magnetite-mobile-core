//! Changes this phone asks a desktop to make to the chain's sidebar.
//!
//! A phone never writes the sidebar (`lib.rs`). It keeps its edits in
//! `edits.json` beside the sidebar it read, lists them in its entry of the
//! `devices` collection (`devices::exchange`), and shows them on top of that
//! sidebar meanwhile, marked as waiting. A desktop applies an edit to its own
//! sidebar, which it then syncs, and puts the edit's id into its entry's
//! `applied`. Once the phone sees that, it stops listing the edit, and drops
//! it when the sidebar it reads shows the change, or after a week (a change
//! the desktop could not make, say to a space deleted meanwhile).
//!
//! An edit that adds something carries the new item's id, so a second
//! desktop applying it finds the item there and leaves it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use magnetite_core::{FavoriteSet, SidebarNode, SidebarSnapshot, Space, TabItem};
use serde::{Deserialize, Serialize};

use crate::SyncError;
use crate::chain::now_ms;

/// An applied edit the sidebar does not show after this long is dropped.
const SHOWN_FOR_SECONDS: i64 = 7 * 24 * 60 * 60;

/// What a phone asks for. The desktop reads the same format
/// (`crates/app/src/devices.rs` in magnetite).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    /// A new pinned tab `item`, last in the space's pinned tree.
    Pin {
        space: String,
        item: String,
        url: String,
        title: String,
    },
    /// A new favorite `item`, last among those of the space's profile.
    Favorite {
        space: String,
        item: String,
        url: String,
        title: String,
    },
    /// A new space `item`, after the others.
    NewSpace {
        item: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        icon: Option<String>,
    },
    /// A new name for a pinned tab, folder, favorite or space.
    Rename { item: String, title: String },
    /// A pinned tab or a favorite goes; the desktop keeps a pinned tab in
    /// its archive.
    Remove { item: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    /// The edit's own id; `item` in the change names what it is about.
    pub id: String,
    /// Unix seconds.
    pub made_at: i64,
    #[serde(flatten)]
    pub change: Change,
}

/// This phone's edits, as kept in `edits.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    /// Not applied by a desktop yet: listed in the `devices` entry.
    #[serde(default)]
    pending: Vec<Edit>,
    /// Applied by a desktop, shown until the sidebar shows them: the desktop
    /// syncs its sidebar after it said so.
    #[serde(default)]
    applied: Vec<(Edit, i64)>,
}

fn path(dir: &Path) -> PathBuf {
    dir.join("edits.json")
}

fn load(dir: &Path) -> Kept {
    std::fs::read(path(dir))
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

/// Written beside the file and moved over it, so a crash leaves the old one.
fn save(dir: &Path, kept: &Kept) -> Result<(), SyncError> {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let part = dir.join("edits.json.part");
        std::fs::write(&part, serde_json::to_vec(kept)?)?;
        std::fs::rename(part, path(dir))
    };
    write().map_err(|e| SyncError::Storage {
        reason: format!("cannot keep the change on this device: {e}"),
    })
}

/// A new id for an edit or an item: random, so ids made on different
/// devices never meet.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system has random numbers");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Keep `change` for the desktop.
pub fn propose(dir: &Path, change: Change) -> Result<(), SyncError> {
    let mut kept = load(dir);
    kept.pending.push(Edit {
        id: new_id(),
        made_at: now_ms() / 1000,
        change,
    });
    save(dir, &kept)
}

/// The edits no desktop has applied yet.
pub fn pending(dir: &Path) -> Vec<Edit> {
    load(dir).pending
}

/// A desktop applied the edits `ids`: no longer listed, still shown.
pub fn applied(dir: &Path, ids: &[String]) -> Result<(), SyncError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut kept = load(dir);
    let now = now_ms() / 1000;
    let (done, waiting) = kept.pending.into_iter().partition(|e| ids.contains(&e.id));
    kept.pending = waiting;
    kept.applied
        .extend(done.into_iter().map(|e: Edit| (e, now)));
    save(dir, &kept)
}

/// Drop the applied edits `snapshot` shows, and those applied long ago.
pub fn settle(dir: &Path, snapshot: &SidebarSnapshot) {
    let mut kept = load(dir);
    let now = now_ms() / 1000;
    let before = kept.applied.len();
    kept.applied
        .retain(|(e, at)| now - at < SHOWN_FOR_SECONDS && !shows(snapshot, &e.change));
    if kept.applied.len() != before {
        save(dir, &kept).ok();
    }
}

pub fn forget(dir: &Path) {
    std::fs::remove_file(path(dir)).ok();
}

/// Put this phone's edits on top of `snapshot`, as the desktop will. Returns
/// the ids of the items they add or change, which wait for a desktop.
pub fn overlay(dir: &Path, snapshot: &mut SidebarSnapshot) -> HashSet<String> {
    let kept = load(dir);
    let edits = kept.applied.iter().map(|(e, _)| e).chain(&kept.pending);
    let mut waiting = HashSet::new();
    for edit in edits {
        if let Some(item) = apply(snapshot, &edit.change) {
            waiting.insert(item);
        }
    }
    waiting
}

/// Make `change` in `snapshot`, unless it is made already. Returns the item
/// it added or renamed.
pub fn apply(snapshot: &mut SidebarSnapshot, change: &Change) -> Option<String> {
    match change {
        Change::Pin {
            space,
            item: id,
            url,
            title,
        } => {
            if find_tab(snapshot, id) {
                return None;
            }
            let space = snapshot.spaces.iter_mut().find(|s| &s.id == space)?;
            space.pinned.push(SidebarNode::Tab(new_tab(id, url, title)));
            Some(id.clone())
        }
        Change::Favorite {
            space,
            item: id,
            url,
            title,
        } => {
            if find_tab(snapshot, id) {
                return None;
            }
            let profile = snapshot
                .spaces
                .iter()
                .find(|s| &s.id == space)?
                .profile_id
                .clone();
            let tab = new_tab(id, url, title);
            match snapshot
                .favorites
                .iter_mut()
                .find(|f| f.profile_id == profile)
            {
                Some(set) => set.items.push(tab),
                None => snapshot.favorites.push(FavoriteSet {
                    profile_id: profile,
                    items: vec![tab],
                }),
            }
            Some(id.clone())
        }
        Change::NewSpace {
            item: id,
            title,
            icon,
        } => {
            if snapshot.spaces.iter().any(|s| &s.id == id) {
                return None;
            }
            // As the desktop makes one: its star and violet, until then the
            // first space's profile (the desktop gives it its default one).
            let first = snapshot.spaces.first()?;
            snapshot.spaces.push(Space {
                id: id.clone(),
                title: title.clone(),
                icon: icon.clone().or_else(|| Some("star".into())),
                accent: Some(magnetite_core::Color {
                    red: 0.55,
                    green: 0.5,
                    blue: 1.0,
                }),
                profile_id: first.profile_id.clone(),
                pinned: Vec::new(),
                today: Vec::new(),
                bookmarks: Vec::new(),
            });
            Some(id.clone())
        }
        Change::Rename { item, title } => rename(snapshot, item, title).then(|| item.clone()),
        Change::Remove { item } => {
            for space in &mut snapshot.spaces {
                remove_pinned(&mut space.pinned, item);
            }
            for set in &mut snapshot.favorites {
                set.items.retain(|t| &t.id != item);
            }
            None
        }
    }
}

/// Whether `snapshot` shows `change` made.
fn shows(snapshot: &SidebarSnapshot, change: &Change) -> bool {
    match change {
        Change::Pin { item, .. } | Change::Favorite { item, .. } => find_tab(snapshot, item),
        Change::NewSpace { item, .. } => snapshot.spaces.iter().any(|s| &s.id == item),
        Change::Rename { item, title } => {
            let mut copy = snapshot.clone();
            // Renamed already, or gone.
            !rename(&mut copy, item, title)
        }
        Change::Remove { item } => !find_tab(snapshot, item),
    }
}

fn new_tab(id: &str, url: &str, title: &str) -> TabItem {
    TabItem {
        home_url: Some(url.into()),
        created_at: Some(now_ms() / 1000),
        ..TabItem::new(id, url, title)
    }
}

/// Whether a pinned tab or favorite `id` is in `snapshot`.
fn find_tab(snapshot: &SidebarSnapshot, id: &str) -> bool {
    fn in_tree(nodes: &[SidebarNode], id: &str) -> bool {
        nodes.iter().any(|n| match n {
            SidebarNode::Tab(t) => t.id == id,
            SidebarNode::Folder(f) => in_tree(&f.children, id),
        })
    }
    snapshot.spaces.iter().any(|s| in_tree(&s.pinned, id))
        || snapshot
            .favorites
            .iter()
            .any(|f| f.items.iter().any(|t| t.id == id))
}

/// Give space, folder, pinned tab or favorite `id` the name `title`. True
/// when that changed something.
fn rename(snapshot: &mut SidebarSnapshot, id: &str, title: &str) -> bool {
    fn set(name: &mut String, title: &str) -> bool {
        let changed = name != title;
        *name = title.to_string();
        changed
    }
    fn in_tree(nodes: &mut [SidebarNode], id: &str, title: &str) -> Option<bool> {
        nodes.iter_mut().find_map(|n| match n {
            SidebarNode::Tab(t) if t.id == id => {
                t.custom_title = true;
                Some(set(&mut t.title, title))
            }
            SidebarNode::Folder(f) if f.id == id => Some(set(&mut f.title, title)),
            SidebarNode::Folder(f) => in_tree(&mut f.children, id, title),
            SidebarNode::Tab(_) => None,
        })
    }
    for space in &mut snapshot.spaces {
        if space.id == id {
            return set(&mut space.title, title);
        }
        if let Some(changed) = in_tree(&mut space.pinned, id, title) {
            return changed;
        }
    }
    let favorite = snapshot
        .favorites
        .iter_mut()
        .flat_map(|f| f.items.iter_mut())
        .find(|t| t.id == id);
    favorite.is_some_and(|t| {
        t.custom_title = true;
        set(&mut t.title, title)
    })
}

/// Remove pinned tab `id` from the tree; folders stay.
fn remove_pinned(nodes: &mut Vec<SidebarNode>, id: &str) {
    nodes.retain(|n| !matches!(n, SidebarNode::Tab(t) if t.id == id));
    for node in nodes {
        if let SidebarNode::Folder(f) = node {
            remove_pinned(&mut f.children, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = r#"{
        "format": "magnetite-sidebar", "version": 1,
        "profiles": [{"id": "p", "name": "Me"}],
        "favorites": [{"profile_id": "p", "items": [{"id": "f1", "url": "https://a.example/", "title": "A"}]}],
        "spaces": [{"id": "s", "title": "Work", "profile_id": "p", "pinned": [
            {"kind": "folder", "id": "d", "title": "Docs", "children": [
                {"kind": "tab", "id": "t1", "url": "https://c.example/", "title": "C"}]}]}]
    }"#;

    fn snapshot() -> SidebarSnapshot {
        serde_json::from_str(SNAPSHOT).unwrap()
    }

    fn dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("magnetite-edits-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn pin(id: &str) -> Change {
        Change::Pin {
            space: "s".into(),
            item: id.into(),
            url: "https://new.example/".into(),
            title: "New".into(),
        }
    }

    #[test]
    fn the_format_is_the_one_the_desktop_reads() {
        let edit = Edit {
            id: "e".into(),
            made_at: 5,
            change: Change::Rename {
                item: "t1".into(),
                title: "Mine".into(),
            },
        };
        assert_eq!(
            serde_json::to_string(&edit).unwrap(),
            r#"{"id":"e","made_at":5,"op":"rename","item":"t1","title":"Mine"}"#
        );
    }

    #[test]
    fn an_edit_is_made_once_however_often_it_is_applied() {
        let mut s = snapshot();
        assert_eq!(apply(&mut s, &pin("n")), Some("n".into()));
        assert_eq!(apply(&mut s, &pin("n")), None);
        assert_eq!(s.spaces[0].pinned.len(), 2);
        let SidebarNode::Tab(tab) = &s.spaces[0].pinned[1] else {
            panic!()
        };
        assert_eq!(tab.home_url.as_deref(), Some("https://new.example/"));
    }

    #[test]
    fn renames_and_removals_reach_into_folders_and_favorites() {
        let mut s = snapshot();
        let rename = |item: &str| Change::Rename {
            item: item.into(),
            title: "Mine".into(),
        };
        assert!(apply(&mut s, &rename("t1")).is_some());
        assert!(apply(&mut s, &rename("f1")).is_some());
        assert!(apply(&mut s, &rename("d")).is_some());
        assert!(apply(&mut s, &rename("s")).is_some());
        assert!(apply(&mut s, &rename("gone")).is_none());
        assert_eq!(s.spaces[0].title, "Mine");
        assert!(s.favorites[0].items[0].custom_title);

        apply(&mut s, &Change::Remove { item: "t1".into() });
        apply(&mut s, &Change::Remove { item: "f1".into() });
        assert!(!find_tab(&s, "t1") && !find_tab(&s, "f1"));
        // The folder stays.
        assert!(matches!(&s.spaces[0].pinned[0], SidebarNode::Folder(f) if f.children.is_empty()));
    }

    #[test]
    fn an_edit_waits_listed_then_shown_until_the_sidebar_has_it() {
        let dir = dir("waits");
        propose(&dir, pin("n")).unwrap();
        let edit = pending(&dir).remove(0);

        let mut shown = snapshot();
        assert_eq!(overlay(&dir, &mut shown), HashSet::from(["n".to_string()]));

        // A desktop applied it: no longer listed, still shown.
        applied(&dir, std::slice::from_ref(&edit.id)).unwrap();
        assert!(pending(&dir).is_empty());
        let mut shown = snapshot();
        assert!(overlay(&dir, &mut shown).contains("n"));

        // An older sidebar keeps it; the one with the change drops it.
        settle(&dir, &snapshot());
        let mut synced = snapshot();
        apply(&mut synced, &edit.change);
        let mut shown = synced.clone();
        assert!(
            overlay(&dir, &mut shown).is_empty(),
            "the sidebar has it: nothing waits"
        );
        settle(&dir, &synced);
        assert!(load(&dir).applied.is_empty());
    }
}
