/*
 * tor.rs
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

//! Manages a per-user Tor subprocess that every outbound connection (relay
//! WebSocket, media/Blossom/NIP-96 HTTP, NIP-05/LNURL lookups, and the
//! webview's own media loads) is routed through.
//!
//! Plume never connects anywhere directly: it spawns a system-installed
//! `tor` binary itself (never a system-wide daemon, never bundled), waits
//! for it to report both an open SOCKS listener and a completed bootstrap,
//! and only then allows any network call to proceed. If Tor isn't
//! installed, fails to start, or exits unexpectedly, every call into
//! [`socks_addr`] fails -- there is no direct-connection fallback.

use std::io::{self, BufRead, BufReader};
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tauri::Emitter;

use crate::{debug_log, warn_log};

/// How long to wait for Tor to both open its SOCKS listener and finish
/// bootstrapping before giving up and failing closed.
const BOOTSTRAP_TIMEOUT: Duration = Duration::from_secs(90);

/// Current state of the managed Tor process.
#[derive(Clone, Debug)]
pub enum TorStatus {
    /// Spawned, waiting for the SOCKS listener and/or bootstrap to complete.
    Starting,
    /// Bootstrapped and accepting connections at this local SOCKS5 address.
    Ready(SocketAddr),
    /// Not usable -- every dial through Tor must fail, never fall back.
    Failed(String),
}

static TOR_STATUS: OnceLock<Arc<Mutex<TorStatus>>> = OnceLock::new();
static TOR_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();

fn status_cell() -> &'static Arc<Mutex<TorStatus>> {
    TOR_STATUS.get_or_init(|| Arc::new(Mutex::new(TorStatus::Starting)))
}

/// Current Tor status snapshot.
pub fn status() -> TorStatus {
    status_cell().lock().unwrap().clone()
}

/// The local SOCKS5 proxy address to dial everything through, once Tor has
/// finished bootstrapping. An error here means "do not connect at all" --
/// every caller must treat it as fail-closed, never as a signal to fall
/// back to a direct connection.
pub fn socks_addr() -> io::Result<SocketAddr> {
    match status() {
        TorStatus::Ready(addr) => Ok(addr),
        TorStatus::Starting => {
            Err(io::Error::new(io::ErrorKind::NotConnected, "Tor is still starting"))
        }
        TorStatus::Failed(reason) => Err(io::Error::new(
            io::ErrorKind::NotConnected,
            format!("Tor is not available: {reason}"),
        )),
    }
}

fn set_status(status: TorStatus, app: &tauri::AppHandle) {
    *status_cell().lock().unwrap() = status.clone();
    let payload = match &status {
        TorStatus::Starting => r#"{"state":"starting"}"#.to_string(),
        TorStatus::Ready(addr) => format!(r#"{{"state":"ready","socks_addr":"{addr}"}}"#),
        TorStatus::Failed(reason) => {
            format!(r#"{{"state":"failed","reason":"{}"}}"#, reason.replace('"', "\\\""))
        }
    };
    if let TorStatus::Failed(reason) = &status {
        warn_log!("[tor] failed: {}", reason);
    }
    let _ = app.emit("tor-status", payload);
}

/// Spawn `tor` as a child process (per-user, owned by this app, never a
/// system-wide daemon) and watch its stdout for the SOCKS listener address
/// and bootstrap progress. Non-blocking -- returns immediately; the result
/// is observed via [`status`] or the `tor-status` event.
pub fn start(app: tauri::AppHandle, base_dir: &str) {
    let data_dir = Path::new(base_dir).join("tor-data");
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        set_status(
            TorStatus::Failed(format!("could not create Tor data directory: {e}")),
            &app,
        );
        return;
    }

    // A previous run that was force-killed (crash, `kill -9`, OS logout)
    // never got to run `shutdown()`, leaving its own Tor child orphaned.
    // Clean it up before spawning a fresh one so repeated crashes don't
    // accumulate stray `tor` processes.
    kill_stale_tor(&data_dir);

    let mut cmd = Command::new("tor");
    cmd.arg("--SocksPort")
        .arg("auto")
        .arg("--DataDirectory")
        .arg(&data_dir)
        .arg("--ClientOnly")
        .arg("1")
        .arg("--Log")
        .arg("notice stdout")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            set_status(
                TorStatus::Failed(
                    "tor is not installed (no `tor` binary found on PATH) -- install Tor \
                     and restart Plume"
                        .to_string(),
                ),
                &app,
            );
            return;
        }
        Err(e) => {
            set_status(TorStatus::Failed(format!("failed to start tor: {e}")), &app);
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            set_status(TorStatus::Failed("tor started with no stdout pipe".to_string()), &app);
            return;
        }
    };

    let _ = std::fs::write(pid_file_path(&data_dir), child.id().to_string());
    *TOR_CHILD.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(child);

    let app_for_reader = app.clone();
    std::thread::spawn(move || watch_bootstrap(stdout, app_for_reader));

    std::thread::spawn(move || {
        std::thread::sleep(BOOTSTRAP_TIMEOUT);
        if matches!(status(), TorStatus::Starting) {
            set_status(
                TorStatus::Failed("tor did not finish bootstrapping in time".to_string()),
                &app,
            );
            shutdown();
        }
    });
}

/// Reads `tor`'s stdout until both its SOCKS listener address and a 100%
/// bootstrap notice have been seen (in either order), then flips the status
/// to [`TorStatus::Ready`]. If the process exits first, flips to
/// [`TorStatus::Failed`] instead.
fn watch_bootstrap(stdout: impl std::io::Read, app: tauri::AppHandle) {
    let mut socks_addr: Option<SocketAddr> = None;
    let mut bootstrapped = false;

    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        debug_log!("[tor] {}", line);

        if socks_addr.is_none() {
            if let Some(addr) = parse_socks_listener_line(&line) {
                socks_addr = Some(addr);
            }
        }
        if line.contains("Bootstrapped 100%") {
            bootstrapped = true;
        }

        if let (Some(addr), true) = (socks_addr, bootstrapped) {
            warn_log!("[tor] bootstrapped, SOCKS proxy at {}", addr);
            set_status(TorStatus::Ready(addr), &app);
            return;
        }
    }

    // stdout closed (process exited) before both signals arrived.
    if matches!(status(), TorStatus::Starting) {
        set_status(TorStatus::Failed("tor process exited before bootstrapping".to_string()), &app);
    }
}

/// Parses Tor's own notice-log line for the address it actually bound --
/// required since we ask for `SocksPort auto`. Deliberately *not* the
/// earlier "Opening Socks listener on 127.0.0.1:0" line -- that's logged
/// before the OS assigns the real port, always showing `:0`. "Opened Socks
/// listener connection (ready) on 127.0.0.1:PORT" is logged once the
/// listener actually has its real, bound port.
fn parse_socks_listener_line(line: &str) -> Option<SocketAddr> {
    let marker = "Opened Socks listener connection (ready) on ";
    let idx = line.find(marker)?;
    let rest = line[idx + marker.len()..].trim();
    rest.parse().ok()
}

/// Kill the managed Tor process, if one is running. Best-effort -- called on
/// app shutdown.
pub fn shutdown() {
    if let Some(cell) = TOR_CHILD.get() {
        if let Some(mut child) = cell.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pid_file_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("plume-tor.pid")
}

/// Kill a Tor process left behind by a previous run that never reached
/// [`shutdown`] (crash, force-quit) -- see [`start`]. Best-effort and
/// Unix-only: the orphan is harmless beyond holding a port and some CPU/RAM,
/// never a security concern, so there's nothing to do on a platform where
/// this check isn't straightforward.
#[cfg(unix)]
fn kill_stale_tor(data_dir: &Path) {
    let pid_file = pid_file_path(data_dir);
    if let Ok(contents) = std::fs::read_to_string(&pid_file) {
        if let Ok(pid) = contents.trim().parse::<u32>() {
            let alive = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if alive {
                warn_log!("[tor] killing a stale tor process from a previous run (pid {})", pid);
                let _ = Command::new("kill").arg(pid.to_string()).output();
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let _ = std::fs::remove_file(&pid_file);
}

#[cfg(not(unix))]
fn kill_stale_tor(_data_dir: &Path) {}
