//! Content blocking on the phone, with the filter lists the desktop offers
//! (`docs/spec/ad-blocking.md` in magnetite): downloaded from their
//! publishers into `filter-lists/` in the app's folder, refreshed after
//! four days, and turned into what the app's web engine takes. On iOS that
//! is WebKit's content rules (`webkit_content_rules`); as on the desktop,
//! only the lists' network rules count.
//!
//! The list table, the download checks and the reading of `!#if` sections
//! repeat the desktop's `crates/app/src/adblock.rs`; the two should become
//! one crate. Every function blocks: call them off the main thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use adblock::lists::{FilterSet, ParseOptions, RuleTypes};

/// A list the apps offer.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FilterList {
    /// As the desktop names it: "easylist", "ublock"…
    pub id: String,
    pub name: String,
    /// When this device last downloaded it, in seconds since 1970.
    pub fetched_at: Option<i64>,
}

/// What `update_filter_lists` did.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ListsUpdate {
    /// A list came, or went: the rules are to be made again.
    pub changed: bool,
    /// Why a download failed (the copy from before stays). The other lists
    /// are still downloaded.
    pub error: Option<String>,
}

struct Offered {
    id: &'static str,
    name: &'static str,
    /// Kept as one list: all of them come, or none.
    urls: &'static [&'static str],
}

/// The desktop's lists. Only whole files: a list that includes others
/// would need those fetched as well.
const LISTS: &[Offered] = &[
    Offered {
        id: "easylist",
        name: "EasyList",
        urls: &["https://easylist.to/easylist/easylist.txt"],
    },
    Offered {
        id: "easyprivacy",
        name: "EasyPrivacy",
        urls: &["https://easylist.to/easylist/easyprivacy.txt"],
    },
    Offered {
        id: "easylist-germany",
        name: "EasyList Germany",
        urls: &["https://easylist.to/easylistgermany/easylistgermany.txt"],
    },
    Offered {
        id: "peter-lowe",
        name: "Peter Lowe's list",
        urls: &[
            "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext",
        ],
    },
    Offered {
        id: "ublock",
        name: "uBlock Origin's lists",
        urls: &[
            "https://ublockorigin.github.io/uAssetsCDN/filters/filters.min.txt",
            "https://ublockorigin.github.io/uAssetsCDN/filters/badware.min.txt",
            "https://ublockorigin.github.io/uAssetsCDN/filters/privacy.min.txt",
            "https://ublockorigin.github.io/uAssetsCDN/filters/quick-fixes.min.txt",
            "https://ublockorigin.github.io/uAssetsCDN/filters/unbreak.min.txt",
        ],
    },
];

/// A list is fetched again after this long: what EasyList asks for.
const REFRESH_AFTER: Duration = Duration::from_secs(4 * 24 * 3600);
/// The largest list taken (EasyList is about 2 MB).
const MAX_LIST: u64 = 16 << 20;

/// The lists offered, with when this device last downloaded each.
#[uniffi::export]
pub fn filter_lists(dir: String) -> Vec<FilterList> {
    let dir = lists_dir(Path::new(&dir));
    LISTS
        .iter()
        .map(|l| FilterList {
            id: l.id.into(),
            name: l.name.into(),
            fetched_at: fetched_at(&dir, l.id).map(|at| {
                let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
                i64::try_from(secs).unwrap_or(i64::MAX)
            }),
        })
        .collect()
}

/// The lists a new installation blocks with, as on the desktop: what uBlock
/// Origin starts with, and the German list where the system speaks German
/// (`language`: its language tag, as "de-AT").
#[uniffi::export]
pub fn recommended_filter_lists(language: Option<String>) -> Vec<String> {
    let german = language.is_some_and(|l| l.to_ascii_lowercase().starts_with("de"));
    (["easylist", "easyprivacy", "ublock"].into_iter())
        .chain(german.then_some("easylist-germany"))
        .map(str::to_string)
        .collect()
}

/// Download the lists `ids` that are missing or older than four days, and
/// delete the lists not in `ids`. The download is the only request: its
/// publisher sees the address and "Magnetite" as the client.
#[uniffi::export]
pub fn update_filter_lists(dir: String, ids: Vec<String>) -> ListsUpdate {
    let dir = lists_dir(Path::new(&dir));
    let mut changed = false;
    for list in LISTS.iter().filter(|l| !ids.iter().any(|id| id == l.id)) {
        changed |= std::fs::remove_file(path(&dir, list.id)).is_ok();
    }
    let mut error = None;
    let now = SystemTime::now();
    for list in LISTS.iter().filter(|l| ids.iter().any(|id| id == l.id)) {
        if !due(&dir, list.id, now) {
            continue;
        }
        match fetch(&dir, list) {
            Ok(()) => changed = true,
            Err(e) => {
                error.get_or_insert(format!("{}: {e}", list.name));
            }
        }
    }
    ListsUpdate { changed, error }
}

/// WebKit's content rules for the lists `ids` that are here, as JSON for
/// `WKContentRuleListStore`; `None` without any. One set of rules for all
/// lists, so that one list's exceptions (`@@`) apply to the others' rules,
/// as they do on the desktop: WebKit applies an exception only within its
/// own rule list.
#[uniffi::export]
pub fn webkit_content_rules(dir: String, ids: Vec<String>) -> Option<String> {
    let dir = lists_dir(Path::new(&dir));
    let mut set = FilterSet::new(true);
    let mut any = false;
    for id in ids.iter().filter(|id| LISTS.iter().any(|l| l.id == *id)) {
        let Ok(text) = std::fs::read_to_string(path(&dir, id)) else {
            continue;
        };
        let options = ParseOptions {
            rule_types: RuleTypes::NetworkOnly,
            ..ParseOptions::default()
        };
        set.add_filter_list(kept_for(&text, &["env_safari", "env_mobile"]), options);
        any = true;
    }
    let (rules, _) = set.into_content_blocking().ok()?;
    (any && !rules.is_empty())
        .then(|| serde_json::to_string(&rules).ok())
        .flatten()
}

fn lists_dir(dir: &Path) -> PathBuf {
    dir.join("filter-lists")
}

fn path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.txt"))
}

fn fetched_at(dir: &Path, id: &str) -> Option<SystemTime> {
    std::fs::metadata(path(dir, id))
        .and_then(|m| m.modified())
        .ok()
}

/// Whether list `id` is missing or older than `REFRESH_AFTER` at `now`. A
/// list from the future (the clock was changed) is no older than now.
fn due(dir: &Path, id: &str, now: SystemTime) -> bool {
    fetched_at(dir, id).is_none_or(|at| now.duration_since(at).unwrap_or_default() >= REFRESH_AFTER)
}

/// Download `list` and keep it: all of its files, or none. The copy from
/// before stays when this fails.
fn fetch(dir: &Path, list: &Offered) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent("Magnetite")
        .build()
        .into();
    let mut text = Vec::new();
    for url in list.urls {
        text.extend(fetch_file(&agent, url)?);
        text.push(b'\n');
    }
    crate::files::write_private(&path(dir, list.id), &text).map_err(|e| e.to_string())
}

fn fetch_file(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let mut res = agent
        .get(url)
        .call()
        .map_err(|e| format!("cannot be downloaded: {e}"))?;
    let html = (res.headers().get("content-type"))
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.trim_start().to_ascii_lowercase().starts_with("text/html"));
    let mut body = Vec::new();
    res.body_mut()
        .as_reader()
        .take(MAX_LIST + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("the download broke off: {e}"))?;
    check(&body, html)?;
    Ok(body)
}

/// Whether a download looks like a filter list: text, not too large, and
/// not a web page (`html`: served as one), such as the sign-in page of a
/// hotel's Wi-Fi. No list starts with `<`.
fn check(body: &[u8], html: bool) -> Result<(), String> {
    if body.len() as u64 > MAX_LIST {
        return Err("it is too large for a filter list".into());
    }
    let text = std::str::from_utf8(body).map_err(|_| "it is not a filter list")?;
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if html || text.starts_with('<') || text.is_empty() {
        return Err("it is not a filter list".into());
    }
    Ok(())
}

/// A list's text without the parts meant for other browsers. uBlock
/// Origin's lists mark those with `!#if <condition>`, `!#else` and
/// `!#endif`, which the filter engine would read as comments and so take
/// both parts. A condition is a name, or `!` and a name; the names in
/// `holding` hold, no other does (none of the `cap_` ones: content rules
/// neither change a page's HTML nor match IP addresses).
fn kept_for(text: &str, holding: &[&str]) -> String {
    let holds = |condition: &str| {
        let (negated, name) = match condition.trim().strip_prefix('!') {
            Some(name) => (true, name.trim()),
            None => (false, condition.trim()),
        };
        holding.contains(&name) != negated
    };
    // Per open `!#if`: whether its lines are kept, the parts around it too.
    let mut kept: Vec<bool> = Vec::new();
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let outer = kept.iter().rev().nth(1).copied().unwrap_or(true);
        if let Some(condition) = line.strip_prefix("!#if ") {
            let here = kept.last().copied().unwrap_or(true);
            kept.push(here && holds(condition));
        } else if line.trim_end() == "!#else" {
            if let Some(last) = kept.last_mut() {
                *last = outer && !*last;
            }
        } else if line.trim_end() == "!#endif" {
            kept.pop();
        } else if kept.last().copied().unwrap_or(true) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_that_is_no_web_page_counts_as_a_list() {
        let list = b"[Adblock Plus 2.0]\n||ads.example^\n";
        assert!(check(list, false).is_ok());
        assert!(check(list, true).is_err(), "served as a web page");
        assert!(check(b"  <!DOCTYPE html><html>Sign in to the Wi-Fi</html>", false).is_err());
        assert!(check("\u{feff}<html><body>".as_bytes(), false).is_err());
        assert!(check(b"\n \n", false).is_err());
    }

    #[test]
    fn parts_for_other_browsers_are_left_out() {
        let text = "a\n!#if env_firefox\nfirefox\n!#else\nnot-firefox\n!#endif\n\
                    !#if !env_mobile\ndesktop\n!#endif\n\
                    !#if env_safari\nsafari\n!#if cap_html_filtering\nhtml\n!#endif\n!#endif\nz\n";
        assert_eq!(
            kept_for(text, &["env_safari", "env_mobile"]),
            "a\nnot-firefox\nsafari\nz\n"
        );
    }

    #[test]
    fn the_lists_here_become_one_set_of_webkit_rules() {
        let dir = std::env::temp_dir().join(format!("blocking-{}", std::process::id()));
        let lists = lists_dir(&dir);
        std::fs::create_dir_all(&lists).unwrap();
        std::fs::write(
            path(&lists, "easylist"),
            "||ads.example^\n/ad-choices.png$image\n",
        )
        .unwrap();
        // An exception in another list undoes the first list's rule.
        std::fs::write(path(&lists, "ublock"), "@@||ads.example/ok.js\n").unwrap();
        let ids = |ids: &[&str]| ids.iter().map(ToString::to_string).collect::<Vec<_>>();
        let dir_s = dir.to_string_lossy().to_string();

        let json = webkit_content_rules(dir_s.clone(), ids(&["easylist", "ublock"])).unwrap();
        let rules: serde_json::Value = serde_json::from_str(&json).unwrap();
        let actions: Vec<&str> = (rules.as_array().unwrap().iter())
            .map(|r| r["action"]["type"].as_str().unwrap())
            .collect();
        // The exceptions after the rules they undo.
        assert_eq!(actions[..2], ["block", "block"]);
        assert!(actions[2..].iter().all(|a| *a == "ignore-previous-rules"));
        assert!(webkit_content_rules(dir_s.clone(), ids(&["easyprivacy"])).is_none());

        // A list no longer chosen goes.
        let update = update_filter_lists(dir_s.clone(), Vec::new());
        assert!(update.changed && update.error.is_none());
        assert!(filter_lists(dir_s).iter().all(|l| l.fetched_at.is_none()));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn german_systems_get_the_german_list_too() {
        let german = recommended_filter_lists(Some("de-AT".into()));
        assert!(german.contains(&"easylist-germany".to_string()));
        assert!(!recommended_filter_lists(None).contains(&"easylist-germany".to_string()));
        assert!(
            LISTS
                .iter()
                .all(|l| l.urls.iter().all(|u| u.starts_with("https://")))
        );
    }
}
