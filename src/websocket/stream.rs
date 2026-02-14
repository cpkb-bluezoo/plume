/*
 * stream.rs
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

//! WebSocket stream: plain TCP or TLS. Plus TLS config helper.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;

use crate::debug_log;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

/// Cached TLS config (loaded once, reused for all connections).
static TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

/// Unified stream: plain TCP or TLS. Implements AsyncRead + AsyncWrite.
pub enum WsStream {
    Plain(TcpStream),
    Tls(TlsStream<TcpStream>),
}

impl AsyncRead for WsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            WsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            WsStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for WsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            WsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            WsStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            WsStream::Plain(s) => Pin::new(s).poll_flush(cx),
            WsStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            WsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            WsStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Install the rustls crypto provider. Must be called once at startup before any TLS use.
/// Selects ring to avoid ambiguity when both ring and aws-lc-rs features are enabled.
pub fn install_crypto_provider() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
}

/// TLS client config for WebSocket connections.
/// Loaded once from the OS native certificate store, then cached for all connections.
pub fn ws_tls_config() -> Arc<ClientConfig> {
    TLS_CONFIG.get_or_init(|| {
        let mut root_store = tokio_rustls::rustls::RootCertStore::empty();
        let cert_result = rustls_native_certs::load_native_certs();
        for cert in cert_result.certs {
            if let Err(e) = root_store.add(cert) {
                println!("Warning: failed to add a native root cert: {}", e);
            }
        }
        debug_log!("Loaded {} root certificates from system store", root_store.len());
        let config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        Arc::new(config)
    }).clone()
}

/// Connect with TLS to host:port, returning a WsStream::Tls.
pub async fn connect_tls(tcp: TcpStream, host: &str) -> io::Result<WsStream> {
    let server_name: ServerName<'static> = ServerName::try_from(host.to_string())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"))?;
    let connector = TlsConnector::from(ws_tls_config());
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    Ok(WsStream::Tls(tls))
}
