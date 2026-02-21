/*
 * static_table.rs
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

//! HPACK static table (RFC 7541 Appendix A).

/// (name, value); value is None for header names that have no default value.
pub const STATIC_TABLE: &[(&str, Option<&str>)] = &[
    ("", None), // index 0 unused
    (":authority", None),
    (":method", Some("GET")),
    (":method", Some("POST")),
    (":path", Some("/")),
    (":path", Some("/index.html")),
    (":scheme", Some("http")),
    (":scheme", Some("https")),
    (":status", Some("200")),
    (":status", Some("204")),
    (":status", Some("206")),
    (":status", Some("304")),
    (":status", Some("400")),
    (":status", Some("404")),
    (":status", Some("500")),
    ("accept-charset", None),
    ("accept-encoding", Some("gzip, deflate")),
    ("accept-language", None),
    ("accept-ranges", None),
    ("accept", None),
    ("access-control-allow-origin", None),
    ("age", None),
    ("allow", None),
    ("authorization", None),
    ("cache-control", None),
    ("content-disposition", None),
    ("content-encoding", None),
    ("content-language", None),
    ("content-length", None),
    ("content-location", None),
    ("content-range", None),
    ("content-type", None),
    ("cookie", None),
    ("date", None),
    ("etag", None),
    ("expect", None),
    ("expires", None),
    ("from", None),
    ("host", None),
    ("if-match", None),
    ("if-modified-since", None),
    ("if-none-match", None),
    ("if-range", None),
    ("if-unmodified-since", None),
    ("last-modified", None),
    ("link", None),
    ("location", None),
    ("max-forwards", None),
    ("proxy-authenticate", None),
    ("proxy-authorization", None),
    ("range", None),
    ("referer", None),
    ("refresh", None),
    ("retry-after", None),
    ("server", None),
    ("set-cookie", None),
    ("strict-transport-security", None),
    ("transfer-encoding", None),
    ("user-agent", None),
    ("vary", None),
    ("via", None),
    ("www-authenticate", None),
];

pub const STATIC_TABLE_SIZE: usize = STATIC_TABLE.len();
