/*
 * decoder.rs
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

//! HPACK decoder (RFC 7541). Decodes header blocks into (name, value) pairs.
//! Supports indexed (static table), literal with/without indexing.

use bytes::Buf;
use std::collections::VecDeque;
use std::io;

use super::static_table::{STATIC_TABLE, STATIC_TABLE_SIZE};

#[derive(Debug, Clone)]
struct Header {
    name: String,
    value: String,
}

/// Callback for each decoded header.
pub trait HeaderHandler {
    fn header(&mut self, name: &str, value: &str);
}

/// HPACK decoder. Uses static table and optional dynamic table.
pub struct Decoder {
    header_table_size: usize,
    dynamic_table: VecDeque<Header>,
    max_size: usize,
}

impl Decoder {
    pub fn new(header_table_size: usize) -> Self {
        Self {
            header_table_size,
            dynamic_table: VecDeque::new(),
            max_size: header_table_size,
        }
    }

    pub fn set_header_table_size(&mut self, size: usize) {
        self.header_table_size = size;
    }

    /// Decode a header block. Calls handler for each header.
    pub fn decode<B: Buf, H: HeaderHandler>(
        &mut self,
        buf: &mut B,
        handler: &mut H,
    ) -> io::Result<()> {
        while buf.has_remaining() {
            let b = buf.get_u8();
            if (b & 0x80) != 0 {
                let index = decode_integer(buf, b, 7)?;
                if index == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "HPACK indexed header index 0",
                    ));
                }
                let (name, value) = self.get_indexed(index)?;
                handler.header(&name, &value);
            } else if (b & 0x40) != 0 {
                let (name, value) = self.get_literal(buf, b, 6)?;
                self.add_to_dynamic(name.clone(), value.clone());
                handler.header(&name, &value);
            } else if (b & 0x20) != 0 {
                let max_size = decode_integer(buf, b, 5)? as usize;
                if max_size > self.header_table_size {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "HPACK dynamic table size exceeds SETTINGS",
                    ));
                }
                self.evict_to(max_size);
                self.max_size = max_size;
            } else {
                let (name, value) = self.get_literal(buf, b, 4)?;
                handler.header(&name, &value);
            }
        }
        Ok(())
    }

    fn get_indexed(&self, index: u64) -> io::Result<(String, String)> {
        if index < STATIC_TABLE_SIZE as u64 {
            let (name, value) = STATIC_TABLE[index as usize];
            Ok((name.to_string(), value.unwrap_or("").to_string()))
        } else {
            let dyn_index = index - STATIC_TABLE_SIZE as u64;
            let idx = dyn_index as usize;
            if idx < self.dynamic_table.len() {
                let h = &self.dynamic_table[idx];
                Ok((h.name.clone(), h.value.clone()))
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HPACK index out of range",
                ))
            }
        }
    }

    fn get_literal<B: Buf>(
        &self,
        buf: &mut B,
        opcode: u8,
        nbits: u8,
    ) -> io::Result<(String, String)> {
        let index = decode_integer(buf, opcode, nbits)?;
        let name = if index == 0 {
            decode_string(buf)?
        } else {
            let (n, _) = self.get_indexed(index)?;
            n
        };
        let value = decode_string(buf)?;
        Ok((name, value))
    }

    fn add_to_dynamic(&mut self, name: String, value: String) {
        let entry_size = name.len() + value.len() + 32;
        while self.dynamic_size() + entry_size > self.max_size
            && !self.dynamic_table.is_empty()
        {
            self.dynamic_table.pop_back();
        }
        if self.dynamic_size() + entry_size <= self.max_size {
            self.dynamic_table.push_front(Header { name, value });
        }
    }

    fn dynamic_size(&self) -> usize {
        self.dynamic_table
            .iter()
            .map(|h| h.name.len() + h.value.len() + 32)
            .sum()
    }

    fn evict_to(&mut self, max: usize) {
        while self.dynamic_size() > max && !self.dynamic_table.is_empty() {
            self.dynamic_table.pop_back();
        }
    }
}

fn decode_integer<B: Buf>(buf: &mut B, opcode: u8, nbits: u8) -> io::Result<u64> {
    let nmask = (1u64 << nbits) - 1;
    let mut value = (opcode & (nmask as u8)) as u64;
    if value < nmask {
        return Ok(value);
    }
    let mut shift = 0u32;
    loop {
        if !buf.has_remaining() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "HPACK integer overflow",
            ));
        }
        let b = buf.get_u8();
        value += ((b & 0x7f) as u64) << shift;
        if (b & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HPACK integer too large",
            ));
        }
    }
    Ok(value)
}

fn decode_string<B: Buf>(buf: &mut B) -> io::Result<String> {
    if !buf.has_remaining() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "HPACK string length",
        ));
    }
    let b = buf.get_u8();
    let huffman = (b & 0x80) != 0;
    let len = decode_integer(buf, b, 7)? as usize;
    if buf.remaining() < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "HPACK string truncated",
        ));
    }
    let mut bytes = vec![0u8; len];
    buf.copy_to_slice(&mut bytes);
    if huffman {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "HPACK Huffman decoding not yet implemented",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HPACK string not UTF-8"))
}
