/*
 * media_proxy.rs
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

//! Backs the `torimg://` custom URI scheme: the webview never fetches a
//! remote image/avatar/banner/media URL directly (which would bypass Tor
//! entirely and leak the real IP plus the webview's own User-Agent to the
//! content host). Instead the frontend rewrites every such URL to
//! `torimg://localhost/<percent-encoded original URL>`, and this module
//! fetches it through the same Tor-routed `HttpClient` used everywhere else
//! in the backend, handing the bytes back to the webview.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::http::{HttpClient, Response as PlumeResponse, ResponseHandler};
use crate::debug_log;

/// Hard cap on simultaneous outbound dials for `torimg://` fetches.
///
/// Each dial (SOCKS handshake to Tor, then a TLS handshake over it) holds
/// at least one file descriptor for as long as it's in flight, and this
/// module makes no attempt to reuse a connection across requests -- every
/// image/avatar/banner gets its own fresh dial (see `HttpClient`'s own doc
/// comment). A feed or note thread can reference dozens of distinct image
/// hosts at once with nothing bounding how many fire simultaneously, which
/// can burst past the process's fd limit ("Too many open files", os error
/// 24) -- particularly under a GUI launch, where macOS's default per-process
/// limit (`launchctl limit maxfiles`) is a mere 256, a small fraction of
/// what an interactive shell typically has. Queuing excess requests instead
/// of letting them all dial at once keeps total open sockets bounded
/// regardless of how many distinct hosts a feed references.
const MAX_CONCURRENT_FETCHES: usize = 16;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static QUEUE: OnceLock<Mutex<VecDeque<(String, tauri::UriSchemeResponder)>>> = OnceLock::new();

fn queue() -> &'static Mutex<VecDeque<(String, tauri::UriSchemeResponder)>> {
    QUEUE.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// Handle one `torimg://localhost/<encoded-url>` request -- dials
/// immediately if under [`MAX_CONCURRENT_FETCHES`], otherwise queues it to
/// start as soon as an in-flight fetch finishes.
pub fn handle(request_path: &str, responder: tauri::UriSchemeResponder) {
    let acquired = IN_FLIGHT
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            if n < MAX_CONCURRENT_FETCHES { Some(n + 1) } else { None }
        })
        .is_ok();
    if acquired {
        start_fetch(request_path, responder);
    } else {
        debug_log!("[torimg] at concurrency limit, queuing: {request_path:?}");
        queue().lock().unwrap().push_back((request_path.to_string(), responder));
    }
}

/// Called once a fetch's `ProxyHandler` finishes (success or failure):
/// starts the next queued request in the slot that just freed, or releases
/// the slot if nothing is queued.
fn release_slot() {
    let next = queue().lock().unwrap().pop_front();
    match next {
        Some((path, responder)) => start_fetch(&path, responder),
        None => {
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn start_fetch(request_path: &str, responder: tauri::UriSchemeResponder) {
    debug_log!("[torimg] request: {:?}", request_path);
    let encoded = request_path.trim_start_matches('/');
    let target = match urlencoding::decode(encoded) {
        Ok(s) => s.into_owned(),
        Err(e) => {
            debug_log!("[torimg] percent-decode failed: {e}");
            return respond_status(responder, 400);
        }
    };
    debug_log!("[torimg] decoded target: {target}");

    let parsed = match url::Url::parse(&target) {
        Ok(u) => u,
        Err(e) => {
            debug_log!("[torimg] url parse failed for {target:?}: {e}");
            return respond_status(responder, 400);
        }
    };
    let secure = match parsed.scheme() {
        "https" => true,
        "http" => false,
        other => {
            debug_log!("[torimg] unsupported scheme {other:?} in {target:?}");
            return respond_status(responder, 400);
        }
    };
    let host = match parsed.host_str() {
        Some(h) => h.to_string(),
        None => {
            debug_log!("[torimg] no host in {target:?}");
            return respond_status(responder, 400);
        }
    };
    let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
    let path = match parsed.query() {
        Some(q) => format!("{}?{}", parsed.path(), q),
        None => parsed.path().to_string(),
    };

    // The sidebar/profile avatar is requested the instant local config
    // loads from disk, almost certainly before Tor has finished
    // bootstrapping -- and unlike a relay fetch, a one-shot `<img>` load
    // has no retry of its own, so an instant "Tor is still starting"
    // failure here means that avatar never loads even once Tor becomes
    // ready moments later. Wait for it instead of failing immediately, but
    // never block whatever thread dispatched this request: only spawn a
    // dedicated wait-then-dial thread when Tor isn't already ready (the
    // overwhelmingly common case once the app's been open a few seconds).
    if matches!(crate::tor::status(), crate::tor::TorStatus::Starting) {
        debug_log!("[torimg] Tor still starting, waiting before dialing {host}:{port}");
        std::thread::spawn(move || {
            if let Err(e) = crate::tor::wait_for_ready(std::time::Duration::from_secs(45)) {
                debug_log!("[torimg] gave up waiting for Tor for {host}:{port}: {e}");
                respond_status(responder, 502);
                return;
            }
            dial_and_respond(host, port, secure, path, responder);
        });
        return;
    }

    dial_and_respond(host, port, secure, path, responder);
}

fn dial_and_respond(host: String, port: u16, secure: bool, path: String, responder: tauri::UriSchemeResponder) {
    debug_log!("[torimg] dialing {host}:{port} secure={secure} path={path}");
    let client = HttpClient::new(&host, port, secure);
    client.get(&path).send(Box::new(ProxyHandler {
        host,
        responder: Some(responder),
        status: 0,
        content_type: None,
        body: Vec::new(),
    }));
}

/// Used only for the early-rejection paths in `start_fetch` (bad encoding,
/// unparseable URL, unsupported scheme, no host) that never get as far as
/// constructing a `ProxyHandler` -- so unlike a real fetch's `ProxyHandler`,
/// this has to release the concurrency slot itself.
fn respond_status(responder: tauri::UriSchemeResponder, status: u16) {
    let response = tauri::http::Response::builder().status(status).body(Vec::new()).unwrap();
    responder.respond(response);
    release_slot();
}

struct ProxyHandler {
    host: String,
    responder: Option<tauri::UriSchemeResponder>,
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl ProxyHandler {
    fn finish(&mut self) {
        let Some(responder) = self.responder.take() else { return };
        debug_log!(
            "[torimg] responding for {}: status={} content_type={:?} body_len={}",
            self.host, self.status, self.content_type, self.body.len()
        );
        let mut builder = tauri::http::Response::builder().status(self.status);
        if let Some(ct) = &self.content_type {
            builder = builder.header(tauri::http::header::CONTENT_TYPE, ct.clone());
        }
        let response = builder.body(std::mem::take(&mut self.body)).unwrap();
        responder.respond(response);
        release_slot();
    }
}

impl ResponseHandler for ProxyHandler {
    fn ok(&mut self, response: &PlumeResponse) {
        debug_log!("[torimg] ok for {}: {}", self.host, response.code);
        self.status = response.code;
    }

    fn error(&mut self, response: &PlumeResponse) {
        debug_log!("[torimg] error status for {}: {}", self.host, response.code);
        self.status = response.code;
    }

    fn header(&mut self, name: &str, value: &str) {
        if name.eq_ignore_ascii_case("content-type") {
            self.content_type = Some(value.to_string());
        }
    }

    fn start_body(&mut self) {}

    fn body_chunk(&mut self, data: &[u8]) {
        self.body.extend_from_slice(data);
    }

    fn end_body(&mut self) {}

    fn complete(&mut self) {
        self.finish();
    }

    fn failed(&mut self, error: &std::io::Error) {
        debug_log!("[torimg] failed for {}: {error}", self.host);
        if self.status == 0 {
            self.status = 502;
        }
        self.finish();
    }
}
