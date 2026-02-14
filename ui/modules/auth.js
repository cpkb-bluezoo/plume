/*
 * modules/auth.js
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

import { state, DEFAULT_RELAYS } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml, showConfirm } from './utils.js';
import { updateUIFromConfig } from './config.js';
import { updateSidebarAvatar } from './profile.js';
import { startInitialFeedFetch } from './feed.js';
import { fetchProfile } from './profile.js';

let _switchView = null;
export function setSwitchView(fn) {
    _switchView = fn;
}

// Three-tier sidebar auth state:
// State 1 (logged out): Home + Profile/Welcome enabled; Messages, Notifications, Bookmarks, Compose muted
// State 2 (npub only): Home, Profile, Notifications, Bookmarks, Settings enabled; Compose + Messages muted
// State 3 (full auth): All enabled
export function updateSidebarAuthState() {
    var hasProfile = !!(state.config && state.config.public_key);
    var hasNsec = !!(state.config && state.config.private_key);
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    var navMessages = document.querySelector('.nav-item[data-view="messages"]');
    var navNotifications = document.querySelector('.nav-item[data-view="notifications"]');
    var navBookmarks = document.querySelector('.nav-item[data-view="bookmarks"]');
    var composeBtn = document.getElementById('compose-btn');

    if (!hasProfile) {
        // State 1: logged out
        if (navMessages) {
            navMessages.classList.add('nav-muted');
            navMessages.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to access messages';
        }
        if (navNotifications) {
            navNotifications.classList.add('nav-muted');
            navNotifications.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to see notifications';
        }
        if (navBookmarks) {
            navBookmarks.classList.add('nav-muted');
            navBookmarks.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to access bookmarks';
        }
        if (composeBtn) {
            composeBtn.classList.add('nav-muted');
            composeBtn.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to compose notes';
        }
    } else if (!hasNsec) {
        // State 2: npub only (read-only)
        if (navMessages) {
            navMessages.classList.add('nav-muted');
            navMessages.dataset.mutedReason = t('welcome.nsecRequired') || 'Private key required to send messages';
        }
        if (navNotifications) {
            navNotifications.classList.remove('nav-muted');
        }
        if (navBookmarks) {
            navBookmarks.classList.remove('nav-muted');
        }
        if (composeBtn) {
            composeBtn.classList.add('nav-muted');
            composeBtn.dataset.mutedReason = t('welcome.nsecRequired') || 'Private key required to publish notes';
        }
    } else {
        // State 3: full auth
        if (navMessages) {
            navMessages.classList.remove('nav-muted');
        }
        if (navNotifications) {
            navNotifications.classList.remove('nav-muted');
        }
        if (navBookmarks) {
            navBookmarks.classList.remove('nav-muted');
        }
        if (composeBtn) {
            composeBtn.classList.remove('nav-muted');
        }
    }
}

export function showMutedTooltip(el) {
    var reason = el.dataset.mutedReason || 'Not available';
    // Remove any existing tooltip
    var existing = document.querySelector('.nav-muted-tooltip');
    if (existing) {
        existing.remove();
    }

    var tip = document.createElement('div');
    tip.className = 'nav-muted-tooltip';
    tip.textContent = reason;
    document.body.appendChild(tip);

    var rect = el.getBoundingClientRect();
    tip.style.left = (rect.right + 12) + 'px';
    tip.style.top = (rect.top + rect.height / 2 - 14) + 'px';
    requestAnimationFrame(function() { tip.classList.add('visible'); });

    setTimeout(function() {
        tip.classList.remove('visible');
        setTimeout(function() { tip.remove(); }, 200);
    }, 2000);
}

// Populate the welcome screen's "Your profiles" list by fetching enriched
// profile data (name, picture) from each profile's config.json via the backend.
export async function populateWelcomeProfiles() {
    var container = document.getElementById('welcome-known-profiles');
    var list = document.getElementById('welcome-profiles-list');
    if (!container || !list) {
        return;
    }

    var profiles = [];
    try {
        var json = await invoke('list_profiles');
        if (json) {
            profiles = JSON.parse(json);
        }
    } catch (e) {
        console.warn('[Plume] Failed to list profiles:', e);
    }

    if (!profiles || profiles.length === 0) {
        container.style.display = 'none';
        return;
    }
    container.style.display = 'block';
    list.innerHTML = '';
    profiles.forEach(function(p) {
        var li = document.createElement('li');
        li.className = 'known-profile-item';
        li.dataset.npub = p.npub;
        var avatarHtml = p.picture
            ? '<img src="' + escapeHtml(p.picture) + '" class="known-profile-avatar" alt="">'
            : '<span class="known-profile-placeholder"><img src="icons/user.svg" alt="" class="nav-icon"></span>';
        var name = p.name || 'Anonymous';
        var shortNpub = p.npub.length > 20 ? p.npub.substring(0, 10) + '...' + p.npub.substring(p.npub.length - 6) : p.npub;
        li.innerHTML = avatarHtml +
            '<div class="known-profile-info">' +
            '<div class="known-profile-name">' + escapeHtml(name) + '</div>' +
            '<div class="known-profile-npub">' + escapeHtml(shortNpub) + '</div>' +
            '</div>';
        li.addEventListener('click', function() { handleProfileSelect(p.npub); });
        // Handle avatar load error
        var img = li.querySelector('.known-profile-avatar');
        if (img) {
            img.addEventListener('error', function() {
                this.style.display = 'none';
                var placeholder = document.createElement('span');
                placeholder.className = 'known-profile-placeholder';
                placeholder.innerHTML = '<img src="icons/user.svg" alt="" class="nav-icon">';
                this.parentNode.insertBefore(placeholder, this);
            });
        }
        list.appendChild(li);
    });
}

export async function handleWelcomeLogin() {
    var npubEl = document.getElementById('welcome-npub');
    var nsecEl = document.getElementById('welcome-nsec');
    var errorEl = document.getElementById('welcome-login-error');
    if (errorEl) {
        errorEl.textContent = '';
    }

    var npub = (npubEl ? npubEl.value : '').trim();
    var nsec = (nsecEl ? nsecEl.value : '').trim();

    if (!npub) {
        if (errorEl) {
            errorEl.textContent = 'Public key is required';
        }
        return;
    }

    try {
        var configJson = await invoke('login_with_keys', {
            public_key: npub,
            private_key: nsec || null
        });
        var cfg = JSON.parse(configJson);
        state.config = cfg;
        state.publicKeyHex = cfg.public_key || null;
        state.publicKeyNpub = null;
        if (cfg.public_key) {
            try { state.publicKeyNpub = await invoke('convert_hex_to_npub', { hex_key: cfg.public_key }); } catch (e) {}
        }
        state.homeFeedMode = cfg.home_feed_mode || 'firehose';
        updateUIFromConfig();
        updateSidebarAuthState();

        // Refresh app config
        try {
            var appJson = await invoke('get_app_config');
            state.appConfig = JSON.parse(appJson);
        } catch (e) {}

        state.initialFeedLoadDone = false;
        state.notes = [];
        _switchView('feed');
        startInitialFeedFetch();
        // Fetch profile from relays in background to update sidebar avatar and local config
        fetchProfile();
    } catch (err) {
        if (errorEl) {
            errorEl.textContent = typeof err === 'string' ? err : (err.message || 'Login failed');
        }
    }
}

export async function handleWelcomeGenerate() {
    var btn = document.getElementById('welcome-generate-btn');
    if (!btn) {
        return;
    }
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var originalText = btn.textContent;

    try {
        btn.disabled = true;
        btn.textContent = t('profile.generating') || 'Generating...';

        var result = await invoke('generate_keypair');
        var keys = JSON.parse(result);

        state.config = state.config || {};
        state.config.public_key = keys.public_key_hex;
        state.config.private_key = keys.private_key_hex;
        state.publicKeyHex = keys.public_key_hex;
        state.publicKeyNpub = keys.npub;
        updateUIFromConfig();
        updateSidebarAuthState();

        // Refresh app config
        try {
            var appJson = await invoke('get_app_config');
            state.appConfig = JSON.parse(appJson);
        } catch (e) {}

        alert(t('profile.newIdentityCreated') + '\n\nPublic Key (npub):\n' + keys.npub + '\n\nSecret Key (nsec):\n' + keys.nsec + '\n\n' + t('profile.saveNsecWarning'));

        state.initialFeedLoadDone = false;
        state.notes = [];
        _switchView('feed');
        startInitialFeedFetch();
    } catch (error) {
        alert((t('errors.failedToGenerateKeys') || 'Failed to generate key pair') + ': ' + error);
    } finally {
        btn.disabled = false;
        btn.textContent = originalText;
    }
}

export async function handleProfileSelect(npub) {
    try {
        var configJson = await invoke('switch_profile', { npub: npub });
        var cfg = JSON.parse(configJson);
        state.config = cfg;
        state.publicKeyHex = cfg.public_key || null;
        state.publicKeyNpub = null;
        if (cfg.public_key) {
            try { state.publicKeyNpub = await invoke('convert_hex_to_npub', { hex_key: cfg.public_key }); } catch (e) {}
        }
        state.homeFeedMode = cfg.home_feed_mode || 'firehose';
        updateUIFromConfig();
        updateSidebarAuthState();

        try {
            var appJson = await invoke('get_app_config');
            state.appConfig = JSON.parse(appJson);
        } catch (e) {}

        state.initialFeedLoadDone = false;
        state.notes = [];
        _switchView('feed');
        startInitialFeedFetch();
        // Fetch profile from relays in background to update sidebar avatar and local config
        fetchProfile();
    } catch (err) {
        alert('Failed to switch profile: ' + (typeof err === 'string' ? err : err.message || err));
    }
}

export async function handleLogout() {
    console.log('[Plume] handleLogout() called');
    try {
        var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        var confirmed = await showConfirm(t('settings.logoutConfirm') || 'Are you sure you want to log out? Your profile data will be kept locally.');
        if (!confirmed) {
            console.log('[Plume] Logout cancelled by user');
            return;
        }
        console.log('[Plume] Logout confirmed, calling backend...');
        try {
            await invoke('logout');
            console.log('[Plume] Backend logout succeeded');
        } catch (e) {
            console.error('[Plume] Logout backend call failed:', e);
        }

        // Reset frontend state – keep default relays so anonymous firehose still works
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
        state.publicKeyHex = null;
        state.publicKeyNpub = null;
        state.profile = null;
        state.viewedProfile = null;
        state.notes = [];
        state.homeFeedMode = 'firehose';
        state.initialFeedLoadDone = false;
        if (state.feedPollIntervalId) {
            clearInterval(state.feedPollIntervalId);
            state.feedPollIntervalId = null;
        }
        state.dmStreamStarted = false;
        state.unreadMessageCount = 0;
        state.selectedConversation = null;

        // Refresh app config for known profiles list
        try {
            var appJson = await invoke('get_app_config');
            state.appConfig = JSON.parse(appJson);
        } catch (e) {
            state.appConfig = { active_profile: null, known_profiles: [] };
        }

        console.log('[Plume] Logout state reset complete, switching to welcome');
        updateSidebarAvatar();
        updateSidebarAuthState();
        await populateWelcomeProfiles();
        _switchView('welcome');
    } catch (err) {
        console.error('[Plume] handleLogout() FAILED:', err);
        alert('Logout error: ' + (err && err.message ? err.message : String(err)));
    }
}
