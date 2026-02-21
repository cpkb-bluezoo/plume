/*
 * encoder.rs
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

//! HPACK encoder (RFC 7541). Encodes headers for HTTP/2 request.
//! Literal without indexing (no Huffman for now).

use bytes::BufMut;
use std::io;

/// Encode a list of (name, value) headers into a buffer. Uses literal
/// without indexing (0x0 prefix).
pub fn encode_headers(
    headers: &[(String, String)],
    out: &mut impl BufMut,
) -> io::Result<()> {
    for (name, value) in headers {
        encode_literal_without_indexing(name.as_bytes(), value.as_bytes(), out)?;
    }
    Ok(())
}

fn encode_literal_without_indexing(
    name: &[u8],
    value: &[u8],
    out: &mut impl BufMut,
) -> io::Result<()> {
    out.put_u8(0x10);
    encode_string(name, out)?;
    encode_string(value, out)?;
    Ok(())
}

fn encode_string(s: &[u8], out: &mut impl BufMut) -> io::Result<()> {
    encode_integer(s.len() as u64, 7, 0, out);
    out.put_slice(s);
    Ok(())
}

fn encode_integer(mut value: u64, nbits: u8, prefix: u8, out: &mut impl BufMut) {
    let max_prefix = (1u64 << nbits) - 1;
    if value < max_prefix {
        out.put_u8(prefix | value as u8);
        return;
    }
    out.put_u8(prefix | max_prefix as u8);
    value -= max_prefix;
    while value >= 128 {
        out.put_u8(0x80 | (value % 128) as u8);
        value /= 128;
    }
    out.put_u8(value as u8);
}
