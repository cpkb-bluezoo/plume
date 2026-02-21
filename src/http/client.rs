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

//! HTTP client: per-host:port, manages connection lifecycle via a
//! background tokio task. Fire-and-forget request API.
//! Supports HTTP/1.1 and HTTP/2 (via ALPN).

use crate::http::request::HttpRequest;
use crate::http::ResponseHandler;

use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::client::TlsStream as TokioTlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;

use crate::http::h1::{H1ResponseHandler, ParseState, ResponseParser};
use crate::http::h2::{
    error_to_string, H2FrameHandler, H2Parser, H2Writer, CONNECTION_PREFACE,
    DEFAULT_MAX_FRAME_SIZE, SETTINGS_HEADER_TABLE_SIZE, SETTINGS_INITIAL_WINDOW_SIZE,
    SETTINGS_MAX_FRAME_SIZE,
};
use crate::http::hpack;
use crate::http::response::Response;

// ============================================================
// Stream + Version Types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    Http1_1,
    Http2,
}

enum HttpStream {
    Plain(TcpStream),
    Tls(TokioTlsStream<TcpStream>),
}

impl AsyncRead for HttpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            HttpStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            HttpStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for HttpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            HttpStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            HttpStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            HttpStream::Plain(s) => Pin::new(s).poll_flush(cx),
            HttpStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        match &mut *self {
            HttpStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            HttpStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

// ============================================================
// Command + HttpClient
// ============================================================

pub(crate) enum Command {
    SendRequest {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        handler: Box<dyn ResponseHandler>,
        has_body: bool,
    },
    BodyChunk {
        data: Vec<u8>,
    },
    EndBody,
    #[allow(dead_code)]
    Close,
}

#[derive(Clone)]
pub struct HttpClient {
    cmd_tx: mpsc::UnboundedSender<Command>,
}

impl HttpClient {
    pub fn new(host: &str, port: u16, secure: bool) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let host_owned = host.to_string();

        tokio::spawn(async move {
            if let Err(e) = connection_task(&host_owned, port, secure, cmd_rx).await {
                eprintln!(
                    "HTTP connection task failed for {}:{}: {}",
                    host_owned, port, e
                );
            }
        });

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

    #[allow(dead_code)]
    pub fn close(&self) {
        let _ = self.cmd_tx.send(Command::Close);
    }
}

// ============================================================
// TLS + Connect
// ============================================================

async fn connect(
    host: &str,
    port: u16,
    secure: bool,
) -> io::Result<(HttpStream, HttpVersion)> {
    let addr = format!("{}:{}", host, port);
    let tcp = TcpStream::connect(&addr).await?;

    if secure {
        let server_name: ServerName<'static> = ServerName::try_from(host.to_string())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid host name"))?;
        let connector = TlsConnector::from(crate::tls::http_tls_config());
        let tls = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        let alpn = tls.get_ref().1.alpn_protocol();
        eprintln!(
            "[http] ALPN result for {}: {:?}",
            host,
            alpn.map(|p| String::from_utf8_lossy(p).to_string())
        );
        let version = alpn
            .map(|p| {
                if p == b"h2" {
                    HttpVersion::Http2
                } else {
                    HttpVersion::Http1_1
                }
            })
            .unwrap_or(HttpVersion::Http1_1);
        Ok((HttpStream::Tls(tls), version))
    } else {
        Ok((HttpStream::Plain(tcp), HttpVersion::Http1_1))
    }
}

// ============================================================
// Connection Task (dispatches to H1 or H2 loop)
// ============================================================

async fn connection_task(
    host: &str,
    port: u16,
    secure: bool,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
) -> io::Result<()> {
    eprintln!("[http] connecting to {}:{} (secure={})", host, port, secure);
    let result = connect(host, port, secure).await;
    let (stream, version) = match result {
        Ok(sv) => {
            eprintln!("[http] connected to {}:{} ({:?})", host, port, sv.1);
            sv
        }
        Err(e) => {
            eprintln!("[http] connect failed {}:{}: {}", host, port, e);
            fail_pending_handlers(&mut cmd_rx, &e);
            return Err(e);
        }
    };

    let (reader, writer) = tokio::io::split(stream);

    let result = match version {
        HttpVersion::Http1_1 => h1_loop(host, port, secure, reader, writer, &mut cmd_rx).await,
        HttpVersion::Http2 => h2_loop(host, port, secure, reader, writer, &mut cmd_rx).await,
    };

    if let Err(ref e) = result {
        eprintln!("[http] loop error for {}:{}: {}", host, port, e);
        fail_pending_handlers(&mut cmd_rx, e);
    }
    result
}

/// Drain any pending commands from the channel and call `failed()` on their handlers.
fn fail_pending_handlers(cmd_rx: &mut mpsc::UnboundedReceiver<Command>, error: &io::Error) {
    while let Ok(cmd) = cmd_rx.try_recv() {
        if let Command::SendRequest { mut handler, .. } = cmd {
            handler.failed(error);
        }
    }
}

// ============================================================
// HTTP/1.1 Connection Loop
// ============================================================

struct H1Driver<'a> {
    status_code: &'a mut Option<(u16, Option<String>)>,
    headers: &'a mut Vec<(String, String)>,
    handler: &'a mut dyn ResponseHandler,
}

impl H1ResponseHandler for H1Driver<'_> {
    fn status(&mut self, code: u16, reason: Option<&str>) {
        *self.status_code = Some((code, reason.map(|s| s.to_string())));
    }
    fn header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }
    fn body_chunk(&mut self, data: &[u8]) {
        self.handler.body_chunk(data);
    }
    fn end_body(&mut self) {
        self.handler.end_body();
    }
    fn trailer(&mut self, name: &str, value: &str) {
        self.handler.header(name, value);
    }
    fn complete(&mut self) {
        self.handler.complete();
    }
}

async fn h1_loop<R, W>(
    host: &str,
    port: u16,
    secure: bool,
    mut reader: R,
    mut writer: W,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut read_buf = BytesMut::with_capacity(8192);
    let mut h1_parser = ResponseParser::new();
    let mut h1_status: Option<(u16, Option<String>)> = None;
    let mut h1_headers: Vec<(String, String)> = Vec::new();
    let mut current_handler: Option<Box<dyn ResponseHandler>> = None;
    let mut body_written: u64 = 0;
    let mut body_total: u64 = 0;
    let mut channel_open = true;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv(), if channel_open => {
                match cmd {
                    Some(Command::SendRequest { method, path, headers, handler, has_body }) => {
                        h1_parser.reset();
                        h1_status = None;
                        h1_headers.clear();
                        body_written = 0;
                        body_total = 0;
                        current_handler = Some(handler);

                        let host_header = if (secure && port != 443) || (!secure && port != 80) {
                            format!("{}:{}", host, port)
                        } else {
                            host.to_string()
                        };

                        let mut req = format!(
                            "{} {} HTTP/1.1\r\nHost: {}\r\n",
                            method, path, host_header
                        );
                        for (k, v) in &headers {
                            if has_body && k.eq_ignore_ascii_case("content-length") {
                                continue;
                            }
                            req.push_str(k);
                            req.push_str(": ");
                            req.push_str(v);
                            req.push_str("\r\n");
                        }
                        if has_body {
                            req.push_str("Transfer-Encoding: chunked\r\n");
                        } else {
                            req.push_str("Connection: keep-alive\r\n");
                        }
                        req.push_str("\r\n");

                        if let Err(e) = writer.write_all(req.as_bytes()).await {
                            if let Some(h) = current_handler.as_mut() {
                                h.failed(&e);
                            }
                            return Err(e);
                        }
                        if !has_body {
                            let _ = writer.flush().await;
                        }
                    }
                    Some(Command::BodyChunk { data }) => {
                        let chunk_header = format!("{:x}\r\n", data.len());
                        let mut write_err = None;
                        if let Err(e) = writer.write_all(chunk_header.as_bytes()).await {
                            write_err = Some(e);
                        }
                        if write_err.is_none() {
                            if let Err(e) = writer.write_all(&data).await {
                                write_err = Some(e);
                            }
                        }
                        if write_err.is_none() {
                            if let Err(e) = writer.write_all(b"\r\n").await {
                                write_err = Some(e);
                            }
                        }
                        if let Some(e) = write_err {
                            if let Some(h) = current_handler.as_mut() {
                                h.failed(&e);
                            }
                            return Err(e);
                        }
                        let _ = writer.flush().await;
                        body_written += data.len() as u64;
                        if let Some(h) = current_handler.as_mut() {
                            h.request_body_written(body_written, body_total);
                        }
                    }
                    Some(Command::EndBody) => {
                        if let Err(e) = writer.write_all(b"0\r\n\r\n").await {
                            if let Some(h) = current_handler.as_mut() {
                                h.failed(&e);
                            }
                            return Err(e);
                        }
                        let _ = writer.flush().await;
                    }
                    Some(Command::Close) => return Ok(()),
                    None => {
                        channel_open = false;
                        if current_handler.is_none() {
                            return Ok(());
                        }
                    }
                }
            }
            result = reader.read_buf(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        if let Some(h) = current_handler.as_mut() {
                            if h1_parser.state() == ParseState::Body {
                                h.end_body();
                                h.complete();
                            } else {
                                h.failed(&io::Error::new(io::ErrorKind::ConnectionReset, "connection closed"));
                            }
                        }
                        return Ok(());
                    }
                    Ok(_) => {
                        if let Some(handler) = current_handler.as_mut() {
                            let mut driver = H1Driver {
                                status_code: &mut h1_status,
                                headers: &mut h1_headers,
                                handler: handler.as_mut(),
                            };
                            if let Err(e) = h1_parser.receive(&mut read_buf, &mut driver) {
                                handler.failed(&e);
                                current_handler = None;
                                continue;
                            }

                            if h1_parser.state() == ParseState::HeadersComplete {
                                let (code, reason) = h1_status.take().unwrap_or((0, None));
                                let content_length = h1_headers
                                    .iter()
                                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, v)| v.trim().parse::<u64>().ok());
                                let chunked = h1_headers.iter().any(|(k, v)| {
                                    k.eq_ignore_ascii_case("transfer-encoding")
                                        && v.contains("chunked")
                                });

                                let response = match reason {
                                    Some(r) => Response::with_reason(code, r),
                                    None => Response::new(code),
                                };
                                if (200..300).contains(&code) {
                                    handler.ok(&response);
                                } else {
                                    handler.error(&response);
                                }
                                for (name, value) in &h1_headers {
                                    handler.header(name, value);
                                }
                                let has_body = chunked
                                    || content_length.map(|cl| cl > 0).unwrap_or(false)
                                    || (content_length.is_none()
                                        && !chunked
                                        && code != 204
                                        && code != 304);
                                if has_body {
                                    handler.start_body();
                                    h1_parser.set_body_mode(content_length, chunked);

                                    if !read_buf.is_empty() {
                                        let mut driver2 = H1Driver {
                                            status_code: &mut h1_status,
                                            headers: &mut h1_headers,
                                            handler: handler.as_mut(),
                                        };
                                        let _ = h1_parser.receive(&mut read_buf, &mut driver2);
                                    }
                                } else {
                                    h1_parser.set_body_mode(Some(0), false);
                                    handler.complete();
                                }
                            }

                            if h1_parser.state() == ParseState::Idle {
                                current_handler = None;
                                if !channel_open {
                                    return Ok(());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(h) = current_handler.as_mut() {
                            h.failed(&e);
                        }
                        return Err(e);
                    }
                }
            }
        }
    }
}

// ============================================================
// HTTP/2 Connection Loop
// ============================================================

const H2_DEFAULT_WINDOW: i32 = 65535;

struct H2StreamState {
    handler: Box<dyn ResponseHandler>,
    status_code: u16,
    body_started: bool,
    header_buf: BytesMut,
    end_stream_on_headers: bool,
    body_written: u64,
    body_total: u64,
}

/// H2 connection state. Implements H2FrameHandler so it receives parsed
/// frames. Outgoing frames are accumulated in `writer` and flushed after
/// each `receive()` call.
struct H2State {
    hpack_decoder: hpack::Decoder,
    writer: H2Writer,
    streams: HashMap<u32, H2StreamState>,
    next_stream_id: u32,
    active_body_stream: Option<u32>,
    max_frame_size: usize,
    send_window: i32,
    goaway: bool,
    /// True once server SETTINGS has been received and ACKed.
    ready: bool,
}

impl H2State {
    fn new() -> Self {
        Self {
            hpack_decoder: hpack::Decoder::new(4096),
            writer: H2Writer::new(),
            streams: HashMap::new(),
            next_stream_id: 1,
            active_body_stream: None,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            send_window: H2_DEFAULT_WINDOW,
            goaway: false,
            ready: false,
        }
    }

    fn alloc_stream_id(&mut self) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id += 2;
        id
    }

    fn encode_request(
        &mut self,
        method: &str,
        path: &str,
        authority: &str,
        headers: &[(String, String)],
        end_stream: bool,
    ) -> io::Result<u32> {
        let stream_id = self.alloc_stream_id();

        let mut h2_headers: Vec<(String, String)> = Vec::with_capacity(4 + headers.len());
        h2_headers.push((":method".into(), method.into()));
        h2_headers.push((":path".into(), path.into()));
        h2_headers.push((":scheme".into(), "https".into()));
        h2_headers.push((":authority".into(), authority.into()));
        for (k, v) in headers {
            h2_headers.push((k.to_lowercase(), v.clone()));
        }

        let mut hpack_buf = BytesMut::with_capacity(256);
        hpack::encode_headers(&h2_headers, &mut hpack_buf)?;
        self.writer
            .write_headers(stream_id, &hpack_buf, end_stream, true)?;
        Ok(stream_id)
    }

    fn write_data(&mut self, stream_id: u32, data: &[u8], end_stream: bool) -> io::Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            let chunk_end = (offset + self.max_frame_size).min(data.len());
            let is_last = chunk_end == data.len();
            let es = end_stream && is_last;
            self.writer
                .write_data(stream_id, &data[offset..chunk_end], es)?;
            offset = chunk_end;
        }
        if data.is_empty() && end_stream {
            self.writer.write_data(stream_id, &[], true)?;
        }
        Ok(())
    }

    fn complete_headers(&mut self, stream_id: u32, end_stream: bool) {
        let header_bytes = {
            let stream = match self.streams.get_mut(&stream_id) {
                Some(s) => s,
                None => return,
            };
            stream.header_buf.split().freeze()
        };

        let mut collector = HeaderCollector::new();
        let mut cursor = header_bytes.as_ref();
        if let Err(e) = self.hpack_decoder.decode(&mut cursor, &mut collector) {
            if let Some(stream) = self.streams.remove(&stream_id) {
                let mut handler = stream.handler;
                handler.failed(&e);
            }
            return;
        }

        let stream = match self.streams.get_mut(&stream_id) {
            Some(s) => s,
            None => return,
        };

        stream.status_code = collector.status_code;
        let response = Response::new(stream.status_code);
        if response.is_success() {
            stream.handler.ok(&response);
        } else {
            stream.handler.error(&response);
        }
        for (name, value) in &collector.headers {
            stream.handler.header(name, value);
        }

        if end_stream {
            stream.handler.complete();
            self.streams.remove(&stream_id);
        } else {
            if let Some(s) = self.streams.get_mut(&stream_id) {
                s.handler.start_body();
                s.body_started = true;
            }
        }
    }

    fn fail_all(&mut self, error: &io::Error) {
        for (_, stream) in self.streams.drain() {
            let mut handler = stream.handler;
            handler.failed(error);
        }
    }
}

/// Collects HPACK-decoded headers, extracting :status.
struct HeaderCollector {
    status_code: u16,
    headers: Vec<(String, String)>,
}

impl HeaderCollector {
    fn new() -> Self {
        Self {
            status_code: 0,
            headers: Vec::new(),
        }
    }
}

impl hpack::HeaderHandler for HeaderCollector {
    fn header(&mut self, name: &str, value: &str) {
        if name == ":status" {
            self.status_code = value.parse().unwrap_or(0);
        } else if !name.starts_with(':') {
            self.headers.push((name.to_string(), value.to_string()));
        }
    }
}

impl H2FrameHandler for H2State {
    fn data_frame_received(&mut self, stream_id: u32, end_stream: bool, data: Bytes) {
        let len = data.len() as u32;
        if len > 0 {
            let _ = self.writer.write_window_update(0, len);
            let _ = self.writer.write_window_update(stream_id, len);
        }

        let should_remove = {
            let stream = match self.streams.get_mut(&stream_id) {
                Some(s) => s,
                None => return,
            };
            if !data.is_empty() {
                stream.handler.body_chunk(&data);
            }
            if end_stream {
                if stream.body_started {
                    stream.handler.end_body();
                }
                stream.handler.complete();
                true
            } else {
                false
            }
        };
        if should_remove {
            self.streams.remove(&stream_id);
        }
    }

    fn headers_frame_received(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        _stream_dependency: u32,
        _exclusive: bool,
        _weight: u8,
        header_block_fragment: Bytes,
    ) {
        let stream = match self.streams.get_mut(&stream_id) {
            Some(s) => s,
            None => return,
        };
        stream.header_buf.extend_from_slice(&header_block_fragment);
        stream.end_stream_on_headers = end_stream;

        if end_headers {
            let es = stream.end_stream_on_headers;
            self.complete_headers(stream_id, es);
        }
    }

    fn continuation_frame_received(
        &mut self,
        stream_id: u32,
        end_headers: bool,
        header_block_fragment: Bytes,
    ) {
        let stream = match self.streams.get_mut(&stream_id) {
            Some(s) => s,
            None => return,
        };
        stream.header_buf.extend_from_slice(&header_block_fragment);

        if end_headers {
            let es = stream.end_stream_on_headers;
            self.complete_headers(stream_id, es);
        }
    }

    fn settings_frame_received(&mut self, ack: bool, settings: Vec<(u16, u32)>) {
        if ack {
            return;
        }
        if !self.ready {
            self.ready = true;
            eprintln!("[http] HTTP/2 handshake complete, ready for requests");
        }
        for (id, value) in &settings {
            match *id {
                SETTINGS_MAX_FRAME_SIZE => {
                    self.max_frame_size = (*value as usize).clamp(
                        DEFAULT_MAX_FRAME_SIZE,
                        crate::http::h2::MAX_MAX_FRAME_SIZE,
                    );
                }
                SETTINGS_INITIAL_WINDOW_SIZE => {
                    self.send_window = *value as i32;
                }
                SETTINGS_HEADER_TABLE_SIZE => {
                    self.hpack_decoder.set_header_table_size(*value as usize);
                }
                _ => {}
            }
        }
        let _ = self.writer.write_settings_ack();
    }

    fn ping_frame_received(&mut self, ack: bool, opaque_data: u64) {
        if !ack {
            let _ = self.writer.write_ping(opaque_data, true);
        }
    }

    fn window_update_frame_received(&mut self, stream_id: u32, increment: u32) {
        if stream_id == 0 {
            self.send_window += increment as i32;
        }
        // Per-stream window tracking would go here for flow-controlled sends
    }

    fn goaway_frame_received(
        &mut self,
        _last_stream_id: u32,
        error_code: u32,
        _debug_data: Bytes,
    ) {
        self.goaway = true;
        if error_code != 0 {
            let msg = format!("GOAWAY: {}", error_to_string(error_code));
            let err = io::Error::new(io::ErrorKind::ConnectionReset, msg);
            self.fail_all(&err);
        }
    }

    fn rst_stream_frame_received(&mut self, stream_id: u32, error_code: u32) {
        if let Some(stream) = self.streams.remove(&stream_id) {
            let msg = format!("RST_STREAM: {}", error_to_string(error_code));
            let err = io::Error::new(io::ErrorKind::ConnectionReset, msg);
            let mut handler = stream.handler;
            handler.failed(&err);
        }
    }

    fn push_promise_frame_received(
        &mut self,
        _stream_id: u32,
        _promised_stream_id: u32,
        _end_headers: bool,
        _header_block_fragment: Bytes,
    ) {
        // We don't use server push; ignore.
    }

    fn priority_frame_received(
        &mut self,
        _stream_id: u32,
        _stream_dependency: u32,
        _exclusive: bool,
        _weight: u8,
    ) {
        // Advisory only; ignore.
    }

    fn frame_error(&mut self, _error_code: u32, _stream_id: u32, message: String) {
        let err = io::Error::new(io::ErrorKind::InvalidData, message);
        self.fail_all(&err);
    }
}

async fn h2_loop<R, W>(
    host: &str,
    port: u16,
    secure: bool,
    mut reader: R,
    mut writer: W,
    cmd_rx: &mut mpsc::UnboundedReceiver<Command>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Send connection preface + empty SETTINGS
    writer.write_all(CONNECTION_PREFACE).await?;
    let mut h2_state = H2State::new();
    let _ = h2_state.writer.write_settings(&[]);
    flush_h2_writes(&mut h2_state, &mut writer).await?;

    let mut read_buf = BytesMut::with_capacity(16384);
    let mut h2_parser = H2Parser::new();
    let mut channel_open = true;

    let authority = if (secure && port != 443) || (!secure && port != 80) {
        format!("{}:{}", host, port)
    } else {
        host.to_string()
    };

    loop {
        if h2_state.goaway {
            return Ok(());
        }

        tokio::select! {
            cmd = cmd_rx.recv(), if channel_open && h2_state.ready => {
                match cmd {
                    Some(Command::SendRequest { method, path, headers, handler, has_body }) => {
                        let stream_id = match h2_state.encode_request(
                            &method, &path, &authority, &headers, !has_body,
                        ) {
                            Ok(id) => id,
                            Err(e) => {
                                let mut h = handler;
                                h.failed(&e);
                                continue;
                            }
                        };
                        h2_state.streams.insert(stream_id, H2StreamState {
                            handler,
                            status_code: 0,
                            body_started: false,
                            header_buf: BytesMut::new(),
                            end_stream_on_headers: false,
                            body_written: 0,
                            body_total: 0,
                        });
                        if has_body {
                            h2_state.active_body_stream = Some(stream_id);
                        }
                        flush_h2_writes(&mut h2_state, &mut writer).await?;
                    }
                    Some(Command::BodyChunk { data }) => {
                        if let Some(stream_id) = h2_state.active_body_stream {
                            if let Err(e) = h2_state.write_data(stream_id, &data, false) {
                                if let Some(stream) = h2_state.streams.get_mut(&stream_id) {
                                    stream.handler.failed(&e);
                                }
                                h2_state.streams.remove(&stream_id);
                                h2_state.active_body_stream = None;
                                continue;
                            }
                            flush_h2_writes(&mut h2_state, &mut writer).await?;

                            if let Some(stream) = h2_state.streams.get_mut(&stream_id) {
                                stream.body_written += data.len() as u64;
                                stream.handler.request_body_written(
                                    stream.body_written,
                                    stream.body_total,
                                );
                            }
                        }
                    }
                    Some(Command::EndBody) => {
                        if let Some(stream_id) = h2_state.active_body_stream.take() {
                            let _ = h2_state.write_data(stream_id, &[], true);
                            flush_h2_writes(&mut h2_state, &mut writer).await?;
                        }
                    }
                    Some(Command::Close) => return Ok(()),
                    None => {
                        channel_open = false;
                        if h2_state.streams.is_empty() {
                            return Ok(());
                        }
                    }
                }
            }
            result = reader.read_buf(&mut read_buf) => {
                match result {
                    Ok(0) => {
                        let err = io::Error::new(io::ErrorKind::ConnectionReset, "connection closed");
                        h2_state.fail_all(&err);
                        return Ok(());
                    }
                    Ok(_) => {
                        if let Err(e) = h2_parser.receive(&mut read_buf, &mut h2_state) {
                            h2_state.fail_all(&e);
                            return Err(e);
                        }
                        flush_h2_writes(&mut h2_state, &mut writer).await?;
                        if !channel_open && h2_state.streams.is_empty() {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        h2_state.fail_all(&e);
                        return Err(e);
                    }
                }
            }
        }
    }
}

async fn flush_h2_writes<W: AsyncWrite + Unpin>(
    h2_state: &mut H2State,
    writer: &mut W,
) -> io::Result<()> {
    let buf = h2_state.writer.take_buffer();
    if !buf.is_empty() {
        writer.write_all(&buf).await?;
    }
    writer.flush().await
}
