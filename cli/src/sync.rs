//! `omawall sync`: downloads every thumbnail of one tag source, outside the main lock.
//! Each query (source + filters) keeps its own record in `syncs.json`: a finished one is not
//! walked again, and an interrupted one resumes at the page it stopped on.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{App, Output, now, output};
use crate::cache;
use crate::config::{self, Source};
use crate::error::{AppError, Result};
use crate::image;
use crate::pages;
use crate::paths::Paths;
use crate::state;
use crate::wallhaven::{self, Query, SearchPage};

pub const SYNC_LOCK: &str = "sync.lock";
const CANCEL_FILE: &str = "sync.cancel";
const SYNC_FILE: &str = "syncs.json";

/// Minimum gap between two API requests, so `next` and `browse` keep rate-limit headroom.
pub const API_SPACING: Duration = Duration::from_secs(2);
pub const RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);
pub const RATE_LIMIT_RETRIES: u32 = 3;
pub const NETWORK_WAIT: Duration = Duration::from_secs(5);

/// Thumbnail failures that only skip that thumbnail.
const SKIPPABLE: [&str; 5] = ["http", "invalid_image", "too_large", "network", "timeout"];

/// Monotonic time and sleeping, injectable so pacing and retries are testable.
pub trait Clock {
    fn elapsed(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

pub struct SystemClock(Instant);

impl SystemClock {
    pub fn new() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncError {
    pub code: String,
    pub message: String,
}

/// One query's record in `syncs.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    pub source: String,
    pub query_key: String,
    pub state: String,
    pub done: u64,
    pub skipped: u64,
    pub total: Option<u64>,
    /// The page being walked (1 before the first page arrives).
    pub page: u64,
    pub last_page: Option<u64>,
    /// `done` and `skipped` before `page`, so a resumed sync can walk `page` again and count right.
    pub page_done: u64,
    pub page_skipped: u64,
    pub error: Option<SyncError>,
    pub started_at: u64,
    pub updated_at: u64,
    /// The saved pages carry each wallpaper's category and colors, so browse filters them
    /// locally. Records from before 0.3.4 lack it and are walked again.
    #[serde(default)]
    pub filters: bool,
}

/// `syncs.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Syncs {
    pub version: u32,
    pub queries: BTreeMap<String, SyncState>,
}

/// Every record, with `running` turned into `stopped` when no sync process holds the lock,
/// and a finished record without `filters` shown as `stopped`, so it can be synced again.
pub fn records(paths: &Paths) -> Result<Syncs> {
    let mut syncs = state::read_json::<Syncs>(&paths.state(SYNC_FILE))?;
    let locked = state::is_locked(&paths.state(SYNC_LOCK));
    for record in syncs.queries.values_mut() {
        if (record.state == "running" && !locked) || (record.state == "done" && !record.filters) {
            record.state = "stopped".into();
        }
    }
    Ok(syncs)
}

/// The query and last page of `source`'s finished sync, when browse can filter its saved pages.
pub fn local_pages(paths: &Paths, source: &Source) -> Result<Option<(Query, u64)>> {
    let query = query_for(source);
    let last_page = records(paths)?
        .queries
        .get(&query.key())
        .filter(|record| record.state == "done")
        .and_then(|record| record.last_page);
    Ok(last_page.map(|last| (query, last)))
}

/// `status.sync`: the running record, or `null`.
pub fn running_view(syncs: &Syncs) -> Value {
    syncs
        .queries
        .values()
        .filter(|record| record.state == "running")
        .max_by_key(|record| record.updated_at)
        .map_or(Value::Null, |record| json!(record))
}

/// `status.sources[].sync`: the record for that query key, or `null`.
pub fn source_view(syncs: &Syncs, query_key: &str) -> Value {
    syncs
        .queries
        .get(query_key)
        .map_or(Value::Null, |record| json!(record))
}

/// `sync --cancel`: asks a running sync to stop.
pub fn cancel(paths: &Paths) -> Result<Output> {
    let running = state::is_locked(&paths.state(SYNC_LOCK));
    if running {
        state::write_atomic(&paths.state(CANCEL_FILE), b"")?;
    }
    Ok(output(json!({"running": running})))
}

/// The query a sync walks: the whole category, both styles and any color, so changing the
/// filters never makes a finished sync look stale. Every filtered view is a subset of it.
pub fn query_for(source: &Source) -> Query {
    Query {
        tag: source.tag,
        categories: "110",
        color: None,
    }
}

/// `sync <sourceKey>`.
pub fn sync(app: &App, key: &str) -> Result<Output> {
    let sources = config::load_sources(&app.paths.config_file)?;
    // `favs` is never a configured key, so it lands here too.
    let source = sources
        .iter()
        .find(|src| src.key == key)
        .ok_or_else(|| AppError::usage(format!("Unknown source: {key}")))?;
    let _lock = state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, app.sync_lock_wait)?;
    let _ = fs::remove_file(app.paths.state(CANCEL_FILE));
    let record = sync_source(app, source, &mut None)?;
    Ok(output(json!({"sync": record})))
}

/// `sync --all`: every configured source in order under one lock. Finished ones are skipped;
/// a cancel or a failure stops the run, and the next run resumes where it stopped.
pub fn sync_all(app: &App) -> Result<Output> {
    let sources = config::load_sources(&app.paths.config_file)?;
    let _lock = state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, app.sync_lock_wait)?;
    let _ = fs::remove_file(app.paths.state(CANCEL_FILE));
    // Pacing carries over from one source to the next.
    let mut last_api = None;
    let mut records = Vec::new();
    for source in &sources {
        let record = sync_source(app, source, &mut last_api)?;
        let canceled = record.state == "canceled";
        records.push(record);
        if canceled {
            break;
        }
    }
    Ok(output(json!({"syncs": records})))
}

/// Syncs one source; the caller holds the sync lock. Returns its record, or the error after
/// recording it as `failed`.
fn sync_source(app: &App, source: &Source, last_api: &mut Option<Duration>) -> Result<SyncState> {
    let query = query_for(source);
    // The lock is ours, so any `running` record left behind belongs to a dead process.
    // A record from before `filters` starts over: its saved pages cannot be filtered.
    let earlier = records(&app.paths)?
        .queries
        .remove(&query.key())
        .filter(|record| record.filters);
    if let Some(done) = earlier.clone().filter(|record| record.state == "done") {
        return Ok(done);
    }
    let started = now();
    let (page, done, skipped) = earlier.as_ref().map_or((1, 0, 0), |record| {
        (record.page, record.page_done, record.page_skipped)
    });
    let mut run = Run {
        app,
        path: app.paths.state(SYNC_FILE),
        state: SyncState {
            source: source.key.clone(),
            query_key: query.key(),
            state: "running".into(),
            done,
            skipped,
            total: earlier.as_ref().and_then(|record| record.total),
            page,
            last_page: earlier.as_ref().and_then(|record| record.last_page),
            page_done: done,
            page_skipped: skipped,
            error: None,
            started_at: started,
            updated_at: started,
            filters: true,
        },
        last_api: *last_api,
    };
    let result = run.write().and_then(|()| run.walk(&query));
    *last_api = run.last_api;
    cache::evict_thumbs(&app.paths.thumbs_dir(), cache::THUMBS_MAX_BYTES);
    match result {
        Ok(end) => {
            run.state.state = end.into();
            run.write()?;
            Ok(run.state)
        }
        Err(e) => {
            run.state.state = "failed".into();
            run.state.error = Some(SyncError {
                code: e.code.into(),
                message: e.message.clone(),
            });
            let _ = run.write();
            Err(e)
        }
    }
}

struct Run<'a> {
    app: &'a App,
    path: PathBuf,
    state: SyncState,
    last_api: Option<Duration>,
}

impl Run<'_> {
    /// Walks pages from `state.page` to the last; returns the end state (`done` or `canceled`).
    fn walk(&mut self, query: &Query) -> Result<&'static str> {
        let thumbs = self.app.paths.thumbs_dir();
        let mut page = self.state.page.max(1);
        loop {
            self.state.page = page;
            self.state.page_done = self.state.done;
            self.state.page_skipped = self.state.skipped;
            if self.take_cancel() {
                return Ok("canceled");
            }
            let result = self.search(query, page)?;
            // Synced sources stay browsable while Wallhaven is down.
            let _ = pages::save(&self.app.paths.pages_dir(), &query.key(), page, &result);
            self.state.total = Some(result.total);
            self.state.last_page = Some(result.last_page);
            // A page's thumbnails download several at a time, like browse; cached ones return
            // at once and count as done.
            for result in image::ensure_thumbs(&*self.app.transport, &result.items, &thumbs) {
                match result {
                    Ok(_) => self.state.done += 1,
                    Err(e) if SKIPPABLE.contains(&e.code) => self.state.skipped += 1,
                    Err(e) => return Err(e),
                }
            }
            if page >= result.last_page {
                return Ok("done");
            }
            page += 1;
            // Record the page boundary, so a resume starts at the next page.
            self.state.page = page;
            self.state.page_done = self.state.done;
            self.state.page_skipped = self.state.skipped;
            self.write()?;
        }
    }

    /// One search page with pacing and the retry rules for rate limits and network errors.
    fn search(&mut self, query: &Query, page: u64) -> Result<SearchPage> {
        let mut rate_retries = 0;
        let mut network_retried = false;
        loop {
            self.pace();
            match wallhaven::search(&*self.app.transport, query, page) {
                Err(e) if e.code == "rate_limited" && rate_retries < RATE_LIMIT_RETRIES => {
                    rate_retries += 1;
                    self.app.clock.sleep(RATE_LIMIT_WAIT);
                }
                Err(e) if matches!(e.code, "network" | "timeout") && !network_retried => {
                    network_retried = true;
                    self.app.clock.sleep(NETWORK_WAIT);
                }
                result => return result,
            }
        }
    }

    /// Keeps API requests at least `API_SPACING` apart.
    fn pace(&mut self) {
        let clock = &self.app.clock;
        if let Some(last) = self.last_api {
            let gap = clock.elapsed().saturating_sub(last);
            if gap < API_SPACING {
                clock.sleep(API_SPACING - gap);
            }
        }
        self.last_api = Some(clock.elapsed());
    }

    /// True (and the request consumed) when `sync.cancel` exists.
    fn take_cancel(&self) -> bool {
        fs::remove_file(self.app.paths.state(CANCEL_FILE)).is_ok()
    }

    /// Rewrites this query's record; only a sync holding the lock writes `syncs.json`.
    fn write(&mut self) -> Result<()> {
        self.state.updated_at = now();
        let mut syncs = state::read_json::<Syncs>(&self.path)?;
        syncs.version = 1;
        syncs
            .queries
            .insert(self.state.query_key.clone(), self.state.clone());
        state::write_json(&self.path, &syncs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{run, test_app};
    use crate::testutil::{FakeClock, FakeTransport, PNG};
    use crate::wallhaven::Transport;
    use crate::wallhaven::tests::{item_json, search_json};
    use std::path::Path;
    use std::sync::Arc;

    // Sync walks the whole category even though the test settings show General only (cat 100).
    const KEY: &str = "tag:711|cat:110|color:none";

    fn page_url(page: u64) -> String {
        Query {
            tag: 711,
            categories: "110",
            color: None,
        }
        .url(page)
    }

    fn thumb_url(id: &str) -> String {
        item_json(id)["thumbs"]["large"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Landscape over two pages: aa1 aa2 | bb1 bb2, every thumbnail served.
    fn two_pages() -> FakeTransport {
        let t = FakeTransport::default();
        t.add(&page_url(1), 200, search_json(&["aa1", "aa2"], 2, 4));
        t.add(&page_url(2), 200, search_json(&["bb1", "bb2"], 2, 4));
        for id in ["aa1", "aa2", "bb1", "bb2"] {
            t.add(&thumb_url(id), 200, PNG.to_vec());
        }
        t
    }

    fn sync_app(name: &str, transport: impl Transport + 'static) -> (App, FakeClock) {
        let mut app = test_app(name, FakeTransport::default());
        app.transport = Box::new(transport);
        let clock = FakeClock::default();
        app.clock = Box::new(clock.clone());
        (app, clock)
    }

    fn sync_file(app: &App) -> SyncState {
        let syncs = state::read_json::<Syncs>(&app.paths.state(SYNC_FILE)).unwrap();
        assert_eq!(syncs.version, 1);
        syncs.queries[KEY].clone()
    }

    fn record(state: &str, page: u64, done: u64) -> SyncState {
        SyncState {
            source: "landscape".into(),
            query_key: KEY.into(),
            state: state.into(),
            done,
            skipped: 0,
            total: Some(4),
            page,
            last_page: Some(2),
            page_done: done,
            page_skipped: 0,
            error: None,
            started_at: 1,
            updated_at: 2,
            filters: true,
        }
    }

    fn save_record(app: &App, record: SyncState) {
        let mut syncs = Syncs {
            version: 1,
            ..Syncs::default()
        };
        syncs.queries.insert(KEY.into(), record);
        state::write_json(&app.paths.state(SYNC_FILE), &syncs).unwrap();
    }

    #[test]
    fn walks_all_pages_counting_cached_and_skipped() {
        let t = two_pages();
        t.add(&thumb_url("bb2"), 404, Vec::new());
        let t = Arc::new(t);
        let (app, clock) = sync_app("sync-happy", t.clone());
        let thumbs = app.paths.thumbs_dir();
        fs::create_dir_all(&thumbs).unwrap();
        fs::write(thumbs.join("aa2.jpg"), PNG).unwrap();
        // A stale cancel request from an earlier run is dropped at start.
        fs::create_dir_all(&app.paths.state_dir).unwrap();
        fs::write(app.paths.state(CANCEL_FILE), b"").unwrap();
        // The main lock stays free for next/browse: sync works while it is held.
        let _main = state::acquire_lock(&app.paths.state_dir, Duration::ZERO).unwrap();

        let out = run(&app, &["sync", "landscape"]).unwrap();
        let sync = &out["sync"];
        assert_eq!(sync["state"], "done");
        assert_eq!(sync["source"], "landscape");
        assert_eq!(sync["queryKey"], KEY);
        assert_eq!(
            (&sync["done"], &sync["skipped"], &sync["total"]),
            (&json!(3), &json!(1), &json!(4))
        );
        assert_eq!((&sync["page"], &sync["lastPage"]), (&json!(2), &json!(2)));
        assert_eq!(sync["error"], Value::Null);
        assert!(sync.get("version").is_none());

        assert!(thumbs.join("aa1.png").is_file() && thumbs.join("bb1.png").is_file());
        // aa2 was cached, so it was never downloaded.
        assert!(!t.calls().contains(&thumb_url("aa2")));
        // The second API request waited for the 2 s spacing.
        assert_eq!(clock.sleeps(), [API_SPACING]);
        assert!(!app.paths.state(CANCEL_FILE).exists());

        assert_eq!(sync_file(&app).state, "done");
        // sync never touches the selection state files.
        for name in ["status.json", "seen.json", "favorites.json"] {
            assert!(!app.paths.state(name).exists(), "{name}");
        }
    }

    /// Requests a cancel while serving `url`.
    struct CancelAt {
        inner: FakeTransport,
        url: String,
        cancel: PathBuf,
    }

    impl Transport for CancelAt {
        fn get(&self, url: &str, timeout: Duration, limit: u64) -> Result<(u16, Vec<u8>)> {
            if url == self.url {
                fs::write(&self.cancel, b"").unwrap();
            }
            self.inner.get(url, timeout, limit)
        }
    }

    #[test]
    fn cancel_stops_before_the_next_page() {
        let (mut app, _) = sync_app("sync-cancel", FakeTransport::default());
        fs::create_dir_all(&app.paths.state_dir).unwrap();
        app.transport = Box::new(CancelAt {
            inner: two_pages(),
            url: thumb_url("aa1"),
            cancel: app.paths.state(CANCEL_FILE),
        });
        let out = run(&app, &["sync", "landscape"]).unwrap();
        // The page in flight finishes; the next one is never requested.
        assert_eq!(out["sync"]["state"], "canceled");
        assert_eq!(out["sync"]["done"], 2);
        assert!(!app.paths.state(CANCEL_FILE).exists());
        assert!(!app.paths.thumbs_dir().join("bb1.png").exists());
        let record = sync_file(&app);
        assert_eq!((record.state.as_str(), record.page), ("canceled", 2));
    }

    #[test]
    fn rate_limit_retries_then_fails() {
        let t = FakeTransport::default();
        t.add(&page_url(1), 429, Vec::new());
        let t = Arc::new(t);
        let (app, clock) = sync_app("sync-429", t.clone());
        let err = run(&app, &["sync", "landscape"]).unwrap_err();
        assert_eq!(err.code, "rate_limited");
        assert_eq!(t.calls(), vec![page_url(1); 4]);
        assert_eq!(clock.sleeps(), [RATE_LIMIT_WAIT; 3]);

        let file = sync_file(&app);
        assert_eq!(file.state, "failed");
        assert_eq!(file.error.unwrap().code, "rate_limited");
        assert_eq!((file.total, file.last_page), (None, None));
        // Failures never reach status.json.
        assert!(!app.paths.state("status.json").exists());
    }

    #[test]
    fn busy_when_sync_lock_held() {
        let (mut app, _) = sync_app("sync-busy", two_pages());
        let _held =
            state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, Duration::ZERO).unwrap();
        app.sync_lock_wait = Duration::ZERO;
        assert_eq!(run(&app, &["sync", "landscape"]).unwrap_err().code, "busy");
        assert!(!app.paths.state(SYNC_FILE).exists());
    }

    #[test]
    fn usage_for_favs_and_unknown_source() {
        let (app, _) = sync_app("sync-usage", FakeTransport::default());
        for key in ["favs", "nope"] {
            let err = run(&app, &["sync", key]).unwrap_err();
            assert_eq!((err.code, err.exit_code()), ("usage", 2), "{key}");
        }
        assert!(!app.paths.state(SYNC_FILE).exists());
    }

    #[test]
    fn status_reports_records_per_source_and_stopped_when_lock_is_free() {
        let (app, _) = sync_app("sync-status", FakeTransport::default());
        let out = run(&app, &["status"]).unwrap();
        assert_eq!(
            (&out["sync"], &out["sources"][0]["sync"]),
            (&Value::Null, &Value::Null)
        );

        save_record(&app, record("running", 1, 5));
        let out = run(&app, &["status"]).unwrap();
        // The process died: nothing runs, and the source shows where it stopped.
        assert_eq!(out["sync"], Value::Null);
        assert_eq!(out["sources"][0]["sync"]["state"], "stopped");
        assert_eq!(out["sources"][0]["sync"]["done"], 5);
        assert_eq!(out["sources"][1]["sync"], Value::Null);

        let _held =
            state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, Duration::ZERO).unwrap();
        let out = run(&app, &["status"]).unwrap();
        assert_eq!(out["sync"]["source"], "landscape");
        assert_eq!(out["sources"][0]["sync"]["state"], "running");
    }

    #[test]
    fn resume_continues_at_the_page_it_stopped_on() {
        let t = Arc::new(two_pages());
        let (app, _) = sync_app("sync-resume", t.clone());
        // Page 1 finished (2 done) before the sync was canceled on page 2.
        save_record(&app, record("canceled", 2, 2));
        let out = run(&app, &["sync", "landscape"]).unwrap();
        assert_eq!(out["sync"]["state"], "done");
        assert_eq!(out["sync"]["done"], 4);
        assert!(
            !t.calls().contains(&page_url(1)),
            "page 1 is not walked again"
        );
        assert!(t.calls().contains(&page_url(2)));
    }

    #[test]
    fn a_finished_sync_is_not_walked_again() {
        let t = Arc::new(two_pages());
        let (app, _) = sync_app("sync-finished", t.clone());
        save_record(&app, record("done", 2, 4));
        let out = run(&app, &["sync", "landscape"]).unwrap();
        assert_eq!(
            (&out["sync"]["state"], &out["sync"]["done"]),
            (&json!("done"), &json!(4))
        );
        assert!(t.calls().is_empty());
    }

    #[test]
    fn records_of_other_queries_are_kept() {
        let (app, _) = sync_app("sync-keep", two_pages());
        let mut other = record("done", 2, 9);
        other.query_key = "tag:37|cat:100|color:none".into();
        other.source = "nature".into();
        let mut syncs = Syncs {
            version: 1,
            ..Syncs::default()
        };
        syncs.queries.insert(other.query_key.clone(), other);
        state::write_json(&app.paths.state(SYNC_FILE), &syncs).unwrap();
        run(&app, &["sync", "landscape"]).unwrap();
        let syncs = state::read_json::<Syncs>(&app.paths.state(SYNC_FILE)).unwrap();
        assert_eq!(syncs.queries.len(), 2);
        assert_eq!(syncs.queries["tag:37|cat:100|color:none"].done, 9);
    }

    #[test]
    fn cancel_command_only_signals_a_running_sync() {
        let (app, _) = sync_app("sync-cancel-cmd", FakeTransport::default());
        let cancel_file = app.paths.state(CANCEL_FILE);
        assert_eq!(run(&app, &["sync", "--cancel"]).unwrap()["running"], false);
        assert!(!Path::new(&cancel_file).exists());

        let _held =
            state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, Duration::ZERO).unwrap();
        assert_eq!(run(&app, &["sync", "--cancel"]).unwrap()["running"], true);
        assert!(cancel_file.is_file());
    }

    #[test]
    fn a_record_from_before_filters_is_walked_again() {
        let t = Arc::new(two_pages());
        let (app, _) = sync_app("sync-legacy", t.clone());
        // A 0.3.3 record: done, but its saved pages lack category and colors.
        let mut legacy = serde_json::to_value(record("done", 2, 4)).unwrap();
        legacy.as_object_mut().unwrap().remove("filters");
        let syncs = json!({"version": 1, "queries": {KEY: legacy}});
        fs::create_dir_all(&app.paths.state_dir).unwrap();
        fs::write(app.paths.state(SYNC_FILE), syncs.to_string()).unwrap();
        assert_eq!(records(&app.paths).unwrap().queries[KEY].state, "stopped");

        let sync = run(&app, &["sync", "landscape"]).unwrap()["sync"].clone();
        assert_eq!((&sync["state"], &sync["done"]), (&json!("done"), &json!(4)));
        assert_eq!(sync["filters"], true);
        assert!(t.calls().contains(&page_url(1)));
    }

    #[test]
    fn browse_filters_a_synced_category_without_wallhaven() {
        let item = |id: &str, category: &str, colors: &[&str]| {
            let mut v = item_json(id);
            v["category"] = json!(category);
            v["colors"] = json!(colors);
            v
        };
        let page = |items: Vec<Value>| {
            serde_json::to_vec(&json!({"data": items, "meta": {"last_page": 2, "total": 4}}))
                .unwrap()
        };
        let t = FakeTransport::default();
        t.add(
            &page_url(1),
            200,
            page(vec![
                item("aa1", "general", &["#66cccc"]),
                item("aa2", "anime", &["#66cccc"]),
            ]),
        );
        // aa2 moved down a page while the sync walked; it still counts once.
        t.add(
            &page_url(2),
            200,
            page(vec![
                item("aa2", "anime", &["#66cccc"]),
                item("bb1", "general", &["#000000"]),
                item("bb2", "general", &["#000000", "#66cccc"]),
            ]),
        );
        for id in ["aa1", "aa2", "bb1", "bb2"] {
            t.add(&thumb_url(id), 200, PNG.to_vec());
        }
        let (mut app, _) = sync_app("sync-local-browse", t);
        run(&app, &["sync", "landscape"]).unwrap();
        // From here on every Wallhaven request fails.
        let offline = Arc::new(FakeTransport::default());
        app.transport = Box::new(offline.clone());
        let ids = |out: &Output| -> Vec<String> {
            out["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["id"].as_str().unwrap().to_string())
                .collect()
        };

        // The test settings show General only, any color.
        let out = run(&app, &["browse", "landscape"]).unwrap();
        assert_eq!(ids(&out), ["aa1", "bb1", "bb2"]);
        assert_eq!((&out["total"], &out["lastPage"]), (&json!(3), &json!(1)));
        assert_eq!(out["cached"], false);
        assert!(out["items"][0]["thumb"].is_string());

        let theme = &app.paths.theme_colors;
        fs::create_dir_all(theme.parent().unwrap()).unwrap();
        fs::write(theme, "accent = \"#66cccc\"\n").unwrap();
        run(&app, &["set", "color", "theme"]).unwrap();
        run(&app, &["set", "style.anime", "true"]).unwrap();
        let out = run(&app, &["browse", "landscape"]).unwrap();
        assert_eq!(ids(&out), ["aa1", "aa2", "bb2"]);
        let totals = state::read_json::<crate::state::Status>(&app.paths.state("status.json"))
            .unwrap()
            .totals;
        assert_eq!(totals["tag:711|cat:110|color:66cccc"], 3);
        assert!(ids(&run(&app, &["browse", "landscape", "--page", "2"]).unwrap()).is_empty());
        assert!(offline.calls().is_empty(), "{:?}", offline.calls());
    }

    fn city_url(page: u64) -> String {
        Query {
            tag: 17,
            categories: "110",
            color: None,
        }
        .url(page)
    }

    /// Two configured sources, landscape then city; city has one wallpaper, cc1.
    fn landscape_and_city(app: &App, t: &FakeTransport) {
        let config = json!({"sources": [
            {"key": "landscape", "label": "Landscape", "tag": 711},
            {"key": "city", "label": "City", "tag": 17}
        ]});
        fs::create_dir_all(app.paths.config_file.parent().unwrap()).unwrap();
        fs::write(&app.paths.config_file, config.to_string()).unwrap();
        t.add(&city_url(1), 200, search_json(&["cc1"], 1, 1));
        t.add(&thumb_url("cc1"), 200, PNG.to_vec());
    }

    fn states(out: &Output) -> Vec<(String, String)> {
        out["syncs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let field = |k: &str| r[k].as_str().unwrap().to_string();
                (field("source"), field("state"))
            })
            .collect()
    }

    #[test]
    fn sync_all_walks_every_category_once() {
        let t = Arc::new(two_pages());
        let (app, _) = sync_app("sync-all", t.clone());
        landscape_and_city(&app, &t);
        let out = run(&app, &["sync", "--all"]).unwrap();
        let both = [
            ("landscape".to_string(), "done".to_string()),
            ("city".to_string(), "done".to_string()),
        ];
        assert_eq!(states(&out), both);
        assert_eq!(out["syncs"][1]["done"], 1);
        // Finished categories are skipped without asking Wallhaven anything.
        let calls = t.calls().len();
        assert_eq!(states(&run(&app, &["sync", "--all"]).unwrap()), both);
        assert_eq!(t.calls().len(), calls);
    }

    #[test]
    fn sync_all_stops_at_a_failed_or_canceled_category() {
        let t = Arc::new(FakeTransport::default());
        t.add(&page_url(1), 500, Vec::new());
        let (app, _) = sync_app("sync-all-failed", t.clone());
        landscape_and_city(&app, &t);
        assert_eq!(
            run(&app, &["sync", "--all"]).unwrap_err().code,
            "unavailable"
        );
        assert!(!t.calls().contains(&city_url(1)));
        assert_eq!(sync_file(&app).state, "failed");

        let (mut app, _) = sync_app("sync-all-canceled", FakeTransport::default());
        let inner = two_pages();
        landscape_and_city(&app, &inner);
        fs::create_dir_all(&app.paths.state_dir).unwrap();
        let cancel = app.paths.state(CANCEL_FILE);
        app.transport = Box::new(CancelAt {
            inner,
            url: thumb_url("aa1"),
            cancel,
        });
        let out = run(&app, &["sync", "--all"]).unwrap();
        assert_eq!(
            states(&out),
            [("landscape".to_string(), "canceled".to_string())]
        );
        assert!(!app.paths.thumbs_dir().join("cc1.png").exists());
    }
}
