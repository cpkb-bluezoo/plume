/*
 * client.rs
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

//! HTTP client: dials through the local Tor SOCKS5 proxy (never directly),
//! then runs hopf-http's H1/H2 session API on top.
//!
//! Connections are pooled and reused by `(host, port, secure)`: a request
//! first tries to check out an already-open, idle connection to the same
//! origin before dialing a fresh one. Every dial (SOCKS handshake, then TLS)
//! costs a round trip through Tor plus a file descriptor for as long as
//! it's open, and a feed/thread full of images from a handful of popular
//! media hosts used to mean a fresh dial *per image* -- wasteful, and (with
//! enough of them in flight at once) capable of exhausting the process's fd
//! limit outright. Idle pooled connections are swept after
//! [`POOL_IDLE_TTL`] so a connection nobody reuses still eventually gets
//! closed rather than sitting open for the life of the process.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use hopf::core::{ConnHandle, ProtocolHandler};
use hopf::http::{
    HttpClientConnection, HttpClientSessionConfig, HttpClientSessionHandle, HttpConnectionHandler,
    HttpLimits, HttpResponseHandler,
};

use crate::debug_log;
use crate::http::request::HttpRequest;
use crate::http::{Response, ResponseHandler};
use crate::net::{runtime, tor_dial};
use crate::warn_log;

pub(crate) enum Command {
    SendRequest {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        handler: Box<dyn ResponseHandler>,
        has_body: bool,
    },
    BodyChunk { data: Vec<u8> },
    EndBody,
}

/// Origin a connection is pooled under. Secure and plaintext connections to
/// the same host:port are never interchangeable, hence `secure` being part
/// of the key.
type PoolKey = (String, u16, bool);

/// How long an idle pooled connection may sit unused before the background
/// sweeper closes it. Deliberately modest: many servers' own keep-alive
/// idle timeout is in this ballpark, so holding connections open much
/// longer mostly risks "reusing" one the peer already closed from its side
/// (harmless -- caught and failed cleanly -- but wasted effort) rather than
/// gaining much.
const POOL_IDLE_TTL: Duration = Duration::from_secs(30);

/// How often the background sweeper checks for expired idle connections.
const POOL_SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// Hard cap on the *total* number of idle pooled connections across every
/// origin. `POOL_IDLE_TTL` alone isn't enough: a note feed or thread
/// commonly references dozens of distinct, mostly single-use hosts, and
/// without this cap every single one of them would sit open (holding a
/// file descriptor) for the full TTL after its one use instead of closing
/// right away like before pooling existed -- trading one fd-exhaustion bug
/// (an unbounded burst of simultaneous dials) for a slower variant of the
/// same thing (an unbounded pile-up of idle-but-still-open connections).
/// Once at the cap, a newly-finished connection is closed immediately
/// rather than pooled.
const MAX_POOLED_IDLE: usize = 16;

struct PooledConn {
    cmd_tx: mpsc::Sender<Command>,
    conn_handle: Option<ConnHandle>,
    idle_since: Instant,
}

static POOL: OnceLock<Mutex<HashMap<PoolKey, Vec<PooledConn>>>> = OnceLock::new();
static SWEEPER_STARTED: OnceLock<()> = OnceLock::new();
static POOLED_COUNT: AtomicUsize = AtomicUsize::new(0);

fn pool() -> &'static Mutex<HashMap<PoolKey, Vec<PooledConn>>> {
    SWEEPER_STARTED.get_or_init(|| {
        std::thread::spawn(sweep_idle_connections);
    });
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn close_pooled(conn: &PooledConn) {
    if let Some(h) = &conn.conn_handle {
        h.close();
    }
    POOLED_COUNT.fetch_sub(1, Ordering::SeqCst);
}

/// Periodically closes and drops any pooled connection that's been idle
/// longer than [`POOL_IDLE_TTL`], so a host visited once and never again
/// doesn't hold a connection (and its fd) open for the rest of the process's
/// life.
fn sweep_idle_connections() {
    loop {
        std::thread::sleep(POOL_SWEEP_INTERVAL);
        let mut guard = pool().lock().unwrap();
        for conns in guard.values_mut() {
            conns.retain(|c| {
                let expired = c.idle_since.elapsed() >= POOL_IDLE_TTL;
                if expired {
                    close_pooled(c);
                }
                !expired
            });
        }
        guard.retain(|_, conns| !conns.is_empty());
    }
}

/// Pop one usable, unexpired idle connection for `key`, closing and
/// discarding any expired ones found along the way.
fn checkout_pooled(key: &PoolKey) -> Option<mpsc::Sender<Command>> {
    let mut guard = pool().lock().unwrap();
    let conns = guard.get_mut(key)?;
    while let Some(conn) = conns.pop() {
        if conn.idle_since.elapsed() < POOL_IDLE_TTL {
            POOLED_COUNT.fetch_sub(1, Ordering::SeqCst);
            return Some(conn.cmd_tx);
        }
        close_pooled(&conn);
    }
    None
}

/// Return a connection to the pool for reuse once it's genuinely idle
/// (its one in-flight request finished, and nothing indicated the peer is
/// closing it) -- unless the pool is already at [`MAX_POOLED_IDLE`], in
/// which case it's closed immediately instead (same as if pooling didn't
/// exist at all).
fn pool_return(key: PoolKey, cmd_tx: mpsc::Sender<Command>, conn_handle: Option<ConnHandle>) {
    let acquired = POOLED_COUNT
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            if n < MAX_POOLED_IDLE { Some(n + 1) } else { None }
        })
        .is_ok();
    if !acquired {
        debug_log!("[http] pool at capacity, closing instead of pooling connection for {key:?}");
        if let Some(h) = &conn_handle {
            h.close();
        }
        return;
    }
    pool()
        .lock()
        .unwrap()
        .entry(key)
        .or_default()
        .push(PooledConn { cmd_tx, conn_handle, idle_since: Instant::now() });
}

#[derive(Clone)]
pub struct HttpClient {
    cmd_tx: mpsc::Sender<Command>,
}

impl HttpClient {
    pub fn new(host: &str, port: u16, secure: bool) -> Self {
        let key: PoolKey = (host.to_string(), port, secure);
        if let Some(cmd_tx) = checkout_pooled(&key) {
            debug_log!("[http] reusing pooled connection to {}:{}", host, port);
            return Self { cmd_tx };
        }

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let cmd_rx_cell = Arc::new(Mutex::new(Some(cmd_rx)));

        let tls = if secure { Some(crate::tls::http_tls_connector()) } else { None };
        let limits = HttpLimits::default();
        let host_owned = host.to_string();
        let cmd_rx_for_factory = Arc::clone(&cmd_rx_cell);
        let cmd_tx_for_factory = cmd_tx.clone();
        let key_for_factory = key.clone();

        let dial_result = tor_dial::dial_via_tor(&runtime::get(), host, port, tls, move || {
            let config = Arc::new(HttpClientSessionConfig {
                host: host_owned.clone(),
                port,
                limits,
                secure,
                handler: Mutex::new(Some(Box::new(PlumeConnHandler {
                    cmd_rx: Arc::clone(&cmd_rx_for_factory),
                    cmd_tx: cmd_tx_for_factory.clone(),
                    key: key_for_factory.clone(),
                }) as Box<dyn HttpConnectionHandler>)),
                stage: std::time::Duration::from_secs(60),
            });
            Box::new(HttpClientConnection::new(config, limits, secure, false)) as Box<dyn ProtocolHandler>
        });

        if let Err(e) = dial_result {
            warn_log!("[http] dial failed for {}:{}: {}", host, port, e);
            if let Some(rx) = cmd_rx_cell.lock().unwrap().take() {
                fail_pending_handlers(rx, &e);
            }
        }

        Self { cmd_tx }
    }

    pub fn get(&self, path: &str) -> HttpRequest {
        HttpRequest::new(self.cmd_tx.clone(), "GET", path)
    }

    pub fn post(&self, path: &str) -> HttpRequest {
        HttpRequest::new(self.cmd_tx.clone(), "POST", path)
    }

    pub fn put(&self, path: &str) -> HttpRequest {
        HttpRequest::new(self.cmd_tx.clone(), "PUT", path)
    }

    pub fn delete(&self, path: &str) -> HttpRequest {
        HttpRequest::new(self.cmd_tx.clone(), "DELETE", path)
    }
}

/// Drain any pending/future commands from the channel and fail their
/// handler -- mirrors what used to happen when the old tokio connection
/// task's dial failed. Runs on a dedicated thread since the `SendRequest`
/// command may not have been queued yet when this is invoked (e.g. Tor
/// not being ready fails the dial synchronously, before the caller has
/// even had a chance to call `.send()`/`.start_body()`).
fn fail_pending_handlers(cmd_rx: mpsc::Receiver<Command>, error: &io::Error) {
    let message = error.to_string();
    std::thread::spawn(move || {
        let err = io::Error::new(io::ErrorKind::NotConnected, message);
        if let Ok(Command::SendRequest { mut handler, .. }) = cmd_rx.recv() {
            handler.failed(&err);
        }
    });
}

/// Bridges hopf-http's connection-level session API to Plume's
/// `HttpRequest`/`ResponseHandler` command-queue API. `on_connected` spawns
/// a dedicated worker thread ([`drive_connection`]) that serves requests
/// for this connection for as long as it stays open and pooled, not just
/// the first one.
///
/// `cmd_rx` is shared with [`HttpClient::new`] (rather than owned outright)
/// because the dial can fail before this handler is ever constructed at
/// all -- whichever side discovers the failure first (`HttpClient::new`'s
/// own `dial_via_tor` call, or this handler's `on_error`) is the one that
/// ends up draining it.
struct PlumeConnHandler {
    cmd_rx: Arc<Mutex<Option<mpsc::Receiver<Command>>>>,
    cmd_tx: mpsc::Sender<Command>,
    key: PoolKey,
}

impl HttpConnectionHandler for PlumeConnHandler {
    fn on_connected(&mut self, session: &mut HttpClientSessionHandle) {
        let cmd_rx = match self.cmd_rx.lock().unwrap().take() {
            Some(rx) => rx,
            None => return,
        };
        let session = session.clone();
        let cmd_tx = self.cmd_tx.clone();
        let key = self.key.clone();
        std::thread::spawn(move || drive_connection(session, cmd_rx, cmd_tx, key));
    }

    fn on_error(&mut self, err: &io::Error) {
        if let Some(rx) = self.cmd_rx.lock().unwrap().take() {
            fail_pending_handlers(rx, err);
        }
    }
}

fn send_err(e: &hopf::http::HttpClientError) -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, e.to_string())
}

/// Runs on a dedicated worker thread (never the reactor thread) for the
/// life of one connection: waits for a `SendRequest`, issues it, streams
/// any body chunks, then -- unlike the old one-shot design -- waits for
/// that request/response cycle to fully finish and, if the connection is
/// still open (no `Connection: close`, no error), returns it to the pool
/// and loops to accept another request instead of tearing the connection
/// down after one use.
fn drive_connection(
    mut session: HttpClientSessionHandle,
    cmd_rx: mpsc::Receiver<Command>,
    cmd_tx: mpsc::Sender<Command>,
    key: PoolKey,
) {
    loop {
        let (method, path, headers, handler, has_body) = match cmd_rx.recv() {
            Ok(Command::SendRequest { method, path, headers, handler, has_body }) => {
                (method, path, headers, handler, has_body)
            }
            // Channel closed: evicted from the pool, or the `HttpClient`
            // that owned us was dropped without ever issuing a request.
            _ => return,
        };

        let mut request = session.method(&method, &path);
        for (name, value) in &headers {
            let _ = request.header(name, value);
        }

        let response_handler: Arc<Mutex<Box<dyn ResponseHandler>>> = Arc::new(Mutex::new(handler));
        let keep_alive = Arc::new(AtomicBool::new(true));
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let response_adapter = Box::new(SharedResponseAdapter {
            handler: Arc::clone(&response_handler),
            keep_alive: Arc::clone(&keep_alive),
            done_tx,
            signaled: AtomicBool::new(false),
        });
        let conn_handle = session.conn_handle();

        let queued = if !has_body {
            match request.send(response_adapter) {
                Ok(()) => {
                    // `send` only queues the request headers in the H1/H2
                    // session codec's internal output buffer -- it's only
                    // ever flushed to the wire from the owning reactor
                    // thread, either when new inbound data arrives (nothing
                    // will, until the peer sees our request) or when
                    // explicitly poked. Since this runs on a dedicated
                    // worker thread, without this poke a bodyless request
                    // sits buffered forever and the request hangs with no
                    // response, no error, nothing.
                    if let Some(h) = &conn_handle {
                        h.poke();
                    }
                    true
                }
                Err(e) => {
                    // Most commonly: a pooled connection the peer already
                    // closed since we last used it. Fail this one request
                    // cleanly rather than leaving its handler hanging --
                    // the next `HttpClient::new()` for this origin will
                    // just dial fresh.
                    response_handler.lock().unwrap().failed(&send_err(&e));
                    false
                }
            }
        } else {
            match request.start_request_body(response_adapter) {
                Ok(()) => stream_body(&mut request, &cmd_rx, &response_handler, conn_handle.as_ref()),
                Err(e) => {
                    response_handler.lock().unwrap().failed(&send_err(&e));
                    false
                }
            }
        };

        if !queued {
            return; // connection already known dead -- don't reuse or re-pool it
        }

        // Block until this request/response cycle fully finishes (hopf-http
        // only resets the session back to `Building` once it does), so we
        // never feed it a second request while the first is still in
        // flight.
        if done_rx.recv().is_err() {
            return;
        }
        if !keep_alive.load(Ordering::SeqCst) {
            return; // peer asked to close; don't pool, don't loop
        }
        pool_return(key.clone(), cmd_tx.clone(), conn_handle);
    }
}

/// Streams body chunk/end-body commands for one request. Returns whether
/// the request was fully handed off; `false` means the handler has already
/// been failed and the connection must not be reused.
fn stream_body(
    request: &mut hopf::http::HttpRequest,
    cmd_rx: &mpsc::Receiver<Command>,
    response_handler: &Arc<Mutex<Box<dyn ResponseHandler>>>,
    conn_handle: Option<&ConnHandle>,
) -> bool {
    let mut bytes_written: u64 = 0;
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Command::BodyChunk { data } => {
                let mut offset = 0;
                while offset < data.len() {
                    match request.request_body_content(&data[offset..]) {
                        Ok(0) => {
                            let (tx, rx) = mpsc::sync_channel::<()>(1);
                            let _ = request.on_body_writable(Box::new(move || {
                                let _ = tx.send(());
                            }));
                            let _ = rx.recv();
                        }
                        Ok(n) => {
                            offset += n;
                            bytes_written += n as u64;
                            if let Some(h) = conn_handle {
                                h.poke();
                            }
                            response_handler
                                .lock()
                                .unwrap()
                                .request_body_written(bytes_written, bytes_written);
                        }
                        Err(e) => {
                            response_handler.lock().unwrap().failed(&send_err(&e));
                            return false;
                        }
                    }
                }
            }
            Command::EndBody => {
                let _ = request.end_request_body();
                if let Some(h) = conn_handle {
                    h.poke();
                }
                return true;
            }
            Command::SendRequest { .. } => {}
        }
    }
    false
}

/// Translates hopf-http's [`HttpResponseHandler`] callbacks into Plume's
/// own [`ResponseHandler`]. Also watches for a `Connection: close` response
/// header and, on the terminal callback (`close`/`failed`), wakes up
/// [`drive_connection`]'s worker thread so it knows the request/response
/// cycle is over and whether the connection is still reusable.
struct SharedResponseAdapter {
    handler: Arc<Mutex<Box<dyn ResponseHandler>>>,
    keep_alive: Arc<AtomicBool>,
    done_tx: mpsc::Sender<()>,
    signaled: AtomicBool,
}

impl SharedResponseAdapter {
    /// Signal completion to `drive_connection` exactly once, however this
    /// request ends (`close` or `failed`).
    fn signal_done(&self) {
        if !self.signaled.swap(true, Ordering::SeqCst) {
            let _ = self.done_tx.send(());
        }
    }
}

impl HttpResponseHandler for SharedResponseAdapter {
    fn ok(&mut self, status: u16) {
        self.handler.lock().unwrap().ok(&Response::new(status));
    }

    fn error(&mut self, status: u16) {
        self.handler.lock().unwrap().error(&Response::new(status));
    }

    fn header(&mut self, name: &str, value: &str) {
        if name.eq_ignore_ascii_case("connection")
            && value.split(',').any(|p| p.trim().eq_ignore_ascii_case("close"))
        {
            self.keep_alive.store(false, Ordering::SeqCst);
        }
        self.handler.lock().unwrap().header(name, value);
    }

    fn start_response_body(&mut self) {
        self.handler.lock().unwrap().start_body();
    }

    fn response_body_content(&mut self, data: &[u8]) {
        self.handler.lock().unwrap().body_chunk(data);
    }

    fn end_response_body(&mut self) {
        self.handler.lock().unwrap().end_body();
    }

    fn response_trailers(&mut self, headers: &hopf::http::Headers) {
        let mut guard = self.handler.lock().unwrap();
        for header in headers.iter() {
            guard.header(&header.name, &header.value);
        }
    }

    fn close(&mut self) {
        self.handler.lock().unwrap().complete();
        self.signal_done();
    }

    fn failed(&mut self, err: io::Error) {
        self.handler.lock().unwrap().failed(&err);
        self.keep_alive.store(false, Ordering::SeqCst);
        self.signal_done();
    }
}

/// White-box test for the "pooling itself can exhaust fds" regression: a
/// burst of connections returned for many *distinct* keys (as a feed full
/// of mostly single-use image hosts would produce) must never push the
/// total idle pool past `MAX_POOLED_IDLE`, however many distinct hosts are
/// involved. Needs no real network -- exercises `pool_return` directly
/// with fake channels.
#[cfg(test)]
mod pool_tests {
    use super::*;

    #[test]
    fn pool_return_respects_max_pooled_idle() {
        let baseline = POOLED_COUNT.load(Ordering::SeqCst);
        let total = MAX_POOLED_IDLE + 10;
        let mut keys = Vec::new();

        for i in 0..total {
            let (tx, _rx) = mpsc::channel::<Command>();
            let key: PoolKey = (format!("plume-pool-test-host-{i}"), 443, true);
            keys.push(key.clone());
            pool_return(key, tx, None);

            let grown = POOLED_COUNT.load(Ordering::SeqCst) - baseline;
            assert!(
                grown <= MAX_POOLED_IDLE,
                "pool grew by {grown} (cap is {MAX_POOLED_IDLE}) after {} returns across distinct hosts",
                i + 1
            );
        }

        assert_eq!(
            POOLED_COUNT.load(Ordering::SeqCst) - baseline,
            MAX_POOLED_IDLE,
            "pool should have filled exactly to its cap, not stopped short or overshot"
        );

        // Leave the shared global pool as this test found it, since other
        // tests in this binary touch the same statics.
        let mut guard = pool().lock().unwrap();
        for key in &keys {
            guard.remove(key);
        }
        drop(guard);
        POOLED_COUNT.store(baseline, Ordering::SeqCst);
    }
}

/// Diagnostic test for the "images never load" report: exercises this
/// exact `HttpClient`/`PlumeConnHandler`/hopf-http path against a real,
/// already-bootstrapped Tor instance (point `PLUME_TEST_SOCKS_ADDR` at its
/// SOCKS address) to tell whether the bug is in this HTTP-over-Tor code or
/// elsewhere (CSP, the custom URI scheme registration, frontend wiring).
#[cfg(test)]
mod diag_tests {
    use super::*;
    use crate::http::Response as PlumeResponse;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    enum Event {
        Ok(u16),
        Error(u16),
        BodyLen(usize),
        Complete,
        Failed(String),
    }

    struct DiagHandler(std_mpsc::Sender<Event>);

    impl ResponseHandler for DiagHandler {
        fn ok(&mut self, response: &PlumeResponse) {
            let _ = self.0.send(Event::Ok(response.code));
        }
        fn error(&mut self, response: &PlumeResponse) {
            let _ = self.0.send(Event::Error(response.code));
        }
        fn header(&mut self, name: &str, value: &str) {
            eprintln!("[diag] header: {name}: {value}");
        }
        fn start_body(&mut self) {}
        fn body_chunk(&mut self, data: &[u8]) {
            let _ = self.0.send(Event::BodyLen(data.len()));
        }
        fn end_body(&mut self) {}
        fn complete(&mut self) {
            let _ = self.0.send(Event::Complete);
        }
        fn failed(&mut self, error: &io::Error) {
            let _ = self.0.send(Event::Failed(error.to_string()));
        }
    }

    #[test]
    fn fetch_real_url_via_running_tor() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let url_str = std::env::var("PLUME_TEST_URL")
            .unwrap_or_else(|_| "https://avatars.githubusercontent.com/u/135021594?v=4".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let parsed = url::Url::parse(&url_str).unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let secure = parsed.scheme() == "https";
        let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
        let path = match parsed.query() {
            Some(q) => format!("{}?{}", parsed.path(), q),
            None => parsed.path().to_string(),
        };

        let (tx, rx) = std_mpsc::channel();
        eprintln!("[diag] dialing {host}:{port} secure={secure} path={path}");
        let client = HttpClient::new(&host, port, secure);
        client.get(&path).send(Box::new(DiagHandler(tx)));

        let mut total_body = 0usize;
        loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(Event::Ok(code)) => eprintln!("[diag] ok: {code}"),
                Ok(Event::Error(code)) => eprintln!("[diag] error status: {code}"),
                Ok(Event::BodyLen(n)) => {
                    total_body += n;
                    eprintln!("[diag] body chunk: {n} bytes (total {total_body})");
                }
                Ok(Event::Complete) => {
                    eprintln!("[diag] complete, total body {total_body} bytes");
                    break;
                }
                Ok(Event::Failed(msg)) => {
                    eprintln!("[diag] FAILED: {msg}");
                    break;
                }
                Err(_) => {
                    panic!("[diag] TIMED OUT waiting for any callback after 60s -- request hung");
                }
            }
        }
    }

    fn fetch_once(host: &str, port: u16, secure: bool, path: &str) {
        let (tx, rx) = std_mpsc::channel();
        let client = HttpClient::new(host, port, secure);
        client.get(path).send(Box::new(DiagHandler(tx)));
        loop {
            match rx.recv_timeout(Duration::from_secs(60)) {
                Ok(Event::Complete) => return,
                Ok(Event::Failed(msg)) => panic!("[diag] fetch failed: {msg}"),
                Ok(_) => {}
                Err(_) => panic!("[diag] TIMED OUT waiting for fetch to complete"),
            }
        }
    }

    /// Counts this process's own established TCP connections to Tor's
    /// SOCKS port via `lsof` -- an external, black-box way to confirm
    /// whether a second request actually reused the pooled connection
    /// (count stays flat) or quietly dialed a fresh one (count goes up).
    fn count_own_tor_sockets(socks_port: u16) -> usize {
        let pid = std::process::id().to_string();
        let output = std::process::Command::new("lsof")
            .args(["-nP", "-p", &pid, "-a", "-i", &format!("TCP:{socks_port}")])
            .output()
            .expect("failed to run lsof");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| l.contains("ESTABLISHED"))
            .count()
    }

    #[test]
    fn fetch_twice_reuses_pooled_connection() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let url_str = std::env::var("PLUME_TEST_URL")
            .unwrap_or_else(|_| "https://avatars.githubusercontent.com/u/135021594?v=4".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let parsed = url::Url::parse(&url_str).unwrap();
        let host = parsed.host_str().unwrap().to_string();
        let secure = parsed.scheme() == "https";
        let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
        let path = match parsed.query() {
            Some(q) => format!("{}?{}", parsed.path(), q),
            None => parsed.path().to_string(),
        };

        fetch_once(&host, port, secure, &path);
        std::thread::sleep(Duration::from_millis(500)); // let it settle back into the pool
        let after_first = count_own_tor_sockets(addr.port());

        fetch_once(&host, port, secure, &path);
        std::thread::sleep(Duration::from_millis(500));
        let after_second = count_own_tor_sockets(addr.port());

        eprintln!("[diag] sockets to Tor's SOCKS port: after 1st fetch={after_first}, after 2nd fetch={after_second}");
        assert_eq!(
            after_first, after_second,
            "second fetch to the same host should have reused the pooled connection, not opened a new one"
        );
    }
}
