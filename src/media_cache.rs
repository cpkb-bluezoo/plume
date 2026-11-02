/*
 * media_cache.rs
 * Copyright (C) 2026 Chris Burdess
 *
 * This file is part of Plume, a Nostr desktop client.
 *
 * Plume is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * Plume is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with Plume.  If not, see <http://www.gnu.org/licenses/>.
 */

//! On-disk HTTP cache backing `media_proxy.rs`'s `torimg://` fetches, so an
//! avatar/banner/image already shown once doesn't re-dial through Tor every
//! time it's painted again on another page. Honors `Cache-Control`
//! (`no-store`, `no-cache`, `max-age`) plus `ETag`/`Last-Modified`
//! validators for cheap conditional revalidation, the same way a browser's
//! own HTTP cache would.
//!
//! Scope deliberately excludes `Expires` (an absolute HTTP-date, which
//! would need a date parser this codebase otherwise has no use for) and the
//! shared-cache-only directives (`public`/`private`/`s-maxage`/...) -- this
//! is a private, single-profile cache, not a shared one. A response with no
//! usable freshness info at all still gets a bounded [`HEURISTIC_FRESHNESS_SECS`]
//! window rather than being refetched on every single paint (RFC 9111
//! §4.2.2 permits heuristic freshness whenever no explicit expiration is
//! given).
//!
//! One entry per source URL, keyed by [`crypto::sha256_hex`] of the URL
//! (stable across runs, sidesteps every filesystem-unsafe character a URL
//! can contain): `<key>.body` (raw response bytes) and `<key>.meta` (a
//! small `key=value` sidecar -- this codebase otherwise reaches for its own
//! push-model JSON parser for wire formats with actual nested structure,
//! which this flat handful of fields doesn't have).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use rand::Rng;

use crate::crypto;
use crate::debug_log;

const HEURISTIC_FRESHNESS_SECS: u64 = 3600;

/// Soft cap on total cache size. Checked probabilistically on writes
/// ([`EVICT_CHECK_PROBABILITY`]) rather than on every single one, since an
/// exact running total isn't worth tracking for a cache this is purely
/// advisory for.
const MAX_CACHE_BYTES: u64 = 500 * 1024 * 1024;
const EVICT_CHECK_PROBABILITY: f64 = 0.02;

static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Call once at startup with the app's base config directory (`~/.plume`).
pub fn init(base_dir: &str) {
    let dir = Path::new(base_dir).join("media_cache");
    if let Err(e) = fs::create_dir_all(&dir) {
        debug_log!("[media_cache] could not create {dir:?}: {e}");
    }
    let _ = CACHE_DIR.set(dir);
}

/// Falls back to a per-process temp directory if [`init`] was never called
/// -- normally only in tests, which exercise this module without going
/// through `main`'s startup sequence at all.
fn cache_dir() -> &'static Path {
    CACHE_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("plume-media-cache-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    })
}

fn key_for(url: &str) -> String {
    crypto::sha256_hex(url.as_bytes())
}

fn body_path(key: &str) -> PathBuf {
    cache_dir().join(format!("{key}.body"))
}

fn meta_path(key: &str) -> PathBuf {
    cache_dir().join(format!("{key}.meta"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// A cached response, ready to hand straight back to the webview.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// A stale entry worth revalidating with a conditional request rather than
/// discarding outright.
#[derive(Debug, Clone)]
pub struct StaleEntry {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub entry: CacheEntry,
}

#[derive(Debug)]
pub enum Lookup {
    /// Still within its freshness window -- serve straight from disk, no
    /// network at all.
    Fresh(CacheEntry),
    /// Expired, but has a validator to revalidate cheaply (and to fall back
    /// to if that revalidation, or a plain fetch, fails outright).
    Stale(StaleEntry),
    /// Nothing usable cached: no entry, or an expired one with no validator
    /// (a conditional request would be a full request anyway).
    Miss,
}

/// Look up `url` in the cache.
pub fn lookup(url: &str) -> Lookup {
    let key = key_for(url);
    let Some(meta) = read_meta(&key) else { return Lookup::Miss };
    let Ok(body) = fs::read(body_path(&key)) else { return Lookup::Miss };
    let entry = CacheEntry { status: meta.status, content_type: meta.content_type.clone(), body };
    if now_secs() < meta.expires_at {
        touch(&key);
        Lookup::Fresh(entry)
    } else if meta.etag.is_some() || meta.last_modified.is_some() {
        Lookup::Stale(StaleEntry { etag: meta.etag, last_modified: meta.last_modified, entry })
    } else {
        Lookup::Miss
    }
}

/// Store a fresh `2xx` response. A no-op (and leaves any previously cached
/// copy untouched) when `Cache-Control: no-store` is present, or the status
/// isn't actually a success.
pub fn store(
    url: &str,
    status: u16,
    content_type: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    cache_control: Option<&str>,
    body: &[u8],
) {
    if !(200..300).contains(&status) {
        return;
    }
    let cc = CacheControlDirectives::parse(cache_control.unwrap_or(""));
    if cc.no_store {
        return;
    }
    let key = key_for(url);
    let now = now_secs();
    let meta = Meta {
        status,
        content_type: content_type.map(String::from),
        etag: etag.map(String::from),
        last_modified: last_modified.map(String::from),
        fetched_at: now,
        expires_at: now.saturating_add(cc.freshness_secs()),
    };
    if let Err(e) = fs::write(body_path(&key), body) {
        debug_log!("[media_cache] write failed for {url}: {e}");
        return;
    }
    write_meta(&key, &meta);
    maybe_evict();
}

/// A `304 Not Modified` on a conditional request: the cached body is still
/// current, so keep it and just refresh the freshness window (and
/// validators, since RFC 9111 §4.3.4 lets a `304` carry updated ones).
/// A no-op if nothing is cached for `url` any more (lost a race with an
/// eviction, say).
pub fn touch_fresh(url: &str, etag: Option<&str>, last_modified: Option<&str>, cache_control: Option<&str>) {
    let key = key_for(url);
    let Some(mut meta) = read_meta(&key) else { return };
    let cc = CacheControlDirectives::parse(cache_control.unwrap_or(""));
    if cc.no_store {
        let _ = fs::remove_file(body_path(&key));
        let _ = fs::remove_file(meta_path(&key));
        return;
    }
    let now = now_secs();
    meta.fetched_at = now;
    meta.expires_at = now.saturating_add(cc.freshness_secs());
    if let Some(e) = etag {
        meta.etag = Some(e.to_string());
    }
    if let Some(lm) = last_modified {
        meta.last_modified = Some(lm.to_string());
    }
    write_meta(&key, &meta);
}

/// Bump the meta file's mtime so a resource served straight from the
/// freshness window (no network round trip, so nothing else touches this
/// file) still counts as recently used for [`maybe_evict`]'s purposes.
fn touch(key: &str) {
    if let Ok(file) = fs::File::open(meta_path(key)) {
        let _ = file.set_modified(SystemTime::now());
    }
}

#[derive(Default)]
struct CacheControlDirectives {
    no_store: bool,
    no_cache: bool,
    max_age: Option<u64>,
}

impl CacheControlDirectives {
    fn parse(value: &str) -> Self {
        let mut out = Self::default();
        for directive in value.split(',') {
            let directive = directive.trim();
            if let Some((name, val)) = directive.split_once('=') {
                if name.trim().eq_ignore_ascii_case("max-age") {
                    out.max_age = val.trim().trim_matches('"').parse::<u64>().ok();
                }
            } else if directive.eq_ignore_ascii_case("no-store") {
                out.no_store = true;
            } else if directive.eq_ignore_ascii_case("no-cache")
                || directive.eq_ignore_ascii_case("must-revalidate")
            {
                out.no_cache = true;
            }
        }
        out
    }

    /// How long a freshly-stored response may be served without
    /// revalidation.
    fn freshness_secs(&self) -> u64 {
        if self.no_cache {
            0
        } else {
            self.max_age.unwrap_or(HEURISTIC_FRESHNESS_SECS)
        }
    }
}

struct Meta {
    status: u16,
    content_type: Option<String>,
    etag: Option<String>,
    last_modified: Option<String>,
    fetched_at: u64,
    expires_at: u64,
}

fn sanitize(value: &str) -> String {
    value.chars().filter(|c| *c != '\n' && *c != '\r').collect()
}

fn write_meta(key: &str, meta: &Meta) {
    let path = meta_path(key);
    let mut out = String::new();
    out.push_str(&format!("status={}\n", meta.status));
    if let Some(ct) = &meta.content_type {
        out.push_str(&format!("content_type={}\n", sanitize(ct)));
    }
    if let Some(e) = &meta.etag {
        out.push_str(&format!("etag={}\n", sanitize(e)));
    }
    if let Some(lm) = &meta.last_modified {
        out.push_str(&format!("last_modified={}\n", sanitize(lm)));
    }
    out.push_str(&format!("fetched_at={}\n", meta.fetched_at));
    out.push_str(&format!("expires_at={}\n", meta.expires_at));
    if let Err(e) = fs::write(&path, out) {
        debug_log!("[media_cache] meta write failed for {path:?}: {e}");
    }
}

fn read_meta(key: &str) -> Option<Meta> {
    let text = fs::read_to_string(meta_path(key)).ok()?;
    let mut status = None;
    let mut content_type = None;
    let mut etag = None;
    let mut last_modified = None;
    let mut fetched_at = None;
    let mut expires_at = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "status" => status = v.parse::<u16>().ok(),
            "content_type" => content_type = Some(v.to_string()),
            "etag" => etag = Some(v.to_string()),
            "last_modified" => last_modified = Some(v.to_string()),
            "fetched_at" => fetched_at = v.parse::<u64>().ok(),
            "expires_at" => expires_at = v.parse::<u64>().ok(),
            _ => {}
        }
    }
    Some(Meta {
        status: status?,
        content_type,
        etag,
        last_modified,
        fetched_at: fetched_at?,
        expires_at: expires_at?,
    })
}

fn maybe_evict() {
    if !rand::thread_rng().gen_bool(EVICT_CHECK_PROBABILITY) {
        return;
    }
    evict_to_cap(MAX_CACHE_BYTES);
}

/// Oldest-by-last-use eviction (see [`touch`] for how "last use" stays
/// accurate even for entries served without a network round trip) down to
/// `max_bytes`, should the cache have grown past it. A plain function of
/// the cap (rather than hardcoding [`MAX_CACHE_BYTES`]) so tests can check
/// the eviction logic itself without writing out hundreds of megabytes.
fn evict_to_cap(max_bytes: u64) {
    let Ok(read_dir) = fs::read_dir(cache_dir()) else { return };

    let mut entries: Vec<(PathBuf, PathBuf, u64, SystemTime)> = Vec::new();
    let mut total: u64 = 0;
    for entry in read_dir.flatten() {
        let meta_path = entry.path();
        if meta_path.extension().and_then(|e| e.to_str()) != Some("meta") {
            continue;
        }
        let Ok(fs_meta) = entry.metadata() else { continue };
        let mtime = fs_meta.modified().unwrap_or(UNIX_EPOCH);
        let body_path = meta_path.with_extension("body");
        let size = fs::metadata(&body_path).map(|m| m.len()).unwrap_or(0);
        total += size;
        entries.push((meta_path, body_path, size, mtime));
    }

    if total <= max_bytes {
        return;
    }
    entries.sort_by_key(|(_, _, _, mtime)| *mtime);
    for (meta_path, body_path, size, _) in entries {
        if total <= max_bytes {
            break;
        }
        let _ = fs::remove_file(&meta_path);
        let _ = fs::remove_file(&body_path);
        total = total.saturating_sub(size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_control_parses_directives_this_cache_understands() {
        let cc = CacheControlDirectives::parse("public, max-age=600, must-revalidate");
        assert_eq!(cc.max_age, Some(600));
        assert!(!cc.no_store);
        assert!(cc.no_cache); // must-revalidate folds into "revalidate once stale"

        let cc = CacheControlDirectives::parse("no-store");
        assert!(cc.no_store);

        let cc = CacheControlDirectives::parse("");
        assert_eq!(cc.freshness_secs(), HEURISTIC_FRESHNESS_SECS);
    }

    #[test]
    fn store_then_lookup_round_trips_a_fresh_entry() {
        let url = "https://example.test/avatar-fresh.jpg";
        store(url, 200, Some("image/jpeg"), Some("\"v1\""), None, Some("max-age=600"), b"bytes");
        match lookup(url) {
            Lookup::Fresh(entry) => {
                assert_eq!(entry.status, 200);
                assert_eq!(entry.content_type, Some("image/jpeg".to_string()));
                assert_eq!(entry.body, b"bytes");
            }
            other => panic!("expected Fresh, got {other:?}"),
        }
    }

    #[test]
    fn no_store_is_never_persisted() {
        let url = "https://example.test/avatar-no-store.jpg";
        store(url, 200, Some("image/jpeg"), None, None, Some("no-store"), b"bytes");
        assert!(matches!(lookup(url), Lookup::Miss));
    }

    #[test]
    fn expired_entry_with_a_validator_comes_back_stale_not_miss() {
        let url = "https://example.test/avatar-stale.jpg";
        store(url, 200, Some("image/jpeg"), Some("\"v1\""), None, Some("max-age=0"), b"bytes");
        match lookup(url) {
            Lookup::Stale(stale) => {
                assert_eq!(stale.etag, Some("\"v1\"".to_string()));
                assert_eq!(stale.entry.body, b"bytes");
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn expired_entry_without_a_validator_is_a_miss() {
        let url = "https://example.test/avatar-stale-no-validator.jpg";
        store(url, 200, Some("image/jpeg"), None, None, Some("max-age=0"), b"bytes");
        assert!(matches!(lookup(url), Lookup::Miss));
    }

    #[test]
    fn touch_fresh_extends_the_freshness_window_and_keeps_the_body() {
        let url = "https://example.test/avatar-304.jpg";
        store(url, 200, Some("image/jpeg"), Some("\"v1\""), None, Some("max-age=0"), b"bytes");
        assert!(matches!(lookup(url), Lookup::Stale(_)));
        touch_fresh(url, Some("\"v2\""), None, Some("max-age=600"));
        match lookup(url) {
            Lookup::Fresh(entry) => assert_eq!(entry.body, b"bytes"),
            other => panic!("expected Fresh after touch_fresh, got {other:?}"),
        }
    }

    #[test]
    fn eviction_drops_oldest_entries_first_until_under_the_cap() {
        let urls = [
            "https://example.test/evict-a.jpg",
            "https://example.test/evict-b.jpg",
            "https://example.test/evict-c.jpg",
        ];
        for url in urls {
            store(url, 200, Some("image/jpeg"), None, None, Some("max-age=600"), &[0u8; 100]);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Cap smaller than all three but big enough for the newest one or two.
        evict_to_cap(150);
        assert!(matches!(lookup(urls[0]), Lookup::Miss), "oldest entry should have been evicted");
        assert!(!matches!(lookup(urls[2]), Lookup::Miss), "newest entry should survive");
    }
}

/// Live, `PLUME_TEST_SOCKS_ADDR`-gated check (same convention as
/// `http::client::diag_tests`) that this module's parsing/freshness/
/// revalidation logic holds up against a *real* server's actual response
/// headers, not just synthetic ones -- the thing the unit tests above
/// can't prove on their own.
#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::http::{HttpClient, Response as PlumeResponse, ResponseHandler};
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    struct FetchResult {
        status: u16,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        cache_control: Option<String>,
        body: Vec<u8>,
    }

    struct CollectingHandler {
        tx: std_mpsc::Sender<FetchResult>,
        status: u16,
        content_type: Option<String>,
        etag: Option<String>,
        last_modified: Option<String>,
        cache_control: Option<String>,
        body: Vec<u8>,
    }

    impl ResponseHandler for CollectingHandler {
        fn ok(&mut self, response: &PlumeResponse) {
            self.status = response.code;
        }
        fn error(&mut self, response: &PlumeResponse) {
            self.status = response.code;
        }
        fn header(&mut self, name: &str, value: &str) {
            if name.eq_ignore_ascii_case("content-type") {
                self.content_type = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("etag") {
                self.etag = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("last-modified") {
                self.last_modified = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("cache-control") {
                self.cache_control = Some(value.to_string());
            }
        }
        fn start_body(&mut self) {}
        fn body_chunk(&mut self, data: &[u8]) {
            self.body.extend_from_slice(data);
        }
        fn end_body(&mut self) {}
        fn complete(&mut self) {
            let _ = self.tx.send(FetchResult {
                status: self.status,
                content_type: self.content_type.clone(),
                etag: self.etag.clone(),
                last_modified: self.last_modified.clone(),
                cache_control: self.cache_control.clone(),
                body: std::mem::take(&mut self.body),
            });
        }
        fn failed(&mut self, error: &std::io::Error) {
            panic!("live fetch failed: {error}");
        }
    }

    fn fetch(url_str: &str, if_none_match: Option<&str>, if_modified_since: Option<&str>) -> FetchResult {
        let parsed = url::Url::parse(url_str).unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let secure = parsed.scheme() == "https";
        let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
        let path = match parsed.query() {
            Some(q) => format!("{}?{}", parsed.path(), q),
            None => parsed.path().to_string(),
        };
        let client = HttpClient::new(&host, port, secure);
        let mut request = client.get(&path);
        if let Some(e) = if_none_match {
            request.header("If-None-Match", e);
        }
        if let Some(lm) = if_modified_since {
            request.header("If-Modified-Since", lm);
        }
        let (tx, rx) = std_mpsc::channel();
        request.send(Box::new(CollectingHandler {
            tx,
            status: 0,
            content_type: None,
            etag: None,
            last_modified: None,
            cache_control: None,
            body: Vec::new(),
        }));
        rx.recv_timeout(Duration::from_secs(30)).expect("fetch timed out")
    }

    #[test]
    fn real_avatar_response_round_trips_through_the_cache() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let url = std::env::var("PLUME_TEST_URL")
            .unwrap_or_else(|_| "https://avatars.githubusercontent.com/u/135021594?v=4".to_string());
        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let first = fetch(&url, None, None);
        assert_eq!(first.status, 200, "expected a real 200 from {url}");
        assert!(!first.body.is_empty());

        store(
            &url,
            first.status,
            first.content_type.as_deref(),
            first.etag.as_deref(),
            first.last_modified.as_deref(),
            first.cache_control.as_deref(),
            &first.body,
        );

        match lookup(&url) {
            Lookup::Fresh(entry) => {
                assert_eq!(entry.body, first.body, "cached body must match what the real server sent");
            }
            Lookup::Stale(stale) => {
                assert_eq!(stale.entry.body, first.body);
                eprintln!(
                    "[live] server gave no positive freshness lifetime (Cache-Control: {:?}); stale immediately, as expected",
                    first.cache_control
                );
            }
            Lookup::Miss => panic!("a 200 with no no-store should always be cached"),
        }

        let Some(etag) = first.etag.clone() else {
            eprintln!("[live] server sent no ETag; skipping conditional-revalidation check");
            return;
        };

        let second = fetch(&url, Some(&etag), first.last_modified.as_deref());
        if second.status == 304 {
            touch_fresh(
                &url,
                second.etag.as_deref().or(Some(&etag)),
                second.last_modified.as_deref().or(first.last_modified.as_deref()),
                second.cache_control.as_deref(),
            );
            assert!(matches!(lookup(&url), Lookup::Fresh(_)), "a 304 revalidation must leave the entry fresh");
        } else {
            eprintln!(
                "[live] server answered the conditional request with {} instead of 304 (still a valid response per RFC 9110 §15.4.5)",
                second.status
            );
        }
    }
}
