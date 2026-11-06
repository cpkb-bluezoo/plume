/*
 * modules/publish.js
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

// Drives the shared "publish and confirm" progress UI used identically by
// every replaceable-event save (profile metadata, main relay list, DM
// relay list): a bare relay OK only ever means "accepted," never "durably
// stored and retrievable," so the backend publishes, re-queries each relay
// for what it actually ended up with, and streams per-relay progress plus
// one final outcome -- see start_publish_and_confirm in main.rs. This is
// the one place that renders that progress and interprets the outcome, so
// all three saves behave consistently rather than each inventing their own
// feedback (or none, which is what prompted this).
//
// `context` must match the backend's own context string ("profile",
// "relays", "dm_relays"). `button` is the Save button to show feedback
// next to and disable while in flight. `startFn` kicks off the backend
// work (an invoke() call) and must resolve once the backend has *started*
// publishing, not once publishing is finished -- completion is signalled
// by the done event, not by this promise.
//
// Resolves once the backend's own done event for this context arrives,
// with { anyConfirmed, confirmedCount, total, adopted }. `adopted` is
// whatever the network actually confirmed (already parsed: a profile
// object, or an array of relay URLs), or null if nothing was confirmed --
// callers decide what, if anything, to write into local state/config from
// that, matching the same "network is truth" rule everywhere else.
export async function publishWithProgress(context, button, startFn) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const originalText = button ? button.textContent : null;
    const progressEl = ensureProgressContainer(button);
    const rows = {};

    if (button) {
        button.disabled = true;
        button.textContent = t('relays.publishing') || 'Publishing…';
    }
    if (progressEl) {
        progressEl.innerHTML = '';
        progressEl.style.display = 'block';
    }

    function addOrUpdateRow(relayUrl, label, statusClass) {
        if (!progressEl) {
            return;
        }
        var row = rows[relayUrl];
        if (!row) {
            row = document.createElement('div');
            row.className = 'publish-progress-row';
            row.innerHTML = '<span class="publish-progress-url"></span><span class="publish-progress-status"></span>';
            progressEl.appendChild(row);
            rows[relayUrl] = row;
        }
        row.querySelector('.publish-progress-url').textContent = relayUrl;
        var statusEl = row.querySelector('.publish-progress-status');
        statusEl.textContent = label;
        statusEl.className = 'publish-progress-status ' + statusClass;
    }

    function appendSummary(text, ok) {
        if (!progressEl) {
            return;
        }
        var el = document.createElement('div');
        el.className = 'publish-progress-summary ' + (ok ? 'success' : 'failed');
        el.textContent = text;
        progressEl.appendChild(el);
    }

    return new Promise(function(resolve) {
        var unlistenProgress = function() {};
        var unlistenDone = function() {};

        function finish(result) {
            unlistenProgress();
            unlistenDone();
            if (button) {
                button.disabled = false;
                button.textContent = originalText;
            }
            resolve(result);
        }

        if (!(window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function')) {
            finish({ anyConfirmed: false, confirmedCount: 0, total: 0, adopted: null });
            return;
        }

        Promise.all([
            window.__TAURI__.event.listen('publish-confirm-progress', function(ev) {
                var data = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
                if (!data || data.context !== context) {
                    return;
                }
                var label = data.confirmed ? (t('relays.confirmed') || 'Confirmed')
                    : data.accepted ? (t('relays.acceptedUnconfirmed') || 'Accepted (unconfirmed)')
                    : (t('relays.publishFailed') || 'Failed');
                addOrUpdateRow(data.relay_url, label, data.confirmed ? 'confirmed' : (data.accepted ? 'accepted' : 'failed'));
            }),
            window.__TAURI__.event.listen('publish-confirm-done', function(ev) {
                var data = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
                if (!data || data.context !== context) {
                    return;
                }
                if (data.any_confirmed) {
                    appendSummary(
                        (t('relays.publishSummaryOk') || 'Confirmed on {confirmed} of {total} relays')
                            .replace('{confirmed}', data.confirmed_count).replace('{total}', data.total),
                        true
                    );
                } else {
                    appendSummary(t('relays.publishSummaryFailed') || 'Could not confirm on any relay.', false);
                }
                finish({
                    anyConfirmed: !!data.any_confirmed,
                    confirmedCount: data.confirmed_count || 0,
                    total: data.total || 0,
                    adopted: data.adopted != null ? data.adopted : null
                });
            })
        ]).then(function(unlisteners) {
            unlistenProgress = unlisteners[0];
            unlistenDone = unlisteners[1];
            startFn().catch(function(err) {
                console.error('Failed to start publish for ' + context + ':', err);
                appendSummary((t('relays.publishStartFailed') || 'Failed to start: ') + (typeof err === 'string' ? err : (err && err.message) || err), false);
                finish({ anyConfirmed: false, confirmedCount: 0, total: 0, adopted: null });
            });
        });
    });
}

function ensureProgressContainer(button) {
    if (!button || !button.parentNode) {
        return null;
    }
    var id = button.id + '-progress';
    var el = document.getElementById(id);
    if (el) {
        return el;
    }
    // A Save button commonly sits in a flex "actions row" (buttons aligned
    // with justify-content: flex-end); inserting right after the button
    // there would make the progress panel another item in that same row
    // instead of a block below it. Insert after the row itself when there
    // is one, falling back to right after the button otherwise.
    var row = button.closest ? button.closest('.form-actions, .form-actions-end') : null;
    var anchor = row || button;
    el = document.createElement('div');
    el.id = id;
    el.className = 'publish-progress';
    anchor.parentNode.insertBefore(el, anchor.nextSibling);
    return el;
}
