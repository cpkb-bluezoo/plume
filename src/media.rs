/*
 * media.rs
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

//! Media upload module supporting Blossom (BUD-01/02/04) and NIP-96 protocols.
//!
//! Pure push pipeline: fire-and-forget HTTP requests, ResponseHandler feeds
//! body chunks into JsonParser, JsonContentHandler extracts results and
//! emits Tauri events.

use std::sync::RwLock;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bytes::BytesMut;
use tauri::Emitter;

use crate::crypto;
use crate::http::{HttpClient, Response, ResponseHandler};
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};
use crate::nostr;
use crate::warn_log;

/// Detected media server protocol.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaProtocol {
    Blossom,
    Nip96 { api_url: String },
}

/// Cached protocol discovery result.
static PROTOCOL_CACHE: RwLock<Option<(String, MediaProtocol)>> = RwLock::new(None);

/// Parameters needed for upload after protocol discovery.
struct UploadParams {
    app: tauri::AppHandle,
    server_url: String,
    file_path: String,
    file_name: String,
    content_type: String,
    secret_key: String,
    upload_id: String,
}

// ============================================================
// Protocol Discovery -> Upload Chain
// ============================================================

/// Start an upload: discovers protocol, then chains into the appropriate
/// upload handler. Fire-and-forget. All results delivered via Tauri events.
/// The file is read from disk by the backend -- the frontend only provides a path.
pub fn start_upload(
    app: tauri::AppHandle,
    server_url: &str,
    file_path: &str,
    file_name: &str,
    content_type: &str,
    secret_key: &str,
    upload_id: &str,
) {
    warn_log!(
        "[upload] start_upload server={} file={} type={} id={}",
        server_url, file_name, content_type, upload_id
    );

    // Check cache first
    let cached = {
        let cache = PROTOCOL_CACHE.read().ok();
        cache.and_then(|c| {
            c.as_ref()
                .filter(|(url, _)| url == server_url)
                .map(|(_, proto)| proto.clone())
        })
    };

    let params = UploadParams {
        app: app.clone(),
        server_url: server_url.to_string(),
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
        content_type: content_type.to_string(),
        secret_key: secret_key.to_string(),
        upload_id: upload_id.to_string(),
    };

    if let Some(ref protocol) = cached {
        warn_log!("[upload] using cached protocol: {:?}", protocol);
        do_upload(protocol.clone(), params);
        return;
    }

    let (host, port, path_prefix, secure) = match parse_url(server_url) {
        Some(v) => v,
        None => {
            emit_upload_failed(&app, upload_id, "Invalid server URL");
            return;
        }
    };

    warn_log!("[upload] discovering protocol via NIP-96 well-known for {}", server_url);
    let nip96_path = format!("{}/.well-known/nostr/nip96.json", path_prefix);
    let client = HttpClient::new(&host, port, secure);
    let handler = Box::new(DiscoveryHandler::new(client.clone(), params));
    client.get(&nip96_path).send(handler);
}

/// Start a delete. Fire-and-forget. No events needed for success.
pub fn start_delete(
    server_url: &str,
    file_hash: &str,
    secret_key: &str,
) {
    let cached = {
        let cache = PROTOCOL_CACHE.read().ok();
        cache.and_then(|c| {
            c.as_ref()
                .filter(|(url, _)| url == server_url)
                .map(|(_, proto)| proto.clone())
        })
    };

    let protocol = cached.unwrap_or(MediaProtocol::Blossom);
    do_delete(protocol, server_url, file_hash, secret_key);
}

fn do_upload(protocol: MediaProtocol, params: UploadParams) {
    match protocol {
        MediaProtocol::Blossom => blossom_upload(params),
        MediaProtocol::Nip96 { api_url } => nip96_upload(params, &api_url),
    }
}

fn do_delete(protocol: MediaProtocol, server_url: &str, file_hash: &str, secret_key: &str) {
    match protocol {
        MediaProtocol::Blossom => blossom_delete(server_url, file_hash, secret_key),
        MediaProtocol::Nip96 { api_url } => nip96_delete(&api_url, file_hash, secret_key),
    }
}

// ============================================================
// Discovery Handler (NIP-96 .well-known check)
// ============================================================

struct DiscoveryHandler {
    #[allow(dead_code)]
    client: HttpClient,
    params: Option<UploadParams>,
    json_parser: JsonParser,
    json_handler: Nip96ApiUrlHandler,
    success: bool,
}

impl DiscoveryHandler {
    fn new(client: HttpClient, params: UploadParams) -> Self {
        Self {
            client,
            params: Some(params),
            json_parser: JsonParser::new(),
            json_handler: Nip96ApiUrlHandler {
                current_key: None,
                api_url: None,
            },
            success: false,
        }
    }
}

impl ResponseHandler for DiscoveryHandler {
    fn ok(&mut self, response: &Response) {
        warn_log!("[upload] discovery: HTTP {}", response.code);
        self.success = true;
    }
    fn error(&mut self, response: &Response) {
        warn_log!("[upload] discovery: HTTP {} (error)", response.code);
        self.success = false;
    }
    fn header(&mut self, _name: &str, _value: &str) {}
    fn start_body(&mut self) {}

    fn body_chunk(&mut self, data: &[u8]) {
        if self.success {
            let mut buf = BytesMut::from(data);
            let _ = self.json_parser.receive(&mut buf, &mut self.json_handler);
        }
    }

    fn end_body(&mut self) {
        let _ = self.json_parser.close(&mut self.json_handler);
    }

    fn complete(&mut self) {
        let params = match self.params.take() {
            Some(p) => p,
            None => return,
        };

        let protocol = if self.success {
            if let Some(api_url) = self.json_handler.api_url.take() {
                let full_api = if api_url.starts_with("http") {
                    api_url
                } else {
                    let base = params.server_url.trim_end_matches('/');
                    format!("{}{}", base, api_url)
                };
                MediaProtocol::Nip96 { api_url: full_api }
            } else {
                MediaProtocol::Blossom
            }
        } else {
            MediaProtocol::Blossom
        };

        warn_log!("[upload] discovery complete -> {:?}", protocol);

        if let Ok(mut cache) = PROTOCOL_CACHE.write() {
            *cache = Some((params.server_url.clone(), protocol.clone()));
        }

        do_upload(protocol, params);
    }

    fn failed(&mut self, error: &std::io::Error) {
        warn_log!("[upload] discovery failed: {}, falling back to Blossom", error);
        if let Some(params) = self.params.take() {
            do_upload(MediaProtocol::Blossom, params);
        }
    }
}

struct Nip96ApiUrlHandler {
    current_key: Option<String>,
    api_url: Option<String>,
}

impl JsonContentHandler for Nip96ApiUrlHandler {
    fn start_object(&mut self) {}
    fn end_object(&mut self) {}
    fn start_array(&mut self) {}
    fn end_array(&mut self) {}
    fn key(&mut self, key: &str) {
        self.current_key = Some(key.to_string());
    }
    fn string_value(&mut self, value: &str) {
        if self.current_key.as_deref() == Some("api_url") {
            self.api_url = Some(value.to_string());
        }
    }
    fn number_value(&mut self, _n: JsonNumber) {}
    fn boolean_value(&mut self, _v: bool) {}
    fn null_value(&mut self) {}
}

// ============================================================
// Blossom Upload (BUD-02)
// ============================================================

fn blossom_upload(params: UploadParams) {
    warn_log!(
        "[upload] blossom_upload: server={} file={} ({} bytes path={})",
        params.server_url, params.file_name, 0, params.file_path
    );
    let file_data = match std::fs::read(&params.file_path) {
        Ok(data) => data,
        Err(e) => {
            emit_upload_failed(
                &params.app,
                &params.upload_id,
                &format!("Failed to read file: {}", e),
            );
            return;
        }
    };

    let file_hash = crypto::sha256_hex(&file_data);
    let auth_event = match crypto::create_blossom_auth_event(
        "upload",
        &file_hash,
        &params.secret_key,
    ) {
        Ok(e) => e,
        Err(e) => {
            emit_upload_failed(&params.app, &params.upload_id, &e);
            return;
        }
    };
    let auth_json = nostr::event_to_json(&auth_event);
    let auth_header = format!("Nostr {}", BASE64.encode(auth_json.as_bytes()));

    let (host, port, path_prefix, secure) = match parse_url(&params.server_url) {
        Some(v) => v,
        None => {
            emit_upload_failed(&params.app, &params.upload_id, "Invalid server URL");
            return;
        }
    };

    let total_bytes = file_data.len() as u64;
    warn_log!(
        "[upload] blossom: file read OK, {} bytes, hash={}, uploading to {}:{}{}",
        total_bytes, file_hash, host, port, path_prefix
    );
    let client = HttpClient::new(&host, port, secure);

    let upload_path = format!("{}/upload", path_prefix);
    let mut req = client.put(&upload_path);
    req.header("Content-Type", &params.content_type);
    req.header("Authorization", &auth_header);

    let handler = Box::new(UploadResponseHandler::new(
        params.app,
        params.upload_id,
        file_hash,
        total_bytes,
    ));

    let body = req.start_body(handler);
    const CHUNK_SIZE: usize = 64 * 1024;
    let mut offset = 0;
    while offset < file_data.len() {
        let end = (offset + CHUNK_SIZE).min(file_data.len());
        body.body_content(&file_data[offset..end]);
        offset = end;
    }
    body.end_body();
    warn_log!("[upload] blossom: all body chunks queued ({} bytes)", total_bytes);
}

// ============================================================
// NIP-96 Upload
// ============================================================

fn nip96_upload(params: UploadParams, api_url: &str) {
    warn_log!(
        "[upload] nip96_upload: api={} file={} path={}",
        api_url, params.file_name, params.file_path
    );
    let file_data = match std::fs::read(&params.file_path) {
        Ok(data) => data,
        Err(e) => {
            emit_upload_failed(
                &params.app,
                &params.upload_id,
                &format!("Failed to read file: {}", e),
            );
            return;
        }
    };

    let upload_url = format!("{}/upload", api_url.trim_end_matches('/'));
    let file_hash = crypto::sha256_hex(&file_data);

    let auth_event = match crypto::create_nip98_auth_event(
        &upload_url,
        "POST",
        None,
        &params.secret_key,
    ) {
        Ok(e) => e,
        Err(e) => {
            emit_upload_failed(&params.app, &params.upload_id, &e);
            return;
        }
    };
    let auth_json = nostr::event_to_json(&auth_event);
    let auth_header = format!("Nostr {}", BASE64.encode(auth_json.as_bytes()));

    let boundary = format!(
        "----PlumeUpload{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    // Build multipart body: preamble + file data + epilogue
    let preamble = format!(
        "--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
        boundary, params.file_name, params.content_type
    );
    let epilogue = format!(
        "\r\n--{}\r\nContent-Disposition: form-data; name=\"content_type\"\r\n\r\n{}\r\n--{}--\r\n",
        boundary, params.content_type, boundary
    );
    let total_bytes = preamble.len() as u64 + file_data.len() as u64 + epilogue.len() as u64;

    let multipart_ct = format!("multipart/form-data; boundary={}", boundary);

    let (host, port, _path_prefix, secure) = match parse_url(api_url) {
        Some(v) => v,
        None => {
            emit_upload_failed(&params.app, &params.upload_id, "Invalid API URL");
            return;
        }
    };

    let client = HttpClient::new(&host, port, secure);

    let upload_path = format!("{}/upload", api_url.trim_end_matches('/'));
    let parsed_path = url::Url::parse(&upload_path)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| "/upload".to_string());

    let mut req = client.post(&parsed_path);
    req.header("Content-Type", &multipart_ct);
    req.header("Authorization", &auth_header);

    let handler = Box::new(UploadResponseHandler::new(
        params.app,
        params.upload_id,
        file_hash,
        total_bytes,
    ));

    let body_handle = req.start_body(handler);

    // Stream: preamble, then file data in chunks, then epilogue
    body_handle.body_content(preamble.as_bytes());

    const CHUNK_SIZE: usize = 64 * 1024;
    let mut offset = 0;
    while offset < file_data.len() {
        let end = (offset + CHUNK_SIZE).min(file_data.len());
        body_handle.body_content(&file_data[offset..end]);
        offset = end;
    }

    body_handle.body_content(epilogue.as_bytes());
    body_handle.end_body();
}

// ============================================================
// Upload Response Handler (shared by Blossom and NIP-96)
// ============================================================

struct UploadResponseHandler {
    app: tauri::AppHandle,
    upload_id: String,
    file_hash: String,
    total_bytes: u64,
    json_parser: JsonParser,
    json_handler: UploadJsonHandler,
    success: bool,
    status_code: u16,
}

impl UploadResponseHandler {
    fn new(
        app: tauri::AppHandle,
        upload_id: String,
        file_hash: String,
        total_bytes: u64,
    ) -> Self {
        Self {
            app,
            upload_id,
            file_hash,
            total_bytes,
            json_parser: JsonParser::new(),
            json_handler: UploadJsonHandler::new(),
            success: false,
            status_code: 0,
        }
    }
}

impl ResponseHandler for UploadResponseHandler {
    fn ok(&mut self, response: &Response) {
        warn_log!("[upload] response: HTTP {} OK", response.code);
        self.success = true;
        self.status_code = response.code;
    }

    fn error(&mut self, response: &Response) {
        self.success = false;
        self.status_code = response.code;
        warn_log!("Upload HTTP error: {}", response.code);
    }

    fn header(&mut self, _name: &str, _value: &str) {}
    fn start_body(&mut self) {}

    fn body_chunk(&mut self, data: &[u8]) {
        if !self.success {
            if let Ok(text) = std::str::from_utf8(data) {
                self.json_handler.error_body.push_str(text);
            }
        }
        let mut buf = BytesMut::from(data);
        let _ = self.json_parser.receive(&mut buf, &mut self.json_handler);
    }

    fn end_body(&mut self) {
        let _ = self.json_parser.close(&mut self.json_handler);
        if !self.success && !self.json_handler.error_body.is_empty() {
            warn_log!(
                "Upload error response body: {}",
                self.json_handler.error_body
            );
        }
    }

    fn complete(&mut self) {
        if self.success {
            if let Some(url) = self.json_handler.url.take() {
                let payload = format!(
                    r#"{{"upload_id":"{}","url":"{}","file_hash":"{}","status":"complete"}}"#,
                    self.upload_id,
                    url.replace('"', "\\\""),
                    self.file_hash
                );
                let _ = self.app.emit("upload-complete", &payload);
            } else {
                warn_log!("Upload: server response missing URL");
                let payload = format!(
                    r#"{{"upload_id":"{}","error":"Server response missing URL","status":"failed"}}"#,
                    self.upload_id
                );
                let _ = self.app.emit("upload-failed", &payload);
            }
        } else {
            let server_msg = self.json_handler.extract_error_message();
            let error_detail = if let Some(msg) = server_msg {
                format!("HTTP {}: {}", self.status_code, msg)
            } else {
                format!("HTTP {}", self.status_code)
            };
            warn_log!("Upload failed: {}", error_detail);
            let payload = format!(
                r#"{{"upload_id":"{}","error":"Upload failed ({})","status":"failed"}}"#,
                self.upload_id,
                error_detail.replace('"', "\\\"")
            );
            let _ = self.app.emit("upload-failed", &payload);
        }
    }

    fn failed(&mut self, error: &std::io::Error) {
        warn_log!("Upload connection failed: {}", error);
        let payload = format!(
            r#"{{"upload_id":"{}","error":"{}","status":"failed"}}"#,
            self.upload_id,
            error.to_string().replace('"', "\\\"")
        );
        let _ = self.app.emit("upload-failed", &payload);
    }

    fn request_body_written(&mut self, bytes_written: u64, _total_bytes: u64) {
        let payload = format!(
            r#"{{"upload_id":"{}","bytes_sent":{},"total_bytes":{},"status":"uploading"}}"#,
            self.upload_id, bytes_written, self.total_bytes
        );
        let _ = self.app.emit("upload-progress", &payload);
    }
}

/// Extracts "url" from upload response JSON.
/// Handles both simple `{"url":"..."}` and NIP-96 `nip94_event.tags[["url","..."]]`.
struct UploadJsonHandler {
    current_key: Option<String>,
    url: Option<String>,
    error_message: Option<String>,
    error_body: String,
    in_tags: bool,
    tag_depth: u32,
    tag_index: u32,
    current_tag_first: Option<String>,
}

impl UploadJsonHandler {
    fn new() -> Self {
        Self {
            current_key: None,
            url: None,
            error_message: None,
            error_body: String::new(),
            in_tags: false,
            tag_depth: 0,
            tag_index: 0,
            current_tag_first: None,
        }
    }

    fn extract_error_message(&self) -> Option<String> {
        self.error_message.clone()
    }
}

impl JsonContentHandler for UploadJsonHandler {
    fn start_object(&mut self) {}
    fn end_object(&mut self) {}
    fn start_array(&mut self) {
        if self.current_key.as_deref() == Some("tags") {
            self.in_tags = true;
            self.tag_depth = 0;
        } else if self.in_tags {
            self.tag_depth += 1;
            self.tag_index = 0;
            self.current_tag_first = None;
        }
    }
    fn end_array(&mut self) {
        if self.in_tags && self.tag_depth > 0 {
            self.tag_depth -= 1;
        } else {
            self.in_tags = false;
        }
    }
    fn key(&mut self, key: &str) {
        self.current_key = Some(key.to_string());
    }
    fn string_value(&mut self, value: &str) {
        if self.in_tags && self.tag_depth > 0 {
            if self.tag_index == 0 {
                self.current_tag_first = Some(value.to_string());
            } else if self.tag_index == 1
                && self.current_tag_first.as_deref() == Some("url")
                && self.url.is_none()
            {
                self.url = Some(value.to_string());
            }
            self.tag_index += 1;
        } else {
            match self.current_key.as_deref() {
                Some("url") if self.url.is_none() => {
                    self.url = Some(value.to_string());
                }
                Some("message") | Some("error") if self.error_message.is_none() => {
                    self.error_message = Some(value.to_string());
                }
                _ => {}
            }
        }
    }
    fn number_value(&mut self, _n: JsonNumber) {}
    fn boolean_value(&mut self, _v: bool) {}
    fn null_value(&mut self) {}
}

// ============================================================
// Blossom Delete (BUD-04)
// ============================================================

fn blossom_delete(server_url: &str, file_hash: &str, secret_key: &str) {
    let auth_event = match crypto::create_blossom_auth_event("delete", file_hash, secret_key)
    {
        Ok(e) => e,
        Err(_) => return,
    };
    let auth_json = nostr::event_to_json(&auth_event);
    let auth_header = format!("Nostr {}", BASE64.encode(auth_json.as_bytes()));

    let (host, port, path_prefix, secure) = match parse_url(server_url) {
        Some(v) => v,
        None => return,
    };

    let client = HttpClient::new(&host, port, secure);
    let delete_path = format!("{}/{}", path_prefix, file_hash);
    let mut req = client.delete(&delete_path);
    req.header("Authorization", &auth_header);
    req.send(Box::new(NoOpHandler));
}

// ============================================================
// NIP-96 Delete
// ============================================================

fn nip96_delete(api_url: &str, file_hash: &str, secret_key: &str) {
    let delete_url = format!("{}/{}", api_url.trim_end_matches('/'), file_hash);

    let auth_event = match crypto::create_nip98_auth_event(
        &delete_url,
        "DELETE",
        None,
        secret_key,
    ) {
        Ok(e) => e,
        Err(_) => return,
    };
    let auth_json = nostr::event_to_json(&auth_event);
    let auth_header = format!("Nostr {}", BASE64.encode(auth_json.as_bytes()));

    let (host, port, path_prefix, secure) = match parse_url(api_url) {
        Some(v) => v,
        None => return,
    };

    let client = HttpClient::new(&host, port, secure);
    let delete_path = format!("{}/{}", path_prefix, file_hash);
    let mut req = client.delete(&delete_path);
    req.header("Authorization", &auth_header);
    req.send(Box::new(NoOpHandler));
}

/// Handler that discards all response data (used for delete).
struct NoOpHandler;

impl ResponseHandler for NoOpHandler {
    fn ok(&mut self, _response: &Response) {}
    fn error(&mut self, _response: &Response) {}
    fn header(&mut self, _name: &str, _value: &str) {}
    fn start_body(&mut self) {}
    fn body_chunk(&mut self, _data: &[u8]) {}
    fn end_body(&mut self) {}
    fn complete(&mut self) {}
    fn failed(&mut self, _error: &std::io::Error) {}
}

// ============================================================
// Helpers
// ============================================================

fn emit_upload_failed(app: &tauri::AppHandle, upload_id: &str, error: &str) {
    warn_log!("Upload failed [{}]: {}", upload_id, error);
    let payload = format!(
        r#"{{"upload_id":"{}","error":"{}","status":"failed"}}"#,
        upload_id,
        error.replace('"', "\\\"")
    );
    let _ = app.emit("upload-failed", &payload);
}

/// Parse a URL into (host, port, path, secure).
fn parse_url(url: &str) -> Option<(String, u16, String, bool)> {
    let url_parsed = url::Url::parse(url).ok()?;
    let secure = url_parsed.scheme() == "https";
    let host = url_parsed.host_str()?.to_string();
    let port = url_parsed
        .port()
        .unwrap_or(if secure { 443 } else { 80 });
    let path = url_parsed.path().to_string();
    let path_prefix = path.trim_end_matches('/').to_string();
    Some((host, port, path_prefix, secure))
}
