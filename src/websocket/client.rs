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

//! WebSocket client: dial through the local Tor SOCKS5 proxy (never
//! directly), perform the HTTP/1.1 Upgrade handshake via hopf-http's
//! `ClientHandler` layer, then hand off to hopf-websocket's frame handling.

use std::io;
use std::sync::{Arc, Mutex};

use hopf::core::ProtocolHandler;
use hopf::http::{ClientHandler, ClientHandlerFactory, ClientWriter, Headers, HttpLimits};
use hopf::http::H1Endpoint;
use hopf::websocket::{WebSocketOpening, WsUpgradeHandler};
use url::Url;

use crate::async_bridge;
use crate::net::{runtime, tor_dial};
use crate::websocket::connection::WebSocketConnection;

/// Max buffered WebSocket message size (relay/DM payloads are small JSON).
const MAX_WS_PAYLOAD: usize = 16 * 1024 * 1024;

/// WebSocket client. Connect with `WebSocketClient::connect(url)`.
pub struct WebSocketClient;

impl WebSocketClient {
    /// Connect to the given WebSocket URL (ws:// or wss://) through Tor and
    /// perform the opening handshake. Resolves once the `101` response has
    /// been validated and the connection is ready for `send_text`/`run`.
    pub async fn connect(url: &str) -> io::Result<WebSocketConnection> {
        let parsed = Url::parse(url)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "URL has no host"))?
            .to_string();
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "URL has no port"))?;
        let path = if parsed.path().is_empty() { "/".to_string() } else { parsed.path().to_string() };
        let use_tls = matches!(parsed.scheme(), "wss" | "https");
        if !matches!(parsed.scheme(), "ws" | "wss") {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "URL scheme must be ws or wss"));
        }

        let (connect_tx, connect_future) = async_bridge::oneshot::<io::Result<WebSocketConnection>>();
        let connect_tx_cell = Arc::new(Mutex::new(Some(connect_tx)));

        let tls = if use_tls { Some(crate::tls::ws_tls_connector()) } else { None };
        let host_for_handshake = host.clone();

        let dial_result = tor_dial::dial_via_tor(&runtime::get(), &host, port, tls, move || {
            let factory: Arc<dyn ClientHandlerFactory> = Arc::new(WsClientFactory {
                host: host_for_handshake.clone(),
                path: path.clone(),
                connect_tx: Arc::clone(&connect_tx_cell),
            });
            Box::new(H1Endpoint::client(factory, HttpLimits::default(), use_tls)) as Box<dyn ProtocolHandler>
        });

        if let Err(e) = dial_result {
            return Err(e);
        }

        connect_future.await
    }
}

struct WsClientFactory {
    host: String,
    path: String,
    connect_tx: Arc<Mutex<Option<async_bridge::OneshotSender<io::Result<WebSocketConnection>>>>>,
}

impl ClientHandlerFactory for WsClientFactory {
    fn create_handler(&self) -> Box<dyn ClientHandler> {
        Box::new(WsHandshakeHandler {
            host: self.host.clone(),
            path: self.path.clone(),
            opening: WebSocketOpening::new(None),
            connect_tx: Arc::clone(&self.connect_tx),
        })
    }
}

/// Drives the HTTP/1.1 Upgrade request/response, then installs
/// hopf-websocket's frame handling once the `101` is validated.
struct WsHandshakeHandler {
    host: String,
    path: String,
    opening: WebSocketOpening,
    connect_tx: Arc<Mutex<Option<async_bridge::OneshotSender<io::Result<WebSocketConnection>>>>>,
}

impl WsHandshakeHandler {
    fn finish(&self, result: io::Result<WebSocketConnection>) {
        if let Some(tx) = self.connect_tx.lock().unwrap().take() {
            tx.send(result);
        }
    }
}

impl ClientHandler for WsHandshakeHandler {
    fn start(&mut self, request: &mut dyn ClientWriter) {
        self.opening.write_request(request, &self.path, &self.host);
    }

    fn switching_protocols(&mut self, request: &mut dyn ClientWriter, headers: &Headers) {
        if let Err(e) = self.opening.validate_response(headers) {
            self.finish(Err(io::Error::new(io::ErrorKind::InvalidData, e)));
            return;
        }

        let conn = request.conn_handle();
        let (connection, event_sink) = WebSocketConnection::new_pair(conn.clone());
        if !request.upgrade(Box::new(WsUpgradeHandler::client(event_sink, MAX_WS_PAYLOAD, conn))) {
            self.finish(Err(io::Error::other("failed to install WebSocket upgrade handler")));
            return;
        }
        self.finish(Ok(connection));
    }

    fn response_headers(&mut self, _request: &mut dyn ClientWriter, headers: &Headers) {
        self.finish(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected 101 Switching Protocols, got {}", headers.status_code()),
        )));
    }

    fn response_complete(&mut self, _request: &mut dyn ClientWriter) {}

    fn request_failed(&mut self, _request: &mut dyn ClientWriter, err: &io::Error) {
        self.finish(Err(io::Error::new(err.kind(), err.to_string())));
    }
}
