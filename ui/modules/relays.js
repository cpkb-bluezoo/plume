/*
 * modules/relays.js
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

import { state } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml } from './utils.js';
import { saveConfig } from './config.js';
import { publishWithProgress } from './publish.js';

// Update the relay list in the UI (with delete button per relay)
export function updateRelayList() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const relayList = document.getElementById('relay-list');
    if (!relayList) {
        return;
    }
    relayList.innerHTML = '';

    if (!state.config) {
        state.config = {};
    }
    if (!Array.isArray(state.config.relays)) {
        state.config.relays = [];
    }

    const deleteLabel = t('settings.relayDelete');
    const unknownTitle = t('relays.statusUnknown');
    state.config.relays.forEach((relay, index) => {
        const li = document.createElement('li');
        li.className = 'relay-item';
        li.dataset.index = String(index);
        const esc = escapeHtml(relay);
        li.innerHTML = `
            <span class="relay-url">${esc}</span>
            <div class="relay-status" id="relay-status-${index}" title="${escapeHtml(unknownTitle)}" aria-label="${escapeHtml(unknownTitle)}"></div>
            <button type="button" class="btn btn-small btn-ghost relay-delete-btn" data-index="${index}" aria-label="${escapeHtml(deleteLabel)}">×</button>
        `;
        relayList.appendChild(li);
    });
}

// Bind relay panel: add, delete, save (so they work after updateRelayList)
export function bindRelayPanelHandlers() {
    var list = document.getElementById('relay-list');
    var addInput = document.getElementById('relay-add-input');
    var addBtn = document.getElementById('relay-add-btn');
    var saveBtn = document.getElementById('settings-relays-save');
    if (!list) {
        return;
    }

    list.removeEventListener('click', handleRelayListClick);
    list.addEventListener('click', handleRelayListClick);

    if (addBtn) {
        addBtn.onclick = function() {
            var url = addInput && addInput.value ? addInput.value.trim() : '';
            if (!url) {
                return;
            }
            if (!url.startsWith('ws://') && !url.startsWith('wss://')) {
                url = 'wss://' + url;
            }
            if (!state.config) {
                state.config = {};
            }
            if (!Array.isArray(state.config.relays)) {
                state.config.relays = [];
            }
            if (state.config.relays.indexOf(url) !== -1) {
                return;
            }
            state.config.relays.push(url);
            updateRelayList();
            bindRelayPanelHandlers();
            runRelayTests();
            if (addInput) {
                addInput.value = '';
            }
        };
    }
    if (addInput && addInput.addEventListener) {
        addInput.onkeydown = function(e) {
            if (e.key === 'Enter') {
                e.preventDefault();
                if (addBtn) {
                    addBtn.click();
                }
            }
        };
    }
    if (saveBtn) {
        saveBtn.onclick = function() {
            var relaysToPublish = (state.config.relays || []).slice();
            publishWithProgress('relays', saveBtn, function() {
                return invoke('start_save_relay_list', { relays: relaysToPublish });
            }).then(function(result) {
                // Only adopt (and persist) what the network actually
                // confirmed, same rule as everywhere else -- if nothing
                // confirmed, the edited list stays on screen, unsaved,
                // rather than being silently discarded or trusted anyway.
                if (result.anyConfirmed && Array.isArray(result.adopted)) {
                    state.config.relays = result.adopted;
                    updateRelayList();
                    bindRelayPanelHandlers();
                    runRelayTests();
                    return saveConfig();
                }
            }).catch(function(err) { console.error('Failed to save relays:', err); });
        };
    }
}

export function handleRelayListClick(e) {
    var target = e.target;
    if (target.classList && target.classList.contains('relay-delete-btn')) {
        var idx = parseInt(target.getAttribute('data-index'), 10);
        if (!state.config || !Array.isArray(state.config.relays) || isNaN(idx) || idx < 0 || idx >= state.config.relays.length) {
            return;
        }
        state.config.relays.splice(idx, 1);
        updateRelayList();
        bindRelayPanelHandlers();
        runRelayTests();
        return;
    }
}

// Tests connectivity for `relayUrls`, updating the status dot at
// `#{idPrefix}{index}` for each one (grey=unknown, green=connected,
// red=failed). Relays currently in the connection backoff list are shown
// red immediately, with retry time, rather than waiting on a doomed
// connection attempt. Shared by both Settings relay panels and the
// read-only Relays tab on the profile page, so "connected" means the same
// thing and looks the same everywhere it's shown.
export function testRelayConnections(relayUrls, idPrefix) {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var connectedTitle = t('relays.statusConnected');
    var failedTitle = t('relays.statusFailed');
    if (!relayUrls || !relayUrls.length) {
        return;
    }

    function runTest(relayUrl, index) {
        var el = document.getElementById(idPrefix + index);
        if (!el) {
            return;
        }
        el.classList.remove('connected', 'failed');
        el.title = t('relays.statusUnknown');
        el.setAttribute('aria-label', t('relays.statusUnknown'));
        invoke('test_relay_connection', { relayUrl: relayUrl })
            .then(function() {
                if (!el.parentNode) {
                    return;
                }
                el.classList.remove('failed');
                el.classList.add('connected');
                el.title = connectedTitle;
                el.setAttribute('aria-label', connectedTitle);
            })
            .catch(function() {
                if (!el.parentNode) {
                    return;
                }
                el.classList.remove('connected');
                el.classList.add('failed');
                el.title = failedTitle;
                el.setAttribute('aria-label', failedTitle);
            });
    }

    // First, check which relays are in the backoff list and mark them red immediately
    invoke('get_relay_backoff_status', { relay_urls: relayUrls })
        .then(function(json) {
            var backoff = {};
            try {
                backoff = JSON.parse(json);
            }
            catch (e) {}

            relayUrls.forEach(function(relayUrl, index) {
                var el = document.getElementById(idPrefix + index);
                if (!el) {
                    return;
                }
                var remaining = backoff[relayUrl];
                if (typeof remaining === 'number' && remaining > 0) {
                    // Relay is in backoff — mark red immediately with retry info
                    el.classList.remove('connected');
                    el.classList.add('failed');
                    var backoffTitle = (failedTitle || 'Failed') + ' (retry in ' + remaining + 's)';
                    el.title = backoffTitle;
                    el.setAttribute('aria-label', backoffTitle);
                }
                else {
                    runTest(relayUrl, index);
                }
            });
        })
        .catch(function() {
            // If backoff check fails, fall back to testing all relays
            relayUrls.forEach(runTest);
        });
}

// Test all of state.config.relays when the Settings relay panel is visible.
export function runRelayTests() {
    if (!state.config || !Array.isArray(state.config.relays)) {
        return;
    }
    testRelayConnections(state.config.relays.slice(), 'relay-status-');
}

// ============================================================
// DM Relay List (NIP-17 trusted message relays tab)
// ============================================================

export function updateDMRelayList() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var relayList = document.getElementById('dm-relay-list');
    if (!relayList) return;
    relayList.innerHTML = '';

    if (!state.config) state.config = {};
    if (!Array.isArray(state.config.dm_relays)) state.config.dm_relays = [];

    var deleteLabel = t('settings.relayDelete');
    var unknownTitle = t('relays.statusUnknown');
    state.config.dm_relays.forEach(function(relay, index) {
        var li = document.createElement('li');
        li.className = 'relay-item';
        li.dataset.index = String(index);
        var esc = escapeHtml(relay);
        li.innerHTML =
            '<span class="relay-url">' + esc + '</span>' +
            '<div class="relay-status" id="dm-relay-status-' + index + '" title="' + escapeHtml(unknownTitle) + '" aria-label="' + escapeHtml(unknownTitle) + '"></div>' +
            '<button type="button" class="btn btn-small btn-ghost relay-delete-btn" data-dm-index="' + index + '" aria-label="' + escapeHtml(deleteLabel) + '">×</button>';
        relayList.appendChild(li);
    });
}

export function bindDMRelayPanelHandlers() {
    var list = document.getElementById('dm-relay-list');
    var addInput = document.getElementById('dm-relay-add-input');
    var addBtn = document.getElementById('dm-relay-add-btn');
    var saveBtn = document.getElementById('settings-dm-relays-save');
    if (!list) return;

    list.removeEventListener('click', handleDMRelayListClick);
    list.addEventListener('click', handleDMRelayListClick);

    if (addBtn) {
        addBtn.onclick = function() {
            var url = addInput && addInput.value ? addInput.value.trim() : '';
            if (!url) return;
            if (!url.startsWith('ws://') && !url.startsWith('wss://')) {
                url = 'wss://' + url;
            }
            if (!state.config) state.config = {};
            if (!Array.isArray(state.config.dm_relays)) state.config.dm_relays = [];
            if (state.config.dm_relays.indexOf(url) !== -1) return;
            state.config.dm_relays.push(url);
            updateDMRelayList();
            bindDMRelayPanelHandlers();
            runDMRelayTests();
            if (addInput) addInput.value = '';
        };
    }
    if (addInput) {
        addInput.onkeydown = function(e) {
            if (e.key === 'Enter') {
                e.preventDefault();
                if (addBtn) addBtn.click();
            }
        };
    }
    if (saveBtn) {
        saveBtn.onclick = function() {
            var dmRelaysToPublish = (state.config.dm_relays || []).slice();
            publishWithProgress('dm_relays', saveBtn, function() {
                return invoke('start_save_dm_relay_list', { dm_relays: dmRelaysToPublish });
            }).then(function(result) {
                if (result.anyConfirmed && Array.isArray(result.adopted)) {
                    state.config.dm_relays = result.adopted;
                    updateDMRelayList();
                    bindDMRelayPanelHandlers();
                    runDMRelayTests();
                    return saveConfig();
                }
            }).catch(function(err) { console.error('Failed to save DM relays:', err); });
        };
    }
}

function handleDMRelayListClick(e) {
    var target = e.target;
    if (target.classList && target.classList.contains('relay-delete-btn')) {
        var idx = parseInt(target.getAttribute('data-dm-index'), 10);
        if (!state.config || !Array.isArray(state.config.dm_relays) || isNaN(idx) || idx < 0 || idx >= state.config.dm_relays.length) return;
        state.config.dm_relays.splice(idx, 1);
        updateDMRelayList();
        bindDMRelayPanelHandlers();
        runDMRelayTests();
    }
}

export function runDMRelayTests() {
    if (!state.config || !Array.isArray(state.config.dm_relays)) {
        return;
    }
    testRelayConnections(state.config.dm_relays.slice(), 'dm-relay-status-');
}

// Tab switching for relay settings
export function initRelayTabs() {
    var container = document.querySelector('.relay-tabs');
    if (!container) return;
    container.addEventListener('click', function(e) {
        var btn = e.target.closest('.relay-tab');
        if (!btn) return;
        var tab = btn.dataset.relayTab;
        container.querySelectorAll('.relay-tab').forEach(function(b) {
            b.classList.toggle('active', b === btn);
        });
        var mainPanel = document.getElementById('relay-tab-main');
        var dmPanel = document.getElementById('relay-tab-dm');
        if (mainPanel) mainPanel.style.display = (tab === 'main') ? 'block' : 'none';
        if (dmPanel) dmPanel.style.display = (tab === 'dm') ? 'block' : 'none';
        if (tab === 'dm') {
            updateDMRelayList();
            bindDMRelayPanelHandlers();
            runDMRelayTests();
        }
    });
}
