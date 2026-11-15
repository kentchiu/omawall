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
use crate::config::{self, Settings};
use crate::error::{AppError, Result};
use crate::image;
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
/// `syncs.json` is rewritten at most this often while downloading thumbnails.
pub const WRITE_INTERVAL: Duration = Duration::from_secs(1);

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
}

/// `syncs.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Syncs {
    pub version: u32,
    pub queries: BTreeMap<String, SyncState>,
}

/// Every record, with `running` turned into `stopped` when no sync process holds the lock.
pub fn records(paths: &Paths) -> Result<Syncs> {
    let mut syncs = state::read_json::<Syncs>(&paths.state(SYNC_FILE))?;
    if !state::is_locked(&paths.state(SYNC_LOCK)) {
        for record in syncs.queries.values_mut() {
            if record.state == "running" {
                record.state = "stopped".into();
            }
        }
    }
    Ok(syncs)
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

/// `sync <sourceKey>`.
pub fn sync(app: &App, key: &str) -> Result<Output> {
    let sources = config::load_sources(&app.paths.config_file)?;
    // `favs` is never a configured key, so it lands here too.
    let source = sources
        .iter()
        .find(|src| src.key == key)
        .ok_or_else(|| AppError::usage(format!("Unknown source: {key}")))?;
    let settings = Settings::load(&app.paths.state("settings.json"), &sources)?;
    let query = app.query(source, &settings, app.color(&settings));

    let _lock = state::acquire_named_lock(&app.paths.state_dir, SYNC_LOCK, app.sync_lock_wait)?;
    let _ = fs::remove_file(app.paths.state(CANCEL_FILE));
    // The lock is ours, so any `running` record left behind belongs to a dead process.
    let earlier = records(&app.paths)?.queries.remove(&query.key());
    if let Some(done) = earlier.as_ref().filter(|record| record.state == "done") {
        return Ok(output(json!({"sync": done})));
    }
    let started = now();
    let (page, done, skipped) = earlier.as_ref().map_or((1, 0, 0), |record| {
        (record.page, record.page_done, record.page_skipped)
    });
    let mut run = Run {
        app,
        path: app.paths.state(SYNC_FILE),
        state: SyncState {
            source: key.into(),
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
        },
        last_api: None,
        last_write: Duration::ZERO,
    };
    let result = run.write().and_then(|()| run.walk(&query));
    cache::evict_thumbs(&app.paths.thumbs_dir(), cache::THUMBS_MAX_BYTES);
    match result {
        Ok(end) => {
            run.state.state = end.into();
            run.write()?;
            Ok(output(json!({"sync": run.state})))
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
    last_write: Duration,
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
            let result = self.search(query, page)?;
            self.state.total = Some(result.total);
            self.state.last_page = Some(result.last_page);
            for item in &result.items {
                if self.take_cancel() {
                    return Ok("canceled");
                }
                // Already cached thumbnails return immediately and count as done.
                match image::ensure_thumb(&*self.app.transport, item, &thumbs) {
                    Ok(_) => self.state.done += 1,
                    Err(e) if SKIPPABLE.contains(&e.code) => self.state.skipped += 1,
                    Err(e) => return Err(e),
                }
                if self.app.clock.elapsed().saturating_sub(self.last_write) >= WRITE_INTERVAL {
                    self.write()?;
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
        self.last_write = self.app.clock.elapsed();
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

    const KEY: &str = "tag:711|cat:100|color:none";

    fn page_url(page: u64) -> String {
        Query {
            tag: 711,
            categories: "100",
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
    fn cancel_stops_before_next_thumbnail() {
        let (mut app, _) = sync_app("sync-cancel", FakeTransport::default());
        fs::create_dir_all(&app.paths.state_dir).unwrap();
        app.transport = Box::new(CancelAt {
            inner: two_pages(),
            url: thumb_url("aa1"),
            cancel: app.paths.state(CANCEL_FILE),
        });
        let out = run(&app, &["sync", "landscape"]).unwrap();
        assert_eq!(out["sync"]["state"], "canceled");
        assert_eq!(out["sync"]["done"], 1);
        assert!(!app.paths.state(CANCEL_FILE).exists());
        assert!(!app.paths.thumbs_dir().join("aa2.png").exists());
        assert_eq!(sync_file(&app).state, "canceled");
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
}
