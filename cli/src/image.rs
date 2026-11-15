use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{AppError, Result};
use crate::state::write_atomic;
use crate::wallhaven::{self, Transport};

pub const EXTENSIONS: [&str; 2] = ["jpg", "png"];

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
    let ext = detect(&bytes)
        .ok_or_else(|| AppError::new("invalid_image", "下載的檔案不是 JPEG 或 PNG 圖片"))?;
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

/// Extension of a local image file by its magic bytes.
pub fn detect_file(path: &Path) -> Result<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 8];
    let mut file = std::fs::File::open(path)
        .map_err(|e| AppError::io(format!("無法讀取 {}", path.display()), e))?;
    let n = file
        .read(&mut head)
        .map_err(|e| AppError::io(format!("無法讀取 {}", path.display()), e))?;
    detect(&head[..n]).ok_or_else(|| {
        AppError::new(
            "invalid_image",
            format!("不是 JPEG 或 PNG 圖片：{}", path.display()),
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
}
