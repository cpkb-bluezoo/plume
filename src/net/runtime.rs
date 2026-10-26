/*
 * runtime.rs
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

//! Process-wide hopf reactor [`Runtime`]. Started once at app startup;
//! every dial (relay WebSocket, HTTP, webview media proxy) registers on
//! this single instance.
//!
//! `hopf_core::Runtime::shutdown` consumes `self` and has no `Drop` impl --
//! but since this `Runtime` is reachable only through a process-wide
//! `OnceLock<Arc<Runtime>>` (handed out to many concurrent dial call
//! sites), there is no point at which we could reliably reclaim sole
//! ownership to call it. We don't try: Rust threads are not joined on
//! process exit the way some other runtimes join theirs -- when `main`
//! returns (or `std::process::exit` runs, which is how Tauri's event loop
//! ends on quit), every thread is terminated immediately regardless of
//! what it's doing, so the reactor threads never outlive the process. The
//! Tor child process is a separate OS process that genuinely would outlive
//! us otherwise, which is why *that* gets an explicit kill (`crate::tor::shutdown`).

use std::sync::{Arc, OnceLock};

use hopf::core::{Runtime, RuntimeConfig};

static RUNTIME: OnceLock<Arc<Runtime>> = OnceLock::new();

/// The process-wide reactor runtime, starting it on first use.
pub fn get() -> Arc<Runtime> {
    RUNTIME
        .get_or_init(|| Arc::new(Runtime::start(RuntimeConfig::default()).expect("failed to start hopf runtime")))
        .clone()
}
