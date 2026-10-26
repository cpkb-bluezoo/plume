/*
 * tls.rs
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

//! Shared TLS configuration. Both WebSocket and HTTP use this to get a
//! cached connector backed by the OS trust store (via hopf-core, AWS-LC).
//! Every TLS connection in Plume is layered on top of an already-established
//! Tor SOCKS5 tunnel (see `crate::net::tor_dial`) via `start_client_tls`,
//! never dialed directly.

use std::sync::OnceLock;

use hopf::core::{public_trust_connector, SharedTlsConnector};

/// TLS connector for WebSocket connections (no ALPN negotiation).
static WS_TLS_CONNECTOR: OnceLock<SharedTlsConnector> = OnceLock::new();

/// TLS connector for HTTP connections (ALPN h2, http/1.1).
static HTTP_TLS_CONNECTOR: OnceLock<SharedTlsConnector> = OnceLock::new();

/// TLS connector for WebSocket connections (no ALPN).
pub fn ws_tls_connector() -> SharedTlsConnector {
    WS_TLS_CONNECTOR.get_or_init(|| public_trust_connector(&[])).clone()
}

/// TLS connector for HTTP connections (ALPN h2, http/1.1).
pub fn http_tls_connector() -> SharedTlsConnector {
    HTTP_TLS_CONNECTOR
        .get_or_init(|| public_trust_connector(&[b"h2", b"http/1.1"]))
        .clone()
}
