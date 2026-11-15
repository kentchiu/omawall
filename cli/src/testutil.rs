//! Test helpers: temp dirs and a fake HTTP transport.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::error::Result;
use crate::wallhaven::Transport;

pub const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10];
pub const PNG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0];

pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omawall-test-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Serves canned responses by exact URL; unknown URLs return 404.
#[derive(Default)]
pub struct FakeTransport {
    responses: RefCell<HashMap<String, (u16, Vec<u8>)>>,
    calls: RefCell<Vec<String>>,
}

impl FakeTransport {
    pub fn add(&self, url: &str, status: u16, body: Vec<u8>) {
        self.responses
            .borrow_mut()
            .insert(url.to_string(), (status, body));
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }
}

impl Transport for FakeTransport {
    fn get(&self, url: &str, _timeout: Duration, _limit: u64) -> Result<(u16, Vec<u8>)> {
        self.calls.borrow_mut().push(url.to_string());
        Ok(self
            .responses
            .borrow()
            .get(url)
            .cloned()
            .unwrap_or((404, Vec::new())))
    }
}
