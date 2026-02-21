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
//! cached `ClientConfig` with native root certificates.

use std::sync::{Arc, OnceLock};
use tokio_rustls::rustls::ClientConfig;

/// TLS config for WebSocket (no ALPN).
static WS_TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

/// TLS config for HTTP (ALPN h2, http/1.1).
static HTTP_TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

/// Install the rustls crypto provider. Must be called once at startup
/// before any TLS use.
pub fn install_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
}

fn load_root_store() -> tokio_rustls::rustls::RootCertStore {
    let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
    let cert_result = rustls_native_certs::load_native_certs();
    for cert in cert_result.certs {
        let _ = root_store.add(cert);
    }
    root_store
}

/// TLS config for WebSocket connections (no ALPN negotiation).
pub fn ws_tls_config() -> Arc<ClientConfig> {
    WS_TLS_CONFIG
        .get_or_init(|| {
            let config = ClientConfig::builder()
                .with_root_certificates(load_root_store())
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// TLS config for HTTP connections (ALPN h2, http/1.1).
pub fn http_tls_config() -> Arc<ClientConfig> {
    HTTP_TLS_CONFIG
        .get_or_init(|| {
            let mut config = ClientConfig::builder()
                .with_root_certificates(load_root_store())
                .with_no_client_auth();
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}
