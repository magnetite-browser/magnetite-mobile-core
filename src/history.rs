//! The pages visited on this phone, to find them again: the address, title
//! and time of each visit, kept for a period the user chooses and never
//! synced. The same record as the desktop's (`docs/spec/history.md` in
//! magnetite), kept in the app's data folder as `history.json`.
//!
//! The app holds one `History` and calls it off the main thread, from any
//! thread. Visits are kept in memory and written at most once a minute, and
//! by `save` (the app going to the background); removing and clearing are
//! written at once. A file that cannot be read is started over, as on the
//! desktop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The most visits kept; the oldest go first.
const MOST: usize = 25_000;
/// The same page again within this many seconds updates its visit.
const SAME_VISIT: i64 = 60 * 60;
/// Visits are written at most this often, in seconds.
const WRITE_EVERY: i64 = 60;
const DAY: i64 = 24 * 60 * 60;

/// One visit of a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Visit {
    pub id: i64,
    pub url: String,
    /// The page's own title; empty while it has none (show its site).
    pub title: String,
    /// Seconds since 1970.
    pub at: i64,
}

#[derive(Serialize, Deserialize)]
struct Kept {
    /// How long visits stay; 0: none are kept.
    keep_days: u32,
    /// Oldest first.
    visits: Vec<Visit>,
    /// The id the next visit gets. Ids only go up, also across a clear, so
    /// that an id a list still holds never names another visit.
    #[serde(default)]
    next_id: i64,
}

impl Default for Kept {
    fn default() -> Self {
        Self {
            keep_days: 90,
            visits: Vec::new(),
            next_id: 1,
        }
    }
}

struct Inner {
    kept: Kept,
    /// Something changed since the last write.
    changed: bool,
    written_at: i64,
}

/// This phone's history, kept in `history.json` in its folder.
#[derive(uniffi::Object)]
pub struct History {
    path: PathBuf,
    inner: Mutex<Inner>,
}

#[uniffi::export]
impl History {
    /// The history kept in `dir`. A file that cannot be read is started
    /// over.
    #[uniffi::constructor]
    pub fn new(dir: String) -> Arc<Self> {
        let path = Path::new(&dir).join("history.json");
        let mut kept: Kept = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        let highest = kept.visits.iter().map(|v| v.id).max().unwrap_or(0);
        kept.next_id = kept.next_id.max(highest + 1);
        // What grew too old while the app was closed goes now.
        let changed = prune(&mut kept, now());
        Arc::new(Self {
            path,
            inner: Mutex::new(Inner {
                kept,
                changed,
                written_at: now(),
            }),
        })
    }

    /// The page at `url` is shown. Only web pages count; the same page again
    /// within an hour (a reload, another `#place`, a trailing slash)
    /// updates the visit already there, which moves to the top.
    pub fn visit(&self, url: String, title: String) {
        let mut inner = self.lock();
        if visit(&mut inner.kept, url, title, now()) {
            inner.changed = true;
        }
        drop(inner);
        self.write_if(|inner| now() - inner.written_at >= WRITE_EVERY);
    }

    /// The page at `url` named itself `title`: its latest visit takes it.
    /// An empty title (a page between two names) changes nothing.
    pub fn name(&self, url: String, title: String) {
        if title.is_empty() {
            return;
        }
        let mut inner = self.lock();
        let key = address(&url);
        if let Some(visit) = inner
            .kept
            .visits
            .iter_mut()
            .rev()
            .find(|v| address(&v.url) == key)
            && visit.title != title
        {
            visit.title = title;
            inner.changed = true;
        }
    }

    /// The visits whose title or address holds `query` (all for an empty
    /// one), newest first, at most `limit`.
    pub fn visits(&self, query: String, limit: u32) -> Vec<Visit> {
        let query = query.trim().to_lowercase();
        let mut inner = self.lock();
        inner.prune();
        inner
            .kept
            .visits
            .iter()
            .rev()
            .filter(|v| {
                query.is_empty()
                    || v.title.to_lowercase().contains(&query)
                    || v.url.to_lowercase().contains(&query)
            })
            .take(limit as usize)
            .cloned()
            .collect()
    }

    /// The pages that match what is typed, for the address field: best
    /// match first (as the apps rank tabs and the chain's pages), then the
    /// most visited, then the newest. Each address once, as last visited.
    pub fn suggest(&self, query: String, limit: u32) -> Vec<Visit> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut inner = self.lock();
        inner.prune();
        // Per address: its latest visit and how many there are.
        let mut pages: Vec<(&Visit, usize)> = Vec::new();
        let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for visit in inner.kept.visits.iter().rev() {
            match index.get(&address(&visit.url)) {
                Some(&i) => pages[i].1 += 1,
                None => {
                    index.insert(address(&visit.url), pages.len());
                    pages.push((visit, 1));
                }
            }
        }
        let mut found: Vec<(&Visit, usize, u8)> = pages
            .into_iter()
            .filter_map(|(v, count)| score(&query, &v.title, &v.url).map(|s| (v, count, s)))
            .collect();
        found.sort_by_key(|&(v, count, score)| std::cmp::Reverse((score, count, v.at)));
        found
            .into_iter()
            .take(limit as usize)
            .map(|(v, ..)| v.clone())
            .collect()
    }

    /// Remove visit `id`.
    pub fn remove(&self, id: i64) {
        self.lock().kept.visits.retain(|v| v.id != id);
        self.write_if(|_| true);
    }

    /// Remove the visits from `since` on (seconds since 1970), or all.
    pub fn clear(&self, since: Option<i64>) {
        self.lock()
            .kept
            .visits
            .retain(|v| since.is_some_and(|since| v.at < since));
        self.write_if(|_| true);
    }

    /// How many days visits stay; 0: none are kept.
    pub fn keep_days(&self) -> u32 {
        self.lock().kept.keep_days
    }

    /// Keep visits for `days` from now on; older ones go at once.
    pub fn set_keep_days(&self, days: u32) {
        self.lock().kept.keep_days = days;
        self.write_if(|_| true);
    }

    /// Write what changed (the app goes to the background).
    pub fn save(&self) {
        self.write_if(|inner| inner.changed);
    }
}

impl History {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drop what is too old, then write when `due`, under the lock: calls
    /// from several threads never write an older state last, or into the
    /// same file at once.
    fn write_if(&self, due: impl Fn(&Inner) -> bool) {
        let mut inner = self.lock();
        if !due(&inner) {
            return;
        }
        inner.prune();
        let Ok(data) = serde_json::to_vec(&inner.kept) else {
            return;
        };
        inner.written_at = now();
        // Tried again with the next change when it fails.
        inner.changed = write(&self.path, &data).is_err();
    }
}

impl Inner {
    /// Drop what grew too old; what goes is written with the next write.
    fn prune(&mut self) {
        if prune(&mut self.kept, now()) {
            self.changed = true;
        }
    }
}

/// Note a visit of `url` at `at`; false when nothing is kept.
fn visit(kept: &mut Kept, url: String, title: String, at: i64) -> bool {
    if kept.keep_days == 0 || !(url.starts_with("https://") || url.starts_with("http://")) {
        return false;
    }
    let key = address(&url);
    // Not across a clock set back: a visit "after" it is no reload.
    let recent = (kept.visits.iter())
        .rposition(|v| address(&v.url) == key && (0..SAME_VISIT).contains(&(at - v.at)));
    let (id, title) = match recent.map(|i| kept.visits.remove(i)) {
        // A title that is still to come stays the one there was.
        Some(before) if title.is_empty() => (before.id, before.title),
        Some(before) => (before.id, title),
        None => {
            kept.next_id += 1;
            (kept.next_id - 1, title)
        }
    };
    kept.visits.push(Visit { id, url, title, at });
    if kept.visits.len() > MOST {
        kept.visits.drain(..kept.visits.len() - MOST);
    }
    true
}

/// Drop visits older than the period kept; all of them with none kept.
/// True when any went.
fn prune(kept: &mut Kept, now: i64) -> bool {
    let before = kept.visits.len();
    if kept.keep_days == 0 {
        kept.visits.clear();
    } else {
        let oldest = now - i64::from(kept.keep_days) * DAY;
        kept.visits.retain(|v| v.at > oldest);
    }
    kept.visits.len() != before
}

/// The address as visits compare it: without `#place`, and without a
/// trailing slash on its path (a query keeps its own).
fn address(url: &str) -> String {
    let url = url.split_once('#').map_or(url, |(before, _)| before);
    let (path, query) = url
        .split_once('?')
        .map_or((url, None), |(p, q)| (p, Some(q)));
    let path = path.strip_suffix('/').unwrap_or(path);
    match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_string(),
    }
}

/// How well `query` (lowercased) matches a page, as the apps rank: its site
/// starting with it is best, then a word of its title, then any part of
/// either, for three letters or more.
fn score(query: &str, title: &str, url: &str) -> Option<u8> {
    let title = title.to_lowercase();
    if site(url).to_lowercase().starts_with(query) {
        return Some(3);
    }
    if title
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        return Some(2);
    }
    let contains = title.contains(query) || url.to_lowercase().contains(query);
    (query.chars().count() >= 3 && contains).then_some(1)
}

/// The host without `www.`.
fn site(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    host.strip_prefix("www.").unwrap_or(host)
}

/// Write `data` whole, readable by its owner only.
fn write(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = path.with_extension("json.part");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(&part)?.write_all(data)?;
    std::fs::rename(part, path)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(kept: &Kept) -> Vec<(&str, &str)> {
        kept.visits
            .iter()
            .map(|v| (v.url.as_str(), v.title.as_str()))
            .collect()
    }

    #[test]
    fn the_same_page_within_an_hour_is_one_visit_moved_to_the_top() {
        let mut kept = Kept::default();
        visit(&mut kept, "https://a.example/".into(), "A".into(), 0);
        visit(&mut kept, "https://b.example/".into(), "B".into(), 10);
        // A reload, another place on the page, no trailing slash.
        assert!(visit(
            &mut kept,
            "https://a.example#top".into(),
            String::new(),
            20
        ));
        assert_eq!(
            titles(&kept),
            [("https://b.example/", "B"), ("https://a.example#top", "A")]
        );
        assert_eq!(kept.visits[1].id, 1, "the visit keeps its id");
        // An hour later it is a visit of its own.
        visit(
            &mut kept,
            "https://a.example/".into(),
            "A".into(),
            20 + SAME_VISIT,
        );
        assert_eq!(kept.visits.len(), 3);
    }

    #[test]
    fn only_web_pages_are_kept_and_nothing_when_history_is_off() {
        let mut kept = Kept::default();
        assert!(!visit(
            &mut kept,
            "file:///etc/passwd".into(),
            String::new(),
            0
        ));
        assert!(!visit(&mut kept, "about:blank".into(), String::new(), 0));
        kept.keep_days = 0;
        assert!(!visit(
            &mut kept,
            "https://a.example/".into(),
            String::new(),
            0
        ));
        assert!(kept.visits.is_empty());
    }

    #[test]
    fn ids_are_never_given_twice_and_a_query_keeps_its_slash() {
        let mut kept = Kept::default();
        visit(&mut kept, "https://a.example/".into(), String::new(), 0);
        visit(&mut kept, "https://b.example/".into(), String::new(), 1);
        kept.visits.clear();
        visit(&mut kept, "https://c.example/".into(), String::new(), 2);
        assert_eq!(kept.visits[0].id, 3, "not an id a list may still hold");

        visit(
            &mut kept,
            "https://s.example/?q=a/".into(),
            String::new(),
            3,
        );
        visit(&mut kept, "https://s.example/?q=a".into(), String::new(), 4);
        assert_eq!(kept.visits.len(), 3, "two searches, two visits");
        // A clock set back: no reload of the visit "after" it.
        visit(&mut kept, "https://c.example".into(), String::new(), 1);
        assert_eq!(kept.visits.len(), 4);
    }

    #[test]
    fn with_history_off_no_visit_stays_even_one_from_the_future() {
        let mut kept = Kept::default();
        visit(
            &mut kept,
            "https://a.example/".into(),
            String::new(),
            100 * DAY,
        );
        kept.keep_days = 0;
        assert!(prune(&mut kept, 0));
        assert!(kept.visits.is_empty());
    }

    #[test]
    fn visits_older_than_the_period_go() {
        let mut kept = Kept {
            keep_days: 7,
            ..Kept::default()
        };
        visit(&mut kept, "https://old.example/".into(), String::new(), 0);
        visit(
            &mut kept,
            "https://new.example/".into(),
            String::new(),
            3 * DAY,
        );
        prune(&mut kept, 8 * DAY);
        assert_eq!(titles(&kept), [("https://new.example/", "")]);
    }

    #[test]
    fn suggestions_rank_the_match_then_how_often_then_how_recent() {
        let history = History {
            path: PathBuf::new(),
            inner: Mutex::new(Inner {
                kept: Kept::default(),
                changed: false,
                written_at: 0,
            }),
        };
        {
            let kept = &mut history.lock().kept;
            let hour = SAME_VISIT;
            // Today's visits: older ones would go with the period.
            let start = now() - 6 * hour;
            visit(kept, "https://wiki.example/a".into(), "Alps".into(), start);
            visit(
                kept,
                "https://news.example/".into(),
                "Wikinews".into(),
                start + hour,
            );
            visit(
                kept,
                "https://www.wikipedia.org/".into(),
                "Wikipedia".into(),
                start + 2 * hour,
            );
            visit(
                kept,
                "https://wiki.example/a".into(),
                "Alps".into(),
                start + 3 * hour,
            );
            visit(
                kept,
                "https://other.example/wiki".into(),
                "Other".into(),
                start + 5 * hour,
            );
        }
        let urls = |query: &str| -> Vec<String> {
            history
                .suggest(query.into(), 10)
                .into_iter()
                .map(|v| v.url)
                .collect()
        };
        assert_eq!(
            urls("wiki"),
            [
                // Site matches: visited twice, then the newer one.
                "https://wiki.example/a",
                "https://www.wikipedia.org/",
                // A word of the title.
                "https://news.example/",
                // Somewhere in the address.
                "https://other.example/wiki",
            ]
        );
        assert!(
            urls("ki").is_empty(),
            "two letters inside a word are no match"
        );
    }

    #[test]
    fn the_file_is_read_back_and_clearing_since_keeps_what_is_older() {
        let dir = std::env::temp_dir().join(format!("history-{}", std::process::id()));
        let history = History::new(dir.to_string_lossy().into());
        history.visit("https://a.example/".into(), String::new());
        history.name("https://a.example/".into(), "A".into());
        history.save();
        let again = History::new(dir.to_string_lossy().into());
        let visits = again.visits(String::new(), 10);
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].title, "A");
        again.clear(Some(visits[0].at + 1));
        assert_eq!(again.visits("a.example".into(), 10).len(), 1);
        again.clear(None);
        assert!(
            History::new(dir.to_string_lossy().into())
                .visits(String::new(), 10)
                .is_empty()
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
