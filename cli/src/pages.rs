//! Saved search pages. Every page fetched from Wallhaven is kept, so browsing, `next` and
//! `apply` keep working from the last good copy while the API is down.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Result;
use crate::state;
use crate::sync::Clock;
use crate::wallhaven::{self, Item, Query, SearchPage, Transport};

/// Wait before the one retry after a transient failure.
pub const RETRY_WAIT: Duration = Duration::from_secs(2);

fn path(dir: &Path, query_key: &str, page: u64) -> PathBuf {
    let folder: String = query_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(folder).join(format!("{page}.json"))
}

pub fn save(dir: &Path, query_key: &str, page: u64, result: &SearchPage) -> Result<()> {
    state::write_json(&path(dir, query_key, page), result)
}

pub fn load(dir: &Path, query_key: &str, page: u64) -> Option<SearchPage> {
    let bytes = fs::read(path(dir, query_key, page)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Runs `call`, once more after `RETRY_WAIT` when it fails transiently.
pub fn retry<T>(clock: &dyn Clock, mut call: impl FnMut() -> Result<T>) -> Result<T> {
    match call() {
        Err(e) if wallhaven::is_transient(&e) => {
            clock.sleep(RETRY_WAIT);
            call()
        }
        other => other,
    }
}

/// A search page from Wallhaven, saved on success; when Wallhaven fails transiently, the
/// saved copy instead. The flag is true when the page came from disk.
pub fn search(
    transport: &dyn Transport,
    clock: &dyn Clock,
    dir: &Path,
    query: &Query,
    page: u64,
) -> Result<(SearchPage, bool)> {
    match retry(clock, || wallhaven::search(transport, query, page)) {
        Ok(found) => {
            // A cache that cannot be written only costs the offline fallback.
            let _ = save(dir, &query.key(), page, &found);
            Ok((found, false))
        }
        Err(e) if wallhaven::is_transient(&e) => load(dir, &query.key(), page)
            .map(|saved| (saved, true))
            .ok_or(e),
        Err(e) => Err(e),
    }
}

/// The saved entry for `id`, from any saved page.
pub fn find_item(dir: &Path, id: &str) -> Option<Item> {
    let folders = fs::read_dir(dir).ok()?;
    for folder in folders.flatten() {
        let Ok(files) = fs::read_dir(folder.path()) else {
            continue;
        };
        for file in files.flatten() {
            let Ok(bytes) = fs::read(file.path()) else {
                continue;
            };
            let Ok(page) = serde_json::from_slice::<SearchPage>(&bytes) else {
                continue;
            };
            if let Some(item) = page.items.into_iter().find(|item| item.id == id) {
                return Some(item);
            }
        }
    }
    None
}

/// `detail` with one retry, else the entry from a saved page.
pub fn detail(transport: &dyn Transport, clock: &dyn Clock, dir: &Path, id: &str) -> Result<Item> {
    match retry(clock, || wallhaven::detail(transport, id)) {
        Err(e) if wallhaven::is_transient(&e) => find_item(dir, id).ok_or(e),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeClock, FakeTransport, temp_dir};
    use crate::wallhaven::tests::{item_json, search_json};

    fn query() -> Query {
        Query {
            tag: 711,
            categories: "100",
            color: None,
        }
    }

    #[test]
    fn a_fetched_page_is_saved_and_served_while_wallhaven_is_down() {
        let dir = temp_dir("pages-fallback");
        let t = FakeTransport::default();
        t.add(&query().url(1), 200, search_json(&["aa1", "aa2"], 3, 70));
        let clock = FakeClock::default();
        let (fresh, cached) = search(&t, &clock, &dir, &query(), 1).unwrap();
        assert!(!cached);
        assert_eq!(load(&dir, &query().key(), 1).unwrap(), fresh);

        t.add(&query().url(1), 502, Vec::new());
        let (saved, cached) = search(&t, &clock, &dir, &query(), 1).unwrap();
        assert!(cached);
        assert_eq!(saved, fresh);
        // One retry after a short wait before falling back.
        assert_eq!(clock.sleeps(), [RETRY_WAIT]);
        assert_eq!(t.calls().len(), 3);
    }

    #[test]
    fn without_a_saved_page_the_error_stands() {
        let dir = temp_dir("pages-missing");
        let t = FakeTransport::default();
        t.add(&query().url(2), 525, Vec::new());
        let err = search(&t, &FakeClock::default(), &dir, &query(), 2).unwrap_err();
        assert_eq!(err.code, "unavailable");
    }

    #[test]
    fn a_request_error_is_not_retried() {
        let dir = temp_dir("pages-404");
        let t = FakeTransport::default();
        t.add(&query().url(1), 404, Vec::new());
        let clock = FakeClock::default();
        assert_eq!(
            search(&t, &clock, &dir, &query(), 1).unwrap_err().code,
            "http"
        );
        assert!(clock.sleeps().is_empty());
        assert_eq!(t.calls().len(), 1);
    }

    #[test]
    fn detail_falls_back_to_a_saved_entry() {
        let dir = temp_dir("pages-detail");
        let t = FakeTransport::default();
        t.add(&query().url(1), 200, search_json(&["aa1", "bb2"], 1, 2));
        search(&t, &FakeClock::default(), &dir, &query(), 1).unwrap();
        t.add("https://wallhaven.cc/api/v1/w/bb2", 502, Vec::new());
        let item = detail(&t, &FakeClock::default(), &dir, "bb2").unwrap();
        assert_eq!(item.full_url, item_json("bb2")["path"].as_str().unwrap());
        // Down and never saved: the API's error stands.
        t.add("https://wallhaven.cc/api/v1/w/zz9", 502, Vec::new());
        assert_eq!(
            detail(&t, &FakeClock::default(), &dir, "zz9")
                .unwrap_err()
                .code,
            "unavailable"
        );
    }
}
