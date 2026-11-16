use std::fs;
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::cache;
use crate::color;
use crate::config::{self, ColorMode, FAVS, Settings, Source};
use crate::error::{AppError, Result};
use crate::image;
use crate::pages;
use crate::paths::Paths;
use crate::select::{self, Rng};
use crate::state::{self, Current, Favorite, Favorites, LastError, Seen, Status, push_unique};
use crate::sync::{self, Clock, SystemClock};
use crate::wallhaven::{self, Item, Query, Transport, UreqTransport};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const PER_PAGE: usize = 24;

pub type Output = Map<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Manual,
    Timer,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Trigger::Manual => "manual",
            Trigger::Timer => "timer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Status,
    Next(Trigger),
    Apply(String),
    /// `fav` toggles the current wallpaper; `fav <id>` any wallpaper.
    Fav(Option<String>),
    Browse {
        source: String,
        page: u64,
    },
    Set {
        name: String,
        value: String,
    },
    Sync(String),
    SyncCancel,
}

impl Command {
    fn trigger(&self) -> Trigger {
        match self {
            Command::Next(trigger) => *trigger,
            _ => Trigger::Manual,
        }
    }

    /// `browse`, `fav` and `set` never notify (`status` and `sync` never reach this).
    fn notifies(&self) -> bool {
        !matches!(
            self,
            Command::Browse { .. } | Command::Fav(_) | Command::Set { .. } | Command::Status
        )
    }
}

const USAGE: &str = "Usage: omawall status | next [--trigger manual|timer] | apply <id> [--trigger manual] | fav [<id>] | browse <source|favs> [--page N] | set <name> <value> | sync <source> | sync --cancel | --version";

fn usage() -> AppError {
    AppError::usage(USAGE)
}

/// Parses argv (without the program name).
pub fn parse_args(args: &[String]) -> Result<Command> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let command = match args.as_slice() {
        ["status"] => Command::Status,
        ["next"] => Command::Next(Trigger::Manual),
        ["next", "--trigger", t] => Command::Next(match *t {
            "manual" => Trigger::Manual,
            "timer" => Trigger::Timer,
            _ => return Err(usage()),
        }),
        ["apply", id] | ["apply", id, "--trigger", "manual"] => {
            if !wallhaven::is_valid_id(id) {
                return Err(AppError::usage(format!("Invalid wallpaper id: {id}")));
            }
            Command::Apply(id.to_string())
        }
        ["fav"] => Command::Fav(None),
        ["fav", id] => {
            if !wallhaven::is_valid_id(id) {
                return Err(AppError::usage(format!("Invalid wallpaper id: {id}")));
            }
            Command::Fav(Some(id.to_string()))
        }
        ["browse", source] => Command::Browse {
            source: source.to_string(),
            page: 1,
        },
        ["browse", source, "--page", n] => Command::Browse {
            source: source.to_string(),
            page: n
                .parse()
                .ok()
                .filter(|&p| p >= 1)
                .ok_or_else(|| AppError::usage("--page must be a positive integer"))?,
        },
        ["set", name, value] => Command::Set {
            name: name.to_string(),
            value: value.to_string(),
        },
        ["sync", "--cancel"] => Command::SyncCancel,
        ["sync", source] => Command::Sync(source.to_string()),
        _ => return Err(usage()),
    };
    Ok(command)
}

/// The single stdout JSON object and the exit code.
pub fn render(result: Result<Output>) -> (String, i32) {
    match result {
        Ok(fields) => {
            let mut out = Map::new();
            out.insert("ok".into(), true.into());
            out.extend(fields);
            (Value::Object(out).to_string(), 0)
        }
        Err(e) => {
            let out = json!({"ok": false, "error": {"code": e.code, "message": e.message}});
            (out.to_string(), e.exit_code())
        }
    }
}

pub(crate) fn output(value: Value) -> Output {
    match value {
        Value::Object(map) => map,
        _ => unreachable!("command outputs are objects"),
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn no_current() -> AppError {
    AppError::new("not_found", "No wallpaper is applied")
}

/// What `next`/`apply` will put on the desktop.
enum Target {
    Remote { item: Item, source: Option<String> },
    Favorite(Favorite),
    Reapply(Current),
}

/// Everything a command reads from disk.
struct Store {
    sources: Vec<Source>,
    settings: Settings,
    seen: Seen,
    favorites: Favorites,
    status: Status,
}

pub struct App {
    pub paths: Paths,
    pub transport: Box<dyn Transport>,
    pub bg_set: String,
    pub notifier: String,
    pub lock_wait: Duration,
    pub sync_lock_wait: Duration,
    pub clock: Box<dyn Clock>,
}

impl App {
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            transport: Box::new(UreqTransport::new()),
            bg_set: "omarchy-theme-bg-set".into(),
            notifier: "omarchy-notification-send".into(),
            lock_wait: Duration::from_secs(10),
            sync_lock_wait: Duration::from_secs(2),
            clock: Box::new(SystemClock::new()),
        }
    }

    pub fn run(&self, command: Command) -> Result<Output> {
        // These never take `<state>/lock`, never record `lastError` and never notify.
        match command {
            Command::Status => return self.status(),
            Command::Sync(key) => return sync::sync(self, &key),
            Command::SyncCancel => return sync::cancel(&self.paths),
            _ => {}
        }
        let trigger = command.trigger();
        let notifies = command.notifies();
        let result = state::acquire_lock(&self.paths.state_dir, self.lock_wait).and_then(|_lock| {
            let result = self.dispatch(command);
            if let Err(e) = &result
                && e.code != "usage"
            {
                self.record_error(e, trigger);
            }
            result
        });
        if let Err(e) = &result
            && notifies
            && trigger == Trigger::Manual
            && e.code != "usage"
        {
            self.notify(&e.message);
        }
        result
    }

    fn dispatch(&self, command: Command) -> Result<Output> {
        match command {
            Command::Next(_) => self.next(),
            Command::Apply(id) => self.apply(&id),
            Command::Fav(id) => self.fav(id.as_deref()),
            Command::Browse { source, page } => self.browse(&source, page),
            Command::Set { name, value } => self.set(&name, &value),
            Command::Status | Command::Sync(_) | Command::SyncCancel => {
                unreachable!("handled in run")
            }
        }
    }

    // ----- commands -----

    fn status(&self) -> Result<Output> {
        let s = self.load()?;
        let color = self.color(&s.settings);
        let syncs = sync::records(&self.paths)?;
        let sources: Vec<Value> = s
            .sources
            .iter()
            .map(|src| {
                let key = self.query(src, &s.settings, color).key();
                json!({
                    "key": src.key, "label": src.label, "tag": src.tag,
                    "selected": s.settings.is_selected(&src.key),
                    "total": s.status.totals.get(&key),
                    // A sync covers the whole category, whatever the filters.
                    "sync": sync::source_view(&syncs, &sync::query_for(src).key()),
                })
            })
            .collect();
        Ok(output(json!({
            "version": VERSION,
            "status": s.status,
            "settings": s.settings,
            "sources": sources,
            "favorites": {"count": s.favorites.items.len(), "selected": s.settings.is_selected(FAVS)},
            "sync": sync::running_view(&syncs),
        })))
    }

    fn next(&self) -> Result<Output> {
        let mut s = self.load()?;
        let color = self.color(&s.settings);
        let current = s.status.current_id().map(str::to_owned);
        let current = current.as_deref();
        let mut keys = s.settings.sources.clone();
        Rng::from_time().shuffle(&mut keys);

        let mut target = None;
        for key in &keys {
            if key == FAVS {
                if select::has_other_favorite(&s.favorites, current)
                    && let Some(fav) = select::pick_fav(&s.favorites, &mut s.seen, current)
                {
                    target = Some(Target::Favorite(fav));
                    break;
                }
                continue;
            }
            let Some(source) = s.sources.iter().find(|src| &src.key == key) else {
                continue;
            };
            let query = self.query(source, &s.settings, color);
            let pages_dir = self.paths.pages_dir();
            let mut search = |q: &Query, page: u64| {
                pages::search(&*self.transport, &*self.clock, &pages_dir, q, page)
                    .map(|(found, _)| found)
            };
            let pick = select::pick_tag(&mut search, &query, &mut s.seen, current)?;
            if let Some(total) = pick.total {
                s.status.totals.insert(query.key(), total);
            }
            if let Some(item) = pick.item {
                target = Some(Target::Remote {
                    item,
                    source: Some(key.clone()),
                });
                break;
            }
        }

        let Some(target) = target else {
            self.save(&s.seen, "seen.json")?;
            self.write_status(&mut s.status)?;
            return Err(AppError::new(
                "no_candidates",
                "No wallpaper available; adjust sources or filters",
            ));
        };
        let applied = self.apply_target(target)?;
        self.save(&s.seen, "seen.json")?;
        self.finish_apply(&mut s.status, applied)
    }

    fn apply(&self, id: &str) -> Result<Output> {
        let mut s = self.load()?;
        let favorite = s
            .favorites
            .items
            .iter()
            .find(|f| f.id == id && Path::new(&f.path).is_file());
        let target = if let Some(fav) = favorite {
            Target::Favorite(fav.clone())
        } else if let Some(cur) = s
            .status
            .current
            .clone()
            .filter(|c| c.id == id && Path::new(&c.path).is_file())
        {
            Target::Reapply(cur)
        } else {
            Target::Remote {
                item: pages::detail(&*self.transport, &*self.clock, &self.paths.pages_dir(), id)?,
                source: None,
            }
        };
        let applied = self.apply_target(target)?;
        push_unique(&mut s.seen.all, id);
        self.save(&s.seen, "seen.json")?;
        self.finish_apply(&mut s.status, applied)
    }

    fn fav(&self, id: Option<&str>) -> Result<Output> {
        let mut s = self.load()?;
        let current = s.status.current.clone();
        let id = match id {
            Some(id) => id.to_string(),
            None => {
                let cur = current.as_ref().ok_or_else(no_current)?;
                if !wallhaven::is_valid_id(&cur.id) {
                    return Err(AppError::new("io", "Invalid wallpaper id in status.json"));
                }
                cur.id.clone()
            }
        };
        let favorite = if s.favorites.contains(&id) {
            self.remove_favorite(&id, &mut s.favorites, &mut s.status)?;
            false
        } else if let Some(cur) = current.filter(|c| c.id == id) {
            let src = Path::new(&cur.path);
            if !src.is_file() {
                return Err(AppError::new(
                    "not_found",
                    "Current wallpaper file not found",
                ));
            }
            let ext = image::detect_file(src)?;
            let dest = self
                .paths
                .favorites_dir
                .join(format!("wallhaven-{}.{ext}", cur.id));
            if dest != src {
                let bytes = fs::read(src)
                    .map_err(|e| AppError::io(format!("Cannot read {}", src.display()), e))?;
                state::write_atomic(&dest, &bytes)?;
            }
            let thumb = self.current_thumb(&cur);
            if let Some(c) = s.status.current.as_mut() {
                c.thumb = thumb.clone();
            }
            s.favorites.items.insert(
                0,
                Favorite {
                    id: cur.id.clone(),
                    path: dest.display().to_string(),
                    thumb,
                    resolution: cur.resolution.clone(),
                    favorites: cur.favorites,
                    file_size: cur.file_size,
                    added_at: now(),
                },
            );
            true
        } else {
            let fav = self.download_favorite(&id)?;
            s.favorites.items.insert(0, fav);
            true
        };
        self.save(&s.favorites, "favorites.json")?;
        self.write_status(&mut s.status)?;
        Ok(output(
            json!({"id": id, "favorite": favorite, "current": s.status.current}),
        ))
    }

    /// Downloads a wallpaper that is not on the desktop straight into the favorites folder.
    fn download_favorite(&self, id: &str) -> Result<Favorite> {
        let item = pages::detail(&*self.transport, &*self.clock, &self.paths.pages_dir(), id)?;
        let path = image::ensure_full(&*self.transport, &item, &self.paths.favorites_dir)?;
        let thumb = image::ensure_thumb(&*self.transport, &item, &self.paths.thumbs_dir()).ok();
        Ok(Favorite {
            id: item.id,
            path: path.display().to_string(),
            thumb: thumb.map(|p| p.display().to_string()),
            resolution: item.resolution,
            favorites: item.favorites,
            file_size: item.file_size,
            added_at: now(),
        })
    }

    fn browse(&self, key: &str, page: u64) -> Result<Output> {
        let mut s = self.load()?;
        let current = s.status.current_id().map(str::to_owned);
        let entry = |id: &str,
                     thumb: Option<String>,
                     resolution: &str,
                     favorites: u64,
                     file_size: u64| {
            json!({
                "id": id, "thumb": thumb, "resolution": resolution, "favorites": favorites, "fileSize": file_size,
                "seen": s.seen.all.iter().any(|x| x == id),
                "favorite": s.favorites.contains(id),
                "current": current.as_deref() == Some(id),
            })
        };

        if key == FAVS {
            let total = s.favorites.items.len();
            let skip = usize::try_from(page - 1)
                .unwrap_or(usize::MAX)
                .saturating_mul(PER_PAGE);
            let items: Vec<Value> = s
                .favorites
                .items
                .iter()
                .skip(skip)
                .take(PER_PAGE)
                .map(|f| {
                    let thumb = f.thumb.clone().filter(|t| Path::new(t).is_file());
                    entry(&f.id, thumb, &f.resolution, f.favorites, f.file_size)
                })
                .collect();
            return Ok(output(json!({
                "source": key, "page": page, "lastPage": total.div_ceil(PER_PAGE).max(1),
                "total": total, "items": items, "cached": false,
            })));
        }

        let source = s
            .sources
            .iter()
            .find(|src| src.key == key)
            .ok_or_else(|| AppError::usage(format!("Unknown source: {key}")))?;
        let query = self.query(source, &s.settings, self.color(&s.settings));
        let pages_dir = self.paths.pages_dir();
        // A synced category is filtered from disk, so browsing it never calls Wallhaven.
        let local = sync::local_pages(&self.paths, source)?.and_then(|(synced, last)| {
            pages::filtered(&pages_dir, &synced, last, &query, page, PER_PAGE)
        });
        let (result, cached) = match local {
            Some(found) => (found, false),
            None => pages::search(&*self.transport, &*self.clock, &pages_dir, &query, page)?,
        };
        let thumbs =
            image::ensure_thumbs(&*self.transport, &result.items, &self.paths.thumbs_dir());
        let mut items = Vec::with_capacity(result.items.len());
        for (item, thumb) in result.items.iter().zip(thumbs) {
            // A thumbnail that cannot be fetched leaves its card blank; only a disk error fails
            // the page, so a saved page still shows while Wallhaven is down.
            let thumb = match thumb {
                Ok(path) => Some(path.display().to_string()),
                Err(e) if e.code == "io" => return Err(e),
                Err(_) => None,
            };
            items.push(entry(
                &item.id,
                thumb,
                &item.resolution,
                item.favorites,
                item.file_size,
            ));
        }
        cache::evict_thumbs(&self.paths.thumbs_dir(), cache::THUMBS_MAX_BYTES);
        // Every rewrite makes the widget reload, so skip it when nothing changed.
        if s.status.totals.insert(query.key(), result.total) != Some(result.total) {
            self.write_status(&mut s.status)?;
        }
        Ok(output(json!({
            "source": key, "page": page, "lastPage": result.last_page,
            "total": result.total, "items": items, "cached": cached,
        })))
    }

    fn set(&self, name: &str, value: &str) -> Result<Output> {
        let mut s = self.load()?;
        s.settings.set(name, value, &s.sources)?;
        self.save(&s.settings, "settings.json")?;
        self.write_status(&mut s.status)?;
        Ok(output(json!({"settings": s.settings})))
    }

    // ----- helpers -----

    fn load(&self) -> Result<Store> {
        let sources = config::load_sources(&self.paths.config_file)?;
        let settings = Settings::load(&self.paths.state("settings.json"), &sources)?;
        Ok(Store {
            sources,
            settings,
            seen: state::read_json(&self.paths.state("seen.json"))?,
            favorites: state::read_json(&self.paths.state("favorites.json"))?,
            status: state::read_json(&self.paths.state("status.json"))?,
        })
    }

    fn save<T: serde::Serialize>(&self, value: &T, name: &str) -> Result<()> {
        state::write_json(&self.paths.state(name), value)
    }

    fn write_status(&self, status: &mut Status) -> Result<()> {
        status.updated_at = now();
        self.save(status, "status.json")
    }

    pub(crate) fn color(&self, settings: &Settings) -> Option<&'static str> {
        match settings.color {
            ColorMode::Theme => color::theme_color(&self.paths.theme_colors),
            ColorMode::None => None,
        }
    }

    pub(crate) fn query(
        &self,
        source: &Source,
        settings: &Settings,
        color: Option<&'static str>,
    ) -> Query {
        Query {
            tag: source.tag,
            categories: settings.categories(),
            color,
        }
    }

    /// Downloads if needed and runs `omarchy-theme-bg-set`.
    fn apply_target(&self, target: Target) -> Result<Current> {
        let current = match target {
            Target::Remote { item, source } => {
                let path = image::ensure_full(&*self.transport, &item, &self.paths.full_dir())?;
                let thumb =
                    image::ensure_thumb(&*self.transport, &item, &self.paths.thumbs_dir()).ok();
                Current {
                    url: format!("https://wallhaven.cc/w/{}", item.id),
                    id: item.id,
                    source,
                    path: path.display().to_string(),
                    thumb: thumb.map(|p| p.display().to_string()),
                    resolution: item.resolution,
                    favorites: item.favorites,
                    file_size: item.file_size,
                    applied_at: 0,
                }
            }
            Target::Favorite(fav) => Current {
                url: format!("https://wallhaven.cc/w/{}", fav.id),
                id: fav.id,
                source: Some(FAVS.into()),
                path: fav.path,
                thumb: fav.thumb.filter(|t| Path::new(t).is_file()),
                resolution: fav.resolution,
                favorites: fav.favorites,
                file_size: fav.file_size,
                applied_at: 0,
            },
            Target::Reapply(current) => current,
        };
        self.set_wallpaper(Path::new(&current.path))?;
        Ok(Current {
            applied_at: now(),
            ..current
        })
    }

    fn finish_apply(&self, status: &mut Status, current: Current) -> Result<Output> {
        let full = Path::new(&current.path).to_path_buf();
        status.current = Some(current);
        status.last_error = None;
        self.write_status(status)?;
        cache::evict_full(&self.paths.full_dir(), cache::FULL_KEEP, Some(&full));
        cache::evict_thumbs(&self.paths.thumbs_dir(), cache::THUMBS_MAX_BYTES);
        Ok(output(json!({"current": status.current})))
    }

    fn set_wallpaper(&self, path: &Path) -> Result<()> {
        let status = Process::new(&self.bg_set)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .map_err(|e| AppError::new("set_failed", format!("Cannot run {}: {e}", self.bg_set)))?;
        if !status.success() {
            let code = status.code().map_or("none".to_string(), |c| c.to_string());
            return Err(AppError::new(
                "set_failed",
                format!("{} failed (exit code {code})", self.bg_set),
            ));
        }
        Ok(())
    }

    /// Existing thumbnail of the current wallpaper, downloading it if missing (best effort).
    fn current_thumb(&self, cur: &Current) -> Option<String> {
        if let Some(thumb) = cur.thumb.as_ref().filter(|t| Path::new(t).is_file()) {
            return Some(thumb.clone());
        }
        if let Some(path) = image::cached(&self.paths.thumbs_dir(), &cur.id) {
            return Some(path.display().to_string());
        }
        let item = wallhaven::detail(&*self.transport, &cur.id).ok()?;
        let path = image::ensure_thumb(&*self.transport, &item, &self.paths.thumbs_dir()).ok()?;
        Some(path.display().to_string())
    }

    /// Drops the favorite entry and deletes only `<favorites>/wallhaven-<id>.<jpg|png>`.
    /// If that file is the current wallpaper, a copy is kept in the full cache first.
    fn remove_favorite(
        &self,
        id: &str,
        favorites: &mut Favorites,
        status: &mut Status,
    ) -> Result<()> {
        favorites.items.retain(|f| f.id != id);
        if !wallhaven::is_valid_id(id) {
            return Ok(());
        }
        for ext in image::EXTENSIONS {
            let name = format!("wallhaven-{id}.{ext}");
            let file = self.paths.favorites_dir.join(&name);
            if !file.is_file() {
                continue;
            }
            let io = |e| AppError::io(format!("Cannot update {}", file.display()), e);
            if let Some(cur) = status
                .current
                .as_mut()
                .filter(|c| Path::new(&c.path) == file)
            {
                let keep = self.paths.full_dir().join(&name);
                if !keep.is_file() {
                    state::write_atomic(&keep, &fs::read(&file).map_err(io)?)?;
                }
                // Repoint the desktop before its file disappears; on failure nothing is deleted.
                self.set_wallpaper(&keep)?;
                cur.path = keep.display().to_string();
            }
            fs::remove_file(&file).map_err(io)?;
        }
        Ok(())
    }

    /// Stores the failure in `status.json`; best effort.
    fn record_error(&self, err: &AppError, trigger: Trigger) {
        let Ok(mut status) = state::read_json::<Status>(&self.paths.state("status.json")) else {
            return;
        };
        status.last_error = Some(LastError {
            code: err.code.into(),
            message: err.message.clone(),
            trigger: trigger.as_str().into(),
            at: now(),
        });
        let _ = self.write_status(&mut status);
    }

    fn notify(&self, message: &str) {
        let _ = Process::new(&self.notifier)
            .args(["--app-name", "omawall", "Wallpaper change failed", message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::testutil::{FakeTransport, JPEG, PNG, temp_dir};
    use crate::wallhaven::tests::{item_json, search_json};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    pub fn test_app(name: &str, transport: FakeTransport) -> App {
        let root = temp_dir(name);
        let paths = Paths::new(
            &root.join("home"),
            &root.join("config"),
            &root.join("state"),
            &root.join("cache"),
        );
        App {
            paths,
            transport: Box::new(transport),
            bg_set: "true".into(),
            notifier: "true".into(),
            // Parallel tests fork children that briefly inherit lock fds, so allow a wait.
            lock_wait: Duration::from_secs(5),
            sync_lock_wait: Duration::from_secs(2),
            clock: Box::new(crate::testutil::FakeClock::default()),
        }
    }

    /// Fake Wallhaven serving `landscape` (tag 711) page 1 with the given ids.
    fn landscape(ids: &[&str]) -> FakeTransport {
        let t = FakeTransport::default();
        let q = Query {
            tag: 711,
            categories: "100",
            color: None,
        };
        t.add(&q.url(1), 200, search_json(ids, 1, ids.len() as u64));
        for id in ids {
            let item = item_json(id);
            t.add(item["path"].as_str().unwrap(), 200, JPEG.to_vec());
            t.add(item["thumbs"]["large"].as_str().unwrap(), 200, PNG.to_vec());
            t.add(
                &format!("https://wallhaven.cc/api/v1/w/{id}"),
                200,
                serde_json::to_vec(&json!({"data": item})).unwrap(),
            );
        }
        t
    }

    pub fn run(app: &App, list: &[&str]) -> Result<Output> {
        app.run(parse_args(&args(list))?)
    }

    fn status_of(app: &App) -> Status {
        state::read_json(&app.paths.state("status.json")).unwrap()
    }

    #[test]
    fn parse_and_usage_errors() {
        assert_eq!(
            parse_args(&args(&["next"])).unwrap(),
            Command::Next(Trigger::Manual)
        );
        assert_eq!(
            parse_args(&args(&["next", "--trigger", "timer"])).unwrap(),
            Command::Next(Trigger::Timer)
        );
        assert_eq!(
            parse_args(&args(&["browse", "favs", "--page", "3"])).unwrap(),
            Command::Browse {
                source: "favs".into(),
                page: 3
            }
        );
        assert_eq!(
            parse_args(&args(&["apply", "abc", "--trigger", "manual"])).unwrap(),
            Command::Apply("abc".into())
        );
        assert_eq!(
            parse_args(&args(&["sync", "landscape"])).unwrap(),
            Command::Sync("landscape".into())
        );
        assert_eq!(
            parse_args(&args(&["sync", "--cancel"])).unwrap(),
            Command::SyncCancel
        );
        for bad in [
            &[][..],
            &["nope"],
            &["next", "--trigger", "cron"],
            &["apply", "../x"],
            &["apply", "abc", "--trigger", "timer"],
            &["browse", "favs", "--page", "0"],
            &["set", "color"],
            &["status", "extra"],
            &["dislike"],
            &["sync"],
            &["sync", "landscape", "--cancel"],
        ] {
            assert_eq!(parse_args(&args(bad)).unwrap_err().code, "usage", "{bad:?}");
        }
    }

    #[test]
    fn envelope_and_exit_codes() {
        let (out, code) = render(Ok(output(json!({"skipped": true}))));
        assert_eq!(code, 0);
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"ok": true, "skipped": true})
        );

        let (out, code) = render(Err(AppError::usage("x")));
        assert_eq!(code, 2);
        assert_eq!(
            serde_json::from_str::<Value>(&out).unwrap(),
            json!({"ok": false, "error": {"code": "usage", "message": "x"}})
        );
        assert_eq!(render(Err(AppError::new("network", "y"))).1, 1);
        assert!(!out.contains('\n'));
    }

    #[test]
    fn status_defaults() {
        let app = test_app("app-status", FakeTransport::default());
        let out = run(&app, &["status"]).unwrap();
        assert_eq!(out["version"], VERSION);
        assert_eq!(out["status"]["current"], Value::Null);
        assert_eq!(out["sources"].as_array().unwrap().len(), 10);
        assert_eq!(
            out["sources"][0],
            json!({"key":"landscape","label":"Landscape","tag":711,"selected":true,"total":null,"sync":null})
        );
        assert_eq!(out["favorites"], json!({"count": 0, "selected": false}));
        assert_eq!(out["sync"], Value::Null);
        assert_eq!(out["settings"]["timer"]["minutes"], 60);
    }

    #[test]
    fn next_applies_and_records() {
        let app = test_app("app-next", landscape(&["aa1", "aa2"]));
        run(&app, &["set", "sources", "landscape"]).unwrap();
        let out = run(&app, &["next"]).unwrap();
        let cur = &out["current"];
        assert_eq!(cur["id"], "aa1");
        assert_eq!(cur["source"], "landscape");
        assert_eq!(cur["url"], "https://wallhaven.cc/w/aa1");
        assert!(
            cur["path"]
                .as_str()
                .unwrap()
                .ends_with("/full/wallhaven-aa1.jpg")
        );
        assert!(cur["thumb"].as_str().unwrap().ends_with("/thumbs/aa1.png"));

        let status = status_of(&app);
        assert_eq!(status.totals["tag:711|cat:100|color:none"], 2);
        assert_eq!(run(&app, &["next"]).unwrap()["current"]["id"], "aa2");
        let seen: Seen = state::read_json(&app.paths.state("seen.json")).unwrap();
        assert_eq!(seen.all, ["aa1", "aa2"]);

        let out = run(&app, &["status"]).unwrap();
        assert_eq!(out["sources"][0]["total"], 2);
    }

    #[test]
    fn failure_keeps_current_and_records_error() {
        let mut app = test_app("app-fail", landscape(&["aa1", "aa2"]));
        run(&app, &["set", "sources", "landscape"]).unwrap();
        run(&app, &["next"]).unwrap();
        app.bg_set = "false".into();
        assert_eq!(run(&app, &["next"]).unwrap_err().code, "set_failed");
        let status = status_of(&app);
        assert_eq!(status.current.unwrap().id, "aa1");
        let err = status.last_error.unwrap();
        assert_eq!(
            (err.code.as_str(), err.trigger.as_str()),
            ("set_failed", "manual")
        );

        app.bg_set = "true".into();
        assert_eq!(
            run(&app, &["next", "--trigger", "timer"]).unwrap()["current"]["id"],
            "aa2"
        );
        assert!(status_of(&app).last_error.is_none());
    }

    #[test]
    fn no_candidates_and_busy() {
        let mut app = test_app("app-empty", FakeTransport::default());
        run(&app, &["set", "sources", ""]).unwrap();
        assert_eq!(
            run(&app, &["next", "--trigger", "timer"]).unwrap_err().code,
            "no_candidates"
        );
        assert_eq!(status_of(&app).last_error.unwrap().trigger, "timer");

        let _held = state::acquire_lock(&app.paths.state_dir, Duration::from_secs(5)).unwrap();
        app.lock_wait = Duration::ZERO;
        assert_eq!(run(&app, &["next"]).unwrap_err().code, "busy");
        // status never takes the lock.
        assert!(run(&app, &["status"]).is_ok());
    }

    #[test]
    fn set_timer_minutes_usage() {
        let app = test_app("app-set", FakeTransport::default());
        let err = run(&app, &["set", "timer.minutes", "3"]).unwrap_err();
        assert_eq!((err.code, err.exit_code()), ("usage", 2));
        assert!(status_of(&app).last_error.is_none());
        let out = run(&app, &["set", "timer.minutes", "30"]).unwrap();
        assert_eq!(out["settings"]["timer"]["minutes"], 30);
    }

    #[test]
    fn fav_toggles_and_ignores_legacy_dislikes() {
        let app = test_app("app-fav", landscape(&["aa1", "aa2", "aa3"]));
        run(&app, &["set", "sources", "landscape"]).unwrap();
        assert_eq!(run(&app, &["fav"]).unwrap_err().code, "not_found");
        // A 0.1.x disliked.json is neither read nor written.
        let legacy = app.paths.state("disliked.json");
        fs::write(&legacy, r#"{"version":1,"ids":["aa1"]}"#).unwrap();
        assert_eq!(run(&app, &["next"]).unwrap()["current"]["id"], "aa1");

        let out = run(&app, &["fav"]).unwrap();
        assert_eq!(out["favorite"], true);
        let fav_file = app.paths.favorites_dir.join("wallhaven-aa1.jpg");
        assert!(fav_file.is_file());
        let favorites: Favorites = state::read_json(&app.paths.state("favorites.json")).unwrap();
        assert_eq!(favorites.items[0].id, "aa1");

        // A stray file not matching the pattern is never touched.
        let stray = app.paths.favorites_dir.join("aa1.jpg");
        fs::write(&stray, JPEG).unwrap();

        // Toggle off and on again.
        assert_eq!(run(&app, &["fav"]).unwrap()["favorite"], false);
        assert!(!fav_file.exists());
        assert!(stray.exists());
        let favorites: Favorites = state::read_json(&app.paths.state("favorites.json")).unwrap();
        assert!(favorites.items.is_empty());
        assert_eq!(run(&app, &["fav"]).unwrap()["favorite"], true);
        assert_eq!(
            fs::read_to_string(&legacy).unwrap(),
            r#"{"version":1,"ids":["aa1"]}"#
        );
    }

    #[test]
    fn fav_by_id_saves_without_applying() {
        let app = test_app("app-fav-id", landscape(&["aa1", "aa2"]));
        run(&app, &["set", "sources", "landscape"]).unwrap();
        // No wallpaper applied yet; an id still works.
        let out = run(&app, &["fav", "aa2"]).unwrap();
        assert_eq!(
            (&out["id"], &out["favorite"]),
            (&json!("aa2"), &json!(true))
        );
        assert!(out["current"].is_null());
        let fav_file = app.paths.favorites_dir.join("wallhaven-aa2.jpg");
        assert!(fav_file.is_file());
        let favorites: Favorites = state::read_json(&app.paths.state("favorites.json")).unwrap();
        assert_eq!(favorites.items[0].id, "aa2");
        assert!(favorites.items[0].thumb.is_some());

        // Favoriting another wallpaper leaves the current one alone.
        assert_eq!(run(&app, &["next"]).unwrap()["current"]["id"], "aa1");
        assert_eq!(run(&app, &["fav", "aa2"]).unwrap()["favorite"], false);
        assert!(!fav_file.exists());
        assert_eq!(status_of(&app).current.unwrap().id, "aa1");

        // The current id behaves like plain `fav`.
        assert_eq!(run(&app, &["fav", "aa1"]).unwrap()["favorite"], true);
        assert!(app.paths.favorites_dir.join("wallhaven-aa1.jpg").is_file());
        assert_eq!(
            parse_args(&args(&["fav", "../x"])).unwrap_err().code,
            "usage"
        );
    }

    #[test]
    fn favs_source_uses_local_file_and_unfav_keeps_current() {
        let mut app = test_app("app-favs", landscape(&["aa1", "aa2"]));
        run(&app, &["set", "sources", "landscape"]).unwrap();
        run(&app, &["next"]).unwrap();
        run(&app, &["fav"]).unwrap();
        run(&app, &["next"]).unwrap();
        run(&app, &["set", "sources", "favs"]).unwrap();
        let out = run(&app, &["next"]).unwrap();
        assert_eq!(out["current"]["source"], "favs");
        let fav_file = app.paths.favorites_dir.join("wallhaven-aa1.jpg");
        assert_eq!(out["current"]["path"], fav_file.display().to_string());

        // Only favorite is current → nothing else to show.
        assert_eq!(run(&app, &["next"]).unwrap_err().code, "no_candidates");

        // Unfav of a favorites-backed current must repoint the desktop first;
        // if that fails, the favorite file stays.
        app.bg_set = "false".into();
        assert_eq!(run(&app, &["fav"]).unwrap_err().code, "set_failed");
        assert!(fav_file.is_file());
        app.bg_set = "true".into();

        // Unfav of a favorites-backed current repoints it to the cache copy.
        let out = run(&app, &["fav"]).unwrap();
        assert_eq!(out["favorite"], false);
        let path = out["current"]["path"].as_str().unwrap().to_string();
        assert!(path.ends_with("/full/wallhaven-aa1.jpg") && Path::new(&path).is_file());
    }

    #[test]
    fn browse_rewrites_status_only_when_the_total_changes() {
        let app = test_app("app-browse-status", landscape(&["aa1", "aa2"]));
        run(&app, &["browse", "landscape"]).unwrap();
        // Mark the file; an unchanged total must leave it alone, since every rewrite
        // makes the widget reload its status while the user scrolls.
        let path = app.paths.state("status.json");
        let mut status: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        status["updatedAt"] = json!(1);
        fs::write(&path, serde_json::to_vec(&status).unwrap()).unwrap();
        run(&app, &["browse", "landscape"]).unwrap();
        let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["updatedAt"], 1);
    }

    #[test]
    fn browse_tag_and_favs() {
        let app = test_app("app-browse", landscape(&["aa1", "aa2"]));
        let out = run(&app, &["browse", "landscape"]).unwrap();
        assert_eq!(
            (
                out["page"].as_u64(),
                out["lastPage"].as_u64(),
                out["total"].as_u64()
            ),
            (Some(1), Some(1), Some(2))
        );
        let first = &out["items"][0];
        assert_eq!(first["id"], "aa1");
        assert!(
            first["thumb"]
                .as_str()
                .unwrap()
                .ends_with("/thumbs/aa1.png")
        );
        assert_eq!(
            (first["seen"].clone(), first["current"].clone()),
            (json!(false), json!(false))
        );
        assert!(first.get("disliked").is_none());
        assert_eq!(status_of(&app).totals["tag:711|cat:100|color:none"], 2);
        assert_eq!(run(&app, &["browse", "nope"]).unwrap_err().code, "usage");

        let out = run(&app, &["browse", "favs", "--page", "2"]).unwrap();
        assert_eq!(out["lastPage"], 1);
        assert_eq!(out["items"], json!([]));
    }

    #[test]
    fn browse_apply_and_next_keep_working_from_saved_pages_while_wallhaven_is_down() {
        let t = std::sync::Arc::new(landscape(&["aa1", "aa2"]));
        let mut app = test_app("app-offline", FakeTransport::default());
        app.transport = Box::new(t.clone());
        run(&app, &["set", "sources", "landscape"]).unwrap();
        assert_eq!(
            run(&app, &["browse", "landscape"]).unwrap()["cached"],
            false
        );

        // The API goes down; the image servers still answer.
        let q = Query {
            tag: 711,
            categories: "100",
            color: None,
        };
        t.add(&q.url(1), 502, Vec::new());
        t.add("https://wallhaven.cc/api/v1/w/aa2", 525, Vec::new());
        let saved = run(&app, &["browse", "landscape"]).unwrap();
        assert_eq!(saved["cached"], true);
        assert_eq!(saved["items"][1]["id"], "aa2");
        assert!(saved["items"][1]["thumb"].is_string());

        assert_eq!(
            run(&app, &["apply", "aa2"]).unwrap()["current"]["id"],
            "aa2"
        );
        assert_eq!(run(&app, &["next"]).unwrap()["current"]["id"], "aa1");
    }

    #[test]
    fn apply_by_id() {
        let app = test_app("app-apply", landscape(&["aa1"]));
        let out = run(&app, &["apply", "aa1"]).unwrap();
        assert_eq!(out["current"]["id"], "aa1");
        assert_eq!(out["current"]["source"], Value::Null);
        assert_eq!(run(&app, &["apply", "zz9"]).unwrap_err().code, "not_found");
    }
}
