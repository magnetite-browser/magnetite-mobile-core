//! The sidebar as a phone shows it: per space its favorites, the pinned tree
//! as rows with a depth, and the Today tabs. What this phone changed and a
//! desktop has not made yet is in it, marked `pending` (`edits`).

use std::collections::HashSet;

use magnetite_core::{SidebarNode, SidebarSnapshot, TabItem};

/// A chain's sidebar at `version`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Sidebar {
    pub version: u64,
    pub spaces: Vec<Space>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Space {
    pub id: String,
    pub title: String,
    /// An emoji, or the name of an icon of the desktop's icon set.
    pub icon: Option<String>,
    pub accent: Option<Accent>,
    /// Pages of spaces with the same profile share cookies and logins.
    pub profile_id: String,
    /// The profile keeps nothing: logins are gone when the app quits.
    pub ephemeral: bool,
    /// The favorites of the space's profile.
    pub favorites: Vec<Page>,
    /// The pinned tree, depth first.
    pub pinned: Vec<Row>,
    pub today: Vec<Page>,
    /// Saved addresses, in folders, depth first. Not tabs: one opens as a
    /// new tab of the space and stays.
    pub bookmarks: Vec<Row>,
    /// Made or renamed on this phone; no desktop has made it yet.
    pub pending: bool,
}

/// sRGB, 0.0 to 1.0 per channel.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Accent {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Page {
    pub id: String,
    pub url: String,
    pub title: String,
    /// Added or renamed on this phone; no desktop has made it yet.
    pub pending: bool,
}

/// One row of the pinned tree; `depth` 0 is the top level.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Row {
    pub depth: u32,
    pub item: RowItem,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RowItem {
    /// The rows below with a greater depth are in the folder. A smart folder
    /// fills itself on the desktop; its items are not in the chain.
    Folder {
        id: String,
        title: String,
        smart: bool,
        /// Renamed on this phone; no desktop has made it yet.
        pending: bool,
    },
    Page {
        page: Page,
    },
}

/// A page a phone can open: web addresses only. `home` picks where pinned
/// tabs and favorites open: at their home address, not where the desktop
/// last left them.
fn page(tab: &TabItem, home: bool, waiting: &HashSet<String>) -> Option<Page> {
    let url = match &tab.home_url {
        Some(h) if home => h,
        _ => &tab.url,
    };
    let web = url.starts_with("https://") || url.starts_with("http://");
    web.then(|| Page {
        id: tab.id.clone(),
        url: url.clone(),
        title: tab.title.clone(),
        pending: waiting.contains(&tab.id),
    })
}

fn rows(nodes: &[SidebarNode], depth: u32, waiting: &HashSet<String>, out: &mut Vec<Row>) {
    for node in nodes {
        match node {
            SidebarNode::Tab(tab) => {
                if let Some(page) = page(tab, true, waiting) {
                    out.push(Row {
                        depth,
                        item: RowItem::Page { page },
                    });
                }
            }
            SidebarNode::Folder(folder) => {
                out.push(Row {
                    depth,
                    item: RowItem::Folder {
                        id: folder.id.clone(),
                        title: folder.title.clone(),
                        smart: folder.smart.is_some(),
                        pending: waiting.contains(&folder.id),
                    },
                });
                rows(&folder.children, depth + 1, waiting, out);
            }
        }
    }
}

/// `snapshot` at `version` as a phone shows it; `waiting`: the ids of what
/// this phone's edits added or renamed.
pub fn sidebar(snapshot: &SidebarSnapshot, version: u64, waiting: &HashSet<String>) -> Sidebar {
    let spaces = snapshot
        .spaces
        .iter()
        .map(|space| {
            let favorites = snapshot
                .favorites
                .iter()
                .filter(|set| set.profile_id == space.profile_id)
                .flat_map(|set| set.items.iter().filter_map(|t| page(t, true, waiting)))
                .collect();
            let (mut pinned, mut bookmarks) = (Vec::new(), Vec::new());
            rows(&space.pinned, 0, waiting, &mut pinned);
            rows(&space.bookmarks, 0, waiting, &mut bookmarks);
            Space {
                id: space.id.clone(),
                title: space.title.clone(),
                icon: space.icon.clone(),
                accent: space.accent.map(|c| {
                    let c = c.sanitised();
                    Accent {
                        red: c.red,
                        green: c.green,
                        blue: c.blue,
                    }
                }),
                profile_id: space.profile_id.clone(),
                ephemeral: snapshot
                    .profiles
                    .iter()
                    .any(|p| p.id == space.profile_id && p.ephemeral),
                favorites,
                pinned,
                today: space
                    .today
                    .iter()
                    .filter_map(|t| page(t, false, waiting))
                    .collect(),
                bookmarks,
                pending: waiting.contains(&space.id),
            }
        })
        .collect();
    Sidebar { version, spaces }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNAPSHOT: &str = r#"{
        "format": "magnetite-sidebar", "version": 1,
        "profiles": [{"id": "p", "name": "Me"}, {"id": "q", "name": "Gone", "ephemeral": true}],
        "favorites": [
            {"profile_id": "p", "items": [{"id": "f1", "url": "https://a.example/deep", "title": "A", "home_url": "https://a.example/"}]},
            {"profile_id": "q", "items": [{"id": "f2", "url": "https://b.example/", "title": "B"}]}
        ],
        "spaces": [
            {"id": "s", "title": "Work", "profile_id": "p",
             "pinned": [
                {"kind": "folder", "id": "d1", "title": "Docs", "children": [
                    {"kind": "folder", "id": "d2", "title": "Inner", "children": [
                        {"kind": "tab", "id": "t1", "url": "https://c.example/now", "title": "C", "home_url": "https://c.example/"}
                    ]},
                    {"kind": "tab", "id": "t2", "url": "file:///etc/hosts", "title": "Local"}
                ]},
                {"kind": "tab", "id": "t3", "url": "https://d.example/", "title": "D"}
             ],
             "today": [{"id": "t4", "url": "https://e.example/", "title": "E"}],
             "bookmarks": [
                {"kind": "folder", "id": "b1", "title": "Reading", "children": [
                    {"kind": "tab", "id": "b2", "url": "https://f.example/", "title": "F"}
                ]},
                {"kind": "tab", "id": "b3", "url": "javascript:alert(1)", "title": "Script"}
             ]},
            {"id": "x", "title": "Private", "profile_id": "q"}
        ]
    }"#;

    fn shown() -> Sidebar {
        sidebar(&serde_json::from_str(SNAPSHOT).unwrap(), 7, &HashSet::new())
    }

    #[test]
    fn pinned_tree_becomes_rows_with_depth_and_home_addresses() {
        let work = &shown().spaces[0];
        let seen: Vec<(u32, &str)> = work
            .pinned
            .iter()
            .map(|r| match &r.item {
                RowItem::Folder { title, .. } => (r.depth, title.as_str()),
                RowItem::Page { page } => (r.depth, page.url.as_str()),
            })
            .collect();
        // The local file is left out: a phone cannot open it.
        assert_eq!(
            seen,
            [
                (0, "Docs"),
                (1, "Inner"),
                (2, "https://c.example/"),
                (0, "https://d.example/")
            ]
        );
    }

    #[test]
    fn bookmarks_become_rows_of_web_pages() {
        let work = &shown().spaces[0];
        let seen: Vec<(u32, &str)> = work
            .bookmarks
            .iter()
            .map(|r| match &r.item {
                RowItem::Folder { title, .. } => (r.depth, title.as_str()),
                RowItem::Page { page } => (r.depth, page.url.as_str()),
            })
            .collect();
        assert_eq!(seen, [(0, "Reading"), (1, "https://f.example/")]);
    }

    #[test]
    fn a_space_shows_the_favorites_of_its_profile() {
        let s = shown();
        assert_eq!(s.spaces[0].favorites[0].url, "https://a.example/");
        assert_eq!(s.spaces[1].favorites[0].id, "f2");
        assert!(s.spaces[1].ephemeral && !s.spaces[0].ephemeral);
    }
}
