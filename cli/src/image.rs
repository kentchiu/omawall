use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{AppError, Result};
use crate::state::write_atomic;
use crate::wallhaven::{self, Transport};

pub const EXTENSIONS: [&str; 2] = ["jpg", "png"];
/// Thumbnails downloaded at once for a browse page.
pub const THUMB_PARALLEL: usize = 6;

/// File extension from magic bytes: JPEG `FF D8 FF`, PNG `89 50 4E 47 0D 0A 1A 0A`.
pub fn detect(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else {
        None
    }
}

/// Existing `<dir>/<stem>.<jpg|png>`.
pub fn cached(dir: &Path, stem: &str) -> Option<PathBuf> {
    EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// Returns the cached file or downloads `url` into `<dir>/<stem>.<detected ext>`.
pub fn ensure(
    transport: &dyn Transport,
    url: &str,
    timeout: Duration,
    limit: u64,
    dir: &Path,
    stem: &str,
) -> Result<PathBuf> {
    if let Some(path) = cached(dir, stem) {
        return Ok(path);
    }
    let bytes = wallhaven::fetch(transport, url, timeout, limit)?;
    let ext = detect(&bytes).ok_or_else(|| {
        AppError::new(
            "invalid_image",
            "Downloaded file is not a JPEG or PNG image",
        )
    })?;
    let path = dir.join(format!("{stem}.{ext}"));
    write_atomic(&path, &bytes)?;
    Ok(path)
}

pub fn ensure_full(
    transport: &dyn Transport,
    item: &wallhaven::Item,
    full_dir: &Path,
) -> Result<PathBuf> {
    let stem = format!("wallhaven-{}", item.id);
    ensure(
        transport,
        &item.full_url,
        wallhaven::FULL_TIMEOUT,
        wallhaven::FULL_LIMIT,
        full_dir,
        &stem,
    )
}

pub fn ensure_thumb(
    transport: &dyn Transport,
    item: &wallhaven::Item,
    thumbs_dir: &Path,
) -> Result<PathBuf> {
    ensure(
        transport,
        &item.thumb_url,
        wallhaven::THUMB_TIMEOUT,
        wallhaven::THUMB_LIMIT,
        thumbs_dir,
        &item.id,
    )
}

/// Thumbnails for a page of items, `THUMB_PARALLEL` downloads at a time.
/// Each result sits at its item's index.
pub fn ensure_thumbs(
    transport: &dyn Transport,
    items: &[wallhaven::Item],
    thumbs_dir: &Path,
) -> Vec<Result<PathBuf>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<PathBuf>>>> =
        items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..THUMB_PARALLEL.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    let result = ensure_thumb(transport, item, thumbs_dir);
                    *slots[i].lock().expect("slot lock") = Some(result);
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("slot lock")
                .expect("every item was handled")
        })
        .collect()
}

/// Extension of a local image file by its magic bytes.
pub fn detect_file(path: &Path) -> Result<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 8];
    let mut file = std::fs::File::open(path)
        .map_err(|e| AppError::io(format!("Cannot read {}", path.display()), e))?;
    let n = file
        .read(&mut head)
        .map_err(|e| AppError::io(format!("Cannot read {}", path.display()), e))?;
    detect(&head[..n]).ok_or_else(|| {
        AppError::new(
            "invalid_image",
            format!("Not a JPEG or PNG image: {}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeTransport, JPEG, PNG};

    #[test]
    fn magic_bytes() {
        assert_eq!(detect(JPEG), Some("jpg"));
        assert_eq!(detect(PNG), Some("png"));
        assert_eq!(detect(b"GIF89a"), None);
        assert_eq!(detect(&[0xFF, 0xD8]), None);
        assert_eq!(detect(&PNG[..7]), None);
        assert_eq!(detect(b""), None);
    }

    #[test]
    fn ensure_uses_detected_extension_and_cache() {
        let dir = crate::testutil::temp_dir("image");
        let t = FakeTransport::default();
        t.add("https://th.wallhaven.cc/lg/ab/abc.jpg", 200, PNG.to_vec());
        t.add(
            "https://th.wallhaven.cc/lg/ab/bad.jpg",
            200,
            b"<html>".to_vec(),
        );
        let get = |url, stem| {
            ensure(
                &t,
                url,
                wallhaven::THUMB_TIMEOUT,
                wallhaven::THUMB_LIMIT,
                &dir,
                stem,
            )
        };
        let path = get("https://th.wallhaven.cc/lg/ab/abc.jpg", "abc").unwrap();
        assert_eq!(path, dir.join("abc.png"));
        assert_eq!(
            get("https://th.wallhaven.cc/lg/ab/abc.jpg", "abc").unwrap(),
            path
        );
        assert_eq!(t.calls().len(), 1);
        assert_eq!(
            get("https://th.wallhaven.cc/lg/ab/bad.jpg", "bad")
                .unwrap_err()
                .code,
            "invalid_image"
        );
        assert!(cached(&dir, "bad").is_none());
        assert_eq!(detect_file(&path).unwrap(), "png");
    }

    /// Records how many downloads run at once.
    struct SlowTransport {
        inner: FakeTransport,
        in_flight: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
    }

    impl Transport for SlowTransport {
        fn get(
            &self,
            url: &str,
            timeout: std::time::Duration,
            limit: u64,
        ) -> Result<(u16, Vec<u8>)> {
            use std::sync::atomic::Ordering::SeqCst;
            let now = self.in_flight.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(now, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            self.in_flight.fetch_sub(1, SeqCst);
            self.inner.get(url, timeout, limit)
        }
    }

    #[test]
    fn ensure_thumbs_downloads_in_parallel_and_keeps_order() {
        let dir = crate::testutil::temp_dir("image-parallel");
        let t = SlowTransport {
            inner: FakeTransport::default(),
            in_flight: Default::default(),
            peak: Default::default(),
        };
        let ids: Vec<String> = (0..10).map(|i| format!("id{i}")).collect();
        let items: Vec<wallhaven::Item> = ids
            .iter()
            .map(|id| wallhaven::Item {
                id: id.clone(),
                full_url: format!("https://w.wallhaven.cc/full/id/wallhaven-{id}.jpg"),
                thumb_url: format!("https://th.wallhaven.cc/lg/id/{id}.jpg"),
                resolution: "3840x2160".into(),
                favorites: 1,
                file_size: 1,
            })
            .collect();
        for item in &items[..9] {
            t.inner.add(&item.thumb_url, 200, JPEG.to_vec());
        }
        let results = ensure_thumbs(&t, &items, &dir);
        assert_eq!(results.len(), 10);
        for (id, result) in ids.iter().zip(&results[..9]) {
            assert_eq!(result.as_ref().unwrap(), &dir.join(format!("{id}.jpg")));
        }
        // The missing one fails in its own slot.
        assert!(results[9].is_err());
        let peak = t.peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(peak > 1 && peak <= THUMB_PARALLEL, "peak {peak}");
    }
}
