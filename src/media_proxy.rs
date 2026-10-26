/*
 * media_proxy.rs
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

//! Backs the `torimg://` custom URI scheme: the webview never fetches a
//! remote image/avatar/banner/media URL directly (which would bypass Tor
//! entirely and leak the real IP plus the webview's own User-Agent to the
//! content host). Instead the frontend rewrites every such URL to
//! `torimg://localhost/<percent-encoded original URL>`, and this module
//! fetches it through the same Tor-routed `HttpClient` used everywhere else
//! in the backend, handing the bytes back to the webview.

use crate::http::{HttpClient, Response as PlumeResponse, ResponseHandler};

/// Handle one `torimg://localhost/<encoded-url>` request.
pub fn handle(request_path: &str, responder: tauri::UriSchemeResponder) {
    let encoded = request_path.trim_start_matches('/');
    let target = match urlencoding::decode(encoded) {
        Ok(s) => s.into_owned(),
        Err(_) => return respond_status(responder, 400),
    };

    let parsed = match url::Url::parse(&target) {
        Ok(u) => u,
        Err(_) => return respond_status(responder, 400),
    };
    let secure = match parsed.scheme() {
        "https" => true,
        "http" => false,
        _ => return respond_status(responder, 400),
    };
    let host = match parsed.host_str() {
        Some(h) => h.to_string(),
        None => return respond_status(responder, 400),
    };
    let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
    let path = match parsed.query() {
        Some(q) => format!("{}?{}", parsed.path(), q),
        None => parsed.path().to_string(),
    };

    let client = HttpClient::new(&host, port, secure);
    client.get(&path).send(Box::new(ProxyHandler {
        responder: Some(responder),
        status: 0,
        content_type: None,
        body: Vec::new(),
    }));
}

fn respond_status(responder: tauri::UriSchemeResponder, status: u16) {
    let response = tauri::http::Response::builder().status(status).body(Vec::new()).unwrap();
    responder.respond(response);
}

struct ProxyHandler {
    responder: Option<tauri::UriSchemeResponder>,
    status: u16,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl ProxyHandler {
    fn finish(&mut self) {
        let Some(responder) = self.responder.take() else { return };
        let mut builder = tauri::http::Response::builder().status(self.status);
        if let Some(ct) = &self.content_type {
            builder = builder.header(tauri::http::header::CONTENT_TYPE, ct.clone());
        }
        let response = builder.body(std::mem::take(&mut self.body)).unwrap();
        responder.respond(response);
    }
}

impl ResponseHandler for ProxyHandler {
    fn ok(&mut self, response: &PlumeResponse) {
        self.status = response.code;
    }

    fn error(&mut self, response: &PlumeResponse) {
        self.status = response.code;
    }

    fn header(&mut self, name: &str, value: &str) {
        if name.eq_ignore_ascii_case("content-type") {
            self.content_type = Some(value.to_string());
        }
    }

    fn start_body(&mut self) {}

    fn body_chunk(&mut self, data: &[u8]) {
        self.body.extend_from_slice(data);
    }

    fn end_body(&mut self) {}

    fn complete(&mut self) {
        self.finish();
    }

    fn failed(&mut self, _error: &std::io::Error) {
        if self.status == 0 {
            self.status = 502;
        }
        self.finish();
    }
}
