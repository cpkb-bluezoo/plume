/*
 * connection.rs
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

//! WebSocket connection: a cheap, cloneable handle over hopf-websocket's
//! post-upgrade `ConnHandle`. Sends write pre-framed bytes straight to the
//! wire (no reactor round-trip needed); incoming frames arrive via an
//! internal [`hopf::websocket::WsEventHandler`] (installed by `client.rs`
//! at upgrade time) and are dispatched into whichever `WebSocketHandler` is
//! attached via [`WebSocketConnection::run`].

use std::io;
use std::sync::{Arc, Mutex};

use hopf::core::ConnHandle;
use hopf::websocket::{
    write_binary, write_close, write_close_empty, write_pong, write_text, WsEventHandler, WsFrameError,
    WsRole, WsSession,
};

use crate::async_bridge::{self, OneshotFuture};
use crate::websocket::WebSocketHandler;

/// Buffered incoming event, held until a handler is attached via [`WebSocketConnection::run`].
enum PendingEvent {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Closed(Option<u16>, String),
    Failed(io::Error),
}

struct Shared {
    handler: Option<Box<dyn WebSocketHandler + Send>>,
    pending: Vec<PendingEvent>,
    done: Option<async_bridge::OneshotSender<io::Result<()>>>,
    finished: bool,
}

/// WebSocket connection after a successful handshake. `send_*` write
/// directly to the wire; `run()` attaches the handler that receives
/// incoming frames and awaits until the connection ends.
#[derive(Clone)]
pub struct WebSocketConnection {
    conn: ConnHandle,
    shared: Arc<Mutex<Shared>>,
}

impl WebSocketConnection {
    /// Builds the connection handle alongside the hopf-websocket event sink
    /// that feeds it -- call once, right after the WS upgrade succeeds
    /// (see `client.rs`), and install the returned handler via
    /// `WsUpgradeHandler::client`.
    pub(crate) fn new_pair(conn: ConnHandle) -> (Self, Box<dyn WsEventHandler>) {
        let shared = Arc::new(Mutex::new(Shared {
            handler: None,
            pending: Vec::new(),
            done: None,
            finished: false,
        }));
        let sink = Box::new(EventSink { shared: Arc::clone(&shared) });
        (Self { conn, shared }, sink)
    }

    /// Send a text frame.
    pub fn send_text(&self, data: &[u8]) -> io::Result<()> {
        let text = std::str::from_utf8(data)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let mut out = Vec::new();
        write_text(&mut out, WsRole::Client, text);
        self.conn.send(out);
        Ok(())
    }

    /// Send a binary frame.
    #[allow(dead_code)]
    pub fn send_binary(&self, data: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        write_binary(&mut out, WsRole::Client, data);
        self.conn.send(out);
        Ok(())
    }

    /// Send a pong frame (e.g. in response to a ping).
    #[allow(dead_code)]
    pub fn send_pong(&self, data: &[u8]) -> io::Result<()> {
        let mut out = Vec::new();
        write_pong(&mut out, WsRole::Client, data);
        self.conn.send(out);
        Ok(())
    }

    /// Send a close frame. Reason is UTF-8; code is optional (e.g. 1000 = normal).
    #[allow(dead_code)]
    pub fn send_close(&self, code: Option<u16>, reason: &str) -> io::Result<()> {
        let mut out = Vec::new();
        match code {
            Some(c) => write_close(&mut out, WsRole::Client, c, reason),
            None => write_close_empty(&mut out, WsRole::Client),
        }
        self.conn.send(out);
        Ok(())
    }

    /// Attach `handler` and wait until the connection closes, fails, or
    /// `handler.should_stop()` becomes true. Replays any frames that
    /// arrived before this call (`opened`/early messages can race ahead of
    /// `run()` when a relay replies unusually fast).
    pub fn run<H: WebSocketHandler + Send + 'static>(&self, handler: H) -> OneshotFuture<io::Result<()>> {
        let (tx, fut) = async_bridge::oneshot();
        let pending = {
            let mut guard = self.shared.lock().unwrap();
            guard.handler = Some(Box::new(handler));
            guard.done = Some(tx);
            std::mem::take(&mut guard.pending)
        };
        if let Some(h) = self.shared.lock().unwrap().handler.as_mut() {
            h.connected();
        }
        for event in pending {
            dispatch(&self.shared, event);
        }
        fut
    }
}

/// Dispatches one event to the attached handler (if any) and finishes the
/// `run()` future when the handler asks to stop or the connection ended.
/// Called either directly from the reactor thread (fresh event, via
/// [`EventSink`]) or from [`WebSocketConnection::run`] (replaying buffered
/// events) -- same path either way, so behavior doesn't depend on which
/// raced first.
fn dispatch(shared: &Arc<Mutex<Shared>>, event: PendingEvent) {
    let mut guard = shared.lock().unwrap();
    if guard.finished {
        return;
    }
    let Some(handler) = guard.handler.as_mut() else {
        guard.pending.push(event);
        return;
    };

    let mut end_result: Option<io::Result<()>> = None;
    match event {
        PendingEvent::Text(data) => {
            handler.text_frame(&data);
            if handler.should_stop() {
                end_result = Some(Ok(()));
            }
        }
        PendingEvent::Binary(data) => {
            handler.binary_frame(&data);
            if handler.should_stop() {
                end_result = Some(Ok(()));
            }
        }
        PendingEvent::Ping(data) => {
            handler.ping(&data);
        }
        PendingEvent::Pong(data) => {
            handler.pong(&data);
        }
        PendingEvent::Closed(code, reason) => {
            handler.close(code, &reason);
            end_result = Some(Ok(()));
        }
        PendingEvent::Failed(err) => {
            handler.failed(&err);
            end_result = Some(Err(err));
        }
    }

    if let Some(result) = end_result {
        guard.finished = true;
        if let Some(tx) = guard.done.take() {
            drop(guard);
            tx.send(result);
        }
    }
}

/// hopf-websocket event sink feeding [`Shared`] from the reactor thread.
struct EventSink {
    shared: Arc<Mutex<Shared>>,
}

impl WsEventHandler for EventSink {
    fn opened(&mut self, _session: &mut WsSession<'_>, _conn: &ConnHandle) {}

    fn text_message(&mut self, _session: &mut WsSession<'_>, text: &str) {
        dispatch(&self.shared, PendingEvent::Text(text.as_bytes().to_vec()));
    }

    fn binary_message(&mut self, _session: &mut WsSession<'_>, data: &[u8]) {
        dispatch(&self.shared, PendingEvent::Binary(data.to_vec()));
    }

    fn ping(&mut self, session: &mut WsSession<'_>, payload: &[u8]) {
        session.send_pong(payload);
        dispatch(&self.shared, PendingEvent::Ping(payload.to_vec()));
    }

    fn pong(&mut self, _session: &mut WsSession<'_>, payload: &[u8]) {
        dispatch(&self.shared, PendingEvent::Pong(payload.to_vec()));
    }

    fn closed(&mut self, _session: &mut WsSession<'_>, code: u16, reason: &str) {
        dispatch(&self.shared, PendingEvent::Closed(Some(code), reason.to_string()));
    }

    fn error(&mut self, err: WsFrameError) {
        dispatch(
            &self.shared,
            PendingEvent::Failed(io::Error::new(io::ErrorKind::InvalidData, err.to_string())),
        );
    }
}
