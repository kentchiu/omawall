use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// Reads a JSON state file; a missing file yields the default value.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            AppError::new("io", format!("Corrupt state file {}: {e}", path.display()))
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(AppError::io(format!("Cannot read {}", path.display()), e)),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("state serializes");
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Writes via a temp file in the same directory, then renames over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let fail = |e| AppError::io(format!("Cannot write {}", path.display()), e);
    let dir = path.parent().expect("state paths have a parent");
    fs::create_dir_all(dir).map_err(fail)?;
    let name = path
        .file_name()
        .expect("state paths have a name")
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(fail)
}

/// Exclusive process lock; released when dropped.
pub struct Lock(#[allow(dead_code)] File);

pub fn acquire_lock(state_dir: &Path, wait: Duration) -> Result<Lock> {
    acquire_named_lock(state_dir, "lock", wait)
}

/// Exclusive lock on `<state_dir>/<name>`; waits up to `wait`, then `busy`.
pub fn acquire_named_lock(state_dir: &Path, name: &str, wait: Duration) -> Result<Lock> {
    let path = state_dir.join(name);
    let fail = |e| AppError::io(format!("Cannot open lock file {}", path.display()), e);
    fs::create_dir_all(state_dir).map_err(fail)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(fail)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock(file)),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(AppError::new("busy", "omawall is busy; try again later"));
            }
            Err(fs::TryLockError::Error(e)) => return Err(fail(e)),
        }
    }
}

/// Whether another open file currently holds the lock on `path` (a missing file is free).
pub fn is_locked(path: &Path) -> bool {
    let Ok(file) = File::options().write(true).open(path) else {
        return false;
    };
    matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
}

fn v1() -> u32 {
    1
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryCursor {
    pub page: u64,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Seen {
    pub version: u32,
    pub queries: BTreeMap<String, QueryCursor>,
    pub favs: Vec<String>,
    pub all: Vec<String>,
}

impl Default for Seen {
    fn default() -> Self {
        Self {
            version: v1(),
            queries: BTreeMap::new(),
            favs: Vec::new(),
            all: Vec::new(),
        }
    }
}

pub fn push_unique(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|x| x == id) {
        list.push(id.to_string());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub id: String,
    pub path: String,
    pub thumb: Option<String>,
    pub resolution: String,
    pub favorites: u64,
    pub file_size: u64,
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    pub version: u32,
    /// Newest first.
    pub items: Vec<Favorite>,
}

impl Default for Favorites {
    fn default() -> Self {
        Self {
            version: v1(),
            items: Vec::new(),
        }
    }
}

impl Favorites {
    pub fn contains(&self, id: &str) -> bool {
        self.items.iter().any(|f| f.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    pub id: String,
    pub source: Option<String>,
    pub path: String,
    pub thumb: Option<String>,
    pub resolution: String,
    pub favorites: u64,
    pub file_size: u64,
    pub url: String,
    pub applied_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    pub code: String,
    pub message: String,
    pub trigger: String,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    pub version: u32,
    pub current: Option<Current>,
    pub last_error: Option<LastError>,
    pub totals: BTreeMap<String, u64>,
    pub updated_at: u64,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            version: v1(),
            current: None,
            last_error: None,
            totals: BTreeMap::new(),
            updated_at: 0,
        }
    }
}

impl Status {
    pub fn current_id(&self) -> Option<&str> {
        self.current.as_ref().map(|c| c.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_roundtrip_and_defaults() {
        let dir = crate::testutil::temp_dir("state");
        let path = dir.join("nested/seen.json");
        let missing: Seen = read_json(&path).unwrap();
        assert_eq!(missing, Seen::default());
        let mut seen = Seen::default();
        push_unique(&mut seen.all, "abc");
        push_unique(&mut seen.all, "abc");
        write_json(&path, &seen).unwrap();
        assert_eq!(read_json::<Seen>(&path).unwrap().all, ["abc"]);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::write(&path, "nope").unwrap();
        assert_eq!(read_json::<Seen>(&path).unwrap_err().code, "io");
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = crate::testutil::temp_dir("lock");
        let held = acquire_lock(&dir, Duration::ZERO).unwrap();
        let err = acquire_lock(&dir, Duration::from_millis(150))
            .err()
            .unwrap();
        assert_eq!(err.code, "busy");
        assert!(is_locked(&dir.join("lock")));
        drop(held);
        // Children forked by parallel tests may briefly inherit the fd, so poll.
        let deadline = Instant::now() + Duration::from_secs(5);
        while is_locked(&dir.join("lock")) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!is_locked(&dir.join("lock")));
        assert!(!is_locked(&dir.join("missing.lock")));
        assert!(acquire_lock(&dir, Duration::from_secs(5)).is_ok());
    }

    #[test]
    fn status_json_shape() {
        let value = serde_json::to_value(Status::default()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"version":1,"current":null,"lastError":null,"totals":{},"updatedAt":0})
        );
    }
}
