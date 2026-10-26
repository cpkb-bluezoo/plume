/*
 * tor_dial.rs
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

//! The one dial path every outbound connection in Plume goes through:
//! SOCKS5 CONNECT to the local Tor process (`crate::tor`), with the target
//! host sent as a domain name so DNS is resolved by Tor, never locally.
//! TLS, when needed, is layered on top of the SOCKS tunnel afterwards --
//! never on top of the dial to the proxy itself.

use std::io;
use std::sync::Arc;

use hopf::core::{Endpoint, ProtocolHandler, Runtime, SecurityInfo, SharedTlsConnector};
use hopf::socks::{socks_connect_config, SocksClientConfig, SocksClientVersion};
use sha2::{Digest, Sha256};

use crate::tor;

/// Dial `target_host:target_port` through the local Tor SOCKS5 proxy,
/// optionally layering TLS on top once the SOCKS tunnel is established.
///
/// `inner_factory` builds the protocol handler (HTTP or WebSocket) that
/// receives `connected`/`security_established`/`receive` exactly as if it
/// were talking to the target directly -- it never sees Tor or SOCKS.
///
/// Fails closed: if Tor isn't [`tor::TorStatus::Ready`], this returns an
/// error immediately rather than falling back to a direct connection.
pub fn dial_via_tor(
    rt: &Arc<Runtime>,
    target_host: &str,
    target_port: u16,
    tls: Option<SharedTlsConnector>,
    inner_factory: impl Fn() -> Box<dyn ProtocolHandler> + Send + Sync + 'static,
) -> io::Result<()> {
    let proxy_addr = tor::socks_addr()?;

    let socks_config = SocksClientConfig::new(SocksClientVersion::Socks5)
        .with_credentials(circuit_isolation_user(target_host), "plume");

    let server_name = target_host.to_string();
    let cfg = socks_connect_config(proxy_addr, socks_config, target_host, target_port, move || {
        match &tls {
            Some(connector) => Box::new(TlsBridgeHandler::new(
                Arc::clone(connector),
                server_name.clone(),
                inner_factory(),
            )) as Box<dyn ProtocolHandler>,
            None => inner_factory(),
        }
    });

    rt.connect(cfg)
}

/// A short, non-identifying per-destination SOCKS5 username so Tor's
/// default `IsolateSOCKSAuth` behavior routes different destination hosts
/// over different circuits -- basic stream isolation between the relays and
/// content hosts Plume talks to, without the credential itself leaking the
/// real hostname.
fn circuit_isolation_user(host: &str) -> String {
    let digest = Sha256::digest(host.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Bridges a SOCKS5 CONNECT tunnel into a TLS session: once the tunnel is
/// up (`connected`), starts the client TLS handshake to `server_name`; once
/// `security_established` fires, every subsequent callback is forwarded to
/// `inner` exactly like it would be after a direct TLS dial.
struct TlsBridgeHandler {
    connector: SharedTlsConnector,
    server_name: String,
    inner: Box<dyn ProtocolHandler>,
}

impl TlsBridgeHandler {
    fn new(connector: SharedTlsConnector, server_name: String, inner: Box<dyn ProtocolHandler>) -> Self {
        Self { connector, server_name, inner }
    }
}

impl ProtocolHandler for TlsBridgeHandler {
    fn connected(&mut self, endpoint: &mut dyn Endpoint) {
        self.inner.connected(endpoint);
        if let Err(e) = endpoint.start_client_tls(Arc::clone(&self.connector), &self.server_name) {
            self.inner.error(endpoint, &io::Error::other(e.to_string()));
            endpoint.close();
        }
    }

    fn receive(&mut self, endpoint: &mut dyn Endpoint, data: &mut &[u8]) {
        self.inner.receive(endpoint, data);
    }

    fn disconnected(&mut self, endpoint: &mut dyn Endpoint) {
        self.inner.disconnected(endpoint);
    }

    fn security_established(&mut self, endpoint: &mut dyn Endpoint, info: &SecurityInfo) {
        self.inner.security_established(endpoint, info);
    }

    fn error(&mut self, endpoint: &mut dyn Endpoint, err: &io::Error) {
        self.inner.error(endpoint, err);
    }
}
