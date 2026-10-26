/*
 * modules/tor-status.js
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

// Blocks the UI until Tor has finished bootstrapping. Plume fails closed:
// no relay or media request is even attempted while Tor isn't ready, so
// this overlay is the user-visible side of that guarantee -- it must be
// visible, not silent.

import { invoke } from './tauri.js';
import { escapeHtml } from './utils.js';

var overlay = null;

function ensureOverlay() {
    if (overlay) {
        return overlay;
    }
    overlay = document.createElement('div');
    overlay.id = 'tor-status-overlay';
    overlay.setAttribute('role', 'alertdialog');
    overlay.setAttribute('aria-live', 'assertive');
    overlay.style.cssText = 'position:fixed;inset:0;z-index:9999;display:flex;' +
        'align-items:center;justify-content:center;flex-direction:column;gap:12px;' +
        'background:rgba(10,10,12,0.96);color:#eee;font-family:system-ui,sans-serif;' +
        'text-align:center;padding:24px;';
    document.body.appendChild(overlay);
    return overlay;
}

function render(state) {
    var el = ensureOverlay();
    if (state.status === 'ready') {
        el.remove();
        overlay = null;
        return;
    }
    el.style.display = 'flex';
    if (state.status === 'failed') {
        el.innerHTML =
            '<div style="font-size:1.1em;font-weight:600;">Tor is not available</div>' +
            '<div style="max-width:420px;opacity:0.85;">' + escapeHtml(state.reason || 'Unknown error') + '</div>' +
            '<div style="max-width:420px;opacity:0.7;font-size:0.9em;">Plume never connects directly -- install Tor ' +
            '(e.g. <code>brew install tor</code> or your package manager\'s <code>tor</code> package) and restart Plume.</div>';
    } else {
        el.innerHTML =
            '<div style="font-size:1.1em;font-weight:600;">Connecting via Tor…</div>' +
            '<div style="opacity:0.7;">Plume routes every connection through Tor. This can take a moment on first launch.</div>';
    }
}

function parseStatusPayload(raw) {
    try {
        var data = typeof raw === 'string' ? JSON.parse(raw) : raw;
        if (data.state === 'ready') {
            return { status: 'ready' };
        }
        if (data.state === 'failed') {
            return { status: 'failed', reason: data.reason || '' };
        }
        return { status: 'starting' };
    } catch (_) {
        return { status: 'starting' };
    }
}

export async function initTorStatusGate() {
    render({ status: 'starting' });

    if (window.__TAURI__ && window.__TAURI__.event) {
        window.__TAURI__.event.listen('tor-status', function(ev) {
            render(parseStatusPayload(ev.payload));
        });
    }

    try {
        var current = await invoke('get_tor_status');
        if (current) {
            render(parseStatusPayload(current));
        }
    } catch (_) {
        // Fall back to whatever the event listener reports.
    }
}
