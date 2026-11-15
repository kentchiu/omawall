use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const FULL_KEEP: usize = 20;
pub const THUMBS_MAX_BYTES: u64 = 2 << 30;

/// Regular files in `dir`, newest first by mtime.
fn files_newest_first(dir: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    e.path(),
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    meta.len(),
                )
            })
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files
}

/// Keeps the `keep` newest files; never deletes `current`. Best effort.
pub fn evict_full(dir: &Path, keep: usize, current: Option<&Path>) {
    for (path, _, _) in files_newest_first(dir).into_iter().skip(keep) {
        if Some(path.as_path()) != current {
            let _ = fs::remove_file(path);
        }
    }
}

/// Deletes oldest files until the total size is at most `max_bytes`. Best effort.
pub fn evict_thumbs(dir: &Path, max_bytes: u64) {
    let files = files_newest_first(dir);
    let mut total: u64 = files.iter().map(|f| f.2).sum();
    for (path, _, len) in files.into_iter().rev() {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make(dir: &Path, name: &str, age_secs: u64, len: usize) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, vec![0u8; len]).unwrap();
        let mtime = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        path
    }

    #[test]
    fn full_keeps_newest_and_current() {
        let dir = crate::testutil::temp_dir("cache-full");
        for i in 0..25 {
            make(&dir, &format!("wallhaven-{i}.jpg"), 100 + i, 1);
        }
        // Oldest file is the current wallpaper.
        let current = dir.join("wallhaven-24.jpg");
        evict_full(&dir, FULL_KEEP, Some(&current));
        let left = files_newest_first(&dir);
        assert_eq!(left.len(), 21);
        assert!(current.exists());
        assert!(dir.join("wallhaven-0.jpg").exists());
        assert!(dir.join("wallhaven-19.jpg").exists());
        assert!(!dir.join("wallhaven-20.jpg").exists());
    }

    #[test]
    fn thumbs_capped_oldest_first() {
        let dir = crate::testutil::temp_dir("cache-thumbs");
        let oldest = make(&dir, "a.jpg", 300, 40);
        let middle = make(&dir, "b.jpg", 200, 40);
        let newest = make(&dir, "c.jpg", 100, 40);
        evict_thumbs(&dir, 100);
        assert!(!oldest.exists());
        assert!(middle.exists() && newest.exists());
        evict_thumbs(&dir, 1000);
        assert!(middle.exists());
    }
}
