use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const API_TIMEOUT: Duration = Duration::from_secs(15);
pub const THUMB_TIMEOUT: Duration = Duration::from_secs(15);
pub const FULL_TIMEOUT: Duration = Duration::from_secs(90);
pub const API_LIMIT: u64 = 2 << 20;
pub const THUMB_LIMIT: u64 = 1 << 20;
pub const FULL_LIMIT: u64 = 30 << 20;

const ALLOWED_HOSTS: [&str; 3] = ["wallhaven.cc", "w.wallhaven.cc", "th.wallhaven.cc"];
const FULL_PREFIX: &str = "https://w.wallhaven.cc/full/";
const THUMB_PREFIX: &str = "https://th.wallhaven.cc/lg/";

/// Minimal HTTP GET. Returns the status code and at most `limit` + 1 body bytes
/// (or a `too_large` error); must never follow redirects.
/// `Sync` so a page of thumbnails can download on several threads.
pub trait Transport: Sync {
    fn get(&self, url: &str, timeout: Duration, limit: u64) -> Result<(u16, Vec<u8>)>;
}

pub struct UreqTransport {
    agent: ureq::Agent,
}

impl UreqTransport {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(concat!("omawall/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }
}

impl Transport for UreqTransport {
    fn get(&self, url: &str, timeout: Duration, limit: u64) -> Result<(u16, Vec<u8>)> {
        let mut response = self
            .agent
            .get(url)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
            .map_err(map_ureq_error)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Ok((status, Vec::new()));
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(map_ureq_error)?;
        Ok((status, body))
    }
}

fn map_ureq_error(err: ureq::Error) -> AppError {
    match err {
        ureq::Error::Timeout(_) => timeout_error(),
        ureq::Error::Io(e) if e.kind() == std::io::ErrorKind::TimedOut => timeout_error(),
        ureq::Error::BodyExceedsLimit(_) => too_large(),
        other => AppError::new("network", format!("Cannot reach Wallhaven: {other}")),
    }
}

fn timeout_error() -> AppError {
    AppError::new("timeout", "Wallhaven timed out")
}

fn too_large() -> AppError {
    AppError::new("too_large", "Download exceeds the size limit")
}

fn http_error(status: u16) -> AppError {
    if status >= 500 {
        return AppError::new(
            "unavailable",
            format!("Wallhaven is having trouble right now (HTTP {status}); try again later"),
        );
    }
    AppError::new("http", format!("Wallhaven returned HTTP {status}"))
}

/// Failures worth one retry and a fall back to saved pages: Wallhaven is slow, down or busy.
pub fn is_transient(e: &AppError) -> bool {
    matches!(
        e.code,
        "network" | "timeout" | "unavailable" | "rate_limited"
    )
}

pub fn invalid_response(why: &str) -> AppError {
    AppError::new(
        "invalid_response",
        format!("Invalid Wallhaven response: {why}"),
    )
}

pub fn is_valid_id(id: &str) -> bool {
    (1..=16).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Only plain `https://<allowed host>/...` URLs (no port, no userinfo).
fn is_allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    ALLOWED_HOSTS.contains(&host)
}

/// GET with host allow-list, status mapping and size limit.
pub fn fetch(
    transport: &dyn Transport,
    url: &str,
    timeout: Duration,
    limit: u64,
) -> Result<Vec<u8>> {
    if !is_allowed_url(url) {
        return Err(invalid_response("URL not allowed"));
    }
    let (status, body) = transport.get(url, timeout, limit)?;
    match status {
        200..=299 => {}
        429 => {
            return Err(AppError::new(
                "rate_limited",
                "Too many Wallhaven requests; try again later",
            ));
        }
        _ => return Err(http_error(status)),
    }
    if body.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(body)
}

/// One tag search with the user's filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub tag: u64,
    pub categories: &'static str,
    pub color: Option<&'static str>,
}

impl Query {
    pub fn key(&self) -> String {
        format!(
            "tag:{}|cat:{}|color:{}",
            self.tag,
            self.categories,
            self.color.unwrap_or("none")
        )
    }

    pub fn url(&self, page: u64) -> String {
        let mut url = format!(
            "https://wallhaven.cc/api/v1/search?q=id:{}&categories={}&purity=100&atleast=3840x2160&ratios=16x9&sorting=favorites&order=desc&page={page}",
            self.tag, self.categories
        );
        if let Some(color) = self.color {
            url.push_str("&colors=");
            url.push_str(color);
        }
        url
    }
}

/// A validated wallpaper entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub full_url: String,
    pub thumb_url: String,
    pub resolution: String,
    pub favorites: u64,
    pub file_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPage {
    pub last_page: u64,
    pub total: u64,
    pub items: Vec<Item>,
}

#[derive(Deserialize)]
struct RawThumbs {
    large: String,
}

#[derive(Deserialize)]
struct RawItem {
    id: String,
    path: String,
    thumbs: RawThumbs,
    resolution: String,
    favorites: u64,
    file_size: u64,
    #[allow(dead_code)]
    file_type: String,
}

#[derive(Deserialize)]
struct RawMeta {
    last_page: u64,
    total: u64,
}

#[derive(Deserialize)]
struct RawSearch {
    data: Vec<RawItem>,
    meta: RawMeta,
}

#[derive(Deserialize)]
struct RawDetail {
    data: RawItem,
}

fn validate(raw: RawItem) -> Result<Item> {
    if !is_valid_id(&raw.id) {
        return Err(invalid_response("invalid id"));
    }
    if !raw.path.starts_with(FULL_PREFIX) || !is_allowed_url(&raw.path) {
        return Err(invalid_response("invalid image URL"));
    }
    if !raw.thumbs.large.starts_with(THUMB_PREFIX) || !is_allowed_url(&raw.thumbs.large) {
        return Err(invalid_response("invalid thumbnail URL"));
    }
    Ok(Item {
        id: raw.id,
        full_url: raw.path,
        thumb_url: raw.thumbs.large,
        resolution: raw.resolution,
        favorites: raw.favorites,
        file_size: raw.file_size,
    })
}

pub fn parse_search(bytes: &[u8]) -> Result<SearchPage> {
    let raw: RawSearch =
        serde_json::from_slice(bytes).map_err(|e| invalid_response(&e.to_string()))?;
    Ok(SearchPage {
        last_page: raw.meta.last_page,
        total: raw.meta.total,
        items: raw.data.into_iter().map(validate).collect::<Result<_>>()?,
    })
}

pub fn parse_detail(bytes: &[u8], id: &str) -> Result<Item> {
    let raw: RawDetail =
        serde_json::from_slice(bytes).map_err(|e| invalid_response(&e.to_string()))?;
    let item = validate(raw.data)?;
    if item.id != id {
        return Err(invalid_response("id mismatch"));
    }
    Ok(item)
}

pub fn search(transport: &dyn Transport, query: &Query, page: u64) -> Result<SearchPage> {
    parse_search(&fetch(transport, &query.url(page), API_TIMEOUT, API_LIMIT)?)
}

pub fn detail(transport: &dyn Transport, id: &str) -> Result<Item> {
    let url = format!("https://wallhaven.cc/api/v1/w/{id}");
    match fetch(transport, &url, API_TIMEOUT, API_LIMIT) {
        Ok(bytes) => parse_detail(&bytes, id),
        Err(e) if e == http_error(404) => Err(AppError::new(
            "not_found",
            format!("Wallpaper {id} not found on Wallhaven"),
        )),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::testutil::FakeTransport;

    pub fn item_json(id: &str) -> serde_json::Value {
        let p = &id[..2.min(id.len())];
        serde_json::json!({
            "id": id,
            "path": format!("https://w.wallhaven.cc/full/{p}/wallhaven-{id}.jpg"),
            "thumbs": {"large": format!("https://th.wallhaven.cc/lg/{p}/{id}.jpg"), "small": "x"},
            "resolution": "3840x2160", "favorites": 10, "file_size": 1000, "file_type": "image/jpeg",
            "category": "general"
        })
    }

    pub fn search_json(ids: &[&str], last_page: u64, total: u64) -> Vec<u8> {
        let data: Vec<_> = ids.iter().map(|id| item_json(id)).collect();
        serde_json::to_vec(&serde_json::json!({
            "data": data, "meta": {"current_page": 1, "last_page": last_page, "per_page": "24", "total": total}
        }))
        .unwrap()
    }

    #[test]
    fn query_key_and_url() {
        let q = Query {
            tag: 711,
            categories: "100",
            color: None,
        };
        assert_eq!(q.key(), "tag:711|cat:100|color:none");
        assert_eq!(
            q.url(2),
            "https://wallhaven.cc/api/v1/search?q=id:711&categories=100&purity=100&atleast=3840x2160&ratios=16x9&sorting=favorites&order=desc&page=2"
        );
        let q = Query {
            tag: 37,
            categories: "110",
            color: Some("66cccc"),
        };
        assert_eq!(q.key(), "tag:37|cat:110|color:66cccc");
        assert!(q.url(1).ends_with("&page=1&colors=66cccc"));
    }

    #[test]
    fn id_validation() {
        assert!(is_valid_id("285e6x"));
        for bad in ["", "ABC", "a/b", "../x", "a b", &"a".repeat(17)] {
            assert!(!is_valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn response_validation() {
        assert_eq!(
            parse_search(&search_json(&["abc123"], 5, 100))
                .unwrap()
                .items[0]
                .id,
            "abc123"
        );

        let mut bad_host = item_json("abc123");
        bad_host["path"] = "https://evil.example/full/ab/wallhaven-abc123.jpg".into();
        let mut thumb_host = item_json("abc123");
        thumb_host["thumbs"]["large"] =
            "https://th.wallhaven.cc.evil.example/lg/ab/abc123.jpg".into();
        let mut http = item_json("abc123");
        http["path"] = "http://w.wallhaven.cc/full/ab/wallhaven-abc123.jpg".into();
        let mut bad_id = item_json("abc123");
        bad_id["id"] = "../../etc".into();
        for item in [bad_host, thumb_host, http, bad_id] {
            let body = serde_json::to_vec(
                &serde_json::json!({"data": [item], "meta": {"last_page": 1, "total": 1}}),
            )
            .unwrap();
            assert_eq!(parse_search(&body).unwrap_err().code, "invalid_response");
        }
        assert_eq!(parse_search(b"{}").unwrap_err().code, "invalid_response");

        let detail = serde_json::to_vec(&serde_json::json!({"data": item_json("abc123")})).unwrap();
        assert_eq!(parse_detail(&detail, "abc123").unwrap().id, "abc123");
        assert_eq!(
            parse_detail(&detail, "zzz").unwrap_err().code,
            "invalid_response"
        );
    }

    #[test]
    fn fetch_rules() {
        let t = FakeTransport::default();
        t.add(
            "https://wallhaven.cc/big",
            200,
            vec![b' '; (API_LIMIT + 1) as usize],
        );
        t.add("https://wallhaven.cc/slow", 429, vec![]);
        t.add("https://wallhaven.cc/gone", 404, vec![]);
        t.add("https://wallhaven.cc/redirect", 302, vec![]);
        t.add("https://wallhaven.cc/down", 502, vec![]);
        t.add("https://wallhaven.cc/tls", 525, vec![]);
        let get = |u: &str| fetch(&t, u, API_TIMEOUT, API_LIMIT).unwrap_err().code;
        assert_eq!(get("https://wallhaven.cc/big"), "too_large");
        assert_eq!(get("https://wallhaven.cc/slow"), "rate_limited");
        assert_eq!(get("https://wallhaven.cc/gone"), "http");
        assert_eq!(get("https://wallhaven.cc/redirect"), "http");
        assert_eq!(get("http://wallhaven.cc/x"), "invalid_response");
        assert_eq!(get("https://wallhaven.cc:8443/x"), "invalid_response");
        assert_eq!(get("https://user@wallhaven.cc/x"), "invalid_response");
        assert_eq!(get("https://example.com/x"), "invalid_response");
        // Disallowed URLs never reach the transport.
        assert_eq!(t.calls().len(), 4);
        // Cloudflare's 5xx pages mean Wallhaven is down, not that the request was wrong.
        assert_eq!(get("https://wallhaven.cc/down"), "unavailable");
        assert_eq!(get("https://wallhaven.cc/tls"), "unavailable");
        assert!(is_transient(
            &fetch(&t, "https://wallhaven.cc/down", API_TIMEOUT, API_LIMIT).unwrap_err()
        ));
        assert!(!is_transient(
            &fetch(&t, "https://wallhaven.cc/gone", API_TIMEOUT, API_LIMIT).unwrap_err()
        ));
    }

    #[test]
    fn detail_404_is_not_found() {
        let t = FakeTransport::default();
        t.add("https://wallhaven.cc/api/v1/w/abc", 404, vec![]);
        assert_eq!(detail(&t, "abc").unwrap_err().code, "not_found");
    }
}
