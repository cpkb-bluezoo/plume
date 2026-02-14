/*
 * modules/config.js
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

import { state, DEFAULT_RELAYS, getEffectiveRelays } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml } from './utils.js';
import { validatePublicKey } from './keys.js';
import { updateSidebarAvatar, updateProfileDisplay } from './profile.js';
import { updateMessagesNavUnread } from './messages.js';
import { updateRelayList } from './relays.js';

// Load configuration from the backend
export async function loadConfig() {
    try {
        const configJson = await invoke('load_config');
        if (configJson) {
            state.config = JSON.parse(configJson);
            if (!Array.isArray(state.config.bookmarks)) {
                state.config.bookmarks = [];
            }
            // Log config without private key to avoid leaking secrets in console
            var safeConfig = Object.assign({}, state.config, { private_key: state.config.private_key ? '[REDACTED]' : null });
            console.log('Config loaded:', safeConfig);

            // Build profile from config fields (profile fields are stored directly in config)
            if (state.config.name && state.config.name !== 'Anonymous') {
                state.profile = {
                    name: state.config.name || null,
                    about: state.config.about || null,
                    picture: state.config.picture || null,
                    nip05: state.config.nip05 || null,
                    banner: state.config.banner || null,
                    website: state.config.website || null,
                    lud16: state.config.lud16 || null,
                };
                state.viewedProfile = state.profile;
            }

            // Parse the public key to get npub format
            if (state.config.public_key) {
                const keyInfo = await validatePublicKey(state.config.public_key);
                if (keyInfo.valid) {
                    state.publicKeyHex = keyInfo.hex;
                    state.publicKeyNpub = keyInfo.npub;
                }
            }

            updateUIFromConfig();
        }
    } catch (error) {
        console.error('Failed to load config:', error);
        // Use default config
        state.config = {
            public_key: '',
            private_key: null,
            relays: DEFAULT_RELAYS.slice(),
            name: 'Anonymous',
            about: null,
            picture: null,
            nip05: null,
            banner: null,
            website: null,
            lud16: null,
            home_feed_mode: 'firehose',
            media_server_url: 'https://blossom.primal.net',
            following: [],
            muted_users: [],
            muted_words: [],
            muted_hashtags: [],
            bookmarks: [],
            default_zap_amount: 42,
            hide_encrypted_notes: true
        };
        updateUIFromConfig();
    }
}

// Save configuration to the backend
export async function saveConfig() {
    try {
        // Sync profile fields into config before saving
        if (state.profile) {
            if (state.profile.name) {
                state.config.name = state.profile.name;
            }
            state.config.about = state.profile.about || null;
            state.config.picture = state.profile.picture || null;
            state.config.nip05 = state.profile.nip05 || null;
            state.config.banner = state.profile.banner || null;
            state.config.website = state.profile.website || null;
            state.config.lud16 = state.profile.lud16 || null;
        }
        const configJson = JSON.stringify(state.config);
        await invoke('save_config', { configJson: configJson });
        console.log('Config saved');
    } catch (error) {
        console.error('Failed to save config:', error);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToSaveSettings') : 'Failed to save settings') + ': ' + error);
    }
}

// Helper: set a button to a "saving" state (disabled + localised text), returns a restore function.
export function setSavingState(btn) {
    if (!btn) {
        return function() {};
    }
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var original = btn.textContent;
    btn.disabled = true;
    btn.textContent = t('editProfileModal.saving') || 'Saving...';
    return function() {
        btn.disabled = false;
        btn.textContent = original || t('accountModal.save') || 'Save';
    };
}

// Update UI elements from the current config
export function updateUIFromConfig() {
    if (!state.config) {
        return;
    }

    state.homeFeedMode = (state.config.home_feed_mode === 'follows') ? 'follows' : 'firehose';

    if (state.profile && state.publicKeyHex) {
        state.profileCache[state.publicKeyHex] = state.profileCache[state.publicKeyHex] || {};
        state.profileCache[state.publicKeyHex].name = state.profile.name != null ? state.profile.name : state.profileCache[state.publicKeyHex].name;
        state.profileCache[state.publicKeyHex].nip05 = state.profile.nip05 != null ? state.profile.nip05 : state.profileCache[state.publicKeyHex].nip05;
        state.profileCache[state.publicKeyHex].picture = state.profile.picture != null ? state.profile.picture : state.profileCache[state.publicKeyHex].picture;
        state.profileCache[state.publicKeyHex].lud16 = state.profile.lud16 != null ? state.profile.lud16 : (state.profileCache[state.publicKeyHex].lud16 || null);
    }

    const nameEl = document.getElementById('input-display-name');
    const pubEl = document.getElementById('input-public-key');
    if (nameEl) {
        nameEl.value = state.config.name || '';
    }
    if (pubEl) {
        pubEl.value = state.config.public_key || '';
    }
    // Private key is NEVER written to the DOM to prevent exfiltration by injected scripts.
    // The input-private-key field is write-only (user types a new key; it is not pre-populated).

    updateSidebarAvatar();
    updateMessagesNavUnread();
    updateProfileDisplay();
    updateRelayList();
    updateFeedInitialState();
}

// Set the feed placeholder based on config: welcome only when keys not configured; loading or noRelays otherwise.
export function updateFeedInitialState() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const container = document.getElementById('notes-container');
    if (!container) {
        return;
    }

    const effectiveRelays = getEffectiveRelays();
    const hasRelays = effectiveRelays.length > 0;
    const hasKeys = !!(state.config && state.config.public_key);

    if (!hasRelays) {
        container.innerHTML = `
            <div class="placeholder-message">
                <p>${escapeHtml(t('feed.noRelays'))}</p>
            </div>
        `;
        return;
    }
    // Relays configured: show "configure keys" only when user has not configured keys
    if (!hasKeys) {
        container.innerHTML = `
            <div class="placeholder-message" id="feed-welcome">
                <p>${escapeHtml(t('feed.welcomeTitle'))}</p>
                <p>${escapeHtml(t('feed.welcomeHint'))}</p>
            </div>
        `;
        return;
    }
    // Keys configured: show loading hint until first note arrives (or feed-eose)
    container.innerHTML = `
        <div class="placeholder-message" id="feed-loading">
            <p>${escapeHtml(t('feed.notesHint'))}</p>
        </div>
    `;
}
