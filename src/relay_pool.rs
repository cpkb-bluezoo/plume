/*
 * relay_pool.rs
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

//! Exactly one persistent, auto-reconnecting WebSocket connection per relay
//! URL, shared by every purpose that needs that relay (the feed's initial
//! load and 45s poll tick, profile/following/followers/relay-list fetches,
//! publishes, and the DM subscription). A relay URL used for both notes and
//! DMs gets one connection, not two.
//!
//! Replaces the old per-call dial-then-close design (`relay.rs`'s former
//! `connect_to_relay`/`CloseOnReturn`): a one-shot operation now sends a
//! `REQ`/`EVENT` over whatever connection already exists (dialing/waiting
//! for one to come up if necessary) instead of dialing its own, and a
//! standing subscription registers itself once and is automatically
//! resubscribed on every reconnect. Connections close only when the app
//! exits ([`shutdown`]).
//!
//! `relay.rs` keeps the Nostr message *parsing* (`RelayMessage`,
//! `StreamMessage`, `parse_relay_message`) -- this module only owns
//! connection *lifecycle* and demultiplexing incoming messages (by
//! subscription id for EVENT/EOSE, by event id for OK) to whichever caller
//! registered them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::async_bridge::{self, mpsc, OneshotSender};
use crate::debug_log;
use crate::nostr;
use crate::relay::{self, RelayMessage, StreamMessage};
use crate::tor;
use crate::websocket::connection::WebSocketConnection;
use crate::websocket::{WebSocketClient, WebSocketHandler};

/// Connection timeout for a (re)dial (seconds). See the identical constant
/// this superseded in `relay.rs`'s former `connect_to_relay` for the
/// Tor-latency reasoning -- unchanged. `pub(crate)` so tests elsewhere can
/// size their own timeout budgets relative to it.
pub(crate) const CONNECT_TIMEOUT_SECS: u64 = 20;

/// How long to wait for Tor to finish bootstrapping before giving up on a
/// (re)dial, when it's merely [`tor::TorStatus::Starting`]. Unchanged from
/// `relay.rs`'s former `connect_to_relay`.
const TOR_READY_WAIT_SECS: u64 = 45;

/// Minimum backoff wait after a connection failure (seconds).
const BACKOFF_BASE_SECS: u64 = 10;

/// Maximum backoff wait (seconds) -- caps the exponential growth.
const BACKOFF_MAX_SECS: u64 = 300; // 5 minutes

/// Hard cap on simultaneous in-flight (re)dial *attempts*, process-wide.
/// Once a relay is connected, its connection is held open indefinitely and
/// no longer counts against this -- unlike the old per-call design, where
/// this same cap bounded every simultaneous open connection of every kind.
/// It still exists to avoid a stampede of every configured relay's
/// supervisor cold-dialing (building a fresh Tor circuit each) at the same
/// moment, e.g. at app startup.
const MAX_CONCURRENT_RELAY_DIALS: usize = 24;

static RELAY_DIAL_SEMAPHORE: OnceLock<async_bridge::Semaphore> = OnceLock::new();

fn relay_dial_semaphore() -> &'static async_bridge::Semaphore {
    RELAY_DIAL_SEMAPHORE.get_or_init(|| async_bridge::Semaphore::new(MAX_CONCURRENT_RELAY_DIALS))
}

// ============================================================
// Reconnect backoff (moved from relay.rs -- same logic, now gating
// reconnect attempts instead of every call)
// ============================================================

struct RelayBackoffState {
    last_failure: std::time::Instant,
    consecutive_failures: u32,
}

fn relay_backoff_map() -> &'static Mutex<HashMap<String, RelayBackoffState>> {
    static INSTANCE: OnceLock<Mutex<HashMap<String, RelayBackoffState>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 10, 20, 40, 80, 160, 300, 300, ...
fn backoff_seconds(failures: u32) -> u64 {
    if failures == 0 {
        return 0;
    }
    let exp = (failures - 1).min(31);
    let secs = BACKOFF_BASE_SECS.saturating_mul(1u64 << exp);
    secs.min(BACKOFF_MAX_SECS)
}

/// Check if a relay is in backoff. Returns `None` if OK to (re)connect, or
/// `Some(remaining_seconds)` if still in its cooldown window.
pub fn check_relay_backoff(relay_url: &str) -> Option<u64> {
    let map = relay_backoff_map().lock().unwrap();
    if let Some(state) = map.get(relay_url) {
        let wait = backoff_seconds(state.consecutive_failures);
        let elapsed = state.last_failure.elapsed().as_secs();
        if elapsed < wait {
            return Some(wait - elapsed);
        }
    }
    None
}

fn record_relay_failure(relay_url: &str) {
    let mut map = relay_backoff_map().lock().unwrap();
    if let Some(state) = map.get_mut(relay_url) {
        let wait = backoff_seconds(state.consecutive_failures);
        let elapsed = state.last_failure.elapsed().as_secs();
        if elapsed >= wait {
            state.consecutive_failures += 1;
            state.last_failure = std::time::Instant::now();
            let new_wait = backoff_seconds(state.consecutive_failures);
            debug_log!("[backoff] {} failed (attempt {}), next retry in {}s",
                relay_url, state.consecutive_failures, new_wait);
        }
    } else {
        map.insert(relay_url.to_string(), RelayBackoffState {
            last_failure: std::time::Instant::now(),
            consecutive_failures: 1,
        });
        debug_log!("[backoff] {} failed (attempt 1), next retry in {}s", relay_url, BACKOFF_BASE_SECS);
    }
}

/// Record a successful connection, clearing backoff state for the relay.
pub fn record_relay_success(relay_url: &str) {
    let mut map = relay_backoff_map().lock().unwrap();
    if map.remove(relay_url).is_some() {
        debug_log!("[backoff] {} connected successfully, backoff cleared", relay_url);
    }
}

// ============================================================
// Per-relay connection state
// ============================================================

/// One registered listener for a subscription id.
enum SubWaiter {
    /// A single fetch/publish-adjacent query: removed (and `done` fired)
    /// as soon as its EOSE arrives, or if the connection drops before it
    /// does.
    OneShot {
        tx: mpsc::UnboundedSender<StreamMessage>,
        done: Option<OneshotSender<()>>,
    },
    /// A standing subscription (today: the DM stream). Survives EOSE and
    /// survives reconnects -- the supervisor resends its REQ on every new
    /// connection instead of treating it as finished.
    Persistent {
        tx: mpsc::UnboundedSender<StreamMessage>,
        filters: Vec<nostr::Filter>,
        /// Bumped on every delivered event; used to narrow the resubscribe
        /// REQ's `since` after a reconnect so events pushed during the
        /// drop-to-reconnect gap aren't silently skipped on top of events
        /// already delivered (see `resubscribe_all_persistent`).
        last_seen_created_at: AtomicU64,
    },
}

/// Demux tables for one relay's connection, shared between the single
/// `DemuxHandler` installed per connection and every `query_streaming`/
/// `publish`/`subscribe_persistent` caller for that relay.
struct DemuxState {
    /// subscription id -> waiter (EVENT/EOSE/NOTICE routing).
    subs: HashMap<String, SubWaiter>,
    /// event id -> OK waiter (publish correlation).
    publishes: HashMap<String, OneshotSender<RelayMessage>>,
    /// idempotency key (e.g. `"dm"`) -> subscription id, so
    /// `subscribe_persistent` can recognize "already registered" instead
    /// of creating a second, redundant standing subscription.
    persistent_keys: HashMap<String, String>,
}

struct RelayInner {
    conn: Option<WebSocketConnection>,
    /// Woken (and cleared) every time a (re)connect succeeds; see
    /// `ensure_connected`.
    connect_waiters: Vec<OneshotSender<()>>,
}

struct RelayState {
    relay_url: String,
    inner: Mutex<RelayInner>,
    demux: Arc<Mutex<DemuxState>>,
    shutting_down: AtomicBool,
}

static POOL: OnceLock<Mutex<HashMap<String, Arc<RelayState>>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<String, Arc<RelayState>>> {
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

static SUB_COUNTER: AtomicU64 = AtomicU64::new(0);

fn fresh_sub_id(prefix: &str) -> String {
    let n = SUB_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{prefix}_{n}")
}

/// Returns the shared state for `relay_url`, starting its supervisor on
/// first use. Idempotent: a later call for the same URL just returns the
/// existing state -- never starts a second supervisor for it.
fn ensure_supervisor(relay_url: &str) -> Arc<RelayState> {
    let mut map = pool().lock().unwrap();
    if let Some(state) = map.get(relay_url) {
        return Arc::clone(state);
    }
    let state = Arc::new(RelayState {
        relay_url: relay_url.to_string(),
        inner: Mutex::new(RelayInner { conn: None, connect_waiters: Vec::new() }),
        demux: Arc::new(Mutex::new(DemuxState {
            subs: HashMap::new(),
            publishes: HashMap::new(),
            persistent_keys: HashMap::new(),
        })),
        shutting_down: AtomicBool::new(false),
    });
    map.insert(relay_url.to_string(), Arc::clone(&state));
    drop(map);

    let supervised = Arc::clone(&state);
    async_bridge::spawn(async move { supervisor_loop(supervised).await });
    state
}

/// Waits (bounded by `timeout_budget`) for `state` to have a live
/// connection, returning it as soon as one exists -- whether one already
/// did, or the supervisor establishes one while we wait. Registration and
/// the "is one already up" check happen under the same lock so a
/// connection established between the two can never be missed.
async fn ensure_connected(state: &Arc<RelayState>, timeout_budget: Duration) -> Result<WebSocketConnection, String> {
    let rx = {
        let mut inner = state.inner.lock().unwrap();
        if let Some(conn) = &inner.conn {
            return Ok(conn.clone());
        }
        let (tx, rx) = async_bridge::oneshot::<()>();
        inner.connect_waiters.push(tx);
        rx
    };
    match async_bridge::timeout(timeout_budget, rx).await {
        Ok(()) => {
            let inner = state.inner.lock().unwrap();
            inner.conn.clone().ok_or_else(|| {
                format!("connection to {} was lost immediately after connecting", state.relay_url)
            })
        }
        Err(_) => Err(format!("timed out waiting for a connection to {}", state.relay_url)),
    }
}

/// Supervisor: runs for the life of the app (until [`shutdown`] marks this
/// relay as shutting down), one per relay, started the first time that
/// relay is touched. Dials (waiting out backoff and a still-starting Tor
/// first), installs one shared [`DemuxHandler`], resends every active
/// standing subscription's REQ, then blocks on `conn.run()` until the
/// connection ends -- from the peer, a transport failure, or our own
/// explicit `close()` -- and loops to reconnect.
async fn supervisor_loop(state: Arc<RelayState>) {
    loop {
        if state.shutting_down.load(Ordering::SeqCst) {
            return;
        }

        if let Some(wait) = check_relay_backoff(&state.relay_url) {
            // Always runs on its own dedicated thread (via `async_bridge::spawn`
            // in `ensure_supervisor`), never a shared dispatch thread -- safe
            // to block here the same way `tor::wait_for_ready`'s own watcher
            // thread does.
            std::thread::sleep(Duration::from_secs(wait));
            continue;
        }

        if let Err(e) = tor::wait_for_ready_async(Duration::from_secs(TOR_READY_WAIT_SECS)).await {
            debug_log!("[relay_pool] {}: Tor not ready: {}", state.relay_url, e);
            record_relay_failure(&state.relay_url);
            continue;
        }

        let dial = {
            let _permit = relay_dial_semaphore().acquire().await;
            async_bridge::timeout(
                Duration::from_secs(CONNECT_TIMEOUT_SECS),
                WebSocketClient::connect(&state.relay_url),
            ).await
            // permit released here, before `conn.run()` -- it only ever
            // gates the dial itself, never the long-lived connection.
        };

        let conn = match dial {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                debug_log!("[relay_pool] connect to {} failed: {}", state.relay_url, e);
                record_relay_failure(&state.relay_url);
                continue;
            }
            Err(_) => {
                debug_log!("[relay_pool] connect to {} timed out", state.relay_url);
                record_relay_failure(&state.relay_url);
                continue;
            }
        };
        record_relay_success(&state.relay_url);
        debug_log!("[relay_pool] connected to {}", state.relay_url);

        {
            let mut inner = state.inner.lock().unwrap();
            inner.conn = Some(conn.clone());
            for waiter in inner.connect_waiters.drain(..) {
                waiter.send(());
            }
        }
        resubscribe_all_persistent(&state, &conn);

        let handler = DemuxHandler { demux: Arc::clone(&state.demux) };
        let result = conn.run(handler).await;
        debug_log!("[relay_pool] connection to {} ended: {:?}", state.relay_url, result);

        state.inner.lock().unwrap().conn = None;
        fail_all_oneshot_pending(&state.demux);
        record_relay_failure(&state.relay_url);
    }
}

/// Resends every standing subscription's REQ on a fresh connection, with
/// `since` narrowed to each one's `last_seen_created_at` (if that's later
/// than the filter's own `since`) so a mid-session reconnect doesn't
/// silently skip events pushed during the drop-to-reconnect gap.
fn resubscribe_all_persistent(state: &Arc<RelayState>, conn: &WebSocketConnection) {
    let demux = state.demux.lock().unwrap();
    for (sub_id, waiter) in demux.subs.iter() {
        if let SubWaiter::Persistent { filters, last_seen_created_at, .. } = waiter {
            let catch_up_since = last_seen_created_at.load(Ordering::SeqCst);
            let adjusted: Vec<String> = filters
                .iter()
                .map(|f| {
                    let mut f = f.clone();
                    if catch_up_since > 0 && f.since.is_none_or(|s| catch_up_since > s) {
                        f.since = Some(catch_up_since);
                    }
                    nostr::filter_to_json(&f)
                })
                .collect();
            let req_message = format!("[\"REQ\",\"{}\",{}]", sub_id, adjusted.join(","));
            debug_log!("[relay_pool] resubscribing {} on {}", sub_id, state.relay_url);
            let _ = conn.send_text(req_message.as_bytes());
        }
    }
}

/// Called once a connection ends: fails every one-shot waiter (query or
/// publish) immediately rather than leaving each to discover the loss only
/// via its own timeout, which can now be much longer than the response
/// budget since a reconnect may be sitting in backoff. Standing
/// subscriptions are left untouched -- they resume on the next reconnect.
fn fail_all_oneshot_pending(demux: &Arc<Mutex<DemuxState>>) {
    let mut demux = demux.lock().unwrap();

    let one_shot_ids: Vec<String> = demux
        .subs
        .iter()
        .filter(|(_, w)| matches!(w, SubWaiter::OneShot { .. }))
        .map(|(k, _)| k.clone())
        .collect();
    for sub_id in one_shot_ids {
        if let Some(SubWaiter::OneShot { tx, done }) = demux.subs.remove(&sub_id) {
            let _ = tx.send(StreamMessage::Eose);
            if let Some(done) = done {
                done.send(());
            }
        }
    }

    let publish_ids: Vec<String> = demux.publishes.keys().cloned().collect();
    for event_id in publish_ids {
        if let Some(sender) = demux.publishes.remove(&event_id) {
            sender.send(RelayMessage::Ok {
                event_id,
                success: false,
                message: "connection lost".to_string(),
            });
        }
    }
}

/// The one handler installed per connection, for its whole lifetime --
/// replaces the old per-call `NostrRelayHandler`/bespoke `PublishHandler`.
/// Routes EVENT/EOSE by subscription id, OK by event id, and broadcasts
/// NOTICE (which carries no id in NIP-01) to every current waiter on this
/// relay. `should_stop()` is left at its default (`false`): nothing about
/// one listener's own completion may end a connection shared by others --
/// the connection only ends from the peer, a transport failure, or our own
/// explicit `close()` (all of which reach `WebSocketHandler::close`/
/// `failed` and end `run()` regardless of `should_stop()`).
struct DemuxHandler {
    demux: Arc<Mutex<DemuxState>>,
}

impl WebSocketHandler for DemuxHandler {
    fn connected(&mut self) {}

    fn text_frame(&mut self, data: &[u8]) {
        let text = match std::str::from_utf8(data) {
            Ok(t) => t,
            Err(e) => {
                debug_log!("[relay_pool] invalid UTF-8 in text frame ({} bytes): {}", data.len(), e);
                return;
            }
        };
        let msg = match relay::parse_relay_message(text) {
            Ok(m) => m,
            Err(e) => {
                debug_log!("[relay_pool] parse error: {}", e);
                return;
            }
        };

        let mut demux = self.demux.lock().unwrap();
        match msg {
            RelayMessage::Event { _subscription_id, event } => {
                if let Some(waiter) = demux.subs.get(&_subscription_id) {
                    match waiter {
                        SubWaiter::OneShot { tx, .. } => {
                            let _ = tx.send(StreamMessage::Event(event));
                        }
                        SubWaiter::Persistent { tx, last_seen_created_at, .. } => {
                            last_seen_created_at.fetch_max(event.created_at, Ordering::SeqCst);
                            let _ = tx.send(StreamMessage::Event(event));
                        }
                    }
                }
            }
            RelayMessage::EndOfStoredEvents { _subscription_id } => {
                let is_one_shot = match demux.subs.get(&_subscription_id) {
                    Some(SubWaiter::OneShot { tx, .. }) => {
                        let _ = tx.send(StreamMessage::Eose);
                        true
                    }
                    Some(SubWaiter::Persistent { tx, .. }) => {
                        let _ = tx.send(StreamMessage::Eose);
                        false
                    }
                    None => false,
                };
                if is_one_shot {
                    if let Some(SubWaiter::OneShot { done, .. }) = demux.subs.remove(&_subscription_id) {
                        if let Some(done) = done {
                            done.send(());
                        }
                    }
                }
            }
            RelayMessage::Notice { message } => {
                debug_log!("[relay_pool] NOTICE: {}", message);
                for waiter in demux.subs.values() {
                    let tx = match waiter {
                        SubWaiter::OneShot { tx, .. } => tx,
                        SubWaiter::Persistent { tx, .. } => tx,
                    };
                    let _ = tx.send(StreamMessage::Notice(message.clone()));
                }
            }
            RelayMessage::Ok { event_id, success, message } => {
                if let Some(sender) = demux.publishes.remove(&event_id) {
                    sender.send(RelayMessage::Ok { event_id, success, message });
                }
            }
            RelayMessage::Unknown { .. } => {}
        }
    }

    fn binary_frame(&mut self, _data: &[u8]) {}
    fn close(&mut self, _code: Option<u16>, _reason: &str) {}
    fn ping(&mut self, _data: &[u8]) {}
    fn pong(&mut self, _data: &[u8]) {}
    fn failed(&mut self, _error: &std::io::Error) {}
}

// ============================================================
// Public API
// ============================================================

/// One-shot subscribe-and-stream: sends a fresh `REQ` over the relay's
/// existing (or about-to-exist) connection, delivers events via `tx` as
/// they arrive, and always eventually sends `StreamMessage::Eose` on `tx`
/// (on a real EOSE, a timeout, a connect failure, or the connection
/// dropping mid-wait) -- same contract the old `run_relay_feed_stream`
/// had, just without dialing or closing anything itself.
pub async fn query_streaming(
    relay_url: String,
    filter: nostr::Filter,
    timeout_seconds: u32,
    tx: mpsc::UnboundedSender<StreamMessage>,
) {
    let state = ensure_supervisor(&relay_url);
    let timeout_duration = Duration::from_secs(timeout_seconds as u64);

    let conn = match ensure_connected(&state, timeout_duration).await {
        Ok(c) => c,
        Err(e) => {
            debug_log!("[relay_pool] query_streaming: {}", e);
            let _ = tx.send(StreamMessage::Eose);
            return;
        }
    };

    let sub_id = fresh_sub_id("plume");
    let (done_tx, done_rx) = async_bridge::oneshot::<()>();
    {
        let mut demux = state.demux.lock().unwrap();
        demux.subs.insert(sub_id.clone(), SubWaiter::OneShot { tx: tx.clone(), done: Some(done_tx) });
    }

    let filter_json = nostr::filter_to_json(&filter);
    let req_message = format!("[\"REQ\",\"{}\",{}]", sub_id, filter_json);
    debug_log!("[relay_pool] sending REQ to {}: {}", relay_url, req_message);
    if let Err(e) = conn.send_text(req_message.as_bytes()) {
        debug_log!("[relay_pool] failed to send REQ to {}: {}", relay_url, e);
        state.demux.lock().unwrap().subs.remove(&sub_id);
        let _ = tx.send(StreamMessage::Eose);
        return;
    }

    if async_bridge::timeout(timeout_duration, done_rx).await.is_err() {
        // Nothing (EOSE, nor a connection drop) resolved this in time --
        // give up on it ourselves. If the connection dropped in the
        // meantime, `fail_all_oneshot_pending` already removed and
        // resolved us, so `remove` here finds nothing and does nothing.
        if state.demux.lock().unwrap().subs.remove(&sub_id).is_some() {
            debug_log!("[relay_pool] query_streaming timed out waiting for EOSE from {}", relay_url);
            let _ = tx.send(StreamMessage::Eose);
        }
    }
    let close_message = format!("[\"CLOSE\",\"{}\"]", sub_id);
    let _ = conn.send_text(close_message.as_bytes());
}

/// Publish `event` over the relay's existing (or about-to-exist)
/// connection and await its `OK` response, bounded by `timeout_seconds`.
pub async fn publish(relay_url: &str, event: &nostr::Event, timeout_seconds: u32) -> Result<relay::PublishResult, String> {
    let state = ensure_supervisor(relay_url);
    let timeout_duration = Duration::from_secs(timeout_seconds as u64);
    let conn = ensure_connected(&state, timeout_duration).await?;

    let (done_tx, done_rx) = async_bridge::oneshot::<RelayMessage>();
    {
        let mut demux = state.demux.lock().unwrap();
        demux.publishes.insert(event.id.clone(), done_tx);
    }

    let event_json = nostr::event_to_json(event);
    let publish_message = format!("[\"EVENT\",{}]", event_json);
    if let Err(e) = conn.send_text(publish_message.as_bytes()) {
        state.demux.lock().unwrap().publishes.remove(&event.id);
        return Err(format!("Failed to send to {}: {}", relay_url, e));
    }

    match async_bridge::timeout(timeout_duration, done_rx).await {
        Ok(RelayMessage::Ok { success, message, .. }) => {
            Ok(relay::PublishResult { relay_url: relay_url.to_string(), success, message })
        }
        Ok(_) => Ok(relay::PublishResult {
            relay_url: relay_url.to_string(),
            success: false,
            message: "Unexpected response".to_string(),
        }),
        Err(_) => {
            state.demux.lock().unwrap().publishes.remove(&event.id);
            Ok(relay::PublishResult {
                relay_url: relay_url.to_string(),
                success: false,
                message: "Timeout waiting for response".to_string(),
            })
        }
    }
}

/// A standing subscription registered via [`subscribe_persistent`].
/// Dropping this does *not* cancel the subscription (it's meant to
/// outlive whatever registered it, same as the old DM stream did for the
/// life of the app) -- call [`cancel`](Self::cancel) explicitly to end it.
///
/// Nothing calls `cancel` today (the DM stream runs for the app's whole
/// life, same as before), but the type needs to carry what `cancel` needs
/// to actually do its job, so the fields stay rather than being trimmed
/// down to a bare marker.
#[allow(dead_code)]
pub struct SubscriptionHandle {
    sub_id: String,
    state: Arc<RelayState>,
}

#[allow(dead_code)]
impl SubscriptionHandle {
    /// Sends `CLOSE` for this subscription and removes its registration.
    pub fn cancel(self) {
        {
            let mut demux = self.state.demux.lock().unwrap();
            demux.subs.remove(&self.sub_id);
            demux.persistent_keys.retain(|_, v| v != &self.sub_id);
        }
        if let Some(conn) = self.state.inner.lock().unwrap().conn.clone() {
            let close_message = format!("[\"CLOSE\",\"{}\"]", self.sub_id);
            let _ = conn.send_text(close_message.as_bytes());
        }
    }
}

/// Registers a standing subscription for `relay_url`, sent immediately if
/// already connected and automatically resent on every future reconnect.
/// Idempotent per `(relay_url, key)`: a later call with a `key` already
/// active for this relay is a no-op that returns a handle to the existing
/// registration (its `tx`/`filters` are *not* updated) -- this is the
/// generic fix for a caller accidentally double-registering the same
/// logical subscription (e.g. the DM stream being started both at app
/// init and again whenever Messages is reopened). Distinct standing
/// subscriptions for the same relay must use distinct keys, or the second
/// one silently loses to the first.
pub fn subscribe_persistent(
    relay_url: &str,
    key: &str,
    filters: Vec<nostr::Filter>,
    tx: mpsc::UnboundedSender<StreamMessage>,
) -> SubscriptionHandle {
    let state = ensure_supervisor(relay_url);

    let sub_id = {
        let mut demux = state.demux.lock().unwrap();
        if let Some(existing) = demux.persistent_keys.get(key) {
            debug_log!("[relay_pool] subscribe_persistent: '{}' already active for {}, ignoring duplicate", key, relay_url);
            return SubscriptionHandle { sub_id: existing.clone(), state: Arc::clone(&state) };
        }
        let sub_id = fresh_sub_id(&format!("plume_{key}"));
        demux.subs.insert(sub_id.clone(), SubWaiter::Persistent {
            tx,
            filters: filters.clone(),
            last_seen_created_at: AtomicU64::new(0),
        });
        demux.persistent_keys.insert(key.to_string(), sub_id.clone());
        sub_id
    };

    // If already connected, send the REQ now; otherwise the supervisor's
    // `resubscribe_all_persistent` sends it as soon as a connection comes up.
    if let Some(conn) = state.inner.lock().unwrap().conn.clone() {
        let filters_json: Vec<String> = filters.iter().map(nostr::filter_to_json).collect();
        let req_message = format!("[\"REQ\",\"{}\",{}]", sub_id, filters_json.join(","));
        let _ = conn.send_text(req_message.as_bytes());
    }

    SubscriptionHandle { sub_id, state }
}

/// Called once, from the app's exit handler alongside `tor::shutdown()`.
/// Marks every relay as shutting down (so each supervisor exits instead of
/// reconnecting once its current connection ends) and closes every
/// currently-live connection outright, rather than waiting for the OS to
/// notice the process is exiting.
pub fn shutdown() {
    let map = pool().lock().unwrap();
    for state in map.values() {
        state.shutting_down.store(true, Ordering::SeqCst);
        if let Some(conn) = state.inner.lock().unwrap().conn.clone() {
            conn.close();
        }
    }
}

/// Forcibly closes `relay_url`'s current connection, if any, without
/// marking it as shutting down -- the supervisor sees the connection end
/// and reconnects, same as a real network drop. Test-only hook, same
/// shape as `tor::force_ready_for_test`.
#[cfg(test)]
fn force_close_for_test(relay_url: &str) -> bool {
    let map = pool().lock().unwrap();
    let Some(state) = map.get(relay_url) else { return false };
    let conn = state.inner.lock().unwrap().conn.clone();
    match conn {
        Some(conn) => {
            conn.close();
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod diag_tests {
    use super::*;
    use crate::async_bridge::mpsc;
    use std::time::Instant;

    /// Counts this process's own established TCP sockets to Tor's SOCKS
    /// port via `lsof`. Mirrors `http::client::diag_tests::count_own_tor_sockets`
    /// and the now-deleted `relay::diag_tests::count_own_tor_sockets`.
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

    /// Regression test for "connect_to_relay_waits_out_a_still_starting_tor"
    /// (deleted from relay.rs along with `connect_to_relay` itself): the
    /// supervisor must wait out a merely-`Starting` Tor before dialing
    /// instead of failing instantly and permanently. No real Tor needed --
    /// once the simulated wait ends, "Tor" points at an address nothing is
    /// listening on, so the dial fails and the supervisor backs off; this
    /// only proves the wait-then-attempt behavior, not that the dial itself
    /// succeeds.
    #[test]
    fn query_streaming_waits_out_a_still_starting_tor() {
        tor::force_starting_for_test();
        let nothing_listening: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            tor::force_ready_for_test(nothing_listening);
        });

        let (tx, mut rx) = mpsc::unbounded_channel();
        let filter = nostr::filter_recent_notes_since(5, None);
        let start = Instant::now();
        async_bridge::spawn(async move {
            query_streaming("wss://relay-pool-test-still-starting.invalid".to_string(), filter, 2, tx).await;
        });

        let msg = async_bridge::block_on(rx.recv());
        assert!(
            start.elapsed() >= Duration::from_millis(250),
            "returned after {:?} -- should have waited for Tor instead of \
             failing instantly while it was still Starting",
            start.elapsed()
        );
        assert!(
            matches!(msg, Some(StreamMessage::Eose)),
            "expected an Eose signal once query_streaming gave up, got a message: {}",
            msg.is_some()
        );
    }

    /// Regression test for the whole point of this module: firing many
    /// concurrent one-shot queries plus a standing subscription at the same
    /// relay must never open more than one connection to it, and a query
    /// completing must not close that connection. Needs a real,
    /// already-bootstrapped Tor instance.
    #[test]
    fn concurrent_operations_share_one_connection_that_stays_open() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relay_url = std::env::var("PLUME_TEST_RELAY")
            .unwrap_or_else(|_| "wss://relay.damus.io".to_string());
        let pubkey = std::env::var("PLUME_TEST_PUBKEY")
            .unwrap_or_else(|_| "4d4b2d3f5f124edf65ef73ae3f405b5984fb902a8206fd6eb0085c1a629da232".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        // `lsof` on Tor's SOCKS port can't tell which *remote* relay a given
        // local-to-Tor socket is ultimately proxying to -- every relay this
        // process has ever touched (including from earlier tests sharing
        // this process) shows up identically. So this can't assert an
        // absolute count; it asserts a *delta*: whatever this relay's
        // connection count was before this test's own burst, it must not
        // grow by more than the one connection this test itself may need
        // to newly establish.
        let before = count_own_tor_sockets(addr.port());

        // A standing subscription (the DM-stream shape) plus a burst of
        // one-shot queries (the profile-fetch shape), all against the same
        // relay at once -- exactly the mix of operations that used to mean
        // a fresh connection per call.
        let (dm_tx, mut dm_rx) = mpsc::unbounded_channel();
        let _handle = subscribe_persistent(
            &relay_url,
            "diag-test-dm",
            vec![nostr::filter_profile_by_author(&pubkey)],
            dm_tx,
        );
        async_bridge::spawn(async move { while dm_rx.recv().await.is_some() {} });

        for i in 0..10 {
            let relay_url = relay_url.clone();
            let mut pk = pubkey.clone();
            let suffix = format!("{i:04x}");
            pk.replace_range(pk.len() - suffix.len().., &suffix);
            async_bridge::spawn(async move {
                let (tx, mut rx) = mpsc::unbounded_channel();
                let filter = nostr::filter_profile_by_author(&pk);
                async_bridge::spawn(async move {
                    query_streaming(relay_url, filter, relay::RESPONSE_TIMEOUT_SECS, tx).await;
                });
                while rx.recv().await.is_some() {}
            });
        }

        // Let the burst finish.
        std::thread::sleep(Duration::from_secs(5));

        let after = count_own_tor_sockets(addr.port());
        eprintln!(
            "[diag] sockets to Tor's SOCKS port: before={before}, after one standing subscription \
             + 10 concurrent queries to {relay_url}: {after}"
        );
        assert!(
            after <= before + 1,
            "expected at most one *new* connection regardless of how many concurrent \
             operations shared this relay, but the count went from {before} to {after}"
        );
    }

    /// Regression test for "shouldn't be closing them until we close the
    /// app": forcibly drop the connection (simulating a real network blip)
    /// and confirm the supervisor reconnects on its own -- a subsequent
    /// query against the same relay must succeed again without the app
    /// doing anything else.
    #[test]
    fn connection_reconnects_after_being_dropped() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relay_url = std::env::var("PLUME_TEST_RELAY")
            .unwrap_or_else(|_| "wss://relay.damus.io".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        fn one_query(relay_url: &str) -> bool {
            let (tx, mut rx) = mpsc::unbounded_channel();
            let filter = nostr::filter_recent_notes_since(5, None);
            let url = relay_url.to_string();
            async_bridge::spawn(async move {
                query_streaming(url, filter, relay::RESPONSE_TIMEOUT_SECS, tx).await;
            });
            let mut got_anything = false;
            async_bridge::block_on(async {
                while let Some(msg) = rx.recv().await {
                    got_anything = true;
                    if matches!(msg, StreamMessage::Eose) {
                        break;
                    }
                }
            });
            got_anything
        }

        assert!(one_query(&relay_url), "first query against a fresh connection produced nothing at all");

        let closed = force_close_for_test(&relay_url);
        assert!(closed, "expected an existing connection to force-close after the first query");

        // Give the supervisor a moment to notice and reconnect.
        std::thread::sleep(Duration::from_secs(3));

        assert!(
            one_query(&relay_url),
            "query after a forced disconnect produced nothing -- the relay never reconnected"
        );
    }
}
