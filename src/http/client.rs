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
//! then runs hopf-http's H1/H2 session API on top. One request per dial --
//! matches every call site in this codebase, which always builds a fresh
//! `HttpClient` per logical request rather than reusing a connection.

use std::io;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use hopf::core::ProtocolHandler;
use hopf::http::{
    HttpClientConnection, HttpClientSessionConfig, HttpClientSessionHandle, HttpConnectionHandler,
    HttpLimits, HttpResponseHandler,
};

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

#[derive(Clone)]
pub struct HttpClient {
    cmd_tx: mpsc::Sender<Command>,
}

impl HttpClient {
    pub fn new(host: &str, port: u16, secure: bool) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let cmd_rx_cell = Arc::new(Mutex::new(Some(cmd_rx)));

        let tls = if secure { Some(crate::tls::http_tls_connector()) } else { None };
        let limits = HttpLimits::default();
        let host_owned = host.to_string();
        let cmd_rx_for_factory = Arc::clone(&cmd_rx_cell);

        let dial_result = tor_dial::dial_via_tor(&runtime::get(), host, port, tls, move || {
            let config = Arc::new(HttpClientSessionConfig {
                host: host_owned.clone(),
                port,
                limits,
                secure,
                handler: Mutex::new(Some(Box::new(PlumeConnHandler {
                    cmd_rx: Arc::clone(&cmd_rx_for_factory),
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
/// `HttpRequest`/`ResponseHandler` command-queue API. One request per
/// connection: `on_connected` spawns a dedicated worker thread that waits
/// for the queued `SendRequest`, issues it, then streams any body chunks.
///
/// `cmd_rx` is shared with [`HttpClient::new`] (rather than owned outright)
/// because the dial can fail before this handler is ever constructed at
/// all -- whichever side discovers the failure first (`HttpClient::new`'s
/// own `dial_via_tor` call, or this handler's `on_error`) is the one that
/// ends up draining it.
struct PlumeConnHandler {
    cmd_rx: Arc<Mutex<Option<mpsc::Receiver<Command>>>>,
}

impl HttpConnectionHandler for PlumeConnHandler {
    fn on_connected(&mut self, session: &mut HttpClientSessionHandle) {
        let cmd_rx = match self.cmd_rx.lock().unwrap().take() {
            Some(rx) => rx,
            None => return,
        };
        let session = session.clone();
        std::thread::spawn(move || drive_request(session, cmd_rx));
    }

    fn on_error(&mut self, err: &io::Error) {
        if let Some(rx) = self.cmd_rx.lock().unwrap().take() {
            fail_pending_handlers(rx, err);
        }
    }
}

/// Runs on a dedicated worker thread (never the reactor thread): waits for
/// the one `SendRequest` this connection exists for, issues it, then
/// streams any body chunks -- backpressure-aware via `on_body_writable`.
fn drive_request(mut session: HttpClientSessionHandle, cmd_rx: mpsc::Receiver<Command>) {
    let (method, path, headers, handler, has_body) = match cmd_rx.recv() {
        Ok(Command::SendRequest { method, path, headers, handler, has_body }) => {
            (method, path, headers, handler, has_body)
        }
        _ => return,
    };

    let mut request = session.method(&method, &path);
    for (name, value) in &headers {
        let _ = request.header(name, value);
    }

    let response_handler: Arc<Mutex<Box<dyn ResponseHandler>>> = Arc::new(Mutex::new(handler));
    let response_adapter = Box::new(SharedResponseAdapter(Arc::clone(&response_handler)));

    if !has_body {
        let _ = request.send(response_adapter);
        return;
    }

    if request.start_request_body(response_adapter).is_err() {
        return;
    }
    let conn_handle = session.conn_handle();
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
                            if let Some(h) = &conn_handle {
                                h.poke();
                            }
                            response_handler
                                .lock()
                                .unwrap()
                                .request_body_written(bytes_written, bytes_written);
                        }
                        Err(_) => return,
                    }
                }
            }
            Command::EndBody => {
                let _ = request.end_request_body();
                if let Some(h) = &conn_handle {
                    h.poke();
                }
                return;
            }
            Command::SendRequest { .. } => {}
        }
    }
}

/// Translates hopf-http's [`HttpResponseHandler`] callbacks into Plume's
/// own [`ResponseHandler`]. Shared (`Arc<Mutex<_>>`) because
/// [`drive_request`]'s upload-progress reporting needs the same handler.
struct SharedResponseAdapter(Arc<Mutex<Box<dyn ResponseHandler>>>);

impl HttpResponseHandler for SharedResponseAdapter {
    fn ok(&mut self, status: u16) {
        self.0.lock().unwrap().ok(&Response::new(status));
    }

    fn error(&mut self, status: u16) {
        self.0.lock().unwrap().error(&Response::new(status));
    }

    fn header(&mut self, name: &str, value: &str) {
        self.0.lock().unwrap().header(name, value);
    }

    fn start_response_body(&mut self) {
        self.0.lock().unwrap().start_body();
    }

    fn response_body_content(&mut self, data: &[u8]) {
        self.0.lock().unwrap().body_chunk(data);
    }

    fn end_response_body(&mut self) {
        self.0.lock().unwrap().end_body();
    }

    fn response_trailers(&mut self, headers: &hopf::http::Headers) {
        let mut guard = self.0.lock().unwrap();
        for header in headers.iter() {
            guard.header(&header.name, &header.value);
        }
    }

    fn close(&mut self) {
        self.0.lock().unwrap().complete();
    }

    fn failed(&mut self, err: io::Error) {
        self.0.lock().unwrap().failed(&err);
    }
}
