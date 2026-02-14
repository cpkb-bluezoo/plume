/*
 * number.rs
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

//! JSON number value (integer or float).

/// A JSON number: integer or floating-point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JsonNumber {
    I64(i64),
    F64(f64),
}

impl JsonNumber {
    #[allow(dead_code)]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            JsonNumber::I64(n) => Some(*n),
            JsonNumber::F64(f) => {
                if f.fract() == 0.0 && *f >= i64::MIN as f64 && *f <= i64::MAX as f64 {
                    Some(*f as i64)
                } else {
                    None
                }
            }
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            JsonNumber::I64(n) => *n as f64,
            JsonNumber::F64(f) => *f,
        }
    }
}
