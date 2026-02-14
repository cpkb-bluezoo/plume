/*
 * modules/settings.js
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
import { escapeHtml, clearValidationErrors, showValidationError } from './utils.js';
import { saveConfig, setSavingState, updateFeedInitialState } from './config.js';
import { hexToNpub, validatePublicKey, validateSecretKey } from './keys.js';
import { updateRelayList, bindRelayPanelHandlers, runRelayTests } from './relays.js';
import { loadFollowsPanel } from './follows.js';
import { loadMutedPanel } from './muting.js';
import { fetchProfile } from './profile.js';
import { startInitialFeedFetch } from './feed.js';

let _updateUIFromConfig = null;
export function setUpdateUIFromConfig(fn) {
    _updateUIFromConfig = fn;
}

// Open settings modal (for Account/Keys – still used for keys)
export function openSettings() {
    clearValidationErrors();
    document.getElementById('settings-modal').classList.add('active');
}

// Close settings modal
export function closeSettings() {
    document.getElementById('settings-modal').classList.remove('active');
}

// Show a settings panel by key. key null = show default placeholder.
// Populate the settings profile edit form from the best available source:
// relay-fetched profile first, then local config as fallback.
export function populateProfilePanel() {
    var cfg = state.config || {};
    var profile = state.profile || {};
    var el;
    el = document.getElementById('edit-profile-name');
    if (el) {
        el.value = profile.name || cfg.name || '';
    }
    el = document.getElementById('edit-profile-nip05');
    if (el) {
        el.value = profile.nip05 || cfg.nip05 || '';
    }
    el = document.getElementById('edit-profile-website');
    if (el) {
        el.value = profile.website || cfg.website || '';
    }
    el = document.getElementById('edit-profile-about');
    if (el) {
        el.value = profile.about || cfg.about || '';
    }
    el = document.getElementById('edit-profile-lud16');
    if (el) {
        el.value = profile.lud16 || cfg.lud16 || '';
    }
    el = document.getElementById('edit-profile-picture');
    if (el) {
        el.value = profile.picture || cfg.picture || '';
    }
    el = document.getElementById('edit-profile-banner');
    if (el) {
        el.value = profile.banner || cfg.banner || '';
    }
}

export function showSettingsPanel(key) {
    var detail = document.getElementById('settings-detail');
    if (!detail) {
        return;
    }
    var panels = detail.querySelectorAll('.settings-panel');
    var defaultEl = document.getElementById('settings-detail-default');
    panels.forEach(function(panel) {
        if (panel.id === 'settings-detail-default') {
            panel.style.display = key ? 'none' : 'flex';
        } else {
            panel.style.display = (panel.id === 'settings-panel-' + key) ? 'block' : 'none';
        }
    });

    document.querySelectorAll('.settings-menu-item').forEach(function(btn) {
        btn.classList.toggle('active', btn.dataset.settings === key);
    });

    if (key === 'keys') {
        populateKeysPanel();
        return;
    }

    if (key === 'profile') {
        populateProfilePanel();
        // If profile hasn't been fetched from relays yet, fetch it now and repopulate
        if (!state.profile && state.config && state.config.public_key) {
            fetchProfile().then(function() { populateProfilePanel(); });
        }
    }
    if (key === 'home-feed') {
        var mode = (state.config && state.config.home_feed_mode === 'follows') ? 'follows' : 'firehose';
        var firehoseRadio = document.getElementById('home-feed-firehose');
        var followsRadio = document.getElementById('home-feed-follows');
        if (firehoseRadio) {
            firehoseRadio.checked = (mode === 'firehose');
        }
        if (followsRadio) {
            followsRadio.checked = (mode === 'follows');
        }
        var hideEncryptedCb = document.getElementById('home-feed-hide-encrypted');
        if (hideEncryptedCb) {
            hideEncryptedCb.checked = !state.config || state.config.hide_encrypted_notes !== false;
        }
    }
    if (key === 'media') {
        var urlEl = document.getElementById('settings-media-server-url');
        if (urlEl) {
            urlEl.value = (state.config && state.config.media_server_url) || 'https://blossom.primal.net';
        }
    }
    if (key === 'follows') {
        loadFollowsPanel();
    }
    if (key === 'muted') {
        loadMutedPanel();
    }
    if (key === 'relays') {
        updateRelayList();
        bindRelayPanelHandlers();
        runRelayTests();
    }
    if (key === 'zaps') {
        var amountEl = document.getElementById('settings-zaps-default-amount');
        if (amountEl) {
            amountEl.value = (state.config && state.config.default_zap_amount != null) ? state.config.default_zap_amount : 42;
        }
    }
}

// Populate Keys panel with npub (nsec is NEVER placed in the DOM)
export async function populateKeysPanel() {
    var npubEl = document.getElementById('settings-keys-npub');
    var nsecEl = document.getElementById('settings-keys-nsec');
    if (!npubEl || !nsecEl) {
        return;
    }
    npubEl.value = '';
    nsecEl.value = '';
    nsecEl.placeholder = state.config && state.config.private_key
        ? (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('accountModal.privateKeyConfigured') || 'Private key configured (hidden)' : 'Private key configured (hidden)')
        : (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('accountModal.privateKeyPlaceholder') || 'nsec1... or hex (optional)' : 'nsec1... or hex (optional)');
    var npubError = document.getElementById('settings-keys-npub-error');
    var nsecError = document.getElementById('settings-keys-nsec-error');
    if (npubError) {
        npubError.textContent = '';
    }
    if (nsecError) {
        nsecError.textContent = '';
    }
    // Show/hide copy nsec button based on whether key exists
    var copyNsecBtn = document.getElementById('settings-keys-copy-nsec');
    if (copyNsecBtn) {
        copyNsecBtn.style.display = (state.config && state.config.private_key) ? 'inline-block' : 'none';
    }
    if (!state.config) {
        return;
    }
    if (state.config.public_key) {
        try {
            var npub = await hexToNpub(state.config.public_key);
            npubEl.value = npub || state.config.public_key;
        } catch (e) {
            npubEl.value = state.config.public_key;
        }
    }
    // Private key is NOT written to the DOM. The input is write-only for entering a new key.
}

// Copy nsec to clipboard without ever placing it in the DOM
export async function copyNsecToClipboard() {
    if (!state.config || !state.config.private_key) {
        return;
    }
    try {
        var nsec = await invoke('convert_hex_to_nsec', { hex_key: state.config.private_key });
        if (nsec && navigator.clipboard) {
            await navigator.clipboard.writeText(nsec);
            var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            alert(t('accountModal.nsecCopied') || 'Private key (nsec) copied to clipboard.');
        }
    } catch (e) {
        console.error('Failed to copy nsec:', e);
        alert('Failed to copy private key.');
    }
}

// Save Keys panel: validate npub/nsec, store hex, save config
export async function saveKeysPanel(event) {
    if (event) {
        event.preventDefault();
    }
    var npubEl = document.getElementById('settings-keys-npub');
    var nsecEl = document.getElementById('settings-keys-nsec');
    var npubError = document.getElementById('settings-keys-npub-error');
    var nsecError = document.getElementById('settings-keys-nsec-error');
    if (!npubEl || !state.config) {
        return;
    }
    if (npubError) {
        npubError.textContent = '';
    }
    if (nsecError) {
        nsecError.textContent = '';
    }
    var publicKeyHex = null;
    var privateKeyHex = null;
    var npubRaw = (npubEl && npubEl.value) ? npubEl.value.trim() : '';
    if (!npubRaw) {
        if (npubError) {
            npubError.textContent = 'Public key is required';
        }
        return;
    }
    var pubResult = await validatePublicKey(npubRaw);
    if (!pubResult.valid) {
        if (npubError) {
            npubError.textContent = pubResult.error || 'Invalid public key';
        }
        return;
    }
    publicKeyHex = pubResult.hex;
    if (nsecEl && nsecEl.value.trim()) {
        var privResult = await validateSecretKey(nsecEl.value.trim());
        if (!privResult.valid) {
            if (nsecError) {
                nsecError.textContent = privResult.error || 'Invalid private key';
            }
            return;
        }
        privateKeyHex = privResult.hex;
    }
    var saveBtn = document.querySelector('#settings-keys-form button[type="submit"]');
    var restoreBtn = setSavingState(saveBtn);
    state.config.public_key = publicKeyHex;
    state.config.private_key = privateKeyHex || state.config.private_key || null;
    state.publicKeyHex = publicKeyHex;
    state.publicKeyNpub = pubResult.npub || null;
    try {
        await saveConfig();
        if (_updateUIFromConfig) {
            _updateUIFromConfig();
        }
    } finally {
        restoreBtn();
    }
}

// Save Home feed mode from settings panel
export function saveHomeFeedModeFromPanel() {
    var followsRadio = document.getElementById('home-feed-follows');
    var mode = (followsRadio && followsRadio.checked) ? 'follows' : 'firehose';
    if (!state.config) {
        state.config = {};
    }
    state.config.home_feed_mode = mode;
    state.homeFeedMode = mode;
    var hideEncryptedCb = document.getElementById('home-feed-hide-encrypted');
    state.config.hide_encrypted_notes = hideEncryptedCb ? hideEncryptedCb.checked : true;
    var restoreBtn = setSavingState(document.getElementById('home-feed-panel-save'));
    saveConfig().then(function() {
        // Clear existing feed state so the next visit to feed reloads with the new mode
        state.initialFeedLoadDone = false;
        state.notes = [];
        if (state.feedPollIntervalId) {
            clearInterval(state.feedPollIntervalId);
            state.feedPollIntervalId = null;
        }
        if (state.currentView === 'feed') {
            updateFeedInitialState();
            startInitialFeedFetch();
        }
        // If not on feed view, it will reload when the user navigates to feed
        // because state.initialFeedLoadDone is false.
    }).catch(function(err) { console.error('Failed to save home feed mode:', err); })
    .finally(restoreBtn);
}

// Save Zaps default amount from settings panel
export function saveZapsFromPanel() {
    var amountEl = document.getElementById('settings-zaps-default-amount');
    if (!state.config || !amountEl) {
        return;
    }
    var raw = parseInt(amountEl.value, 10);
    var amount = isNaN(raw) ? 42 : Math.max(1, Math.min(1000000, raw));
    state.config.default_zap_amount = amount;
    amountEl.value = amount;
    var restoreBtn = setSavingState(document.getElementById('settings-zaps-save'));
    saveConfig()
        .catch(function(err) { console.error('Failed to save zaps settings:', err); })
        .finally(restoreBtn);
}

// Save media server URL from settings panel
export function saveMediaServerFromPanel() {
    var urlEl = document.getElementById('settings-media-server-url');
    if (!state.config || !urlEl) {
        return;
    }
    state.config.media_server_url = (urlEl.value && urlEl.value.trim()) || 'https://blossom.primal.net';
    var restoreBtn = setSavingState(document.getElementById('settings-media-save'));
    saveConfig()
        .catch(function(err) { console.error('Failed to save media server URL:', err); })
        .finally(restoreBtn);
}

// Handle settings form submission
export async function handleSettingsSubmit(event) {
    event.preventDefault();
    clearValidationErrors();

    const displayName = document.getElementById('input-display-name').value.trim();
    const publicKeyInput = document.getElementById('input-public-key').value.trim();
    const privateKeyInput = document.getElementById('input-private-key').value.trim();

    // Validate public key (if provided)
    let publicKeyHex = '';
    let publicKeyNpub = '';

    if (publicKeyInput) {
        const pubKeyResult = await validatePublicKey(publicKeyInput);
        if (!pubKeyResult.valid) {
            showValidationError('input-public-key', pubKeyResult.error);
            return;
        }
        publicKeyHex = pubKeyResult.hex;
        publicKeyNpub = pubKeyResult.npub;
    }

    // Validate private key (if provided)
    let privateKeyHex = null;

    if (privateKeyInput) {
        const privKeyResult = await validateSecretKey(privateKeyInput);
        if (!privKeyResult.valid) {
            showValidationError('input-private-key', privKeyResult.error);
            return;
        }
        privateKeyHex = privKeyResult.hex;
    }

    // Update config - store hex format internally
    state.config.name = displayName;
    state.config.public_key = publicKeyHex;
    state.config.private_key = privateKeyHex;

    // Update our display cache
    state.publicKeyHex = publicKeyHex;
    state.publicKeyNpub = publicKeyNpub;

    // Save and update UI
    await saveConfig();
    if (_updateUIFromConfig) {
        _updateUIFromConfig();
    }
    closeSettings();

    // Show success feedback
    console.log('Settings saved successfully');
}
