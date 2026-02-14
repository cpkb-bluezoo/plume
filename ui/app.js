/*
 * app.js
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

console.log('[Plume] app.js script parsing/executing (first line)');

// Debug logging - outputs to browser console (view with Cmd+Option+I)
function debugLog(message) {
    console.log('[Plume]', message);
}

// Global error handler
window.onerror = function(message, source, lineno, colno, error) {
    console.error('[Plume] ERROR:', message, 'at', source, lineno, colno);
    return false;
};

window.onunhandledrejection = function(event) {
    console.error('[Plume] PROMISE ERROR:', event.reason);
};

// ============================================================
// Global State
// ============================================================

// Default relays used for anonymous firehose browsing when no user config is loaded
const DEFAULT_RELAYS = ['wss://relay.damus.io', 'wss://relay.primal.net', 'wss://nos.lol'];

// Returns the relay list to use: user's configured relays if available, otherwise defaults
function getEffectiveRelays() {
    if (state.config && Array.isArray(state.config.relays) && state.config.relays.length > 0) {
        return state.config.relays;
    }
    return DEFAULT_RELAYS;
}

// Application state
const state = {
    appConfig: null,
    config: {
        public_key: '',
        private_key: null,
        relays: DEFAULT_RELAYS.slice(),
        name: 'Anonymous',
        about: null, picture: null, nip05: null, banner: null, website: null, lud16: null,
        home_feed_mode: 'firehose',
        media_server_url: 'https://blossom.primal.net',
        following: [], muted_users: [], muted_words: [], muted_hashtags: [], bookmarks: [],
        default_zap_amount: 42
    },
    currentView: 'feed',
    notes: [],
    loading: false,
    publicKeyHex: null,
    publicKeyNpub: null,
    profile: null,
    profileLoading: false,
    homeFeedMode: 'firehose',
    initialFeedLoadDone: false,
    feedPollIntervalId: null,
    // pubkey (hex) -> { name, nip05, picture } for note authors
    profileCache: {},
    // When set, compose is a reply to this note
    replyingTo: null,
    // Profile page: null = current user, or hex pubkey of the user being viewed
    viewedProfilePubkey: null,
    // Profile data for the profile page (own or other); state.profile is always current user for sidebar
    viewedProfile: null,
    // Profile feed: notes for the currently viewed user (streamed or batch)
    profileNotes: [],
    profileFeedStreamNoteIndex: 0,
    profileNotesForPubkey: null, // pubkey for which profileNotes was loaded (so tab switch can reuse)
    viewedProfileRelays: null,   // relay URLs for the currently displayed user (NIP-65); null = not loaded
    viewedProfileRelaysForPubkey: null,
    bookmarkNotes: [],  // Notes currently shown on bookmarks page (for repost/like lookup)
    likedNoteIds: {},   // noteId -> true (notes we've liked this session; shows filled heart)
    ownFollowingPubkeys: [],  // Hex pubkeys we follow (for Follow/Unfollow button state)
    // Note detail page
    noteDetailSubjectId: null,
    noteDetailSubject: null,
    noteDetailAncestors: [],
    noteDetailReplies: [],   // [{ note, indent }, ...]
    noteDetailPreviousView: 'feed',
    // Unread DMs count for sidebar Messages icon (filled icon + badge). Set by DM sync when implemented.
    unreadMessageCount: 0,
    // Messages view
    selectedConversation: null,   // other_pubkey (hex) or null
    openConversationWith: null,   // when opening Messages from Profile "Message", set to that pubkey
    dmStreamStarted: false,
    // Follows settings panel: working copy [{ pubkey (hex), checked, listOrder }], sort key
    followsPanelList: [],
    followsPanelSort: 'name',
    followsPanelLoading: false,
    // Track where user was before entering edit-profile in settings (so Save navigates back)
    editProfilePreviousView: null,
    // Muted users panel: working copy [{ pubkey, checked }], no config change until Save
    mutedUsersPanelList: []
};

// ============================================================
// UI Helpers
// ============================================================

// Custom confirm dialog (native confirm() is blocked in Tauri 2.0 webviews)
function showConfirm(message) {
    return new Promise(function(resolve) {
        var modal = document.getElementById('confirm-dialog');
        var msgEl = document.getElementById('confirm-dialog-message');
        var okBtn = document.getElementById('confirm-dialog-ok');
        var cancelBtn = document.getElementById('confirm-dialog-cancel');
        if (!modal || !msgEl || !okBtn || !cancelBtn) {
            // Fallback: just resolve true if DOM elements are missing
            resolve(true);
            return;
        }
        msgEl.textContent = message;
        modal.classList.add('active');
        modal.setAttribute('aria-hidden', 'false');

        function cleanup() {
            modal.classList.remove('active');
            modal.setAttribute('aria-hidden', 'true');
            okBtn.removeEventListener('click', onOk);
            cancelBtn.removeEventListener('click', onCancel);
            modal.removeEventListener('click', onBackdrop);
        }
        function onOk() { cleanup(); resolve(true); }
        function onCancel() { cleanup(); resolve(false); }
        function onBackdrop(e) { if (e.target === modal) { cleanup(); resolve(false); } }

        okBtn.addEventListener('click', onOk);
        cancelBtn.addEventListener('click', onCancel);
        modal.addEventListener('click', onBackdrop);
        okBtn.focus();
    });
}

// ============================================================
// Tauri API Helpers
// ============================================================

// Call a Tauri command (function defined in Rust)
async function invoke(command, args = {}) {
    // __TAURI__ is injected by Tauri when the app runs
    if (window.__TAURI__ && window.__TAURI__.core) {
        return await window.__TAURI__.core.invoke(command, args);
    } else {
        console.warn('Tauri API not available - running in browser?');
        return null;
    }
}

// ============================================================
// Key Management
// ============================================================

// Parse a key and get its info (hex, npub, nsec)
async function parseKey(key) {
    if (!key || key.trim() === '') {
        return null;
    }
    
    try {
        const resultJson = await invoke('parse_key', { key: key });
        if (resultJson) {
            return JSON.parse(resultJson);
        }
    } catch (error) {
        console.error('Failed to parse key:', error);
    }
    return null;
}

// Convert public key to hex format
async function publicKeyToHex(key) {
    try {
        return await invoke('convert_public_key_to_hex', { key: key });
    } catch (error) {
        console.error('Failed to convert public key:', error);
        return null;
    }
}

// Convert hex to npub format
async function hexToNpub(hexKey) {
    try {
        return await invoke('convert_hex_to_npub', { hex_key: hexKey });
    } catch (error) {
        console.error('Failed to convert to npub:', error);
        return null;
    }
}

// Convert secret key to hex format
async function secretKeyToHex(key) {
    try {
        return await invoke('convert_secret_key_to_hex', { key: key });
    } catch (error) {
        console.error('Failed to convert secret key:', error);
        return null;
    }
}

// Validate and normalize a public key (returns hex or null)
async function validatePublicKey(key) {
    if (!key || key.trim() === '') {
        return { valid: false, hex: null, npub: null, error: 'Key is empty' };
    }
    
    try {
        const hex = await publicKeyToHex(key);
        if (hex) {
            const npub = await hexToNpub(hex);
            return { valid: true, hex: hex, npub: npub, error: null };
        }
    } catch (error) {
        return { valid: false, hex: null, npub: null, error: error.toString() };
    }
    
    return { valid: false, hex: null, npub: null, error: 'Invalid key format' };
}

// Validate and normalize a secret key (returns hex or null)
async function validateSecretKey(key) {
    if (!key || key.trim() === '') {
        return { valid: true, hex: null, error: null }; // Empty is OK (optional)
    }
    
    try {
        const hex = await secretKeyToHex(key);
        if (hex) {
            return { valid: true, hex: hex, error: null };
        }
    } catch (error) {
        return { valid: false, hex: null, error: error.toString() };
    }
    
    return { valid: false, hex: null, error: 'Invalid key format' };
}

// ============================================================
// Configuration Management
// ============================================================

// Load configuration from the backend
async function loadConfig() {
    try {
        const configJson = await invoke('load_config');
        if (configJson) {
            state.config = JSON.parse(configJson);
            if (!Array.isArray(state.config.bookmarks)) state.config.bookmarks = [];
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
            default_zap_amount: 42
        };
        updateUIFromConfig();
    }
}

// Save configuration to the backend
async function saveConfig() {
    try {
        // Sync profile fields into config before saving
        if (state.profile) {
            if (state.profile.name) state.config.name = state.profile.name;
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
function setSavingState(btn) {
    if (!btn) return function() {};
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var original = btn.textContent;
    btn.disabled = true;
    btn.textContent = t('editProfileModal.saving') || 'Saving...';
    return function() {
        btn.disabled = false;
        btn.textContent = original || t('accountModal.save') || 'Save';
    };
}

// Update sidebar profile avatar from config or profile (whichever has a picture).
// Called whenever the local profile is updated: login, config load, profile fetch from relays, etc.
function updateSidebarAvatar() {
    var sidebarAvatar = document.getElementById('sidebar-avatar');
    var sidebarPlaceholder = document.getElementById('sidebar-avatar-placeholder');
    if (!sidebarAvatar || !sidebarPlaceholder) return;
    // Prefer profile picture (fetched from relays), fall back to config picture (local)
    var pic = (state.profile && state.profile.picture) || (state.config && state.config.picture) || null;
    if (pic) {
        sidebarAvatar.src = pic;
        sidebarAvatar.style.display = 'block';
        sidebarPlaceholder.style.display = 'none';
        sidebarAvatar.onerror = function() {
            sidebarAvatar.style.display = 'none';
            sidebarPlaceholder.style.display = 'flex';
        };
    } else {
        sidebarAvatar.style.display = 'none';
        sidebarPlaceholder.style.display = 'flex';
    }
}

// Update Messages nav item: filled icon and unread badge when state.unreadMessageCount > 0.
function updateMessagesNavUnread() {
    const wrap = document.getElementById('messages-nav-icon-wrap');
    const icon = document.getElementById('messages-nav-icon');
    const badge = document.getElementById('messages-unread-badge');
    if (!wrap || !icon || !badge) return;
    const n = state.unreadMessageCount || 0;
    if (n > 0) {
        wrap.classList.add('has-unread');
        icon.src = 'icons/envelope-filled.svg';
        badge.textContent = n > 99 ? '99+' : String(n);
        badge.setAttribute('aria-hidden', 'false');
    } else {
        wrap.classList.remove('has-unread');
        icon.src = 'icons/envelope.svg';
        badge.textContent = '';
        badge.setAttribute('aria-hidden', 'true');
    }
}

// ============================================================
// Messages view (DMs)
// ============================================================

function shortenPubkey(pubkey) {
    if (!pubkey || pubkey.length < 20) return pubkey || '';
    return pubkey.slice(0, 8) + '…' + pubkey.slice(-8);
}

async function loadMessagesView() {
    const listEl = document.querySelector('.messages-list');
    const emptyEl = document.querySelector('.messages-chat-empty');
    const paneEl = document.querySelector('.messages-chat-pane');
    if (!listEl) return;

    if (!state.config || !state.config.public_key) {
        listEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.noIdentityYet') : 'Configure keys in Settings.')) + '</p></div>';
        return;
    }

    try {
        const json = await invoke('get_conversations');
        let conversations = json ? JSON.parse(json) : [];
        const openWith = state.openConversationWith;
        if (openWith && !conversations.some(function(c) { return (c.other_pubkey || '').toLowerCase() === (openWith || '').toLowerCase(); })) {
            conversations = conversations.concat([{ other_pubkey: openWith, last_created_at: 0 }]);
        }
        state.conversationsList = conversations;

        if (conversations.length === 0) {
            listEl.innerHTML = '<div class="placeholder-message"><p data-i18n="messages.noConversations"></p></div>';
        } else {
            const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            let html = '';
            for (let i = 0; i < conversations.length; i++) {
                const c = conversations[i];
                const other = c.other_pubkey || '';
                const name = (state.profileCache && state.profileCache[other] && state.profileCache[other].name) ? escapeHtml(state.profileCache[other].name) : shortenPubkey(other);
                const ts = c.last_created_at ? new Date(c.last_created_at * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';
                html += '<div class="conversation-item" role="button" tabindex="0" data-other-pubkey="' + escapeHtml(other) + '" title="' + escapeHtml(other) + '"><span class="conversation-item-name">' + name + '</span><span class="conversation-item-meta">' + escapeHtml(ts) + '</span></div>';
            }
            listEl.innerHTML = html;
        }

        if (!state.dmStreamStarted && getEffectiveRelays().length > 0) {
            state.dmStreamStarted = true;
            invoke('start_dm_stream').catch(function(e) { console.warn('start_dm_stream:', e); });
        }

        state.openConversationWith = null;
        if (openWith) {
            selectConversation(openWith);
        }
    } catch (e) {
        console.error('get_conversations failed:', e);
        listEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(String(e && e.message ? e.message : e)) + '</p></div>';
    }
}

function selectConversation(otherPubkeyHex) {
    state.selectedConversation = otherPubkeyHex;
    const paneEl = document.querySelector('.messages-chat-pane');
    const emptyEl = document.querySelector('.messages-chat-empty');
    document.querySelectorAll('.conversation-item').forEach(function(el) {
        el.classList.toggle('active', (el.getAttribute('data-other-pubkey') || '').toLowerCase() === (otherPubkeyHex || '').toLowerCase());
    });
    if (!otherPubkeyHex) {
        if (paneEl) paneEl.style.display = 'none';
        if (emptyEl) emptyEl.style.display = 'flex';
        return;
    }
    if (emptyEl) emptyEl.style.display = 'none';
    if (paneEl) paneEl.style.display = 'flex';
    loadConversationMessages(otherPubkeyHex);
}

async function loadConversationMessages(otherPubkeyHex) {
    const container = document.getElementById('messages-chat-messages');
    if (!container) return;
    container.innerHTML = '<p class="placeholder-message">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('noteDetail.loading') : 'Loading…') + '</p>';
    try {
        const json = await invoke('get_messages', { otherPubkeyHex: otherPubkeyHex });
        const messages = json ? JSON.parse(json) : [];
        renderMessages(container, messages);
    } catch (e) {
        console.error('get_messages failed:', e);
        container.innerHTML = '<p class="placeholder-message">' + escapeHtml(String(e && e.message ? e.message : e)) + '</p>';
    }
}

function renderMessages(container, messages) {
    if (!container) return;
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (messages.length === 0) {
        container.innerHTML = '<p class="placeholder-message">' + (t('messages.selectOrStart') || 'No messages yet.') + '</p>';
        return;
    }
    let html = '';
    for (let i = 0; i < messages.length; i++) {
        const m = messages[i];
        const cls = m.is_outgoing ? 'message-bubble message-outgoing' : 'message-bubble message-incoming';
        html += '<div class="' + cls + '" data-id="' + escapeHtml(m.id) + '"><div class="message-content">' + escapeHtml(m.content) + '</div><div class="message-meta">' + escapeHtml(new Date(m.created_at * 1000).toLocaleString()) + '</div></div>';
    }
    container.innerHTML = html;
    container.scrollTop = container.scrollHeight;
}

async function sendMessage() {
    const other = state.selectedConversation;
    const input = document.getElementById('message-input');
    if (!other || !input) return;
    const text = (input.value || '').trim();
    if (!text) return;
    try {
        await invoke('send_dm', { recipientPubkey: other, plaintext: text });
        input.value = '';
        const container = document.getElementById('messages-chat-messages');
        if (container && container.querySelector('.message-bubble')) {
            const m = { id: '', content: text, created_at: Math.floor(Date.now() / 1000), is_outgoing: true };
            const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            const html = '<div class="message-bubble message-outgoing" data-id=""><div class="message-content">' + escapeHtml(text) + '</div><div class="message-meta">' + escapeHtml(new Date().toLocaleString()) + '</div></div>';
            container.insertAdjacentHTML('beforeend', html);
            container.scrollTop = container.scrollHeight;
        } else {
            loadConversationMessages(other);
        }
    } catch (e) {
        console.error('send_dm failed:', e);
        alert(e && e.message ? e.message : String(e));
    }
}

// Update UI elements from the current config
function updateUIFromConfig() {
    if (!state.config) return;

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
    if (nameEl) nameEl.value = state.config.name || '';
    if (pubEl) pubEl.value = state.config.public_key || '';
    // Private key is NEVER written to the DOM to prevent exfiltration by injected scripts.
    // The input-private-key field is write-only (user types a new key; it is not pre-populated).

    updateSidebarAvatar();
    updateMessagesNavUnread();
    updateProfileDisplay();
    updateRelayList();
    updateFeedInitialState();
}

// Set the feed placeholder based on config: welcome only when keys not configured; loading or noRelays otherwise.
function updateFeedInitialState() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const container = document.getElementById('notes-container');
    if (!container) return;

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

// ============================================================
// Profile Management
// ============================================================

// Fetch profile for the profile page (own or viewed user)
async function fetchProfile() {
    if (state.profileLoading) return;
    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn && (!state.config || !state.config.public_key)) {
        state.viewedProfile = null;
        updateProfileDisplay();
        return;
    }

    state.profileLoading = true;
    try {
        if (viewingOwn) {
            const profileJson = await invoke('fetch_own_profile');
            if (profileJson && profileJson !== '{}') {
                state.profile = JSON.parse(profileJson);
                state.viewedProfile = state.profile;
                // Sync fetched profile fields into config so sidebar avatar and
                // other UI elements stay current without a page reload.
                if (state.config && state.profile) {
                    var changed = false;
                    ['name', 'about', 'picture', 'nip05', 'banner', 'website', 'lud16'].forEach(function(f) {
                        if (state.profile[f] && state.profile[f] !== state.config[f]) {
                            state.config[f] = state.profile[f];
                            changed = true;
                        }
                    });
                    if (changed) {
                        updateSidebarAvatar();
                        // Persist the updated config in the background
                        saveConfig();
                    }
                }
            } else {
                state.profile = null;
                state.viewedProfile = null;
            }
        } else {
            const profileJson = await invoke('fetch_profile', {
                pubkey: state.viewedProfilePubkey,
                relay_urls: getEffectiveRelays()
            });
            if (profileJson && profileJson !== '{}') {
                state.viewedProfile = JSON.parse(profileJson);
            } else {
                state.viewedProfile = null;
            }
        }
        updateProfileDisplay();
        if (state.viewedProfile) {
            var cacheKey = viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;
            if (cacheKey) {
                state.profileCache[cacheKey] = {
                    name: state.viewedProfile.name || null,
                    nip05: state.viewedProfile.nip05 || null,
                    picture: state.viewedProfile.picture || null,
                    lud16: state.viewedProfile.lud16 || null
                };
            }
        }
        if (viewingOwn) {
            loadProfileFeed(); // load own notes/relays tabs
        } else if (state.viewedProfilePubkey) {
            fetchFollowingAndFollowersForUser(state.viewedProfilePubkey);
            loadProfileFeed(); // fire-and-forget: stream or fetch notes in background
            // Know if we follow this user (for Follow/Unfollow button)
            fetchFollowing().then(function(data) {
                if (data && data.contacts) state.ownFollowingPubkeys = data.contacts.map(function(c) { return c.pubkey; });
                else state.ownFollowingPubkeys = [];
                updateFollowButtonState();
            });
        }
    } catch (error) {
        console.error('Failed to fetch profile:', error);
        state.viewedProfile = null;
        updateProfileDisplay();
    } finally {
        state.profileLoading = false;
    }
}

// Open profile page for a user (from note card avatar/name click)
function openProfileForUser(pubkey) {
    if (!pubkey) return;
    state.viewedProfilePubkey = pubkey;
    state.viewedProfile = null;
    state.viewedProfileRelaysForPubkey = null; // so Relays tab fetches this user's list
    switchView('profile');
}

// Get the npub string for the currently viewed profile (for QR modal). Returns a Promise.
function getProfileNpub() {
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn && state.publicKeyNpub) return Promise.resolve(state.publicKeyNpub);
    if (viewingOwn && state.config && state.config.public_key) {
        return invoke('convert_hex_to_npub', { hex_key: state.config.public_key })
            .then(function(n) { return n || state.config.public_key || ''; })
            .catch(function() { return state.config.public_key || ''; });
    }
    if (!viewingOwn && state.viewedProfilePubkey) {
        var key = state.viewedProfilePubkey;
        if (key.length === 64 && /^[a-fA-F0-9]+$/.test(key)) {
            return invoke('convert_hex_to_npub', { hex_key: key }).then(function(n) { return n || key; });
        }
        return Promise.resolve(key);
    }
    return Promise.resolve('');
}

function openProfileQRModal() {
    var modal = document.getElementById('profile-qr-modal');
    var wrap = document.getElementById('profile-qr-image-wrap');
    var npubInput = document.getElementById('profile-qr-npub-input');
    if (!modal || !wrap || !npubInput) return;
    wrap.innerHTML = '';
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    var openModal = function() { modal.classList.add('active'); };

    getProfileNpub().then(function(npub) {
        var raw = npub || (viewingOwn && (state.config && state.config.public_key) ? state.config.public_key : '') || '';
        var setQRAndOpen = function() { openModal(); };

        function showAndQR(npubString) {
            npubInput.value = npubString;
            if (!npubString) {
                setQRAndOpen();
                return;
            }
            invoke('generate_qr_svg', { data: npubString })
                .then(function(svgString) {
                    if (!svgString) { setQRAndOpen(); return; }
                    var themed = svgString
                        .replace(/fill="#000000"/g, 'fill="currentColor"')
                        .replace(/fill="#000"/g, 'fill="currentColor"')
                        .replace(/fill='#000'/g, 'fill="currentColor"')
                        .replace(/fill="#ffffff"/g, 'fill="transparent"')
                        .replace(/fill="#fff"/g, 'fill="transparent"')
                        .replace(/fill='#fff'/g, 'fill="transparent"');
                    wrap.innerHTML = themed;
                    setQRAndOpen();
                })
                .catch(function(err) {
                    console.warn('QR generation failed:', err);
                    setQRAndOpen();
                });
        }

        if (raw.length === 64 && /^[a-fA-F0-9]+$/.test(raw)) {
            invoke('convert_hex_to_npub', { hex_key: raw })
                .then(function(n) { showAndQR(n || raw); })
                .catch(function() { showAndQR(raw); });
        } else {
            showAndQR(raw);
        }
    }).catch(function() {
        npubInput.value = viewingOwn && state.config && state.config.public_key ? state.config.public_key : '';
        openModal();
    });
}

function closeProfileQRModal() {
    var modal = document.getElementById('profile-qr-modal');
    if (modal) modal.classList.remove('active');
}

// Navigate to Settings with Profile panel open (replaces opening edit profile modal)
function openEditProfileInSettings() {
    state.editProfilePreviousView = state.currentView;
    state.settingsPanelRequested = 'profile';
    switchView('settings');
}

function handleEditProfileSubmit(e) {
    if (e && e.preventDefault) e.preventDefault();
    debugLog('Edit profile submit/OK clicked');

    // Show saving state on the button
    var saveBtn = document.querySelector('#edit-profile-form button[type="submit"]');
    var restoreBtn = setSavingState(saveBtn);

    var nameEl = document.getElementById('edit-profile-name');
    var nip05El = document.getElementById('edit-profile-nip05');
    var websiteEl = document.getElementById('edit-profile-website');
    var aboutEl = document.getElementById('edit-profile-about');
    var lud16El = document.getElementById('edit-profile-lud16');
    var pictureEl = document.getElementById('edit-profile-picture');
    var bannerEl = document.getElementById('edit-profile-banner');
    var profile = {};
    var name = (nameEl ? nameEl.value : '') || '';
    var nip05 = (nip05El ? nip05El.value : '') || '';
    var website = (websiteEl ? websiteEl.value : '') || '';
    var about = (aboutEl ? aboutEl.value : '') || '';
    var lud16 = (lud16El ? lud16El.value : '') || '';
    var picture = (pictureEl ? pictureEl.value : '') || '';
    var banner = (bannerEl ? bannerEl.value : '') || '';
    name = name.trim();
    nip05 = nip05.trim();
    website = website.trim();
    about = about.trim();
    lud16 = lud16.trim();
    picture = picture.trim();
    banner = banner.trim();
    if (name) profile.name = name;
    if (nip05) profile.nip05 = nip05;
    if (website) profile.website = website;
    if (about) profile.about = about;
    if (lud16) profile.lud16 = lud16;
    if (picture) profile.picture = picture;
    if (banner) profile.banner = banner;
    var profileJson = JSON.stringify(profile);
    invoke('set_profile_metadata', { profileJson: profileJson })
        .then(function() {
            if (state.config) {
                if (name) state.config.name = name;
                if (about) state.config.about = about;
                if (picture) state.config.picture = picture;
                if (nip05) state.config.nip05 = nip05;
                if (banner) state.config.banner = banner;
                if (website) state.config.website = website;
                if (lud16) state.config.lud16 = lud16;
            }
            return fetchProfile();
        })
        .then(function() {
            if (state.profile && state.config) {
                state.config.name = state.profile.name || state.config.name;
                state.config.about = state.profile.about || state.config.about;
                state.config.picture = state.profile.picture || state.config.picture;
                state.config.nip05 = state.profile.nip05 || state.config.nip05;
                state.config.banner = state.profile.banner || state.config.banner;
                state.config.website = state.profile.website || state.config.website;
                state.config.lud16 = state.profile.lud16 || state.config.lud16;
            }
            updateProfileDisplay();
            updateSidebarAvatar();
            // Navigate back to where the user was before editing profile
            var prevView = state.editProfilePreviousView;
            state.editProfilePreviousView = null;
            if (prevView && prevView !== 'settings') {
                switchView(prevView);
            }
        })
        .catch(function(err) {
            console.error('Failed to save profile:', err);
            alert(typeof err === 'string' ? err : (err?.message || 'Failed to save profile'));
        })
        .finally(restoreBtn);
}

// Whether the note should be shown on the current profile tab (notes / replies / zaps).
function profileNoteMatchesTab(note, tab) {
    if (tab === 'zaps') return false;
    if (tab === 'notes') return note.kind === 1 || note.kind === 6;
    if (tab === 'replies') {
        return note.kind === 1 && note.tags && note.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
    }
    return true;
}

// Append a single note to #profile-feed (streaming). Dedupes by id; inserts in sorted position. Returns true if appended.
function appendProfileNoteCardSync(note) {
    if (!note || (note.kind !== 1 && note.kind !== 6)) return false;
    if (isNoteMuted(note)) return false;
    var container = document.getElementById('profile-feed');
    var effectivePubkey = getEffectiveProfilePubkey();
    if (!container || !effectivePubkey) return false;
    var tab = state.profileTab || 'notes';
    if (!profileNoteMatchesTab(note, tab)) return false;
    if (state.profileNotes.some(function(n) { return n.id === note.id; })) return false;

    state.profileNotes.push(note);
    state.profileNotes.sort(function(a, b) { return (b.created_at || 0) - (a.created_at || 0); });
    var idx = state.profileNotes.findIndex(function(n) { return n.id === note.id; });

    var placeholder = container.querySelector('.placeholder-message');
    if (placeholder) placeholder.remove();

    var noteIndex = state.profileFeedStreamNoteIndex++;
    var card = note.kind === 6 ? createRepostCard(note, noteIndex, 'profile-') : (function() {
        var replyToPubkey = getReplyToPubkey(note);
        return createNoteCard(note, noteIndex, 'profile-', replyToPubkey);
    })();
    var viewedPubkey = effectivePubkey ? String(effectivePubkey).toLowerCase() : '';
    if (viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
        var profileForAvatar = state.viewedProfile || (effectivePubkey === state.publicKeyHex ? state.profile : null);
        if (profileForAvatar && profileForAvatar.picture) setCardAvatar(card, profileForAvatar.picture);
    }
    if (idx === 0) {
        container.insertBefore(card, container.firstChild);
    } else if (idx >= container.children.length) {
        container.appendChild(card);
    } else {
        container.insertBefore(card, container.children[idx]);
    }
    if (note.kind === 1) verifyNote(note, noteIndex, 'profile-');
    if (note.kind === 6) verifyRepostOriginal(note, noteIndex, 'profile-');
    ensureProfilesForNotes([note]);
    resolveNostrEmbeds(card);
    return true;
}

// Effective pubkey for the profile page: when viewing own profile (viewedProfilePubkey null or self), returns publicKeyHex; otherwise viewedProfilePubkey.
function getEffectiveProfilePubkey() {
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    return viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;
}

// Load content for the currently viewed profile into #profile-feed (notes/replies/zaps/relays by tab).
// Non-blocking: shows loading state, then fetches/streams in background like home feed.
// When viewing own profile, viewedProfilePubkey is null; we use state.publicKeyHex for notes and state.config.relays for Relays tab.
async function loadProfileFeed() {
    var container = document.getElementById('profile-feed');
    if (!container || !state.config) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var tab = state.profileTab || 'notes';
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    var effectivePubkey = viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;

    if (tab === 'relays') {
        loadProfileRelays();
        return;
    }

    if (!effectivePubkey) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('profile.noIdentityYet')) + '</p></div>';
        return;
    }

    // Reuse already-loaded notes when just switching tab (notes <-> replies <-> zaps)
    if (state.profileNotes.length > 0 && state.profileNotesForPubkey === effectivePubkey) {
        var filtered = state.profileNotes.filter(function(n) { return profileNoteMatchesTab(n, tab); });
        displayProfileNotes(filtered);
        return;
    }

    container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.notesHint')) + '</p></div>';
    var feedRelays = getEffectiveRelays();
    if (!feedRelays.length) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noRelays')) + '</p></div>';
        return;
    }

    var authors = [effectivePubkey];
    var viewedPubkeyAtStart = effectivePubkey;

    var useStream = window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function';
    if (useStream) {
        state.profileNotes = [];
        state.profileNotesForPubkey = viewedPubkeyAtStart;
        state.profileFeedStreamNoteIndex = 0;
        var unlisten = { note: function() {}, eose: function() {} };
        try {
            var listeners = await Promise.all([
                window.__TAURI__.event.listen('profile-feed-note', function(event) {
                    var payload = event.payload;
                    var note = typeof payload === 'string' ? JSON.parse(payload) : payload;
                    if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) return;
                    if ((note.kind !== 1 && note.kind !== 6) || (note.pubkey && String(note.pubkey).toLowerCase() !== String(viewedPubkeyAtStart).toLowerCase())) return;
                    appendProfileNoteCardSync(note);
                }),
                window.__TAURI__.event.listen('profile-feed-eose', function() {
                    if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) return;
                    unlisten.note();
                    unlisten.eose();
                    var c = document.getElementById('profile-feed');
                    if (c && c.querySelectorAll('.note-card').length === 0 && !c.querySelector('.placeholder-message')) {
                        c.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noNotes')) + '</p></div>';
                    }
                })
            ]);
            unlisten.note = listeners[0];
            unlisten.eose = listeners[1];
            await invoke('start_feed_stream', {
                relay_urls: feedRelays,
                limit: FEED_LIMIT,
                authors: authors,
                since: null,
                stream_context: 'profile'
            });
        } catch (err) {
            console.error('Profile stream failed:', err);
            if (getEffectiveProfilePubkey() === viewedPubkeyAtStart && container) {
                container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
            }
            unlisten.note();
            unlisten.eose();
        }
        return;
    }

    // Batch fallback: fetch in background, then display (non-blocking). profile_feed=true so we get reposts (kind 6).
    state.profileNotesForPubkey = viewedPubkeyAtStart;
    fetchFeedNotes(feedRelays, authors, null, true).then(function(notes) {
        if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) return;
        var feedNotes = notes ? notes.filter(function(n) { return n.kind === 1 || n.kind === 6; }) : [];
        state.profileNotes = feedNotes;
        if (tab === 'notes') {
            displayProfileNotes(feedNotes);
        } else if (tab === 'replies') {
            var replies = feedNotes.filter(function(n) {
                return n.kind === 1 && n.tags && n.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
            });
            displayProfileNotes(replies);
        } else if (tab === 'zaps') {
            displayProfileNotes([]);
        } else {
            displayProfileNotes(feedNotes);
        }
    }).catch(function(e) {
        console.error('Profile feed failed:', e);
        if (getEffectiveProfilePubkey() === viewedPubkeyAtStart && container) {
            container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
        }
    });
}

// Load relay list for the profile Relays tab: own = config.relays, other = fetch NIP-65 kind 10002.
function loadProfileRelays() {
    var container = document.getElementById('profile-feed');
    if (!container) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;

    if (viewingOwn) {
        var relays = getEffectiveRelays();
        displayProfileRelays(relays);
        return;
    }

    if (state.viewedProfileRelaysForPubkey === state.viewedProfilePubkey && state.viewedProfileRelays) {
        displayProfileRelays(state.viewedProfileRelays);
        return;
    }

    var fetchRelays = getEffectiveRelays();
    container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.notesHint')) + '</p></div>';
    if (!fetchRelays.length) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noRelays')) + '</p></div>';
        return;
    }
    var pubkey = state.viewedProfilePubkey;
    invoke('fetch_relay_list', { pubkey: pubkey, relayUrls: fetchRelays })
        .then(function(json) {
            if (state.viewedProfilePubkey !== pubkey) return;
            var relays = [];
            try {
                if (json) relays = JSON.parse(json);
                if (!Array.isArray(relays)) relays = [];
            } catch (e) { relays = []; }
            state.viewedProfileRelays = relays;
            state.viewedProfileRelaysForPubkey = pubkey;
            displayProfileRelays(relays);
        })
        .catch(function(e) {
            console.error('Fetch relay list failed:', e);
            if (state.viewedProfilePubkey === pubkey && container) {
                container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
            }
        });
}

function displayProfileRelays(relays) {
    var container = document.getElementById('profile-feed');
    if (!container) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (!relays || relays.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noRelays')) + '</p></div>';
        return;
    }
    container.innerHTML = '';
    var ul = document.createElement('ul');
    ul.className = 'profile-relay-list';
    relays.forEach(function(url) {
        var li = document.createElement('li');
        li.className = 'profile-relay-item';
        li.textContent = url;
        ul.appendChild(li);
    });
    container.appendChild(ul);
}

// Render note cards into #profile-feed (uses id prefix 'profile-' for verification badges).
function displayProfileNotes(notes) {
    var container = document.getElementById('profile-feed');
    if (!container) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    container.innerHTML = '';
    notes = (notes || []).filter(function(n) { return !isNoteMuted(n); });
    if (notes.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noNotes')) + '</p></div>';
        return;
    }
    var effectivePubkey = getEffectiveProfilePubkey();
    var viewedPubkey = effectivePubkey ? String(effectivePubkey).toLowerCase() : '';
    var viewedProfile = state.viewedProfile || (effectivePubkey === state.publicKeyHex ? state.profile : null);
    notes.sort(function(a, b) { return (b.created_at || 0) - (a.created_at || 0); });
    var noteIndex = 0;
    var prefix = 'profile-';
    notes.forEach(function(note) {
        if (note.kind !== 1 && note.kind !== 6) return;
        var card = note.kind === 6 ? createRepostCard(note, noteIndex, prefix) : (function() {
            var replyToPubkey = getReplyToPubkey(note);
            return createNoteCard(note, noteIndex, prefix, replyToPubkey);
        })();
        container.appendChild(card);
        if (viewedProfile && viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
            setCardAvatar(card, viewedProfile.picture);
        }
        if (note.kind === 1) verifyNote(note, noteIndex, prefix);
        if (note.kind === 6) verifyRepostOriginal(note, noteIndex, prefix);
        noteIndex++;
    });
    ensureProfilesForNotes(notes);
    resolveNostrEmbeds(container);
}

// Generate a new key pair
async function generateNewKeyPair() {
    debugLog('generateNewKeyPair called');
    const btn = document.getElementById('generate-keys-btn');
    if (!btn) {
        debugLog('ERROR: Button not found in generateNewKeyPair');
        return;
    }
    const originalText = btn.textContent;
    
    // Skip confirmation for now - just generate
    debugLog('Proceeding to generate keys...');
    
    try {
        btn.disabled = true;
        btn.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.generating') : 'Generating...');
        debugLog('Calling invoke generate_keypair...');
        
        const result = await invoke('generate_keypair');
        debugLog('Got result: ' + result);
        const keys = JSON.parse(result);
        
        debugLog('New key pair generated: ' + keys.npub);
        
        // Update the state
        state.config.public_key = keys.public_key_hex;
        state.config.private_key = keys.private_key_hex;
        state.publicKeyNpub = keys.npub;
        
        // Update the UI
        updateUIFromConfig();
        
        // Show the keys to the user
        const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        alert(t('profile.newIdentityCreated') + '\n\nPublic Key (npub):\n' + keys.npub + '\n\nSecret Key (nsec):\n' + keys.nsec + '\n\n' + t('profile.saveNsecWarning'));
        
    } catch (error) {
        debugLog('ERROR generating key pair: ' + error);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToGenerateKeys') : 'Failed to generate key pair') + ': ' + error);
    } finally {
        btn.disabled = false;
        btn.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.generateKeyPair') : originalText);
    }
}

// Update the profile display (profile page from state.viewedProfile; sidebar from state.profile)
function updateProfileDisplay() {
    const nameEl = document.getElementById('profile-name');
    const aboutEl = document.getElementById('profile-about');
    const pictureEl = document.getElementById('profile-picture');
    const placeholderEl = document.getElementById('profile-placeholder');
    const bannerEl = document.getElementById('profile-banner');
    const nip05El = document.getElementById('profile-nip05');
    const websiteEl = document.getElementById('profile-website');
    const lightningEl = document.getElementById('profile-lightning');
    const lud16El = document.getElementById('profile-lud16');
    const sidebarAvatar = document.getElementById('sidebar-avatar');
    const sidebarPlaceholder = document.getElementById('sidebar-avatar-placeholder');
    const editProfileBtn = document.getElementById('edit-profile-btn');
    const followBtn = document.getElementById('follow-btn');
    const messageUserBtn = document.getElementById('message-user-btn');
    const muteBtn = document.getElementById('mute-btn');

    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    const profile = state.viewedProfile;
    // When viewing another user without profile yet, use cache so we can show name/picture if we had it
    const cache = !viewingOwn && state.viewedProfilePubkey ? state.profileCache[state.viewedProfilePubkey] : null;

    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (profile) {
        if (nameEl) nameEl.textContent = profile.name || (viewingOwn ? state.config?.name : null) || t('profile.anonymous');
        if (aboutEl) aboutEl.textContent = profile.about || '';
        if (pictureEl && placeholderEl) {
            if (profile.picture) {
                pictureEl.src = profile.picture;
                pictureEl.style.display = 'block';
                placeholderEl.style.display = 'none';
                pictureEl.onerror = () => {
                    pictureEl.style.display = 'none';
                    placeholderEl.style.display = 'flex';
                };
            } else {
                pictureEl.style.display = 'none';
                placeholderEl.style.display = 'flex';
            }
        }
        if (bannerEl) {
            bannerEl.style.backgroundImage = profile.banner ? `url('${profile.banner}')` : '';
        }
        if (nip05El) {
            nip05El.textContent = profile.nip05 || '';
            nip05El.style.display = profile.nip05 ? 'block' : 'none';
        }
        if (websiteEl) {
            if (profile.website) {
                websiteEl.href = profile.website;
                // Show the URL without protocol prefix for a cleaner look
                var displayUrl = profile.website.replace(/^https?:\/\//, '').replace(/\/$/, '');
                websiteEl.textContent = displayUrl;
                websiteEl.style.display = 'block';
            } else {
                websiteEl.style.display = 'none';
            }
        }
        if (lightningEl && lud16El) {
            lud16El.textContent = profile.lud16 || '';
            lightningEl.style.display = profile.lud16 ? 'inline' : 'none';
        }
        var joinedEl = document.getElementById('profile-joined');
        if (joinedEl) {
            if (profile.created_at) {
                var d = new Date(profile.created_at * 1000);
                joinedEl.textContent = d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
            } else {
                joinedEl.textContent = '—';
            }
        }
    } else {
        var displayName = viewingOwn ? (state.config?.name || t('profile.notConfigured')) : (cache && cache.name ? cache.name : '…');
        if (nameEl) nameEl.textContent = displayName;
        if (aboutEl) aboutEl.textContent = '';
        if (pictureEl && placeholderEl) {
            if (cache && cache.picture) {
                pictureEl.src = cache.picture;
                pictureEl.style.display = 'block';
                placeholderEl.style.display = 'none';
                pictureEl.onerror = function() { pictureEl.style.display = 'none'; placeholderEl.style.display = 'flex'; };
            } else {
                pictureEl.style.display = 'none';
                placeholderEl.style.display = 'flex';
            }
        }
        if (bannerEl) bannerEl.style.backgroundImage = '';
        if (nip05El) nip05El.style.display = 'none';
        if (websiteEl) websiteEl.style.display = 'none';
        if (lightningEl) lightningEl.style.display = 'none';
        var joinedEl = document.getElementById('profile-joined');
        if (joinedEl) joinedEl.textContent = '—';
    }

    var qrBtn = document.getElementById('profile-qr-btn');
    if (qrBtn) {
        var hasPubkey = (viewingOwn && (state.publicKeyNpub || state.config?.public_key)) || (!viewingOwn && state.viewedProfilePubkey);
        qrBtn.classList.toggle('visible', !!hasPubkey);
    }

    const noKeyNotice = document.getElementById('no-key-notice');
    if (noKeyNotice) {
        noKeyNotice.classList.toggle('hidden', !viewingOwn || !!state.config?.public_key);
    }

    if (editProfileBtn) editProfileBtn.style.display = viewingOwn ? 'block' : 'none';
    if (followBtn) followBtn.style.display = viewingOwn ? 'none' : 'block';
    if (messageUserBtn) messageUserBtn.style.display = viewingOwn ? 'none' : 'flex';
    if (muteBtn) {
        muteBtn.style.display = viewingOwn ? 'none' : 'block';
        muteBtn.textContent = (state.viewedProfilePubkey && isUserMuted(state.viewedProfilePubkey)) ? t('profile.unmute') : t('profile.mute');
    }

    if (sidebarAvatar && sidebarPlaceholder) {
        const pic = state.profile?.picture || state.config?.picture;
        if (pic) {
            sidebarAvatar.src = pic;
            sidebarAvatar.style.display = 'block';
            sidebarPlaceholder.style.display = 'none';
            sidebarAvatar.onerror = () => {
                sidebarAvatar.style.display = 'none';
                sidebarPlaceholder.style.display = 'flex';
            };
        } else {
            sidebarAvatar.style.display = 'none';
            sidebarPlaceholder.style.display = 'flex';
        }
    }
}

// ============================================================
// Following / Followers Management
// ============================================================

// Fetch following and followers (for own profile)
async function fetchFollowingAndFollowers() {
    if (!state.config || !state.config.public_key) return;
    return fetchFollowingAndFollowersForUser(state.config.public_key);
}

// Fetch following and followers for any user (profile page counts). pubkey can be hex or npub.
async function fetchFollowingAndFollowersForUser(pubkey) {
    var relays = getEffectiveRelays();
    if (!relays.length || !pubkey) return;
    var fc = document.getElementById('following-count');
    var fl = document.getElementById('followers-count');
    if (fc) fc.textContent = '…';
    if (fl) fl.textContent = '…';

    var followingResult = null;
    var followersResult = null;
    try {
        followingResult = await invoke('fetch_following', { pubkey: pubkey, relayUrls: relays });
        followersResult = await invoke('fetch_followers', { pubkey: pubkey, relayUrls: relays });
    } catch (e) {
        console.error('Failed to fetch following/followers:', e);
        if (fc) fc.textContent = '0';
        if (fl) fl.textContent = '0';
        return;
    }
    if (followingResult) {
        try {
            var data = JSON.parse(followingResult);
            displayFollowing(data);
        } catch (_) {
            if (fc) fc.textContent = '0';
        }
    }
    if (followersResult) {
        try {
            var data = JSON.parse(followersResult);
            displayFollowers(data);
        } catch (_) {
            if (fl) fl.textContent = '0';
        }
    }
}

// Fetch following (who you follow)
async function fetchFollowing() {
    try {
        const json = await invoke('fetch_own_following');
        if (json) {
            return JSON.parse(json);
        }
    } catch (error) {
        console.error('Failed to fetch following:', error);
    }
    return null;
}

// Update Follow/Unfollow button label and state when viewing another user's profile
function updateFollowButtonState() {
    const followBtn = document.getElementById('follow-btn');
    if (!followBtn) return;
    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn) return;
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const pk = (state.viewedProfilePubkey || '').toLowerCase();
    const isFollowing = !!(state.ownFollowingPubkeys && pk && state.ownFollowingPubkeys.some(function(p) { return String(p).toLowerCase() === pk; }));
    followBtn.textContent = isFollowing ? (t('profile.unfollow') || 'Unfollow') : (t('profile.follow') || 'Follow');
    followBtn.dataset.following = isFollowing ? '1' : '0';
}

// Follow or unfollow the currently viewed profile user. Updates contact list and publishes to relays immediately.
async function handleFollowClick() {
    if (!state.viewedProfilePubkey || state.viewedProfilePubkey === state.publicKeyHex) return;
    const followBtn = document.getElementById('follow-btn');
    const currentlyFollowing = followBtn && followBtn.dataset.following === '1';
    const add = !currentlyFollowing;
    followBtn && (followBtn.disabled = true);
    try {
        await invoke('update_contact_list', { add: add, targetPubkey: state.viewedProfilePubkey });
        var pk = String(state.viewedProfilePubkey).toLowerCase();
        if (add) {
            if (!state.ownFollowingPubkeys) state.ownFollowingPubkeys = [];
            if (!state.ownFollowingPubkeys.some(function(p) { return String(p).toLowerCase() === pk; })) {
                state.ownFollowingPubkeys.push(state.viewedProfilePubkey);
            }
        } else {
            if (state.ownFollowingPubkeys) {
                state.ownFollowingPubkeys = state.ownFollowingPubkeys.filter(function(p) { return String(p).toLowerCase() !== pk; });
            }
        }
        updateFollowButtonState();
    } catch (e) {
        console.error('Follow/unfollow failed:', e);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to update follow') + ': ' + e);
    } finally {
        if (followBtn) followBtn.disabled = false;
    }
}

// Fetch followers (who follows you)
async function fetchFollowers() {
    try {
        const json = await invoke('fetch_own_followers');
        if (json) {
            return JSON.parse(json);
        }
    } catch (error) {
        console.error('Failed to fetch followers:', error);
    }
    return null;
}

// Display following list (updates count on profile page)
function displayFollowing(data) {
    const countEl = document.getElementById('following-count');
    if (!countEl) return;
    const count = data.contacts ? data.contacts.length : 0;
    countEl.textContent = count.toString();
}

// Display followers list (updates count on profile page)
function displayFollowers(data) {
    const countEl = document.getElementById('followers-count');
    if (!countEl) return;
    const count = data.followers ? data.followers.length : 0;
    countEl.textContent = count.toString();
}

// Create a follow item element
function createFollowItem(pubkey, petname) {
    const shortKey = shortenKey(pubkey);
    
    const item = document.createElement('div');
    item.className = 'follow-item';
    item.innerHTML = `
        <div class="follow-avatar">👤</div>
        <div class="follow-info">
            <div class="follow-name">${petname ? escapeHtml(petname) : escapeHtml(shortKey)}</div>
            <div class="follow-pubkey">${escapeHtml(shortKey)}</div>
        </div>
    `;
    
    return item;
}

// Show message in following list
function showFollowingMessage(message) {
    const listEl = document.getElementById('following-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Show message in followers list
function showFollowersMessage(message) {
    const listEl = document.getElementById('followers-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Switch between following/followers tabs
function switchFollowTab(tabName) {
    // Update tabs
    document.querySelectorAll('.follow-tab').forEach(tab => {
        tab.classList.remove('active');
        if (tab.dataset.tab === tabName) {
            tab.classList.add('active');
        }
    });
    
    // Update lists
    document.getElementById('following-list').classList.remove('active');
    document.getElementById('followers-list').classList.remove('active');
    
    if (tabName === 'following') {
        document.getElementById('following-list').classList.add('active');
    } else {
        document.getElementById('followers-list').classList.add('active');
    }
}

// ============================================================
// View Management
// ============================================================

// Whether a note (by event id) is in the user's bookmarks
function isNoteBookmarked(noteId) {
    return !!(state.config && Array.isArray(state.config.bookmarks) && state.config.bookmarks.indexOf(noteId) !== -1);
}

// Load and render the bookmarks view (fetch events by ids from config)
async function loadBookmarksView() {
    const container = document.getElementById('bookmarks-container');
    if (!container) return;
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const ids = state.config && Array.isArray(state.config.bookmarks) ? state.config.bookmarks : [];
    const relays = getEffectiveRelays();
    if (ids.length === 0 || relays.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('bookmarks.noBookmarks')) + '</p></div>';
        return;
    }
    container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.notesHint') || 'Loading…') + '</p></div>';
    try {
        const resultJson = await invoke('fetch_events_by_ids', { relay_urls: relays, ids: ids });
        var notes = resultJson ? JSON.parse(resultJson) : [];
        notes = notes.filter(function(n) { return !isNoteMuted(n); });
        container.innerHTML = '';
        if (notes.length === 0) {
            container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('bookmarks.noBookmarks')) + '</p></div>';
            return;
        }
        notes.forEach(function(note, i) {
            var replyToPubkey = getReplyToPubkey(note);
            var card = createNoteCard(note, i, 'bookmark-', replyToPubkey, true);
            container.appendChild(card);
        });
        state.bookmarkNotes = notes;
        notes.forEach(function(note, i) {
            verifyNote(note, i, 'bookmark-');
        });
        await ensureProfilesForNotes(notes);
        // Explicitly set avatars for bookmark cards (in case selector missed, or to use cache we already had)
        notes.forEach(function(note, i) {
            var profile = state.profileCache[note.pubkey];
            if (profile && profile.picture) {
                var cardEl = container.children[i];
                if (cardEl) setCardAvatar(cardEl, profile.picture);
            }
        });
        resolveNostrEmbeds(container);
    } catch (e) {
        console.error('Failed to load bookmarks:', e);
        state.bookmarkNotes = [];
        const msg = t('errors.loadFailed') || 'Failed to load bookmarks';
        const detail = (e && (e.message || String(e))) ? ': ' + (e.message || String(e)) : '';
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(msg + detail) + '</p></div>';
    }
}

// Build threaded replies: map parent_id -> [children], then flatten with indent (BFS).
function buildReplyThread(replies, subjectId) {
    var byParent = {};
    byParent[subjectId] = [];
    replies.forEach(function(note) {
        var pid = getParentEventId(note) || subjectId;
        if (!byParent[pid]) byParent[pid] = [];
        byParent[pid].push(note);
    });
    var out = [];
    function addChildren(parentId, indent) {
        var list = byParent[parentId];
        if (!list) return;
        list.sort(function(a, b) { return (a.created_at || 0) - (b.created_at || 0); });
        list.forEach(function(note) {
            out.push({ note: note, indent: indent });
            addChildren(note.id, indent + 1);
        });
    }
    addChildren(subjectId, 0);
    return out;
}

// Open the note detail page for a note (by id or full note object). Fetches subject, ancestors, replies; then switches view and renders.
async function openNoteDetail(noteIdOrNote) {
    var noteId = typeof noteIdOrNote === 'string' ? noteIdOrNote : (noteIdOrNote && noteIdOrNote.id);
    if (!noteId) return;
    if (state.currentView === 'note-detail' && state.noteDetailSubjectId === noteId) return;
    state.noteDetailPreviousView = state.currentView;
    state.noteDetailSubjectId = noteId;
    state.noteDetailSubject = null;
    state.noteDetailAncestors = [];
    state.noteDetailReplies = [];

    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var relays = getEffectiveRelays();
    if (!relays.length) {
        state.currentView = 'note-detail';
        switchView('note-detail');
        renderNoteDetailPage();
        return;
    }

    switchView('note-detail');
    var ancestorsEl = document.getElementById('note-detail-ancestors');
    var subjectWrap = document.getElementById('note-detail-subject-wrap');
    var repliesEl = document.getElementById('note-detail-replies');
    if (ancestorsEl) ancestorsEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('noteDetail.loading')) + '</p></div>';
    if (subjectWrap) subjectWrap.innerHTML = '';
    if (repliesEl) repliesEl.innerHTML = '';

    var subject = typeof noteIdOrNote === 'object' && noteIdOrNote.id === noteId ? noteIdOrNote : null;
    if (!subject) {
        try {
            var res = await invoke('fetch_events_by_ids', { relay_urls: relays, ids: [noteId] });
            var arr = res ? JSON.parse(res) : [];
            subject = arr.length ? arr[0] : null;
        } catch (e) {
            console.error('Failed to fetch subject note:', e);
        }
    }
    if (!subject) {
        if (ancestorsEl) ancestorsEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
        return;
    }
    state.noteDetailSubject = subject;

    var ancestors = [];
    var current = subject;
    var seen = {};
    while (current) {
        var parentId = getParentEventId(current);
        if (!parentId || seen[parentId]) break;
        seen[parentId] = true;
        try {
            var r = await invoke('fetch_events_by_ids', { relay_urls: relays, ids: [parentId] });
            var a = r ? JSON.parse(r) : [];
            if (!a.length) break;
            var parentNote = a[0];
            ancestors.unshift(parentNote);
            current = parentNote;
        } catch (_) { break; }
    }
    state.noteDetailAncestors = ancestors;

    var repliesRaw = [];
    try {
        var replyJson = await invoke('fetch_replies_to_event', { relay_urls: relays, event_id: noteId, limit: 500 });
        repliesRaw = replyJson ? JSON.parse(replyJson) : [];
    } catch (e) {
        console.error('Failed to fetch replies:', e);
    }
    state.noteDetailReplies = buildReplyThread(repliesRaw, noteId).filter(function(item) { return !isNoteMuted(item.note); });
    renderNoteDetailPage();
}

function renderNoteDetailPage() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var ancestorsEl = document.getElementById('note-detail-ancestors');
    var subjectWrap = document.getElementById('note-detail-subject-wrap');
    var repliesEl = document.getElementById('note-detail-replies');
    var replyContent = document.getElementById('note-detail-reply-content');
    if (!ancestorsEl || !subjectWrap || !repliesEl) return;

    ancestorsEl.innerHTML = '';
    var visibleAncestors = (state.noteDetailAncestors || []).filter(function(n) { return !isNoteMuted(n); });
    if (visibleAncestors.length) {
        visibleAncestors.forEach(function(note, i) {
            var replyToPubkey = getReplyToPubkey(note);
            var card = createNoteCard(note, 'ancestor-' + i, 'note-detail-ancestor-', replyToPubkey);
            ancestorsEl.appendChild(card);
        });
        ensureProfilesForNotes(visibleAncestors);
        resolveNostrEmbeds(ancestorsEl);
    }

    subjectWrap.innerHTML = '';
    if (state.noteDetailSubject) {
        var sub = state.noteDetailSubject;
        if (isNoteMuted(sub)) {
            subjectWrap.innerHTML = '<div class="placeholder-message note-detail-muted"><p>' + escapeHtml(t('feed.mutedContent') || 'This note is from a muted account or contains muted content.') + '</p></div>';
        } else {
            var replyToPubkey = getReplyToPubkey(sub);
            var card = createNoteCard(sub, 0, 'note-detail-subject-', replyToPubkey);
            card.classList.add('note-detail-subject-card');
            subjectWrap.appendChild(card);
            ensureProfilesForNotes([sub]);
            resolveNostrEmbeds(subjectWrap);
        }
    }

    if (replyContent) replyContent.value = '';

    repliesEl.innerHTML = '';
    if (state.noteDetailReplies.length) {
        state.noteDetailReplies.forEach(function(item, i) {
            var wrap = document.createElement('div');
            wrap.className = 'note-detail-reply-item';
            wrap.setAttribute('data-indent', Math.min(5, item.indent));
            var replyToPubkey = getReplyToPubkey(item.note);
            var card = createNoteCard(item.note, i, 'note-detail-reply-', replyToPubkey);
            wrap.appendChild(card);
            repliesEl.appendChild(wrap);
        });
        ensureProfilesForNotes(state.noteDetailReplies.map(function(x) { return x.note; }));
        resolveNostrEmbeds(repliesEl);
    } else {
        repliesEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('noteDetail.noReplies')) + '</p></div>';
    }
}

// Switch to a different view
function switchView(viewName) {
    console.log('[Plume] switchView(' + viewName + ') called');
    const viewEl = document.getElementById('view-' + viewName);
    if (!viewEl) {
        console.warn('[Plume] switchView: element #view-' + viewName + ' not found');
        return;
    }

    document.querySelectorAll('.nav-item').forEach(item => {
        item.classList.remove('active');
        if (item.dataset.view === viewName) {
            item.classList.add('active');
        }
        // Welcome view highlights the profile icon in sidebar
        if (viewName === 'welcome' && item.dataset.view === 'profile') {
            item.classList.add('active');
        }
    });

    document.querySelectorAll('.view').forEach(view => {
        view.classList.remove('active');
    });
    viewEl.classList.add('active');
    state.currentView = viewName;

    if (viewName === 'profile') {
        updateProfileDisplay(); // Paint requested user (or placeholder) immediately
        var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
        if (viewingOwn) {
            state.profileNotes = [];
            state.profileNotesForPubkey = null;
            var feedEl = document.getElementById('profile-feed');
            if (feedEl) {
                var msg = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.notesAppearHere') : 'Notes appear here');
                feedEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(msg) + '</p></div>';
            }
        }
        fetchProfile(); // Fetch metadata and notes in background (no await)
        if (viewingOwn) {
            fetchFollowingAndFollowers();
        }
    }
    if (viewName === 'messages') {
        state.unreadMessageCount = 0;
        updateMessagesNavUnread();
        loadMessagesView();
    }
    if (viewName === 'bookmarks') {
        loadBookmarksView();
    }
    if (viewName === 'settings') {
        var panel = state.settingsPanelRequested || null;
        state.settingsPanelRequested = null;
        showSettingsPanel(panel);
    }
    // When switching to Home, always re-display existing notes so they're visible immediately,
    // then request incremental updates for any new notes since the user was away.
    if (viewName === 'feed') {
        if (state.notes.length > 0) {
            displayNotes(state.notes);
        }
        if (state.initialFeedLoadDone) {
            if (state.homeFeedMode === 'firehose') {
                fetchNotesFirehoseOnHomeClick();
            } else {
                pollForNewNotes();
            }
        } else {
            // Feed mode was changed or this is first load — fetch from scratch
            updateFeedInitialState();
            startInitialFeedFetch();
        }
    }
}

// ============================================================
// Modal Management
// ============================================================

// Open settings modal (for Account/Keys – still used for keys)
function openSettings() {
    clearValidationErrors();
    document.getElementById('settings-modal').classList.add('active');
}

// Close settings modal
function closeSettings() {
    document.getElementById('settings-modal').classList.remove('active');
}

// Show a settings panel by key. key null = show default placeholder.
// Populate the settings profile edit form from the best available source:
// relay-fetched profile first, then local config as fallback.
function populateProfilePanel() {
    var cfg = state.config || {};
    var profile = state.profile || {};
    var el;
    el = document.getElementById('edit-profile-name');
    if (el) el.value = profile.name || cfg.name || '';
    el = document.getElementById('edit-profile-nip05');
    if (el) el.value = profile.nip05 || cfg.nip05 || '';
    el = document.getElementById('edit-profile-website');
    if (el) el.value = profile.website || cfg.website || '';
    el = document.getElementById('edit-profile-about');
    if (el) el.value = profile.about || cfg.about || '';
    el = document.getElementById('edit-profile-lud16');
    if (el) el.value = profile.lud16 || cfg.lud16 || '';
    el = document.getElementById('edit-profile-picture');
    if (el) el.value = profile.picture || cfg.picture || '';
    el = document.getElementById('edit-profile-banner');
    if (el) el.value = profile.banner || cfg.banner || '';
}

function showSettingsPanel(key) {
    var detail = document.getElementById('settings-detail');
    if (!detail) return;
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
        if (firehoseRadio) firehoseRadio.checked = (mode === 'firehose');
        if (followsRadio) followsRadio.checked = (mode === 'follows');
    }
    if (key === 'media') {
        var urlEl = document.getElementById('settings-media-server-url');
        if (urlEl) urlEl.value = (state.config && state.config.media_server_url) || 'https://blossom.primal.net';
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
        if (amountEl) amountEl.value = (state.config && state.config.default_zap_amount != null) ? state.config.default_zap_amount : 42;
    }
}

// Populate Keys panel with npub (nsec is NEVER placed in the DOM)
async function populateKeysPanel() {
    var npubEl = document.getElementById('settings-keys-npub');
    var nsecEl = document.getElementById('settings-keys-nsec');
    if (!npubEl || !nsecEl) return;
    npubEl.value = '';
    nsecEl.value = '';
    nsecEl.placeholder = state.config && state.config.private_key
        ? (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('accountModal.privateKeyConfigured') || 'Private key configured (hidden)' : 'Private key configured (hidden)')
        : (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('accountModal.privateKeyPlaceholder') || 'nsec1... or hex (optional)' : 'nsec1... or hex (optional)');
    var npubError = document.getElementById('settings-keys-npub-error');
    var nsecError = document.getElementById('settings-keys-nsec-error');
    if (npubError) npubError.textContent = '';
    if (nsecError) nsecError.textContent = '';
    // Show/hide copy nsec button based on whether key exists
    var copyNsecBtn = document.getElementById('settings-keys-copy-nsec');
    if (copyNsecBtn) copyNsecBtn.style.display = (state.config && state.config.private_key) ? 'inline-block' : 'none';
    if (!state.config) return;
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
async function copyNsecToClipboard() {
    if (!state.config || !state.config.private_key) return;
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
async function saveKeysPanel(event) {
    if (event) event.preventDefault();
    var npubEl = document.getElementById('settings-keys-npub');
    var nsecEl = document.getElementById('settings-keys-nsec');
    var npubError = document.getElementById('settings-keys-npub-error');
    var nsecError = document.getElementById('settings-keys-nsec-error');
    if (!npubEl || !state.config) return;
    if (npubError) npubError.textContent = '';
    if (nsecError) nsecError.textContent = '';
    var publicKeyHex = null;
    var privateKeyHex = null;
    var npubRaw = (npubEl && npubEl.value) ? npubEl.value.trim() : '';
    if (!npubRaw) {
        if (npubError) npubError.textContent = 'Public key is required';
        return;
    }
    var pubResult = await validatePublicKey(npubRaw);
    if (!pubResult.valid) {
        if (npubError) npubError.textContent = pubResult.error || 'Invalid public key';
        return;
    }
    publicKeyHex = pubResult.hex;
    if (nsecEl && nsecEl.value.trim()) {
        var privResult = await validateSecretKey(nsecEl.value.trim());
        if (!privResult.valid) {
            if (nsecError) nsecError.textContent = privResult.error || 'Invalid private key';
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
        updateUIFromConfig();
    } finally {
        restoreBtn();
    }
}

// Save Home feed mode from settings panel
function saveHomeFeedModeFromPanel() {
    var followsRadio = document.getElementById('home-feed-follows');
    var mode = (followsRadio && followsRadio.checked) ? 'follows' : 'firehose';
    if (!state.config) state.config = {};
    state.config.home_feed_mode = mode;
    state.homeFeedMode = mode;
    var restoreBtn = setSavingState(document.getElementById('home-feed-panel-save'));
    saveConfig().then(function() {
        // Clear existing feed state so the next visit to feed reloads with the new mode
        state.initialFeedLoadDone = false;
        state.notes = [];
        if (state.feedPollIntervalId) { clearInterval(state.feedPollIntervalId); state.feedPollIntervalId = null; }
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
function saveZapsFromPanel() {
    var amountEl = document.getElementById('settings-zaps-default-amount');
    if (!state.config || !amountEl) return;
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
function saveMediaServerFromPanel() {
    var urlEl = document.getElementById('settings-media-server-url');
    if (!state.config || !urlEl) return;
    state.config.media_server_url = (urlEl.value && urlEl.value.trim()) || 'https://blossom.primal.net';
    var restoreBtn = setSavingState(document.getElementById('settings-media-save'));
    saveConfig()
        .catch(function(err) { console.error('Failed to save media server URL:', err); })
        .finally(restoreBtn);
}

// Muted: ensure config arrays exist
function ensureMutedConfig() {
    if (!state.config) return;
    if (!Array.isArray(state.config.muted_users)) state.config.muted_users = [];
    if (!Array.isArray(state.config.muted_words)) state.config.muted_words = [];
    if (!Array.isArray(state.config.muted_hashtags)) state.config.muted_hashtags = [];
}

function isUserMuted(pubkey) {
    if (!pubkey || !state.config || !Array.isArray(state.config.muted_users)) return false;
    var pk = String(pubkey).toLowerCase();
    return state.config.muted_users.some(function(p) { return String(p).toLowerCase() === pk; });
}

// True if the note should be hidden by mute filters (muted user, muted word in content, or muted hashtag in tags).
function isNoteMuted(note) {
    if (!note || !state.config) return false;
    ensureMutedConfig();
    var pubkey = (note.pubkey || '').toLowerCase();
    if (state.config.muted_users.some(function(p) { return String(p).toLowerCase() === pubkey; })) return true;
    if (note.kind === 1) {
        var content = (note.content || '').toLowerCase();
        var words = state.config.muted_words || [];
        for (var w = 0; w < words.length; w++) {
            if (content.indexOf(String(words[w]).toLowerCase()) !== -1) return true;
        }
        var tags = note.tags || [];
        var mutedHashtags = (state.config.muted_hashtags || []).map(function(h) { return String(h).toLowerCase().replace(/^#/, ''); });
        for (var t = 0; t < tags.length; t++) {
            var tag = tags[t];
            if (Array.isArray(tag) && tag[0] === 't' && tag[1]) {
                var tagVal = String(tag[1]).toLowerCase().replace(/^#/, '');
                if (mutedHashtags.indexOf(tagVal) !== -1) return true;
            }
        }
    }
    return false;
}

// Mute or unmute the currently viewed profile user. Updates local config immediately and saves to disk.
function handleMuteClick() {
    var pubkey = state.viewedProfilePubkey;
    if (!pubkey || !state.config) return;
    var muteBtn = document.getElementById('mute-btn');
    if (muteBtn) muteBtn.disabled = true;
    ensureMutedConfig();
    var pk = String(pubkey).toLowerCase();
    var idx = state.config.muted_users.findIndex(function(p) { return String(p).toLowerCase() === pk; });
    if (idx === -1) {
        state.config.muted_users.push(pubkey);
    } else {
        state.config.muted_users.splice(idx, 1);
    }
    saveConfig()
        .then(function() {
            updateProfileDisplay();
        })
        .catch(function(err) {
            console.error('Failed to save mute:', err);
            alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToSaveSettings') : 'Failed to save') + ': ' + err);
        })
        .finally(function() {
            if (muteBtn) muteBtn.disabled = false;
        });
}

async function loadMutedPanel() {
    ensureMutedConfig();
    var users = state.config.muted_users || [];
    state.mutedUsersPanelList = users.map(function(pubkey) { return { pubkey: pubkey, checked: true }; });
    var ulUsers = document.getElementById('muted-users-list');
    if (ulUsers) {
        ulUsers.innerHTML = '<li class="follows-list-placeholder">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('settings.followsLoading') : 'Loading…') + '</li>';
    }
    if (users.length > 0) {
        var notes = users.map(function(p) { return { pubkey: p }; });
        await ensureProfilesForNotes(notes);
    }
    renderMutedPanels();
}

function renderMutedPanels() {
    ensureMutedConfig();
    var words = state.config.muted_words || [];
    var hashtags = state.config.muted_hashtags || [];
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var ulUsers = document.getElementById('muted-users-list');
    var ulWords = document.getElementById('muted-words-list');
    var ulHashtags = document.getElementById('muted-hashtags-list');

    if (ulUsers && state.mutedUsersPanelList) {
        var list = state.mutedUsersPanelList;
        ulUsers.className = 'follows-list';
        if (list.length === 0) {
            ulUsers.innerHTML = '<li class="follows-list-placeholder">' + escapeHtml(t('settings.mutedUsersEmpty') || 'No muted users') + '</li>';
        } else {
            ulUsers.innerHTML = '';
            list.forEach(function(item) {
                var profile = state.profileCache[item.pubkey];
                var name = (profile && profile.name) ? profile.name : shortenKey(item.pubkey);
                var nip05 = (profile && profile.nip05) ? profile.nip05 : '';
                var picture = (profile && profile.picture) ? profile.picture : null;
                var li = document.createElement('li');
                li.className = 'follows-list-item';
                li.dataset.pubkey = item.pubkey;
                var imgHtml = picture
                    ? '<img src="' + escapeHtml(picture) + '" alt="" class="follows-item-avatar">'
                    : '';
                li.innerHTML = '<label class="follows-item-row">' +
                    '<input type="checkbox" class="follows-item-checkbox muted-user-checkbox" ' + (item.checked ? 'checked' : '') + ' data-pubkey="' + escapeHtml(item.pubkey) + '">' +
                    '<span class="follows-item-avatar-wrap">' + imgHtml + '<span class="follows-item-avatar-fallback" style="' + (picture ? 'display:none' : '') + '">' + (name ? name.charAt(0).toUpperCase() : '?') + '</span></span>' +
                    '<span class="follows-item-info">' +
                    '<span class="follows-item-name">' + escapeHtml(name) + '</span>' +
                    (nip05 ? '<span class="follows-item-nip05">' + escapeHtml(nip05) + '</span>' : '') +
                    '</span></label>';
                // Attach image error handler without inline JS (CSP-safe)
                var img = li.querySelector('.follows-item-avatar');
                if (img) img.addEventListener('error', function() { this.style.display = 'none'; this.nextElementSibling.style.display = 'inline-flex'; });
                ulUsers.appendChild(li);
            });
        }
        ulUsers.querySelectorAll('.muted-user-checkbox').forEach(function(cb) {
            cb.addEventListener('change', function() {
                var pubkey = cb.dataset.pubkey;
                var item = state.mutedUsersPanelList.find(function(x) { return x.pubkey === pubkey; });
                if (item) item.checked = cb.checked;
            });
        });
    }

    if (ulWords) {
        ulWords.innerHTML = words.length ? words.map(function(w) {
            return '<li data-word="' + escapeHtml(w) + '"><span>' + escapeHtml(w) + '</span><button type="button" class="muted-item-remove">' + escapeHtml(t('app.close') || 'Remove') + '</button></li>';
        }).join('') : '';
    }
    if (ulHashtags) {
        ulHashtags.innerHTML = hashtags.length ? hashtags.map(function(h) {
            return '<li data-hashtag="' + escapeHtml(h) + '"><span>#' + escapeHtml(h) + '</span><button type="button" class="muted-item-remove">' + escapeHtml(t('app.close') || 'Remove') + '</button></li>';
        }).join('') : '';
    }
}

function saveMutedFromPanel() {
    ensureMutedConfig();
    if (state.mutedUsersPanelList) {
        state.config.muted_users = state.mutedUsersPanelList.filter(function(x) { return x.checked; }).map(function(x) { return x.pubkey; });
    }
    var restoreBtn = setSavingState(document.getElementById('settings-muted-save'));
    saveConfig().then(function() {
        var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        alert(t('settings.mutedSaved') || 'Muted lists saved.');
    }).catch(function(err) { console.error('Failed to save muted lists:', err); })
    .finally(restoreBtn);
}

// ============================================================
// Follows settings panel
// ============================================================

async function loadFollowsPanel() {
    var listEl = document.getElementById('follows-list');
    var addInput = document.getElementById('follows-add-input');
    if (!listEl) return;
    state.followsPanelLoading = true;
    listEl.innerHTML = '<li class="follows-list-placeholder">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('settings.followsLoading') || 'Loading…' : 'Loading…') + '</li>';
    if (addInput) addInput.value = '';
    try {
        var data = await fetchFollowing();
        var contacts = (data && data.contacts) ? data.contacts : [];
        var notes = contacts.map(function(c) { return { pubkey: c.pubkey }; });
        await ensureProfilesForNotes(notes);
        state.followsPanelList = contacts.map(function(c, i) {
            return { pubkey: c.pubkey, checked: true, listOrder: i };
        });
        state.followsPanelSort = getFollowsPanelSort();
        renderFollowsPanel();
    } catch (e) {
        console.error('Failed to load follows:', e);
        listEl.innerHTML = '<li class="follows-list-placeholder">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.loadFailed') : 'Failed to load') + '</li>';
    } finally {
        state.followsPanelLoading = false;
    }
}

function renderFollowsPanel() {
    var listEl = document.getElementById('follows-list');
    if (!listEl) return;
    var list = state.followsPanelList || [];
    var sort = state.followsPanelSort || 'name';
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    var sorted = list.slice().sort(function(a, b) {
        if (sort === 'order') return (a.listOrder !== undefined ? a.listOrder : 0) - (b.listOrder !== undefined ? b.listOrder : 0);
        var ad = getAuthorDisplay(a.pubkey);
        var bd = getAuthorDisplay(b.pubkey);
        if (sort === 'name') {
            var an = (ad.name || '').toLowerCase();
            var bn = (bd.name || '').toLowerCase();
            return an.localeCompare(bn);
        }
        if (sort === 'nip05') {
            var anip = (ad.nip05 || '').toLowerCase();
            var bnip = (bd.nip05 || '').toLowerCase();
            return anip.localeCompare(bnip);
        }
        return 0;
    });

    if (sorted.length === 0) {
        listEl.innerHTML = '<li class="follows-list-placeholder">' + escapeHtml(t('settings.followsEmpty') || 'No follows yet') + '</li>';
        return;
    }

    listEl.innerHTML = '';
    sorted.forEach(function(item) {
        var profile = state.profileCache[item.pubkey];
        var name = (profile && profile.name) ? profile.name : shortenKey(item.pubkey);
        var nip05 = (profile && profile.nip05) ? profile.nip05 : '';
        var picture = (profile && profile.picture) ? profile.picture : null;
        var li = document.createElement('li');
        li.className = 'follows-list-item';
        li.dataset.pubkey = item.pubkey;
        var imgHtml = picture
                    ? '<img src="' + escapeHtml(picture) + '" alt="" class="follows-item-avatar">'
                    : '';
        li.innerHTML = '<label class="follows-item-row">' +
            '<input type="checkbox" class="follows-item-checkbox" ' + (item.checked ? 'checked' : '') + ' data-pubkey="' + escapeHtml(item.pubkey) + '">' +
            '<span class="follows-item-avatar-wrap">' + imgHtml + '<span class="follows-item-avatar-fallback" style="' + (picture ? 'display:none' : '') + '">' + (name ? name.charAt(0).toUpperCase() : '?') + '</span></span>' +
            '<span class="follows-item-info">' +
            '<span class="follows-item-name">' + escapeHtml(name) + '</span>' +
            (nip05 ? '<span class="follows-item-nip05">' + escapeHtml(nip05) + '</span>' : '') +
            '</span></label>';
        listEl.appendChild(li);
        // Attach image error handler without inline JS (CSP-safe)
        var img = li.querySelector('.follows-item-avatar');
        if (img) img.addEventListener('error', function() { this.style.display = 'none'; this.nextElementSibling.style.display = 'inline-flex'; });
    });

    listEl.querySelectorAll('.follows-item-checkbox').forEach(function(cb) {
        cb.addEventListener('change', function() {
            var pubkey = cb.dataset.pubkey;
            var item = state.followsPanelList.find(function(x) { return x.pubkey === pubkey; });
            if (item) item.checked = cb.checked;
        });
    });
}

function getFollowsPanelSort() {
    var btn = document.querySelector('.follows-sort-btn.active');
    return (btn && btn.dataset.followsSort) ? btn.dataset.followsSort : 'name';
}

function saveFollowsPanel() {
    var pubkeys = (state.followsPanelList || []).filter(function(x) { return x.checked; }).map(function(x) { return x.pubkey; });
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var restoreBtn = setSavingState(document.getElementById('settings-follows-save'));
    invoke('set_contact_list', { pubkeys: pubkeys })
        .then(function() {
            state.ownFollowingPubkeys = pubkeys;
            // Keep local config in sync so follows-mode feed works immediately
            if (state.config) state.config.following = pubkeys.slice();
            var msg = t('settings.followsSaved') || 'Follow list saved and published.';
            alert(msg);
            loadFollowsPanel();
        })
        .catch(function(err) {
            console.error('Failed to save follows:', err);
            alert((t('errors.failedToPublish') || 'Failed to publish') + ': ' + err);
        })
        .finally(restoreBtn);
}

// Clear validation error displays
function clearValidationErrors() {
    document.querySelectorAll('.validation-error').forEach(el => {
        el.textContent = '';
        el.style.display = 'none';
    });
    document.querySelectorAll('.form-group input').forEach(el => {
        el.classList.remove('invalid');
    });
}

// Show validation error for an input
function showValidationError(inputId, message) {
    const input = document.getElementById(inputId);
    const errorEl = document.getElementById(inputId + '-error');
    
    if (input) {
        input.classList.add('invalid');
    }
    if (errorEl) {
        errorEl.textContent = message;
        errorEl.style.display = 'block';
    }
}

// ============================================================
// Relay Management
// ============================================================

// Update the relay list in the UI (with delete button per relay)
function updateRelayList() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const relayList = document.getElementById('relay-list');
    if (!relayList) return;
    relayList.innerHTML = '';

    if (!state.config) state.config = {};
    if (!Array.isArray(state.config.relays)) state.config.relays = [];

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
function bindRelayPanelHandlers() {
    var list = document.getElementById('relay-list');
    var addInput = document.getElementById('relay-add-input');
    var addBtn = document.getElementById('relay-add-btn');
    var saveBtn = document.getElementById('settings-relays-save');
    if (!list) return;

    list.removeEventListener('click', handleRelayListClick);
    list.addEventListener('click', handleRelayListClick);

    if (addBtn) {
        addBtn.onclick = function() {
            var url = addInput && addInput.value ? addInput.value.trim() : '';
            if (!url) return;
            if (!url.startsWith('ws://') && !url.startsWith('wss://')) url = 'wss://' + url;
            if (!state.config) state.config = {};
            if (!Array.isArray(state.config.relays)) state.config.relays = [];
            if (state.config.relays.indexOf(url) !== -1) return;
            state.config.relays.push(url);
            updateRelayList();
            bindRelayPanelHandlers();
            runRelayTests();
            if (addInput) addInput.value = '';
        };
    }
    if (addInput && addInput.addEventListener) {
        addInput.onkeydown = function(e) {
            if (e.key === 'Enter') {
                e.preventDefault();
                if (addBtn) addBtn.click();
            }
        };
    }
    if (saveBtn) {
        saveBtn.onclick = function() {
            saveConfig().catch(function(err) { console.error('Failed to save relays:', err); });
        };
    }
}

function handleRelayListClick(e) {
    var target = e.target;
    if (target.classList && target.classList.contains('relay-delete-btn')) {
        var idx = parseInt(target.getAttribute('data-index'), 10);
        if (!state.config || !Array.isArray(state.config.relays) || isNaN(idx) || idx < 0 || idx >= state.config.relays.length) return;
        state.config.relays.splice(idx, 1);
        updateRelayList();
        bindRelayPanelHandlers();
        runRelayTests();
        return;
    }
}

// Test all relays asynchronously when the relay list panel is visible; update status dots (grey=unknown, green=ok, red=failed)
function runRelayTests() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var connectedTitle = t('relays.statusConnected');
    var failedTitle = t('relays.statusFailed');
    if (!state.config || !Array.isArray(state.config.relays)) return;
    state.config.relays.forEach(function(relayUrl, index) {
        var el = document.getElementById('relay-status-' + index);
        if (!el) return;
        el.classList.remove('connected', 'failed');
        el.title = t('relays.statusUnknown');
        el.setAttribute('aria-label', t('relays.statusUnknown'));
        invoke('test_relay_connection', { relayUrl: relayUrl })
            .then(function(result) {
                if (!el.parentNode) return;
                el.classList.remove('failed');
                el.classList.add('connected');
                el.title = connectedTitle;
                el.setAttribute('aria-label', connectedTitle);
            })
            .catch(function(err) {
                if (!el.parentNode) return;
                el.classList.remove('connected');
                el.classList.add('failed');
                el.title = failedTitle;
                el.setAttribute('aria-label', failedTitle);
            });
    });
}

// ============================================================
// Note Fetching (async, non-blocking; merge/sort; incremental poll)
// ============================================================

const FEED_LIMIT = 50;
const POLL_INTERVAL_MS = 45000;

// Returns list of hex pubkeys for "follows" mode, or null for firehose.
// Uses the locally cached following list from config first for instant results,
// then falls back to fetching from relays (which also updates the local cache).
async function getHomeFeedAuthors() {
    if (state.homeFeedMode !== 'follows') return null;
    if (!state.config || !state.config.public_key) return null;
    // Use locally cached following list if available
    if (state.config.following && state.config.following.length > 0) {
        return state.config.following.slice();
    }
    // Fall back to fetching from relays (also caches locally via backend)
    try {
        const json = await invoke('fetch_own_following');
        if (!json) return null;
        const data = JSON.parse(json);
        const contacts = data.contacts || [];
        if (contacts.length === 0) return null;
        const pubkeys = contacts.map(c => c.pubkey).filter(Boolean);
        // Update local state so subsequent calls are instant
        if (pubkeys.length > 0) {
            state.config.following = pubkeys;
        }
        return pubkeys;
    } catch (e) {
        console.error('Failed to get following for feed:', e);
        return null;
    }
}

// Low-level fetch: relayUrls, optional authors (hex), optional since (unix ts). profileFeed true = include reposts (kind 6) for profile.
async function fetchFeedNotes(relayUrls, authors, since, profileFeed) {
    if (!relayUrls || relayUrls.length === 0) return [];
    const notesJson = await invoke('fetch_notes_from_relays', {
        relay_urls: relayUrls,
        limit: FEED_LIMIT,
        authors: authors && authors.length ? authors : null,
        since: since ?? null,
        profile_feed: profileFeed === true ? true : null
    });
    if (!notesJson) return [];
    const notes = JSON.parse(notesJson);
    return Array.isArray(notes) ? notes : [];
}

// Merge new notes into state.notes. isIncremental: true = append new ones below the fold; false = replace and sort.
function mergeNotesIntoState(newNotes, isIncremental) {
    if (!newNotes || newNotes.length === 0 && !isIncremental) return;
    const seen = new Set(state.notes.map(n => n.id));
    if (!isIncremental) {
        state.notes = newNotes.slice();
        state.notes.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
        return;
    }
    const added = newNotes.filter(n => n.id && !seen.has(n.id));
    if (added.length === 0) return;
    added.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    state.notes = state.notes.concat(added);
}

// Start initial feed fetch: async stream (each note shown as it arrives) when in Tauri; else batch fetch.
async function startInitialFeedFetch() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    updateFeedInitialState();
    const effectiveRelays = getEffectiveRelays();
    if (effectiveRelays.length === 0) {
        return;
    }

    const useStream = window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function';

    if (useStream) {
        // Stream mode: backend emits "feed-note" per note and "feed-eose" when done. UI stays responsive.
        state.loading = true;
        feedStreamNoteIndex = 0;
        state.notes = [];

        let unlistenNote = function() {};
        let unlistenEose = function() {};

        try {
            unlistenNote = await window.__TAURI__.event.listen('feed-note', function(event) {
                const payload = event.payload;
                const note = typeof payload === 'string' ? JSON.parse(payload) : payload;
                appendNoteCardToFeed(note);
            });
            unlistenEose = await window.__TAURI__.event.listen('feed-eose', function() {
                state.loading = false;
                state.initialFeedLoadDone = true;
                // Start periodic polling for new notes (both firehose and follows modes)
                if (state.feedPollIntervalId) clearInterval(state.feedPollIntervalId);
                if (state.homeFeedMode === 'follows') {
                    getHomeFeedAuthors().then(function(authors) {
                        if (authors && authors.length > 0) {
                            state.feedPollIntervalId = setInterval(pollForNewNotes, POLL_INTERVAL_MS);
                        }
                    });
                } else {
                    state.feedPollIntervalId = setInterval(function() { fetchNotesFirehoseOnHomeClick(); }, POLL_INTERVAL_MS);
                }
                unlistenNote();
                unlistenEose();
                // If no notes arrived, show empty state
                if (state.notes.length === 0) {
                    const container = document.getElementById('notes-container');
                    if (container) {
                        container.innerHTML = `
                            <div class="placeholder-message">
                                <p>${escapeHtml(t('feed.noTextNotes'))}</p>
                                <p>${escapeHtml(t('feed.tryRelays'))}</p>
                            </div>
                        `;
                    }
                }
            });

            let authors = null;
            if (state.homeFeedMode === 'follows') {
                authors = await getHomeFeedAuthors();
                if (!authors || authors.length === 0) authors = null;
            }
            await invoke('start_feed_stream', {
                relay_urls: effectiveRelays,
                limit: FEED_LIMIT,
                authors: authors,
                since: null
            });
        } catch (error) {
            console.error('Feed stream failed:', error);
            state.loading = false;
            unlistenNote();
            unlistenEose();
            showMessage(t('feed.feedFailed'));
        }
        return;
    }

    // Fallback: batch fetch (e.g. in browser or no event API)
    state.loading = true;
    try {
        let authors = null;
        if (state.homeFeedMode === 'follows') {
            authors = await getHomeFeedAuthors();
            if (!authors || authors.length === 0) authors = null;
        }
        const notes = await fetchFeedNotes(effectiveRelays, authors, null);
        mergeNotesIntoState(notes, false);
        displayNotes(state.notes);
        state.initialFeedLoadDone = true;
        // Start periodic polling for new notes (both firehose and follows modes)
        if (state.feedPollIntervalId) clearInterval(state.feedPollIntervalId);
        if (state.homeFeedMode === 'follows' && authors && authors.length > 0) {
            state.feedPollIntervalId = setInterval(pollForNewNotes, POLL_INTERVAL_MS);
        } else if (state.homeFeedMode === 'firehose') {
            state.feedPollIntervalId = setInterval(function() { fetchNotesFirehoseOnHomeClick(); }, POLL_INTERVAL_MS);
        }
    } catch (error) {
        console.error('Initial feed fetch failed:', error);
        showMessage(t('feed.feedFailed'));
    } finally {
        state.loading = false;
    }
}

// Incremental poll (follows mode only). Fetches notes since latest we have; appends below the fold.
async function pollForNewNotes() {
    const relays = getEffectiveRelays();
    if (!relays.length || state.loading) return;
    const authors = await getHomeFeedAuthors();
    if (!authors || authors.length === 0) return;
    const since = state.notes.length
        ? Math.max(...state.notes.map(n => n.created_at || 0))
        : 0;
    try {
        const notes = await fetchFeedNotes(relays, authors, since);
        if (notes.length === 0) return;
        mergeNotesIntoState(notes, true);
        displayNotes(state.notes);
    } catch (e) {
        console.error('Feed poll failed:', e);
    }
}

// Firehose: fetch new notes when user opens Home (no auto-poll).
async function fetchNotesFirehoseOnHomeClick() {
    const relays = getEffectiveRelays();
    if (!relays.length || state.loading) return;
    const since = state.notes.length
        ? Math.max(...state.notes.map(n => n.created_at || 0))
        : 0;
    state.loading = true;
    try {
        const notes = await fetchFeedNotes(relays, null, since);
        if (notes.length > 0) {
            mergeNotesIntoState(notes, true);
            displayNotes(state.notes);
        }
    } catch (e) {
        console.error('Firehose fetch failed:', e);
    } finally {
        state.loading = false;
    }
}

// Show a message in the notes container
function showMessage(message) {
    const container = document.getElementById('notes-container');
    container.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Next note index for streamed cards (verification badge id, etc.)
let feedStreamNoteIndex = 0;

// Queue of notes waiting to be inserted (so we can drain one-per-frame and keep UI responsive).
var feedNoteQueue = [];
var feedNoteDrainScheduled = false;

function scheduleFeedNoteDrain() {
    if (feedNoteDrainScheduled) return;
    feedNoteDrainScheduled = true;
    requestAnimationFrame(function drainFeedNoteQueue() {
        feedNoteDrainScheduled = false;
        if (feedNoteQueue.length === 0) return;
        var note = feedNoteQueue.shift();
        var result = appendNoteCardToFeedSync(note);
        if (result.index !== -1) {
            ensureProfilesForNotes([note]);
            verifyNote(note, result.index);
            if (result.card) resolveNostrEmbeds(result.card);
        }
        if (feedNoteQueue.length > 0) scheduleFeedNoteDrain();
    });
}

// Append a single note card to the feed (streaming). Dedupes by id; inserts in sorted position.
// Returns { index, card } where index is -1 if skipped.
function appendNoteCardToFeedSync(note) {
    if (!note || note.kind !== 1) return { index: -1, card: null };
    if (isNoteMuted(note)) return { index: -1, card: null };
    if (state.notes.some(function(n) { return n.id === note.id; })) return { index: -1, card: null };

    const container = document.getElementById('notes-container');
    if (!container) return { index: -1, card: null };

    state.notes.push(note);
    state.notes.sort(function(a, b) { return (b.created_at || 0) - (a.created_at || 0); });
    const idx = state.notes.findIndex(function(n) { return n.id === note.id; });

    const placeholder = document.getElementById('feed-loading') || document.getElementById('feed-welcome');
    if (placeholder) placeholder.remove();

    const noteIndex = feedStreamNoteIndex++;
    const replyToPubkey = getReplyToPubkey(note);
    const card = createNoteCard(note, noteIndex, '', replyToPubkey);
    if (idx === 0) {
        container.insertBefore(card, container.firstChild);
    } else if (idx >= container.children.length) {
        container.appendChild(card);
    } else {
        container.insertBefore(card, container.children[idx]);
    }
    return { index: noteIndex, card: card };
}

function appendNoteCardToFeed(note) {
    if (!note || note.kind !== 1) return;
    if (state.notes.some(function(n) { return n.id === note.id; })) return;
    feedNoteQueue.push(note);
    scheduleFeedNoteDrain();
}

// ============================================================
// Note Display
// ============================================================

// Get display name and NIP-05 for a pubkey from profile cache (NIP-05 is the verified identity, e.g. user@domain.com).
function getAuthorDisplay(pubkey) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const c = state.profileCache[pubkey];
    if (c) {
        return { name: c.name || t('profile.anonymous'), nip05: c.nip05 || '' };
    }
    return { name: '…', nip05: '' };
}

// Fetch profiles for note authors (and reply-to targets) and update cache + DOM.
async function ensureProfilesForNotes(notes) {
    const relays = getEffectiveRelays();
    if (relays.length === 0) return;
    var pubkeys = notes.map(n => n.pubkey).filter(Boolean);
    notes.forEach(function(n) {
        var p = getReplyToPubkey(n);
        if (p) pubkeys.push(p);
        if (n.kind === 6 && n.content && n.content.trim()) {
            try {
                var parsed = JSON.parse(n.content);
                if (parsed && parsed.pubkey) pubkeys.push(parsed.pubkey);
            } catch (_) {}
        }
    });
    const unique = [...new Set(pubkeys)];
    const toFetch = unique.filter(p => !state.profileCache[p]);
    if (toFetch.length === 0) return;
    await Promise.all(toFetch.map(async (pubkey) => {
        try {
            const json = await invoke('fetch_profile', { pubkey, relay_urls: relays });
            if (!json || json === '{}') return;
            const p = JSON.parse(json);
            state.profileCache[pubkey] = {
                name: p.name || null,
                nip05: p.nip05 || null,
                picture: p.picture || null,
                lud16: p.lud16 || null
            };
        } catch (_) { /* ignore */ }
    }));
    // Update cards for all authors that we have in cache (including just-seeded viewed profile)
    unique.forEach(function(pubkey) {
        var profile = state.profileCache[pubkey];
        if (!profile) return;
        var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        var name = profile.name || t('profile.anonymous');
        var nip05 = profile.nip05 || '';
        document.querySelectorAll('.note-card[data-pubkey="' + escapeCssAttr(pubkey) + '"]').forEach(function(card) {
            var isRepost = card.classList.contains('note-card-repost');
            if (isRepost) {
                var reposterNameEl = card.querySelector('.note-repost-header .note-reposter-name');
                if (reposterNameEl) reposterNameEl.textContent = name;
            } else {
                var nameEl = card.querySelector('.note-head .note-author-name');
                var nip05El = card.querySelector('.note-head .note-author-nip05');
                if (nameEl) nameEl.textContent = name;
                if (nip05El) {
                    nip05El.textContent = nip05;
                    nip05El.style.display = nip05 ? '' : 'none';
                }
                var avatar = card.querySelector('.note-avatar');
                if (avatar && profile.picture) setCardAvatar(card, profile.picture);
            }
            var replyToLink = card.querySelector('.note-reply-to-link[data-pubkey="' + escapeCssAttr(pubkey) + '"]');
            if (replyToLink) replyToLink.textContent = name;
        });
        updateZapButtons();
        document.querySelectorAll('.note-card[data-original-pubkey="' + escapeCssAttr(pubkey) + '"]').forEach(function(card) {
            var nameEl = card.querySelector('.note-original-row .note-author-name');
            var nip05El = card.querySelector('.note-original-row .note-author-nip05');
            if (nameEl) nameEl.textContent = name;
            if (nip05El) {
                nip05El.textContent = nip05;
                nip05El.style.display = nip05 ? '' : 'none';
            }
            var avatar = card.querySelector('.note-original-row .note-avatar');
            if (avatar && profile.picture) {
                var fallback = avatar.querySelector('.avatar-fallback');
                var img = avatar.querySelector('img');
                if (img) {
                    img.src = profile.picture;
                    img.alt = '';
                    img.style.display = '';
                    if (fallback) fallback.style.display = 'none';
                } else {
                    var newImg = document.createElement('img');
                    newImg.src = profile.picture;
                    newImg.alt = '';
                    newImg.onerror = function() { if (fallback) fallback.style.display = 'flex'; };
                    avatar.insertBefore(newImg, avatar.firstChild);
                    if (fallback) fallback.style.display = 'none';
                }
            }
        });
    });
}

function escapeCssAttr(s) {
    return String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

// NIP-10: get the direct parent event id from a note's "e" tags. Returns null if not a reply.
function getParentEventId(note) {
    if (!note.tags || !note.tags.length) return null;
    for (var i = 0; i < note.tags.length; i++) {
        var tag = note.tags[i];
        if (Array.isArray(tag) && tag[0] === 'e' && tag[1]) {
            var marker = tag[3] || '';
            if (marker === 'reply') return tag[1];
        }
    }
    // Some clients omit the marker; last "e" is often the reply target
    var lastE = null;
    for (var j = 0; j < note.tags.length; j++) {
        var tagItem = note.tags[j];
        if (Array.isArray(tagItem) && tagItem[0] === 'e' && tagItem[1]) lastE = tagItem[1];
    }
    return lastE;
}

// Get the pubkey of the user being replied to (first "p" tag) when note is a reply (has "e" tag). Returns null if not a reply.
function getReplyToPubkey(note) {
    if (!note.tags || !note.tags.length) return null;
    var hasE = note.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
    if (!hasE) return null;
    for (var i = 0; i < note.tags.length; i++) {
        var tag = note.tags[i];
        if (Array.isArray(tag) && tag[0] === 'p' && tag[1]) return tag[1];
    }
    return null;
}

function setCardAvatar(card, pictureUrl) {
    if (!card || !pictureUrl) return;
    var avatar = card.querySelector('.note-avatar');
    if (!avatar) return;
    var fallback = avatar.querySelector('.avatar-fallback');
    var img = avatar.querySelector('img');
    if (img) {
        img.src = pictureUrl;
        img.alt = '';
        img.style.display = '';
        if (fallback) fallback.style.display = 'none';
    } else {
        img = document.createElement('img');
        img.src = pictureUrl;
        img.alt = '';
        img.onerror = function() { if (fallback) fallback.style.display = 'flex'; };
        avatar.insertBefore(img, avatar.firstChild);
        if (fallback) fallback.style.display = 'none';
    }
}

// Default emoji for quick like (heart). Common social/media emojis for long-press picker.
var DEFAULT_LIKE_EMOJI = '❤️';
var LIKE_EMOJI_LIST = ['❤️','🤙','👍','😂','😢','😡','🎉','🔥','👀','💯','❤️‍🔥','😍','🤔','👏','🙏','😭','🤣','💀','✨','💪'];

// Whether the current user has liked this note (we only know from this session).
function isNoteLiked(noteId) {
    return state.likedNoteIds && state.likedNoteIds[noteId];
}

// Whether the current user has a Lightning address (for sending/receiving zaps).
function selfHasLud16() {
    if (state.profile && state.profile.lud16 && state.profile.lud16.trim()) return true;
    if (state.publicKeyHex && state.profileCache[state.publicKeyHex] && state.profileCache[state.publicKeyHex].lud16) return true;
    return false;
}

// Whether the given pubkey's profile has a Lightning address (for zapping them).
function targetHasLud16(pubkey) {
    if (!pubkey || !state.profileCache) return false;
    var p = state.profileCache[pubkey];
    return !!(p && p.lud16 && p.lud16.trim());
}

// Update zap buttons: muted + disabled when self or target lack LUD16.
function updateZapButtons() {
    var selfOk = selfHasLud16();
    document.querySelectorAll('.note-action[data-action="zap"]').forEach(function(btn) {
        var targetPubkey = btn.getAttribute('data-zap-target-pubkey');
        var targetOk = targetPubkey && targetHasLud16(targetPubkey);
        if (selfOk && targetOk) {
            btn.disabled = false;
            btn.classList.remove('zap-muted');
            btn.removeAttribute('title');
        } else {
            btn.disabled = true;
            btn.classList.add('zap-muted');
            btn.setAttribute('title', (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('note.zapNoWallet') : 'Zap requires you and the author to have a Lightning address'));
        }
    });
}

// Request a zap invoice and open it with the user's wallet (lightning: URL).
function performZap(targetPubkey, eventId, zapBtn) {
    if (!targetPubkey || !state.config || !state.profileCache) return;
    var profile = state.profileCache[targetPubkey];
    if (!profile || !profile.lud16 || !profile.lud16.trim()) return;
    var amount = (state.config.default_zap_amount != null && state.config.default_zap_amount >= 1)
        ? state.config.default_zap_amount
        : 42;
    if (zapBtn) zapBtn.disabled = true;
    invoke('request_zap_invoice', {
        target_lud16: profile.lud16.trim(),
        amount_sats: amount,
        event_id: eventId || '',
        target_pubkey: targetPubkey
    })
        .then(function(result) {
            var data = typeof result === 'string' ? JSON.parse(result) : result;
            if (data && data.pr) {
                var url = data.pr.indexOf('ln') === 0 ? 'lightning:' + data.pr : data.pr;
                window.open(url, '_blank');
            }
        })
        .catch(function(err) {
            console.error('Zap failed:', err);
            alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to get zap invoice') + ': ' + err);
        })
        .finally(function() {
            if (zapBtn) zapBtn.disabled = false;
        });
}

// Perform a like (reaction) and update UI on success
function performLike(noteId, pubkey, emoji, likeBtn) {
    if (!noteId || !pubkey) return;
    var btn = likeBtn;
    if (btn) btn.disabled = true;
    invoke('post_reaction', { eventId: noteId, authorPubkey: pubkey, emoji: emoji || DEFAULT_LIKE_EMOJI })
        .then(function() {
            if (!state.likedNoteIds) state.likedNoteIds = {};
            state.likedNoteIds[noteId] = true;
            if (btn) {
                var img = btn.querySelector('img');
                if (img) img.src = 'icons/heart-filled.svg';
                btn.classList.add('liked');
            }
        })
        .catch(function(err) {
            console.error('Reaction failed:', err);
            alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to publish reaction') + ': ' + err);
        })
        .finally(function() { if (btn) btn.disabled = false; });
}

// Long-press state for like button (1s = show emoji modal)
var likeLongPressTimer = null;
var likeLongPressTriggered = false;
var likeButtonMouseDown = null; // { noteId, pubkey, button }

function openLikeEmojiModal(noteId, pubkey, likeBtn) {
    state.pendingLikeNoteId = noteId;
    state.pendingLikePubkey = pubkey;
    state.pendingLikeBtn = likeBtn;
    var list = document.getElementById('like-emoji-list');
    if (list) {
        list.innerHTML = '';
        LIKE_EMOJI_LIST.forEach(function(emoji) {
            var btn = document.createElement('button');
            btn.type = 'button';
            btn.className = 'like-emoji-btn';
            btn.textContent = emoji;
            btn.addEventListener('click', function() {
                performLike(noteId, pubkey, emoji, likeBtn);
                closeLikeEmojiModal();
            });
            list.appendChild(btn);
        });
    }
    var modal = document.getElementById('like-emoji-modal');
    if (modal) {
        modal.classList.add('active');
        modal.setAttribute('aria-hidden', 'false');
    }
    document.addEventListener('keydown', likeEmojiModalEscapeHandler);
}

function closeLikeEmojiModal() {
    state.pendingLikeNoteId = null;
    state.pendingLikePubkey = null;
    state.pendingLikeBtn = null;
    var modal = document.getElementById('like-emoji-modal');
    if (modal) {
        modal.classList.remove('active');
        modal.setAttribute('aria-hidden', 'true');
    }
    document.removeEventListener('keydown', likeEmojiModalEscapeHandler);
}

function likeEmojiModalEscapeHandler(e) {
    if (e.key === 'Escape') {
        e.preventDefault();
        closeLikeEmojiModal();
    }
}

function handleLikeMouseDown(e) {
    var likeBtn = e.target.closest('.note-action[data-action="like"]');
    if (!likeBtn) return;
    var noteId = likeBtn.dataset.noteId;
    var pubkey = likeBtn.dataset.pubkey;
    if (!noteId || !pubkey) return;
    if (likeLongPressTimer) clearTimeout(likeLongPressTimer);
    likeLongPressTimer = null;
    likeLongPressTriggered = false;
    likeButtonMouseDown = { noteId: noteId, pubkey: pubkey, button: likeBtn };
    likeLongPressTimer = setTimeout(function() {
        likeLongPressTimer = null;
        likeLongPressTriggered = true;
        openLikeEmojiModal(noteId, pubkey, likeBtn);
        likeButtonMouseDown = null;
    }, 1000);
}

function handleLikeMouseUp(e) {
    if (likeLongPressTimer) {
        clearTimeout(likeLongPressTimer);
        likeLongPressTimer = null;
    }
    if (likeButtonMouseDown && !likeLongPressTriggered) {
        var btn = likeButtonMouseDown.button;
        var noteId = likeButtonMouseDown.noteId;
        var pubkey = likeButtonMouseDown.pubkey;
        var releaseInside = e.target && btn && btn.contains(e.target);
        if (releaseInside) {
            performLike(noteId, pubkey, DEFAULT_LIKE_EMOJI, btn);
        }
    }
    likeButtonMouseDown = null;
}

function handleLikeMouseLeave(e) {
    var likeBtn = e.target.closest('.note-action[data-action="like"]');
    if (likeBtn && likeButtonMouseDown && likeButtonMouseDown.button === likeBtn) {
        var related = e.relatedTarget;
        if (!related || !likeBtn.contains(related)) {
            if (likeLongPressTimer) clearTimeout(likeLongPressTimer);
            likeLongPressTimer = null;
            likeButtonMouseDown = null;
        }
    }
}

// Create HTML for a note card: name, tick, NIP-05, time; content; action bar. idPrefix avoids id clashes. replyToPubkey adds "Replying to [name]" when set. isBookmarked toggles bookmark icon.
function createNoteCard(note, noteIndex, idPrefix, replyToPubkey, isBookmarked) {
    if (idPrefix === undefined) idPrefix = '';
    if (isBookmarked === undefined) isBookmarked = isNoteBookmarked(note.id);
    var liked = isNoteLiked(note.id);
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const time = formatTimestamp(note.created_at);
    const { name: displayName, nip05 } = getAuthorDisplay(note.pubkey);
    const processedContent = processNoteContent(note.content);
    const safePubkey = escapeHtml(note.pubkey || '');
    const safeId = escapeHtml(note.id || '');
    var replyToName = '';
    var safeReplyToPubkey = '';
    if (replyToPubkey) {
        var replyToDisplay = getAuthorDisplay(replyToPubkey);
        replyToName = replyToDisplay.name || shortenKey(replyToPubkey);
        safeReplyToPubkey = escapeHtml(replyToPubkey);
    }
    const replyingToLabel = t('note.replyingTo');

    const card = document.createElement('div');
    card.className = 'note-card';
    card.dataset.noteIndex = noteIndex;
    card.dataset.noteId = safeId;
    card.dataset.pubkey = note.pubkey || '';
    const viewProfile = t('note.viewProfile');
    const verifying = t('note.verifying');
    const verifyId = idPrefix + 'verify-' + noteIndex;
    var replyContextHtml = replyToPubkey
        ? '<div class="note-reply-context">' + escapeHtml(replyingToLabel) + ' <button type="button" class="note-reply-to-link note-author-link" data-pubkey="' + safeReplyToPubkey + '" title="' + escapeHtml(viewProfile) + '">' + escapeHtml(replyToName) + '</button></div>'
        : '';
    card.innerHTML = `
        <div class="note-top-row">
            <button type="button" class="note-avatar note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}" aria-label="${escapeHtml(viewProfile)}"><span class="avatar-fallback">?</span></button>
            <div class="note-head">
                <div class="note-head-line">
                    <button type="button" class="note-author-name note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(displayName)}</button>
                    <span class="note-verification" id="${escapeHtml(verifyId)}" title="${escapeHtml(verifying)}"><span class="verify-pending">·</span></span>
                    <span class="note-author-nip05" ${nip05 ? '' : 'style="display:none"'}>${escapeHtml(nip05)}</span>
                    <span class="note-time">${escapeHtml(time)}</span>
                </div>
                ${replyContextHtml}
                <div class="note-content">${processedContent}</div>
                <div class="note-actions">
                    <button type="button" class="note-action" title="${escapeHtml(t('note.reply'))}" aria-label="${escapeHtml(t('note.reply'))}" data-action="reply" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/reply.svg" alt="${escapeHtml(t('note.reply'))}" class="icon-reply"></button>
                    <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safePubkey}" data-zap-event-id="${safeId}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"></button>
                    <button type="button" class="note-action${liked ? ' liked' : ''}" title="${escapeHtml(t('note.like'))}" aria-label="${escapeHtml(t('note.like'))}" data-action="like" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/${liked ? 'heart-filled' : 'heart'}.svg" alt="${escapeHtml(t('note.like'))}" class="icon-heart"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
                    <button type="button" class="note-action" title="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" aria-label="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" data-action="bookmark" data-note-id="${safeId}"><img src="icons/${isBookmarked ? 'bookmark-filled' : 'bookmark'}.svg" alt="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" class="icon-bookmark"></button>
                </div>
            </div>
        </div>
    `;
    return card;
}

// Create a card for a kind 6 repost event. Header: [icon] reposter reposted (muted). Body: original author avatar/name/tick/nip05/age + content.
function createRepostCard(repostEvent, noteIndex, idPrefix) {
    if (idPrefix === undefined) idPrefix = '';
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const { name: reposterName } = getAuthorDisplay(repostEvent.pubkey);
    const safePubkey = escapeHtml(repostEvent.pubkey || '');
    const safeId = escapeHtml(repostEvent.id || '');
    var parsed = null;
    var innerContent = '';
    if (repostEvent.content && repostEvent.content.trim()) {
        try {
            parsed = JSON.parse(repostEvent.content);
            if (parsed && typeof parsed.content === 'string') innerContent = processNoteContent(parsed.content);
            else innerContent = escapeHtml(t('note.repostedNote') || 'Reposted a note');
        } catch (_) {
            innerContent = escapeHtml(t('note.repostedNote') || 'Reposted a note');
        }
    } else {
        innerContent = escapeHtml(t('note.repostedNote') || 'Reposted a note');
    }
    const viewProfile = t('note.viewProfile');
    const repostedLabel = t('note.reposted');
    var origBlock = '';
    var safeOrigPubkey = '';
    var origTime = '';
    var origName = '…';
    var origNip05 = '';
    var dataOriginalPubkey = '';
    if (parsed && parsed.pubkey) {
        safeOrigPubkey = escapeHtml(parsed.pubkey);
        dataOriginalPubkey = parsed.pubkey;
        origTime = formatTimestamp(parsed.created_at || 0);
        var origDisplay = getAuthorDisplay(parsed.pubkey);
        origName = origDisplay.name || '…';
        origNip05 = origDisplay.nip05 || '';
        var verifyId = idPrefix + 'repost-orig-verify-' + noteIndex;
        origBlock = `
            <div class="note-original-row note-top-row">
                <button type="button" class="note-avatar note-author-link" data-pubkey="${safeOrigPubkey}" title="${escapeHtml(viewProfile)}" aria-label="${escapeHtml(viewProfile)}"><span class="avatar-fallback">?</span></button>
                <div class="note-head">
                    <div class="note-head-line">
                        <button type="button" class="note-author-name note-author-link" data-pubkey="${safeOrigPubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(origName)}</button>
                        <span class="note-verification" id="${escapeHtml(verifyId)}" title=""><span class="verify-pending">·</span></span>
                        <span class="note-author-nip05" ${origNip05 ? '' : 'style="display:none"'}>${escapeHtml(origNip05)}</span>
                        <span class="note-time">${escapeHtml(origTime)}</span>
                    </div>
                </div>
            </div>
        `;
    } else {
        origBlock = '';
    }
    const card = document.createElement('div');
    card.className = 'note-card note-card-repost';
    card.dataset.noteIndex = noteIndex;
    card.dataset.noteId = safeId;
    card.dataset.pubkey = repostEvent.pubkey || '';
    if (dataOriginalPubkey) card.dataset.originalPubkey = dataOriginalPubkey;
    card.innerHTML = `
        <div class="note-repost-header">
            <img src="icons/repost.svg" alt="" class="note-repost-header-icon" aria-hidden="true">
            <button type="button" class="note-author-name note-author-link note-reposter-name" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(reposterName)}</button>
            <span class="note-repost-header-text">${escapeHtml(repostedLabel)}</span>
        </div>
        ${origBlock}
        <div class="note-content">${innerContent}</div>
        <div class="note-actions">
            <button type="button" class="note-action" title="${escapeHtml(t('note.reply'))}" aria-label="${escapeHtml(t('note.reply'))}" data-action="reply" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/reply.svg" alt="${escapeHtml(t('note.reply'))}" class="icon-reply"></button>
            <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safeOrigPubkey || ''}" data-zap-event-id="${parsed && parsed.id ? escapeHtml(parsed.id) : ''}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"></button>
            <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
        </div>
    `;
    return card;
}

// Verify a note's signature. idPrefix optional (e.g. 'profile-' for profile feed).
async function verifyNote(note, noteIndex, idPrefix) {
    if (idPrefix === undefined) idPrefix = '';
    try {
        const noteJson = JSON.stringify(note);
        const resultJson = await invoke('verify_event', { eventJson: noteJson });
        
        if (resultJson) {
            const result = JSON.parse(resultJson);
            updateVerificationBadge(noteIndex, result, idPrefix);
        }
    } catch (error) {
        console.error('Verification failed for note', noteIndex, error);
        updateVerificationBadge(noteIndex, { valid: false, error: error.toString() }, idPrefix);
    }
}

// Update the verification badge for a note (badgeSuffix optional, e.g. 'repost-orig-verify-' for repost embedded note)
function updateVerificationBadge(noteIndex, result, idPrefix, badgeSuffix) {
    if (idPrefix === undefined) idPrefix = '';
    var suffix = badgeSuffix !== undefined ? badgeSuffix : 'verify-';
    const badgeEl = document.getElementById(idPrefix + suffix + noteIndex);
    if (!badgeEl) return;
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (result.valid) {
        const title = t('note.signatureVerified');
        badgeEl.innerHTML = '<span class="verify-valid" title="' + escapeHtml(title) + '">✓</span>';
        badgeEl.title = title;
    } else {
        const errorMsg = result.error || t('note.invalidSignature');
        badgeEl.innerHTML = '<span class="verify-invalid" title="' + escapeHtml(errorMsg) + '">✗</span>';
        badgeEl.title = errorMsg;
    }
}

// Verify the embedded note inside a kind 6 repost and update the original-author verification badge
async function verifyRepostOriginal(repostEvent, noteIndex, idPrefix) {
    if (idPrefix === undefined) idPrefix = '';
    if (!repostEvent.content || !repostEvent.content.trim()) return;
    try {
        var parsed = JSON.parse(repostEvent.content);
        if (!parsed || !parsed.pubkey) return;
        var noteJson = JSON.stringify(parsed);
        var resultJson = await invoke('verify_event', { eventJson: noteJson });
        if (resultJson) {
            var result = JSON.parse(resultJson);
            updateVerificationBadge(noteIndex, result, idPrefix, 'repost-orig-verify-');
        }
    } catch (_) {}
}

// Shorten a key for display
function shortenKey(key) {
    if (!key || key.length <= 16) {
        return key || '';
    }
    return key.substring(0, 8) + '...' + key.substring(key.length - 4);
}

// Format a Unix timestamp to human-readable relative time (e.g. 1min, 4h, 2 months)
function formatTimestamp(timestamp) {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now - date;
    const diffSec = Math.floor(diffMs / 1000);
    const diffMin = Math.floor(diffSec / 60);
    const diffHour = Math.floor(diffMin / 60);
    const diffDay = Math.floor(diffHour / 24);
    const diffMonth = Math.floor(diffDay / 30);
    const diffYear = Math.floor(diffDay / 365);

    if (diffSec < 60) return 'now';
    if (diffMin < 60) return diffMin === 1 ? '1min' : diffMin + 'min';
    if (diffHour < 24) return diffHour + 'h';
    if (diffDay < 30) return diffDay === 1 ? '1 day' : diffDay + ' days';
    if (diffMonth < 12) return diffMonth === 1 ? '1 month' : diffMonth + ' months';
    return diffYear === 1 ? '1 year' : diffYear + ' years';
}

// Validate and sanitize a URL: only allow http/https schemes, strip control characters.
// Returns the sanitized URL or null if unsafe.
function sanitizeUrl(url) {
    if (!url) return null;
    var trimmed = url.trim();
    // Only allow http: and https: schemes
    if (!/^https?:\/\//i.test(trimmed)) return null;
    // Block URLs containing control characters, quotes, or angle brackets that could break attributes
    if (/[\x00-\x1f"'<>`]/.test(trimmed)) return null;
    // Block javascript: in any encoding (e.g., via entity or percent-encoding in the already-escaped output)
    if (/javascript\s*:/i.test(trimmed)) return null;
    if (/data\s*:/i.test(trimmed)) return null;
    if (/vbscript\s*:/i.test(trimmed)) return null;
    return trimmed;
}

// Maximum recursion depth for embedded nostr: note references
var NOSTR_EMBED_MAX_DEPTH = 5;

// Process note content - find and embed images/videos and nostr: URIs.
// Content is HTML-escaped first to neutralize any injected tags/scripts,
// then safe URLs are converted to media elements and links.
// depth: recursion depth for nested nostr: note embeds (0 = top-level)
function processNoteContent(content, depth) {
    if (depth === undefined) depth = 0;

    // Escape HTML first - this is the primary XSS defense
    let html = escapeHtml(content);

    // Handle nostr: URIs (before URL linkification so they don't get turned into <a> tags)
    var nostrRegex = /nostr:(n(?:event|profile|pub|ote)1[a-z0-9]+)/gi;
    html = html.replace(nostrRegex, function(fullMatch, bech32) {
        var lower = bech32.toLowerCase();
        if (lower.startsWith('npub1') || lower.startsWith('nprofile1')) {
            // Profile reference: inline link placeholder (resolved async after DOM insert)
            return '<a class="nostr-profile-link" data-nostr-ref="' + escapeHtml(lower) + '" data-pubkey="" href="#">@' + escapeHtml(shortenKey(lower)) + '</a>';
        }
        if (lower.startsWith('note1') || lower.startsWith('nevent1')) {
            // Note/event reference: embed if within depth limit
            if (depth < NOSTR_EMBED_MAX_DEPTH) {
                return '<div class="nostr-embed-placeholder" data-nostr-ref="' + escapeHtml(lower) + '" data-depth="' + depth + '"><span class="embed-loading">Loading referenced note...</span></div>';
            }
            // Over depth limit: show as plain text
            return escapeHtml(fullMatch);
        }
        return escapeHtml(fullMatch);
    });
    
    // Find image URLs and convert to img tags (only safe http/https URLs)
    const imageAlt = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('content.image') : 'Image');
    const safeImageAlt = escapeHtml(imageAlt);
    const imageRegex = /(https?:\/\/[^\s]+\.(jpg|jpeg|png|gif|webp)(\?[^\s]*)?)/gi;
    html = html.replace(imageRegex, function(match) {
        var safe = sanitizeUrl(match);
        return safe ? '<img src="' + safe + '" alt="' + safeImageAlt + '" loading="lazy">' : escapeHtml(match);
    });
    
    // Find video URLs and convert to video tags
    const videoRegex = /(https?:\/\/[^\s]+\.(mp4|webm|mov)(\?[^\s]*)?)/gi;
    html = html.replace(videoRegex, function(match) {
        var safe = sanitizeUrl(match);
        return safe ? '<video src="' + safe + '" controls preload="metadata"></video>' : escapeHtml(match);
    });
    
    // Convert plain URLs to links (but not ones we already converted to media/link tags)
    const urlRegex = /(?<!src=")(https?:\/\/[^\s<]+)(?![^<]*>)/gi;
    html = html.replace(urlRegex, function(match) {
        var safe = sanitizeUrl(match);
        return safe ? '<a href="' + safe + '" target="_blank" rel="noopener noreferrer">' + escapeHtml(match) + '</a>' : escapeHtml(match);
    });
    
    return html;
}

// Create a compact embedded note card for nostr: URI references.
// No action bar, clickable to open full note detail.
function createEmbeddedNoteCard(note, depth) {
    var time = formatTimestamp(note.created_at);
    var display = getAuthorDisplay(note.pubkey);
    var displayName = display.name;
    var processedContent = processNoteContent(note.content, depth + 1);
    var safePubkey = escapeHtml(note.pubkey || '');
    var safeId = escapeHtml(note.id || '');

    var card = document.createElement('div');
    card.className = 'note-card-embed';
    card.dataset.noteId = safeId;
    card.dataset.pubkey = safePubkey;
    card.innerHTML =
        '<div class="embed-top-row">' +
            '<button type="button" class="note-avatar note-author-link embed-avatar" data-pubkey="' + safePubkey + '"><span class="avatar-fallback">?</span></button>' +
            '<div class="embed-head">' +
                '<span class="embed-author note-author-link" data-pubkey="' + safePubkey + '">' + escapeHtml(displayName) + '</span>' +
                '<span class="embed-time">' + escapeHtml(time) + '</span>' +
            '</div>' +
        '</div>' +
        '<div class="embed-content">' + processedContent + '</div>';
    return card;
}

// Resolve nostr: URI placeholders and profile links inside a container.
// Called after note cards are inserted into the DOM.
async function resolveNostrEmbeds(container) {
    if (!container) return;

    // --- 1. Collect all placeholders and profile links ---
    var embedPlaceholders = container.querySelectorAll('.nostr-embed-placeholder[data-nostr-ref]');
    var profileLinks = container.querySelectorAll('.nostr-profile-link[data-nostr-ref]');
    if (embedPlaceholders.length === 0 && profileLinks.length === 0) return;

    // --- 2. Decode all bech32 references in parallel ---
    var decoded = {};  // bech32 -> decoded JSON object
    var allRefs = new Set();
    embedPlaceholders.forEach(function(el) { allRefs.add(el.dataset.nostrRef); });
    profileLinks.forEach(function(el) { allRefs.add(el.dataset.nostrRef); });

    await Promise.all(Array.from(allRefs).map(async function(ref) {
        try {
            var json = await invoke('decode_nostr_uri', { bech32_str: ref });
            if (json) decoded[ref] = JSON.parse(json);
        } catch (e) {
            console.warn('[Plume] Failed to decode nostr URI:', ref, e);
        }
    }));

    // --- 3. Resolve profile links (npub / nprofile) ---
    var profilePubkeys = new Set();
    profileLinks.forEach(function(link) {
        var d = decoded[link.dataset.nostrRef];
        if (!d) return;
        var pk = d.pubkey || null;
        if (pk) {
            link.dataset.pubkey = pk;
            profilePubkeys.add(pk);
        }
    });

    // Fetch any missing profiles
    var toFetch = Array.from(profilePubkeys).filter(function(pk) { return !state.profileCache[pk]; });
    if (toFetch.length > 0) {
        var relays = getEffectiveRelays();
        await Promise.all(toFetch.map(async function(pk) {
            try {
                var pjson = await invoke('fetch_profile', { pubkey: pk, relay_urls: relays });
                if (pjson && pjson !== '{}') {
                    var p = JSON.parse(pjson);
                    state.profileCache[pk] = { name: p.name || null, nip05: p.nip05 || null, picture: p.picture || null, lud16: p.lud16 || null };
                }
            } catch (_) {}
        }));
    }

    // Update profile link display text
    profileLinks.forEach(function(link) {
        var pk = link.dataset.pubkey;
        if (!pk) return;
        var cached = state.profileCache[pk];
        var name = (cached && cached.name) ? cached.name : shortenKey(pk);
        link.textContent = '@' + name;
    });

    // --- 4. Resolve note/event embeds ---
    var eventIdsToFetch = new Set();
    var embedInfo = {};  // bech32 -> { eventId, relayHints }
    embedPlaceholders.forEach(function(el) {
        var d = decoded[el.dataset.nostrRef];
        if (!d) return;
        var eid = d.event_id;
        if (eid) {
            eventIdsToFetch.add(eid);
            embedInfo[el.dataset.nostrRef] = { eventId: eid, relayHints: d.relays || [] };
        }
    });

    // Batch-fetch all referenced events
    var fetchedEvents = {};  // eventId -> event object
    if (eventIdsToFetch.size > 0) {
        // Use relay hints merged with effective relays, deduplicating
        var allRelayHints = new Set(getEffectiveRelays());
        Object.values(embedInfo).forEach(function(info) {
            (info.relayHints || []).forEach(function(r) { allRelayHints.add(r); });
        });
        try {
            var idsArr = Array.from(eventIdsToFetch);
            var eventsJson = await invoke('fetch_events_by_ids', { relay_urls: Array.from(allRelayHints), ids: idsArr });
            var events = eventsJson ? JSON.parse(eventsJson) : [];
            events.forEach(function(ev) { fetchedEvents[ev.id] = ev; });
        } catch (e) {
            console.warn('[Plume] Failed to fetch embedded events:', e);
        }

        // Ensure profiles for embedded note authors
        var embeddedNotes = Object.values(fetchedEvents);
        if (embeddedNotes.length > 0) {
            await ensureProfilesForNotes(embeddedNotes);
        }
    }

    // Replace placeholders with embedded note cards
    embedPlaceholders.forEach(function(el) {
        var info = embedInfo[el.dataset.nostrRef];
        if (!info) { el.remove(); return; }
        var ev = fetchedEvents[info.eventId];
        if (!ev) {
            // Could not fetch: show as a link to the note
            el.innerHTML = '<span class="embed-not-found">Referenced note not found</span>';
            return;
        }
        var depth = parseInt(el.dataset.depth, 10) || 0;
        var card = createEmbeddedNoteCard(ev, depth);
        el.replaceWith(card);

        // Recursively resolve any nostr: URIs inside the embedded card
        resolveNostrEmbeds(card);

        // Update avatar for the embedded card author
        var cached = state.profileCache[ev.pubkey];
        if (cached && cached.picture) {
            var avatarBtn = card.querySelector('.embed-avatar');
            if (avatarBtn) {
                avatarBtn.innerHTML = '<img src="' + escapeHtml(cached.picture) + '" class="sidebar-avatar" alt="" style="width:24px;height:24px;border-radius:50%;">';
            }
        }
    });
}

// Display notes in the feed
function displayNotes(notes) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const container = document.getElementById('notes-container');
    container.innerHTML = '';
    notes = (notes || []).filter(function(n) { return !isNoteMuted(n); });
    if (notes.length === 0) {
        container.innerHTML = `
            <div class="placeholder-message">
                <p>${escapeHtml(t('feed.noNotes'))}</p>
                <p>${escapeHtml(t('feed.notesHint'))}</p>
            </div>
        `;
        return;
    }
    
    let noteIndex = 0;
    const notesToVerify = [];
    
    notes.forEach(note => {
        // Only show text notes (kind 1)
        if (note.kind === 1) {
            const card = createNoteCard(note, noteIndex);
            container.appendChild(card);
            notesToVerify.push({ note, index: noteIndex });
            noteIndex++;
        }
    });
    
    // If no kind 1 notes were found
    if (container.children.length === 0) {
        container.innerHTML = `
            <div class="placeholder-message">
                <p>${escapeHtml(t('feed.noTextNotes'))}</p>
                <p>${escapeHtml(t('feed.tryRelays'))}</p>
            </div>
        `;
        return;
    }
    
    verifyNotesAsync(notesToVerify);
    ensureProfilesForNotes(notes);
    resolveNostrEmbeds(container);
}

// Verify notes asynchronously
async function verifyNotesAsync(notesToVerify) {
    // Verify in batches to avoid overwhelming the backend
    for (const { note, index } of notesToVerify) {
        // Don't await - let them run in parallel
        verifyNote(note, index);
        
        // Small delay to avoid hammering the backend
        await new Promise(resolve => setTimeout(resolve, 10));
    }
}

// ============================================================
// Utility Functions
// ============================================================

// Escape HTML to prevent XSS
function escapeHtml(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// ============================================================
// Event Handlers
// ============================================================

// Handle settings form submission
async function handleSettingsSubmit(event) {
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
    updateUIFromConfig();
    closeSettings();
    
    // Show success feedback
    console.log('Settings saved successfully');
}

// ============================================================
// Compose / Posting
// ============================================================

// State for compose
let isPosting = false;

// Open the compose modal (optionally as a reply: openCompose({ id, pubkey, name }))
function openCompose(replyingTo) {
    state.replyingTo = replyingTo || null;
    const modal = document.getElementById('compose-modal');
    const replyCtx = document.getElementById('compose-reply-context');
    const replyName = document.getElementById('compose-reply-name');
    if (replyCtx) replyCtx.style.display = state.replyingTo ? 'block' : 'none';
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (replyName && state.replyingTo) replyName.textContent = state.replyingTo.name ? `@${state.replyingTo.name}` : t('note.replyLabel');
    modal.classList.add('active');
    const content = document.getElementById('compose-content');
    if (content) content.value = '';
    const charCountEl = document.getElementById('compose-char-count');
    if (charCountEl) charCountEl.textContent = t('composeModal.charCount', { count: 0 });
    hideComposeError();
    hideComposeStatus();
    enableComposeButton();
    setTimeout(() => content && content.focus(), 100);
}

// Close the compose modal
function closeCompose() {
    state.replyingTo = null;
    document.getElementById('compose-modal').classList.remove('active');
}

// Show error in compose modal
function showComposeError(message) {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = message;
    errorEl.classList.add('visible');
}

// Hide compose error
function hideComposeError() {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = '';
    errorEl.classList.remove('visible');
}

// Show status in compose modal
function showComposeStatus(message, isSuccess = false) {
    const statusEl = document.getElementById('compose-status');
    statusEl.textContent = message;
    statusEl.classList.add('visible');
    if (isSuccess) {
        statusEl.classList.add('success');
    } else {
        statusEl.classList.remove('success');
    }
}

// Hide compose status
function hideComposeStatus() {
    const statusEl = document.getElementById('compose-status');
    statusEl.textContent = '';
    statusEl.classList.remove('visible');
    statusEl.classList.remove('success');
}

// Disable compose button during posting
function disableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = true;
        const text = document.getElementById('compose-btn-text');
        if (text) text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.posting') : 'Posting…');
    }
}

// Enable compose button
function enableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = false;
        const text = document.getElementById('compose-btn-text');
        if (text) text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.post') : 'Post');
    }
}

// Update character count
function updateCharCount() {
    const textarea = document.getElementById('compose-content');
    const count = textarea ? textarea.value.length : 0;
    const el = document.getElementById('compose-char-count');
    if (el && window.PlumeI18n && window.PlumeI18n.t) {
        el.textContent = window.PlumeI18n.t('composeModal.charCount', { count: count });
    } else if (el) {
        el.textContent = count + ' / 10000';
    }
}

// Handle compose form submission
async function handleComposeSubmit(event) {
    event.preventDefault();
    
    if (isPosting) {
        return;
    }
    
    const content = document.getElementById('compose-content').value.trim();
    
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    // Validate content
    if (!content) {
        showComposeError(t('composeModal.contentRequired'));
        return;
    }
    
    if (content.length > 10000) {
        showComposeError(t('composeModal.tooLong'));
        return;
    }
    
    // Check if we have a private key
    if (!state.config || !state.config.private_key) {
        showComposeError(t('composeModal.noPrivateKey'));
        return;
    }
    
    isPosting = true;
    hideComposeError();
    showComposeStatus(t('composeModal.signingPublishing'));
    disableComposeButton();
    
    const replyTo = state.replyingTo ? { event_id: state.replyingTo.id, pubkey: state.replyingTo.pubkey } : null;
    try {
        const resultJson = await invoke('post_note', {
            content,
            replyToEventId: replyTo ? replyTo.event_id : null,
            replyToPubkey: replyTo ? replyTo.pubkey : null
        });
        const result = JSON.parse(resultJson);
        
        console.log('Post result:', result);
        
        if (result.success_count > 0) {
            const msg = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishedSuccess', { success: result.success_count, total: result.total_count }) : `Published to ${result.success_count} of ${result.total_count} relay(s)`);
            showComposeStatus(msg, true);
            
            state.replyingTo = null;
            setTimeout(() => {
                closeCompose();
                if (state.homeFeedMode === 'follows') pollForNewNotes();
                else fetchNotesFirehoseOnHomeClick();
            }, 1500);
        } else {
            // All relays failed
            let errorMessage = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishFailed') : 'Failed to publish to any relay');
            if (result.results && result.results.length > 0) {
                const firstError = result.results[0].message;
                if (firstError) {
                    errorMessage += ': ' + firstError;
                }
            }
            showComposeError(errorMessage);
            hideComposeStatus();
            enableComposeButton();
        }
    } catch (error) {
        console.error('Failed to post note:', error);
        showComposeError((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.postFailed') : 'Failed to post note') + ': ' + error);
        hideComposeStatus();
        enableComposeButton();
    } finally {
        isPosting = false;
    }
}

// ============================================================
// Initialization
// ============================================================

// Initialize the application
// ============================================================
// Multi-Profile / Auth State
// ============================================================

// Three-tier sidebar auth state:
// State 1 (logged out): Home + Profile/Welcome enabled; Messages, Notifications, Bookmarks, Compose muted
// State 2 (npub only): Home, Profile, Notifications, Bookmarks, Settings enabled; Compose + Messages muted
// State 3 (full auth): All enabled
function updateSidebarAuthState() {
    var hasProfile = !!(state.config && state.config.public_key);
    var hasNsec = !!(state.config && state.config.private_key);
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    var navMessages = document.querySelector('.nav-item[data-view="messages"]');
    var navNotifications = document.querySelector('.nav-item[data-view="notifications"]');
    var navBookmarks = document.querySelector('.nav-item[data-view="bookmarks"]');
    var composeBtn = document.getElementById('compose-btn');

    if (!hasProfile) {
        // State 1: logged out
        if (navMessages) { navMessages.classList.add('nav-muted'); navMessages.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to access messages'; }
        if (navNotifications) { navNotifications.classList.add('nav-muted'); navNotifications.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to see notifications'; }
        if (navBookmarks) { navBookmarks.classList.add('nav-muted'); navBookmarks.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to access bookmarks'; }
        if (composeBtn) { composeBtn.classList.add('nav-muted'); composeBtn.dataset.mutedReason = t('welcome.identityRequired') || 'Log in to compose notes'; }
    } else if (!hasNsec) {
        // State 2: npub only (read-only)
        if (navMessages) { navMessages.classList.add('nav-muted'); navMessages.dataset.mutedReason = t('welcome.nsecRequired') || 'Private key required to send messages'; }
        if (navNotifications) navNotifications.classList.remove('nav-muted');
        if (navBookmarks) navBookmarks.classList.remove('nav-muted');
        if (composeBtn) { composeBtn.classList.add('nav-muted'); composeBtn.dataset.mutedReason = t('welcome.nsecRequired') || 'Private key required to publish notes'; }
    } else {
        // State 3: full auth
        if (navMessages) navMessages.classList.remove('nav-muted');
        if (navNotifications) navNotifications.classList.remove('nav-muted');
        if (navBookmarks) navBookmarks.classList.remove('nav-muted');
        if (composeBtn) composeBtn.classList.remove('nav-muted');
    }
}

function showMutedTooltip(el) {
    var reason = el.dataset.mutedReason || 'Not available';
    // Remove any existing tooltip
    var existing = document.querySelector('.nav-muted-tooltip');
    if (existing) existing.remove();

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
async function populateWelcomeProfiles() {
    var container = document.getElementById('welcome-known-profiles');
    var list = document.getElementById('welcome-profiles-list');
    if (!container || !list) return;

    var profiles = [];
    try {
        var json = await invoke('list_profiles');
        if (json) profiles = JSON.parse(json);
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
        if (img) img.addEventListener('error', function() {
            this.style.display = 'none';
            var placeholder = document.createElement('span');
            placeholder.className = 'known-profile-placeholder';
            placeholder.innerHTML = '<img src="icons/user.svg" alt="" class="nav-icon">';
            this.parentNode.insertBefore(placeholder, this);
        });
        list.appendChild(li);
    });
}

async function handleWelcomeLogin() {
    var npubEl = document.getElementById('welcome-npub');
    var nsecEl = document.getElementById('welcome-nsec');
    var errorEl = document.getElementById('welcome-login-error');
    if (errorEl) errorEl.textContent = '';

    var npub = (npubEl ? npubEl.value : '').trim();
    var nsec = (nsecEl ? nsecEl.value : '').trim();

    if (!npub) {
        if (errorEl) errorEl.textContent = 'Public key is required';
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
        switchView('feed');
        startInitialFeedFetch();
        // Fetch profile from relays in background to update sidebar avatar and local config
        fetchProfile();
    } catch (err) {
        if (errorEl) errorEl.textContent = typeof err === 'string' ? err : (err.message || 'Login failed');
    }
}

async function handleWelcomeGenerate() {
    var btn = document.getElementById('welcome-generate-btn');
    if (!btn) return;
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
        switchView('feed');
        startInitialFeedFetch();
    } catch (error) {
        alert((t('errors.failedToGenerateKeys') || 'Failed to generate key pair') + ': ' + error);
    } finally {
        btn.disabled = false;
        btn.textContent = originalText;
    }
}

async function handleProfileSelect(npub) {
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
        switchView('feed');
        startInitialFeedFetch();
        // Fetch profile from relays in background to update sidebar avatar and local config
        fetchProfile();
    } catch (err) {
        alert('Failed to switch profile: ' + (typeof err === 'string' ? err : err.message || err));
    }
}

async function handleLogout() {
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
            default_zap_amount: 42
        };
        state.publicKeyHex = null;
        state.publicKeyNpub = null;
        state.profile = null;
        state.viewedProfile = null;
        state.notes = [];
        state.homeFeedMode = 'firehose';
        state.initialFeedLoadDone = false;
        if (state.feedPollIntervalId) { clearInterval(state.feedPollIntervalId); state.feedPollIntervalId = null; }

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
        switchView('welcome');
    } catch (err) {
        console.error('[Plume] handleLogout() FAILED:', err);
        alert('Logout error: ' + (err && err.message ? err.message : String(err)));
    }
}

async function init() {
    try {
        console.log('[Plume] init() entered');

        // Wire the UI first with no awaits – so nothing can block before buttons work
        updateMessagesNavUnread();
        var navItems = document.querySelectorAll('.nav-item[data-view]');
        console.log('[Plume] nav items found: ' + navItems.length);
        document.querySelector('.sidebar-logo')?.addEventListener('click', function(e) {
            e.preventDefault();
            switchView('feed');
        });
        navItems.forEach(function(item) {
            item.addEventListener('click', function(e) {
                e.preventDefault();
                var view = item.dataset.view;
                if (!view) return;
                // Muted nav items show a tooltip instead of navigating
                if (item.classList.contains('nav-muted')) {
                    showMutedTooltip(item);
                    return;
                }
                // Profile icon: show welcome when logged out, profile when logged in
                if (view === 'profile') {
                    if (!state.config || !state.config.public_key) {
                        switchView('welcome');
                        return;
                    }
                    state.viewedProfilePubkey = null;
                    state.viewedProfile = state.profile;
                }
                switchView(view);
            });
        });
        // Compose button also respects muted state
        var composeBtnEl = document.getElementById('compose-btn');
        if (composeBtnEl) {
            var origComposeHandler = null;
            composeBtnEl.addEventListener('click', function(e) {
                if (composeBtnEl.classList.contains('nav-muted')) {
                    e.stopImmediatePropagation();
                    showMutedTooltip(composeBtnEl);
                }
            }, true); // capture phase so it fires before the existing handler
        }

        // Wire welcome screen buttons (sync, before any awaits)
        document.getElementById('welcome-login-btn')?.addEventListener('click', handleWelcomeLogin);
        document.getElementById('welcome-generate-btn')?.addEventListener('click', handleWelcomeGenerate);

        // Wire logout button (sync, before any awaits)
        var logoutBtnEl = document.getElementById('logout-btn');
        if (logoutBtnEl) {
            logoutBtnEl.addEventListener('click', handleLogout);
            console.log('[Plume] logout button wired');
        } else {
            console.warn('[Plume] logout-btn element NOT found');
        }

        console.log('[Plume] nav + logo listeners attached');

        // Settings modal (Account) – open from Settings menu
        const closeSettingsBtn = document.getElementById('close-settings');
        const settingsModal = document.getElementById('settings-modal');
        if (closeSettingsBtn) closeSettingsBtn.addEventListener('click', closeSettings);
        if (settingsModal) {
            settingsModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) closeSettings();
            });
        }

        // Settings page menu – show corresponding panel on the right
        var settingsMenuBtns = document.querySelectorAll('.settings-menu-item');
        console.log('[Plume] settings menu items: ' + settingsMenuBtns.length);
        settingsMenuBtns.forEach(function(btn) {
            btn.addEventListener('click', function() {
                console.log('[Plume] settings panel clicked: ' + (btn.dataset.settings || '?'));
                showSettingsPanel(btn.dataset.settings);
            });
        });

        document.getElementById('home-feed-panel-save')?.addEventListener('click', saveHomeFeedModeFromPanel);
        document.getElementById('settings-media-save')?.addEventListener('click', saveMediaServerFromPanel);
        document.getElementById('settings-muted-save')?.addEventListener('click', saveMutedFromPanel);
        document.getElementById('settings-follows-save')?.addEventListener('click', saveFollowsPanel);
        document.getElementById('settings-zaps-save')?.addEventListener('click', saveZapsFromPanel);
        var settingsKeysForm = document.getElementById('settings-keys-form');
        if (settingsKeysForm) settingsKeysForm.addEventListener('submit', function(e) { e.preventDefault(); saveKeysPanel(e); });
        document.getElementById('settings-keys-copy-nsec')?.addEventListener('click', copyNsecToClipboard);

        // Messages view: conversation list, send button, Message from profile
        var messagesListEl = document.querySelector('.messages-list');
        if (messagesListEl) {
            messagesListEl.addEventListener('click', function(e) {
                var item = e.target.closest('.conversation-item');
                if (item) {
                    var other = item.getAttribute('data-other-pubkey');
                    if (other) selectConversation(other);
                }
            });
        }
        document.getElementById('message-send-btn')?.addEventListener('click', sendMessage);
        var messageInput = document.getElementById('message-input');
        if (messageInput) {
            messageInput.addEventListener('keydown', function(e) {
                if (e.key === 'Enter') { e.preventDefault(); sendMessage(); }
            });
        }
        document.getElementById('message-user-btn')?.addEventListener('click', function() {
            var pk = state.viewedProfilePubkey;
            if (pk) {
                state.openConversationWith = pk;
                switchView('messages');
            }
        });

        if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
            window.__TAURI__.event.listen('dm-received', function(ev) {
                var payload = ev.payload;
                var otherPubkey = Array.isArray(payload) ? payload[0] : (payload && payload.other_pubkey);
                if (!otherPubkey) return;
                var norm = (state.selectedConversation || '').toLowerCase();
                var otherNorm = (otherPubkey || '').toLowerCase();
                if (state.currentView === 'messages' && norm === otherNorm) {
                    loadConversationMessages(state.selectedConversation);
                } else {
                    state.unreadMessageCount = (state.unreadMessageCount || 0) + 1;
                    updateMessagesNavUnread();
                }
            });
        }

        // Follows panel: sort buttons, Add, Save (Save bound above)
        document.querySelectorAll('.follows-sort-btn').forEach(function(btn) {
            btn.addEventListener('click', function() {
                document.querySelectorAll('.follows-sort-btn').forEach(function(b) { b.classList.remove('active'); });
                this.classList.add('active');
                state.followsPanelSort = this.dataset.followsSort || 'name';
                renderFollowsPanel();
            });
        });
        document.getElementById('follows-add-btn')?.addEventListener('click', function() {
            var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            var input = document.getElementById('follows-add-input');
            if (!input) return;
            var raw = (input.value || '').trim();
            if (!raw) return;
            if (state.followsPanelLoading) {
                alert(t('settings.followsStillLoading') || 'Follow list is still loading, please wait.');
                return;
            }
            validatePublicKey(raw).then(function(r) {
                if (!r.valid || !r.hex) {
                    alert(t('settings.followsInvalidKey') || 'Invalid public key. Please enter a valid npub or 64-character hex key.');
                    return;
                }
                var hex = r.hex;
                if (!state.followsPanelList) state.followsPanelList = [];
                var exists = state.followsPanelList.some(function(x) { return (x.pubkey || '').toLowerCase() === hex.toLowerCase(); });
                if (exists) {
                    alert(t('settings.followsAlreadyExists') || 'This key is already in your follow list.');
                    input.value = '';
                    return;
                }
                state.followsPanelList.push({ pubkey: hex, checked: true, listOrder: state.followsPanelList.length });
                input.value = '';
                renderFollowsPanel();
                // Fetch profile for the new entry so it shows name/avatar
                ensureProfilesForNotes([{ pubkey: hex }]).then(function() {
                    renderFollowsPanel();
                });
            }).catch(function(err) {
                console.error('Follow add validation error:', err);
                alert(t('settings.followsInvalidKey') || 'Invalid public key. Please enter a valid npub or 64-character hex key.');
            });
        });
        document.getElementById('muted-user-add-btn')?.addEventListener('click', function() {
            var input = document.getElementById('muted-user-add-input');
            if (!input) return;
            var raw = (input.value || '').trim();
            if (!raw) return;
            validatePublicKey(raw).then(function(r) {
                if (!r.valid || !r.hex) return;
                var hex = r.hex;
                if (!state.mutedUsersPanelList) state.mutedUsersPanelList = [];
                var exists = state.mutedUsersPanelList.some(function(x) { return (x.pubkey || '').toLowerCase() === hex.toLowerCase(); });
                if (exists) return;
                state.mutedUsersPanelList.push({ pubkey: hex, checked: true });
                input.value = '';
                renderMutedPanels();
            });
        });

        // Muted tabs
        document.querySelectorAll('.muted-tab').forEach(function(tab) {
            tab.addEventListener('click', function() {
                var tabKey = this.dataset.mutedTab;
                document.querySelectorAll('.muted-tab').forEach(function(x) { x.classList.remove('active'); });
                document.querySelectorAll('.muted-tab-panel').forEach(function(x) { x.style.display = 'none'; });
                var panel = document.getElementById('muted-tab-' + tabKey);
                if (panel) panel.style.display = 'block';
                this.classList.add('active');
            });
        });
        document.getElementById('muted-word-add')?.addEventListener('click', function() {
            var input = document.getElementById('muted-word-input');
            if (!input || !state.config) return;
            var w = (input.value && input.value.trim()) || '';
            if (!w) return;
            ensureMutedConfig();
            if (state.config.muted_words.indexOf(w) === -1) state.config.muted_words.push(w);
            input.value = '';
            renderMutedPanels();
        });
        document.getElementById('muted-hashtag-add')?.addEventListener('click', function() {
            var input = document.getElementById('muted-hashtag-input');
            if (!input || !state.config) return;
            var h = (input.value && input.value.trim()).replace(/^#/, '') || '';
            if (!h) return;
            ensureMutedConfig();
            if (state.config.muted_hashtags.indexOf(h) === -1) state.config.muted_hashtags.push(h);
            input.value = '';
            renderMutedPanels();
        });
        document.getElementById('settings-detail')?.addEventListener('click', function(e) {
            var remove = e.target.closest('.muted-item-remove');
            if (!remove || !state.config) return;
            var li = remove.closest('li');
            if (!li) return;
            ensureMutedConfig();
            if (li.dataset.word) {
                state.config.muted_words = state.config.muted_words.filter(function(w) { return w !== li.dataset.word; });
            } else if (li.dataset.hashtag) {
                state.config.muted_hashtags = state.config.muted_hashtags.filter(function(h) { return h !== li.dataset.hashtag; });
            }
            renderMutedPanels();
        });
        
        // Set up settings form
        const settingsForm = document.getElementById('settings-form');
        if (settingsForm) settingsForm.addEventListener('submit', handleSettingsSubmit);
        
        // Set up compose modal
        const composeBtn = document.getElementById('compose-btn');
        const closeComposeBtn = document.getElementById('close-compose');
        const cancelComposeBtn = document.getElementById('cancel-compose');
        const composeModal = document.getElementById('compose-modal');
        if (composeBtn) composeBtn.addEventListener('click', function() { console.log('[Plume] compose btn clicked'); openCompose(); });
        if (closeComposeBtn) closeComposeBtn.addEventListener('click', closeCompose);
        if (cancelComposeBtn) cancelComposeBtn.addEventListener('click', closeCompose);
        if (composeModal) {
            composeModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) {
                    closeCompose();
                }
            });
        }

        var profileQrBtn = document.getElementById('profile-qr-btn');
        var closeProfileQrBtn = document.getElementById('close-profile-qr');
        var profileQrModal = document.getElementById('profile-qr-modal');
        if (profileQrBtn) profileQrBtn.addEventListener('click', openProfileQRModal);
        if (closeProfileQrBtn) closeProfileQrBtn.addEventListener('click', closeProfileQRModal);
        if (profileQrModal) {
            profileQrModal.addEventListener('click', function(e) {
                if (e.target === e.currentTarget) closeProfileQRModal();
            });
        }

        var editProfileBtn = document.getElementById('edit-profile-btn');
        var editProfileForm = document.getElementById('edit-profile-form');
        if (editProfileBtn) editProfileBtn.addEventListener('click', openEditProfileInSettings);
        if (editProfileForm) editProfileForm.addEventListener('submit', function(e) { e.preventDefault(); handleEditProfileSubmit(e); });

        var followBtn = document.getElementById('follow-btn');
        if (followBtn) followBtn.addEventListener('click', handleFollowClick);
        var muteBtn = document.getElementById('mute-btn');
        if (muteBtn) muteBtn.addEventListener('click', handleMuteClick);
        
        // Set up compose form
        const composeForm = document.getElementById('compose-form');
        const composeContent = document.getElementById('compose-content');
        if (composeForm) composeForm.addEventListener('submit', handleComposeSubmit);
        if (composeContent) composeContent.addEventListener('input', updateCharCount);

        // Generate keys button (on profile when no key)
        debugLog('Looking for generate-keys-btn...');
        const generateKeysBtn = document.getElementById('generate-keys-btn');
        debugLog('generateKeysBtn found: ' + (generateKeysBtn ? 'YES' : 'NO'));
        if (generateKeysBtn) {
            generateKeysBtn.addEventListener('click', function(e) {
                debugLog('=== GENERATE KEYS BUTTON CLICKED ===');
                generateNewKeyPair();
            });
            debugLog('Generate keys button listener attached');
        } else {
            debugLog('ERROR: Generate keys button not found in DOM!');
        }
        
        document.querySelectorAll('.notif-tab').forEach(function(tab) {
            tab.addEventListener('click', function() {
                document.querySelectorAll('.notif-tab').forEach(function(el) { el.classList.remove('active'); });
                tab.classList.add('active');
                state.notifFilter = tab.dataset.filter;
                // TODO: filter notifications by state.notifFilter
            });
        });

        document.querySelectorAll('.profile-tab').forEach(function(tabEl) {
            tabEl.addEventListener('click', function() {
                document.querySelectorAll('.profile-tab').forEach(function(t) { t.classList.remove('active'); });
                tabEl.classList.add('active');
                state.profileTab = tabEl.dataset.tab;
                loadProfileFeed();
            });
        });

        // Note card: reply button and author link (avatar/name -> profile). Same behavior for home feed and profile feed.
        function handleNoteCardClick(e) {
            // Nostr profile link (from nostr:npub or nostr:nprofile in note content)
            var profileLink = e.target.closest('.nostr-profile-link');
            if (profileLink && profileLink.dataset.pubkey) {
                e.preventDefault();
                openProfileForUser(profileLink.dataset.pubkey);
                return;
            }
            // Embedded note card (from nostr:nevent or nostr:note in note content)
            var embedCard = e.target.closest('.note-card-embed');
            if (embedCard && embedCard.dataset.noteId) {
                // Don't navigate if user clicked a profile link inside the embed
                if (e.target.closest('.note-author-link') || e.target.closest('.nostr-profile-link')) {
                    // let it fall through to the author link handler below
                } else {
                    e.preventDefault();
                    openNoteDetail(embedCard.dataset.noteId);
                    return;
                }
            }
            var authorLink = e.target.closest('.note-author-link');
            if (authorLink && authorLink.dataset.pubkey) {
                e.preventDefault();
                openProfileForUser(authorLink.dataset.pubkey);
                return;
            }
            var replyBtn = e.target.closest('.note-action[data-action="reply"]');
            if (replyBtn) {
                e.preventDefault();
                var card = replyBtn.closest('.note-card');
                var noteId = (replyBtn.dataset.noteId || (card && card.dataset.noteId)) || '';
                if (!noteId) return;
                var note = (state.notes && state.notes.find(function(n) { return n.id === noteId; })) ||
                    (state.profileNotes && state.profileNotes.find(function(n) { return n.id === noteId; })) ||
                    (state.bookmarkNotes && state.bookmarkNotes.find(function(n) { return n.id === noteId; }));
                if (!note && state.noteDetailReplies) {
                    var found = state.noteDetailReplies.find(function(x) { return x.note.id === noteId; });
                    if (found) note = found.note;
                }
                if (!note && state.noteDetailAncestors) note = state.noteDetailAncestors.find(function(n) { return n.id === noteId; });
                if (!note && state.noteDetailSubject && state.noteDetailSubject.id === noteId) note = state.noteDetailSubject;
                openNoteDetail(note || noteId);
                return;
            }
            var zapBtn = e.target.closest('.note-action[data-action="zap"]');
            if (zapBtn && !zapBtn.disabled) {
                e.preventDefault();
                e.stopPropagation();
                var targetPubkey = zapBtn.getAttribute('data-zap-target-pubkey');
                var eventId = zapBtn.getAttribute('data-zap-event-id') || (zapBtn.closest('.note-card') && zapBtn.closest('.note-card').dataset.noteId);
                if (targetPubkey) performZap(targetPubkey, eventId, zapBtn);
                return;
            }
            var likeBtn = e.target.closest('.note-action[data-action="like"]');
            if (likeBtn) {
                e.preventDefault();
                e.stopPropagation();
                return;
            }
            var repostBtn = e.target.closest('.note-action[data-action="repost"]');
            if (repostBtn) {
                e.preventDefault();
                var card = repostBtn.closest('.note-card');
                var noteId = (repostBtn.dataset.noteId || (card && card.dataset.noteId)) || '';
                var pubkey = (repostBtn.dataset.pubkey || (card && card.dataset.pubkey)) || '';
                if (!noteId || !pubkey) return;
                var note = (state.notes && state.notes.find(function(n) { return n.id === noteId; })) ||
                    (state.profileNotes && state.profileNotes.find(function(n) { return n.id === noteId; })) ||
                    (state.bookmarkNotes && state.bookmarkNotes.find(function(n) { return n.id === noteId; }));
                var contentOpt = note ? JSON.stringify(note) : null;
                repostBtn.disabled = true;
                invoke('post_repost', { eventId: noteId, authorPubkey: pubkey, contentOptional: contentOpt })
                    .then(function() {
                        repostBtn.classList.add('reposted');
                    })
                    .catch(function(err) {
                        console.error('Repost failed:', err);
                        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to publish repost') + ': ' + err);
                    })
                    .finally(function() { repostBtn.disabled = false; });
                return;
            }
            var bookmarkBtn = e.target.closest('.note-action[data-action="bookmark"]');
            if (bookmarkBtn) {
                e.preventDefault();
                var noteId = bookmarkBtn.dataset.noteId || (bookmarkBtn.closest('.note-card') && bookmarkBtn.closest('.note-card').dataset.noteId);
                if (!noteId || !state.config) return;
                if (!Array.isArray(state.config.bookmarks)) state.config.bookmarks = [];
                var idx = state.config.bookmarks.indexOf(noteId);
                if (idx === -1) {
                    state.config.bookmarks.push(noteId);
                } else {
                    state.config.bookmarks.splice(idx, 1);
                }
                saveConfig();
                var img = bookmarkBtn.querySelector('img');
                var nowBookmarked = idx === -1;
                if (img) img.src = nowBookmarked ? 'icons/bookmark-filled.svg' : 'icons/bookmark.svg';
                var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
                var label = nowBookmarked ? (t('note.unbookmark') || 'Unbookmark') : (t('note.bookmark') || 'Bookmark');
                bookmarkBtn.setAttribute('title', label);
                bookmarkBtn.setAttribute('aria-label', label);
                if (img) img.setAttribute('alt', label);
                // In bookmarks view we do not remove the card on unbookmark; list refreshes when user leaves and returns.
                return;
            }
        }
        var notesContainer = document.getElementById('notes-container');
        if (notesContainer) {
            notesContainer.addEventListener('click', handleNoteCardClick);
            notesContainer.addEventListener('mousedown', handleLikeMouseDown);
            notesContainer.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var profileFeed = document.getElementById('profile-feed');
        if (profileFeed) {
            profileFeed.addEventListener('click', handleNoteCardClick);
            profileFeed.addEventListener('mousedown', handleLikeMouseDown);
            profileFeed.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var bookmarksContainer = document.getElementById('bookmarks-container');
        if (bookmarksContainer) {
            bookmarksContainer.addEventListener('click', handleNoteCardClick);
            bookmarksContainer.addEventListener('mousedown', handleLikeMouseDown);
            bookmarksContainer.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        document.addEventListener('mouseup', handleLikeMouseUp);

        var noteDetailRepliesEl = document.getElementById('note-detail-replies');
        if (noteDetailRepliesEl) {
            noteDetailRepliesEl.addEventListener('click', handleNoteCardClick);
            noteDetailRepliesEl.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailRepliesEl.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var noteDetailAncestorsEl = document.getElementById('note-detail-ancestors');
        if (noteDetailAncestorsEl) {
            noteDetailAncestorsEl.addEventListener('click', handleNoteCardClick);
            noteDetailAncestorsEl.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailAncestorsEl.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var noteDetailSubjectWrap = document.getElementById('note-detail-subject-wrap');
        if (noteDetailSubjectWrap) {
            noteDetailSubjectWrap.addEventListener('click', handleNoteCardClick);
            noteDetailSubjectWrap.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailSubjectWrap.addEventListener('mouseleave', handleLikeMouseLeave);
        }

        document.getElementById('close-like-emoji-modal')?.addEventListener('click', closeLikeEmojiModal);
        var likeEmojiModal = document.getElementById('like-emoji-modal');
        if (likeEmojiModal) {
            likeEmojiModal.addEventListener('click', function(e) {
                if (e.target === likeEmojiModal) closeLikeEmojiModal();
            });
        }

        var noteDetailBack = document.getElementById('note-detail-back');
        if (noteDetailBack) noteDetailBack.addEventListener('click', function() {
            switchView(state.noteDetailPreviousView || 'feed');
        });

        console.log('[Plume] all sync listeners attached, about to await i18n...');
        await (window.PlumeI18n && window.PlumeI18n.init ? window.PlumeI18n.init() : Promise.resolve());
        console.log('[Plume] i18n done');

        // Load app-level config to determine active profile
        var appConfig = null;
        try {
            var appConfigJson = await invoke('get_app_config');
            appConfig = JSON.parse(appConfigJson);
        } catch (e) {
            console.log('[Plume] No app config, fresh start');
            appConfig = { active_profile: null, known_profiles: [] };
        }
        state.appConfig = appConfig;
        console.log('[Plume] App config loaded, active_profile:', appConfig.active_profile || 'none');

        if (appConfig.active_profile) {
            // Logged in – load profile config and go to feed
            await loadConfig();
            updateSidebarAuthState();
            switchView('feed');
            // Fetch profile from relays in background to update sidebar avatar and local config
            fetchProfile();
        } else {
            // Not logged in – show welcome screen
            updateSidebarAuthState();
            await populateWelcomeProfiles();
            switchView('welcome');
        }
        console.log('[Plume] loadConfig done');

        var noteDetailReplyBtn = document.getElementById('note-detail-reply-btn');
        var noteDetailReplyContent = document.getElementById('note-detail-reply-content');
        if (noteDetailReplyBtn && noteDetailReplyContent) {
            noteDetailReplyBtn.addEventListener('click', async function() {
                var content = noteDetailReplyContent.value.trim();
                if (!content) return;
                if (!state.noteDetailSubject || !state.noteDetailSubjectId) return;
                var sub = state.noteDetailSubject;
                noteDetailReplyBtn.disabled = true;
                try {
                    var resultJson = await invoke('post_note', {
                        content: content,
                        replyToEventId: state.noteDetailSubjectId,
                        replyToPubkey: sub.pubkey || null
                    });
                    var result = JSON.parse(resultJson);
                    if (result.success_count > 0) {
                        noteDetailReplyContent.value = '';
                        var replyJson = await invoke('fetch_replies_to_event', {
                            relay_urls: getEffectiveRelays(),
                            event_id: state.noteDetailSubjectId,
                            limit: 500
                        });
                        var repliesRaw = replyJson ? JSON.parse(replyJson) : [];
                        state.noteDetailReplies = buildReplyThread(repliesRaw, state.noteDetailSubjectId);
                        renderNoteDetailPage();
                    } else {
                        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishFailed') : 'Failed to publish'));
                    }
                } catch (e) {
                    console.error('Reply failed:', e);
                    alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.postFailed') : 'Failed to post') + ': ' + e);
                } finally {
                    noteDetailReplyBtn.disabled = false;
                }
            });
        }

        if (state.appConfig && state.appConfig.active_profile) {
            console.log('[Plume] calling startInitialFeedFetch...');
            startInitialFeedFetch();
        }
        console.log('[Plume] init() completed successfully');
    } catch (error) {
        console.error('[Plume] init() FAILED:', error);
        alert('Initialization error: ' + (error && error.message ? error.message : String(error)));
    }
}

// Run initialization when the page is ready (DOMContentLoaded may have already fired when script runs at end of body)
function runInit() {
    console.log('[Plume] runInit() called, readyState=' + document.readyState);
    init();
}
console.log('[Plume] app.js loaded, readyState=' + document.readyState);
if (document.readyState === 'loading') {
    console.log('[Plume] Waiting for DOMContentLoaded...');
    document.addEventListener('DOMContentLoaded', runInit);
} else {
    console.log('[Plume] Document already ready, calling runInit() now');
    runInit();
}
