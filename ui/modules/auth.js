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
import { escapeHtml, showConfirm, torImgUrl } from './utils.js';
import { updateUIFromConfig } from './config.js';
import { updateSidebarAvatar } from './profile.js';
import { startInitialFeedFetch } from './feed.js';
import { fetchProfile, adoptOwnRelayListFromNetwork } from './profile.js';
import { fetchAndMergeOwnMuteList } from './muting.js';
import { adoptOwnFollowingFromNetwork } from './follows.js';

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
    var hasNsec = !!(state.config && (state.config.private_key || state.config.key_storage === 'keyring'));
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
            ? '<img src="' + escapeHtml(torImgUrl(p.picture)) + '" class="known-profile-avatar" alt="">'
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

// Self-contained promise-based modal, same technique as showConfirm() in
// utils.js: wires its own handlers per call and tears them down after.
// A dedicated modal (not the generic confirm dialog) since this choice
// benefits from its own explanatory copy and the user specifically asked
// for an unambiguous, modal-only prompt.
function showKeyringPrompt() {
    return new Promise(function(resolve) {
        var modal = document.getElementById('keyring-prompt-modal');
        var yesBtn = document.getElementById('keyring-prompt-yes');
        var noBtn = document.getElementById('keyring-prompt-no');
        var closeBtn = document.getElementById('close-keyring-prompt');
        if (!modal || !yesBtn || !noBtn) {
            resolve(false);
            return;
        }
        modal.classList.add('active');
        modal.setAttribute('aria-hidden', 'false');

        function cleanup() {
            modal.classList.remove('active');
            modal.setAttribute('aria-hidden', 'true');
            yesBtn.removeEventListener('click', onYes);
            noBtn.removeEventListener('click', onNo);
            if (closeBtn) {
                closeBtn.removeEventListener('click', onNo);
            }
            modal.removeEventListener('click', onBackdrop);
        }
        function onYes() { cleanup(); resolve(true); }
        function onNo() { cleanup(); resolve(false); }
        function onBackdrop(e) {
            if (e.target === modal) {
                cleanup();
                resolve(false);
            }
        }

        yesBtn.addEventListener('click', onYes);
        noBtn.addEventListener('click', onNo);
        if (closeBtn) {
            closeBtn.addEventListener('click', onNo);
        }
        modal.addEventListener('click', onBackdrop);
        noBtn.focus();
    });
}

// Offers to move a freshly-entered or freshly-generated private key into
// the OS keyring. No-op when there's no keyring backend on this machine,
// or when no private key was actually just provided (a watch-only npub
// login has nothing to move). "No" leaves today's plaintext behavior
// completely unchanged -- this is purely additive and reversible later
// from Settings -> Keys.
async function maybeOfferKeyringStorage(hadPrivateKey) {
    if (!hadPrivateKey || !state.keyringAvailable) {
        return;
    }
    var wantsKeyring = await showKeyringPrompt();
    if (!wantsKeyring) {
        return;
    }
    try {
        var configJson = await invoke('set_key_storage_mode', { mode: 'keyring' });
        state.config = JSON.parse(configJson);
    } catch (e) {
        console.error('Failed to move private key to system keyring:', e);
    }
}

export async function handleWelcomeLogin() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var npubEl = document.getElementById('welcome-npub');
    var nsecEl = document.getElementById('welcome-nsec');
    var errorEl = document.getElementById('welcome-login-error');

    function showError(message) {
        if (errorEl) {
            errorEl.textContent = message;
            errorEl.style.display = 'block';
        }
    }

    if (errorEl) {
        errorEl.textContent = '';
        errorEl.style.display = 'none';
    }

    var npub = (npubEl ? npubEl.value : '').trim();
    var nsec = (nsecEl ? nsecEl.value : '').trim();

    if (!npub && !nsec) {
        showError(t('welcome.keyRequired') || 'Enter a public key or a private key');
        return;
    }

    try {
        if (!npub) {
            // Only a private key was given: derive the matching public key
            // from it instead of demanding the user supply both.
            var derivedJson = await invoke('derive_public_key', { private_key: nsec });
            npub = JSON.parse(derivedJson).npub;
        }

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
        // Fetch the user's own profile first, before the feed's own burst of
        // relay queries -- otherwise it's queued behind them and the sidebar
        // avatar/name take just as long to appear as the first notes do,
        // even though there's far less to fetch.
        fetchProfile();
        startInitialFeedFetch();
        // The relay list just used to start the feed is only ever a
        // bootstrap (the hardcoded defaults for a brand new profile); once
        // the user's own published NIP-65 list is known, switch to it.
        adoptOwnRelayListFromNetwork();
        // Same reasoning for the follows list: it needs to be in memory at
        // startup (for follows-mode feed filtering, same as the profile),
        // not fetched fresh only when Settings -> Follows happens to open.
        adoptOwnFollowingFromNetwork();
        // Mutes were local-only until now, with no way to learn about a
        // mute list another client published -- merge it in.
        fetchAndMergeOwnMuteList();
        // Offer keyring storage only for a freshly-typed private key, not
        // when selecting an existing saved profile (that profile's storage
        // mode was already decided at its own first login).
        await maybeOfferKeyringStorage(!!nsec);
    } catch (err) {
        showError(typeof err === 'string' ? err : (err.message || t('errors.loginFailed') || 'Login failed'));
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

        // generate_keypair always produces a fresh private key.
        await maybeOfferKeyringStorage(true);

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
        // Fetch the user's own profile first -- see handleWelcomeLogin for why.
        fetchProfile();
        startInitialFeedFetch();
        // See handleWelcomeLogin: adopt the real NIP-65 relay list once known.
        adoptOwnRelayListFromNetwork();
        // See handleWelcomeLogin: adopt the real follows list too.
        adoptOwnFollowingFromNetwork();
        // See handleWelcomeLogin: merge in the network's mute list too.
        fetchAndMergeOwnMuteList();
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
            key_storage: 'plaintext',
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
