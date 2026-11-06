/*
 * modules/profile.js
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

import { state, getEffectiveRelays, FEED_LIMIT } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml, debugLog, formatTimestamp, torImgUrl } from './utils.js';
import { saveConfig, applyProfileFieldsToConfig, updateFeedInitialState } from './config.js';
import { publishWithProgress } from './publish.js';
import { isNoteMuted, isUserMuted } from './muting.js';
import { createNoteCard, createRepostCard, createArticleCard, verifyNote, verifyRepostOriginal, ensureProfilesForNotes, setCardAvatar, getReplyToPubkey, resolveNostrEmbeds, fetchAndDisplayZapTotals } from './notes.js';
import { fetchFollowingAndFollowers, fetchFollowingAndFollowersForUser, fetchFollowing, updateFollowButtonState } from './follows.js';
import { updateRelayList, testRelayConnections } from './relays.js';
import { startInitialFeedFetch } from './feed.js';

// Lazy import to avoid circular dependency with views.js
let _switchView = null;
export function setSwitchView(fn) {
    _switchView = fn;
}

// Lazy import to avoid circular dependency with config.js (updateUIFromConfig)
let _updateUIFromConfig = null;
export function setUpdateUIFromConfig(fn) {
    _updateUIFromConfig = fn;
}

// Low-level fetch for profile feed (used by loadProfileFeed batch fallback)
async function fetchFeedNotes(relayUrls, authors, since, profileFeed) {
    if (!relayUrls || relayUrls.length === 0) {
        return [];
    }
    const notesJson = await invoke('fetch_notes_from_relays', {
        relay_urls: relayUrls,
        limit: FEED_LIMIT,
        authors: authors && authors.length ? authors : null,
        since: since ?? null,
        profile_feed: profileFeed === true ? true : null
    });
    if (!notesJson) {
        return [];
    }
    const notes = JSON.parse(notesJson);
    return Array.isArray(notes) ? notes : [];
}

// Update sidebar profile avatar from config or profile (whichever has a picture).
// Called whenever the local profile is updated: login, config load, profile fetch from relays, etc.
export function updateSidebarAvatar() {
    var sidebarAvatar = document.getElementById('sidebar-avatar');
    var sidebarPlaceholder = document.getElementById('sidebar-avatar-placeholder');
    if (!sidebarAvatar || !sidebarPlaceholder) {
        return;
    }
    // Prefer profile picture (fetched from relays), fall back to config picture (local)
    var pic = (state.profile && state.profile.picture) || (state.config && state.config.picture) || null;
    if (pic) {
        sidebarAvatar.src = torImgUrl(pic);
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

// Fetch profile for the profile page (own or viewed user)
export async function fetchProfile() {
    if (state.profileLoading) {
        return;
    }
    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn && (!state.config || !state.config.public_key)) {
        state.viewedProfile = null;
        updateProfileDisplay();
        return;
    }

    state.profileLoading = true;
    try {
        if (viewingOwn) {
            // Event-driven: each relay reports in on its own schedule, and
            // the backend emits own-profile-update every time one arrives
            // with a newer profile than anything seen so far, rather than
            // making us wait for the slowest relay before showing anything.
            await new Promise(function(resolve) {
                var unlistenUpdate = function() {};
                var unlistenDone = function() {};

                function applyUpdate(profileJson) {
                    state.profile = JSON.parse(profileJson);
                    state.viewedProfile = state.profile;
                    updateProfileDisplay();
                    // Sync fetched profile fields into config so sidebar avatar and
                    // other UI elements stay current without a page reload.
                    if (state.config && state.profile) {
                        var changed = ['name', 'about', 'picture', 'nip05', 'banner', 'website', 'lud16']
                            .some(function(f) { return state.profile[f] && state.profile[f] !== state.config[f]; });
                        applyProfileFieldsToConfig(state.profile);
                        if (changed) {
                            updateSidebarAvatar();
                            // Persist the updated config in the background
                            saveConfig();
                        }
                    }
                }

                function finish() {
                    unlistenUpdate();
                    unlistenDone();
                    // Deliberately does NOT null out state.profile/viewedProfile
                    // when this particular call found nothing (!gotAny) -- this
                    // fetch can run more than once in a session (login, relay
                    // adoption, every profile-page visit), and a later attempt
                    // that happens to come back empty (e.g. queried a relay set
                    // that doesn't have the kind:0 event this time) must not
                    // erase data an earlier successful attempt already found.
                    // Leaving them untouched keeps whatever was last known good.
                    resolve();
                }

                if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
                    Promise.all([
                        window.__TAURI__.event.listen('own-profile-update', function(ev) { applyUpdate(ev.payload); }),
                        window.__TAURI__.event.listen('own-profile-done', function() { finish(); })
                    ]).then(function(unlisteners) {
                        unlistenUpdate = unlisteners[0];
                        unlistenDone = unlisteners[1];
                        invoke('start_own_profile_fetch').catch(function(err) {
                            console.error('Failed to start own profile fetch:', err);
                            finish();
                        });
                    });
                } else {
                    finish();
                }
            });
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
                if (data && data.contacts) {
                    state.ownFollowingPubkeys = data.contacts.map(function(c) { return c.pubkey; });
                } else {
                    state.ownFollowingPubkeys = [];
                }
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

// The locally configured relay list is only ever a bootstrap: a hardcoded
// default for a brand new profile, or whatever was last known. The user's
// own NIP-65 relay list (kind 10002), published to the network, is the real
// preference -- once we can reach it (over the bootstrap list), it should
// take over rather than sitting unused until the next manual edit in
// Settings. Safe to call on every login/startup: a no-op once the local
// list already matches, and harmless if nothing has been published yet
// (the bootstrap list it was called with just keeps being used).
export async function adoptOwnRelayListFromNetwork() {
    if (!state.config || !state.config.public_key) {
        return;
    }
    var bootstrapRelays = getEffectiveRelays();
    if (!bootstrapRelays.length) {
        return;
    }
    try {
        var json = await invoke('fetch_relay_list', { pubkey: state.config.public_key, relayUrls: bootstrapRelays });
        var urls = json ? JSON.parse(json) : [];
        if (!Array.isArray(urls) || urls.length === 0) {
            return;
        }
        var current = Array.isArray(state.config.relays) ? state.config.relays : [];
        var changed = urls.length !== current.length || urls.some(function(u) { return current.indexOf(u) === -1; });
        if (!changed) {
            return;
        }
        state.config.relays = urls;
        await saveConfig();
        updateRelayList();

        // fetchProfile's own-profile path takes no relay list of its own --
        // it reads whatever is already saved to disk -- so the very first
        // fetch (fired before this one resolves) ran against the bootstrap
        // list and likely came back empty or partial if the profile's kind
        // 0 event isn't on any of those relays. Re-fetch now that the real
        // list is persisted, so metadata/avatar/notes actually fill in
        // instead of staying blank for the rest of the session.
        //
        // fetchProfile silently no-ops while a call is already in flight,
        // and that earlier call (fired at the same time as this one, against
        // the bootstrap list) may still be running -- wait for it to settle
        // first so this retry isn't the one that gets dropped.
        for (var waited = 0; state.profileLoading && waited < 30000; waited += 200) {
            await new Promise(function(resolve) { setTimeout(resolve, 200); });
        }
        fetchProfile();

        // The feed may already be loading, or have already loaded, against
        // the bootstrap relays -- restart it against the real list now that
        // we know it, the same way a manual relay-settings change does.
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
    } catch (e) {
        console.warn('[Plume] Failed to fetch own relay list:', e);
    }
}

// Open profile page for a user (from note card avatar/name click)
export function openProfileForUser(pubkey) {
    if (!pubkey) {
        return;
    }
    state.viewedProfilePubkey = pubkey;
    state.viewedProfile = null;
    state.viewedProfileRelaysForPubkey = null; // so Relays tab fetches this user's list
    if (_switchView) {
        _switchView('profile');
    }
}

// Get the npub string for the currently viewed profile (for QR modal). Returns a Promise.
export function getProfileNpub() {
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn && state.publicKeyNpub) {
        return Promise.resolve(state.publicKeyNpub);
    }
    if (viewingOwn && state.config && state.config.public_key) {
        return invoke('convert_hex_to_npub', { hex_key: state.config.public_key })
            .then(function(n) { return n || state.config.public_key || ''; })
            .catch(function() { return state.config.public_key || ''; });
    }
    if (!viewingOwn && state.viewedProfilePubkey) {
        var key = state.viewedProfilePubkey;
        if (key.length === 64 && /^[a-fA-F0-9]+$/.test(key)) {
            return invoke('convert_hex_to_npub', { hex_key: key })
                .then(function(n) { return n || key; })
                .catch(function() { return key; });
        }
        return Promise.resolve(key);
    }
    return Promise.resolve('');
}

export function openProfileQRModal() {
    var modal = document.getElementById('profile-qr-modal');
    var wrap = document.getElementById('profile-qr-image-wrap');
    var npubInput = document.getElementById('profile-qr-npub-input');
    if (!modal || !wrap || !npubInput) {
        return;
    }
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
                    if (!svgString) {
                        setQRAndOpen();
                        return;
                    }
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
        var fallback = '';
        if (viewingOwn && state.config && state.config.public_key) {
            fallback = state.config.public_key;
        } else if (state.viewedProfilePubkey) {
            fallback = state.viewedProfilePubkey;
        }
        npubInput.value = fallback;
        openModal();
    });
}

export function closeProfileQRModal() {
    var modal = document.getElementById('profile-qr-modal');
    if (modal) {
        modal.classList.remove('active');
    }
}

// Navigate to Settings with Profile panel open (replaces opening edit profile modal)
export function openEditProfileInSettings() {
    state.editProfilePreviousView = state.currentView;
    state.settingsPanelRequested = 'profile';
    if (_switchView) {
        _switchView('settings');
    }
}

export function handleEditProfileSubmit(e) {
    if (e && e.preventDefault) {
        e.preventDefault();
    }
    debugLog('Edit profile submit/OK clicked');
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    var saveBtn = document.querySelector('#edit-profile-form button[type="submit"]');

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
    if (name) {
        profile.name = name;
    }
    if (nip05) {
        profile.nip05 = nip05;
    }
    if (website) {
        profile.website = website;
    }
    if (about) {
        profile.about = about;
    }
    if (lud16) {
        profile.lud16 = lud16;
    }
    if (picture) {
        profile.picture = picture;
    }
    if (banner) {
        profile.banner = banner;
    }
    var profileJson = JSON.stringify(profile);
    publishWithProgress('profile', saveBtn, function() {
        return invoke('start_save_profile', { profile_json: profileJson });
    }).then(function(result) {
        // Only adopt (and navigate away on) what the network actually
        // confirmed -- same rule as the relay-list saves. If nothing
        // confirmed, the form stays open with its entered values so the
        // user can see the failure and retry, rather than being bounced
        // back as if it had worked.
        if (!result.anyConfirmed || !result.adopted) {
            alert(t('profile.publishFailed') || 'Could not confirm the profile update on any relay. Please try again.');
            return;
        }
        state.profile = result.adopted;
        state.viewedProfile = state.profile;
        applyProfileFieldsToConfig(state.profile);
        updateProfileDisplay();
        updateSidebarAvatar();
        // Navigate back to where the user was before editing profile
        var prevView = state.editProfilePreviousView;
        state.editProfilePreviousView = null;
        if (prevView && prevView !== 'settings' && _switchView) {
            _switchView(prevView);
        }
        return saveConfig();
    }).catch(function(err) {
        console.error('Failed to save profile:', err);
        alert(typeof err === 'string' ? err : (err?.message || 'Failed to save profile'));
    });
}

// Whether the note should be shown on the current profile tab (notes / replies / zaps).
function isReplyNote(note) {
    return note.kind === 1 && note.tags && note.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
}

export function profileNoteMatchesTab(note, tab) {
    if (tab === 'zaps') {
        return false;
    }
    if (tab === 'notes') {
        // Originally-authored notes only -- a kind 1 reply belongs on the
        // Replies tab, not duplicated here too.
        return (note.kind === 1 && !isReplyNote(note)) || note.kind === 6 || note.kind === 30023;
    }
    if (tab === 'replies') {
        return isReplyNote(note);
    }
    return true;
}

// Append a single note to #profile-feed (streaming). Dedupes by id; inserts in sorted position. Returns true if appended.
export function appendProfileNoteCardSync(note) {
    if (!note || (note.kind !== 1 && note.kind !== 6 && note.kind !== 30023)) {
        return false;
    }
    if (isNoteMuted(note)) {
        return false;
    }
    var container = document.getElementById('profile-feed');
    var effectivePubkey = getEffectiveProfilePubkey();
    if (!container || !effectivePubkey) {
        return false;
    }
    var tab = state.profileTab || 'notes';
    if (!profileNoteMatchesTab(note, tab)) {
        return false;
    }
    if (state.profileNotes.some(function(n) { return n.id === note.id; })) {
        return false;
    }

    state.profileNotes.push(note);
    state.profileNotes.sort(function(a, b) { return (b.created_at || 0) - (a.created_at || 0); });
    var idx = state.profileNotes.findIndex(function(n) { return n.id === note.id; });

    var placeholder = container.querySelector('.placeholder-message');
    if (placeholder) {
        placeholder.remove();
    }

    var noteIndex = state.profileFeedStreamNoteIndex++;
    var card;
    if (note.kind === 6) {
        card = createRepostCard(note, noteIndex, 'profile-');
    } else if (note.kind === 30023) {
        card = createArticleCard(note, noteIndex, 'profile-');
    } else {
        var replyToPubkey = getReplyToPubkey(note);
        card = createNoteCard(note, noteIndex, 'profile-', replyToPubkey);
    }
    var viewedPubkey = effectivePubkey ? String(effectivePubkey).toLowerCase() : '';
    if (note.kind !== 6 && viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
        var profileForAvatar = state.viewedProfile || (effectivePubkey === state.publicKeyHex ? state.profile : null);
        if (profileForAvatar && profileForAvatar.picture) {
            setCardAvatar(card, profileForAvatar.picture);
        }
    }
    if (idx === 0) {
        container.insertBefore(card, container.firstChild);
    } else if (idx >= container.children.length) {
        container.appendChild(card);
    } else {
        container.insertBefore(card, container.children[idx]);
    }
    if (note.kind === 1 || note.kind === 30023) {
        verifyNote(note, noteIndex, 'profile-');
    }
    if (note.kind === 6) {
        verifyRepostOriginal(note, noteIndex, 'profile-');
    }
    ensureProfilesForNotes([note]);
    resolveNostrEmbeds(card);
    return true;
}

// Effective pubkey for the profile page: when viewing own profile (viewedProfilePubkey null or self), returns publicKeyHex; otherwise viewedProfilePubkey.
export function getEffectiveProfilePubkey() {
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    return viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;
}

// Load content for the currently viewed profile into #profile-feed (notes/replies/zaps/relays by tab).
// Non-blocking: shows loading state, then fetches/streams in background like home feed.
// When viewing own profile, viewedProfilePubkey is null; we use state.publicKeyHex for notes and state.config.relays for Relays tab.
export async function loadProfileFeed() {
    var container = document.getElementById('profile-feed');
    if (!container || !state.config) {
        return;
    }
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var tab = state.profileTab || 'notes';
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    var effectivePubkey = viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;

    if (tab === 'relays') {
        loadProfileRelays();
        return;
    }
    if (tab === 'zaps') {
        loadProfileZaps();
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
                    if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) {
                        return;
                    }
                    if ((note.kind !== 1 && note.kind !== 6 && note.kind !== 30023) || (note.pubkey && String(note.pubkey).toLowerCase() !== String(viewedPubkeyAtStart).toLowerCase())) {
                        return;
                    }
                    appendProfileNoteCardSync(note);
                }),
                window.__TAURI__.event.listen('profile-feed-eose', function() {
                    if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) {
                        return;
                    }
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
        if (getEffectiveProfilePubkey() !== viewedPubkeyAtStart) {
            return;
        }
        var feedNotes = notes ? notes.filter(function(n) { return n.kind === 1 || n.kind === 6 || n.kind === 30023; }) : [];
        state.profileNotes = feedNotes;
        if (tab === 'notes') {
            displayProfileNotes(feedNotes);
        } else if (tab === 'replies') {
            var replies = feedNotes.filter(function(n) {
                return n.kind === 1 && n.tags && n.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
            });
            displayProfileNotes(replies);
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
export function loadProfileRelays() {
    var container = document.getElementById('profile-feed');
    if (!container) {
        return;
    }
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
            if (state.viewedProfilePubkey !== pubkey) {
                return;
            }
            var relays = [];
            try {
                if (json) {
                    relays = JSON.parse(json);
                }
                if (!Array.isArray(relays)) {
                    relays = [];
                }
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

// Same markup, styling, and connection indicator as the Settings relay
// list (.relay-item / .relay-url / .relay-status), just without the add
// input, Enter-to-add, or delete button -- this is a read-only view of
// someone's relay list, not an editor.
function displayProfileRelays(relays) {
    var container = document.getElementById('profile-feed');
    if (!container) {
        return;
    }
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (!relays || relays.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noRelays')) + '</p></div>';
        return;
    }
    var unknownTitle = t('relays.statusUnknown');
    container.innerHTML = '';
    var ul = document.createElement('ul');
    ul.className = 'relay-list';
    relays.forEach(function(url, index) {
        var li = document.createElement('li');
        li.className = 'relay-item';
        li.innerHTML =
            '<span class="relay-url">' + escapeHtml(url) + '</span>' +
            '<div class="relay-status" id="profile-relay-status-' + index + '" title="' + escapeHtml(unknownTitle) + '" aria-label="' + escapeHtml(unknownTitle) + '"></div>';
        ul.appendChild(li);
    });
    container.appendChild(ul);
    testRelayConnections(relays, 'profile-relay-status-');
}

// Load zap receipts for the profile Zaps tab.
function loadProfileZaps() {
    var container = document.getElementById('profile-feed');
    if (!container) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    var pubkey = viewingOwn ? state.publicKeyHex : state.viewedProfilePubkey;
    if (!pubkey) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('profile.noIdentityYet')) + '</p></div>';
        return;
    }
    var fetchRelays = getEffectiveRelays();
    if (!fetchRelays.length) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.noRelays')) + '</p></div>';
        return;
    }
    container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.notesHint')) + '</p></div>';
    invoke('fetch_zap_receipts', { pubkey: pubkey, relay_urls: fetchRelays, limit: 50 })
        .then(function(json) {
            if (getEffectiveProfilePubkey() !== pubkey) return;
            var receipts = [];
            try { receipts = JSON.parse(json); } catch (e) { receipts = []; }
            displayProfileZaps(receipts);
            if (receipts.length > 0) {
                var senderPubkeys = receipts.map(function(z) { return z.sender_pubkey; }).filter(Boolean);
                var unique = senderPubkeys.filter(function(pk, i) { return senderPubkeys.indexOf(pk) === i; });
                var missing = unique.filter(function(pk) { return !state.profileCache || !state.profileCache[pk]; });
                if (missing.length > 0) {
                    ensureProfilesForNotes(missing.map(function(pk) { return { pubkey: pk }; }));
                }
            }
        })
        .catch(function(e) {
            console.error('Fetch zap receipts failed:', e);
            if (getEffectiveProfilePubkey() === pubkey && container) {
                container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
            }
        });
}

function displayProfileZaps(receipts) {
    var container = document.getElementById('profile-feed');
    if (!container) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    container.innerHTML = '';
    if (!receipts || receipts.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('profile.noZaps') || 'No zaps yet') + '</p></div>';
        return;
    }
    receipts.forEach(function(zap) {
        var card = document.createElement('div');
        card.className = 'zap-card';
        var cached = state.profileCache && state.profileCache[zap.sender_pubkey] ? state.profileCache[zap.sender_pubkey] : null;
        var senderName = (cached && cached.name) ? escapeHtml(cached.name) : escapeHtml(zap.sender_pubkey.substring(0, 12) + '…');
        var picture = cached && cached.picture ? cached.picture : null;
        var avatarHtml = picture
            ? '<img src="' + escapeHtml(torImgUrl(picture)) + '" alt="" class="zap-avatar" onerror="this.style.display=\'none\';this.nextElementSibling.style.display=\'flex\'">'
              + '<span class="zap-avatar zap-avatar-placeholder" style="display:none"><img src="icons/user.svg" alt="" class="icon-sm"></span>'
            : '<span class="zap-avatar zap-avatar-placeholder"><img src="icons/user.svg" alt="" class="icon-sm"></span>';
        var sats = zap.amount_sats || 0;
        var amountStr = sats >= 1000000 ? (sats / 1000000).toFixed(1).replace(/\.0$/, '') + 'M'
                      : sats >= 1000 ? (sats / 1000).toFixed(1).replace(/\.0$/, '') + 'k'
                      : String(sats);
        var time = formatTimestamp(zap.created_at);
        var messageHtml = zap.message ? '<div class="zap-message">' + escapeHtml(zap.message) + '</div>' : '';
        card.innerHTML =
            '<div class="zap-card-row">' +
                '<div class="zap-card-avatar">' + avatarHtml + '</div>' +
                '<div class="zap-card-body">' +
                    '<div class="zap-card-header">' +
                        '<span class="zap-sender note-author-link" data-pubkey="' + escapeHtml(zap.sender_pubkey) + '">' + senderName + '</span>' +
                        '<span class="zap-amount">⚡ ' + escapeHtml(amountStr) + ' sats</span>' +
                        '<span class="zap-time">' + escapeHtml(time) + '</span>' +
                    '</div>' +
                    messageHtml +
                '</div>' +
            '</div>';
        container.appendChild(card);
    });
}

// Render note cards into #profile-feed (uses id prefix 'profile-' for verification badges).
export function displayProfileNotes(notes) {
    var container = document.getElementById('profile-feed');
    if (!container) {
        return;
    }
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
        if (note.kind !== 1 && note.kind !== 6 && note.kind !== 30023) {
            return;
        }
        var card;
        if (note.kind === 6) {
            card = createRepostCard(note, noteIndex, prefix);
        } else if (note.kind === 30023) {
            card = createArticleCard(note, noteIndex, prefix);
        } else {
            var replyToPubkey = getReplyToPubkey(note);
            card = createNoteCard(note, noteIndex, prefix, replyToPubkey);
        }
        container.appendChild(card);
        if (note.kind !== 6 && viewedProfile && viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
            setCardAvatar(card, viewedProfile.picture);
        }
        if (note.kind === 1 || note.kind === 30023) {
            verifyNote(note, noteIndex, prefix);
        }
        if (note.kind === 6) {
            verifyRepostOriginal(note, noteIndex, prefix);
        }
        noteIndex++;
    });
    ensureProfilesForNotes(notes);
    resolveNostrEmbeds(container);
    fetchAndDisplayZapTotals();
}

// Generate a new key pair
export async function generateNewKeyPair() {
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
        if (_updateUIFromConfig) {
            _updateUIFromConfig();
        }

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
export function updateProfileDisplay() {
    var followerAvatarsEl = document.getElementById('follower-avatars');
    if (followerAvatarsEl) {
        followerAvatarsEl.innerHTML = '';
    }

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
        if (nameEl) {
            nameEl.textContent = profile.name || (viewingOwn ? state.config?.name : null) || t('profile.anonymous');
        }
        if (aboutEl) {
            aboutEl.textContent = profile.about || '';
        }
        if (pictureEl && placeholderEl) {
            if (profile.picture) {
                pictureEl.src = torImgUrl(profile.picture);
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
            bannerEl.style.backgroundImage = profile.banner ? `url('${torImgUrl(profile.banner)}')` : '';
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
        if (nameEl) {
            nameEl.textContent = displayName;
        }
        if (aboutEl) {
            aboutEl.textContent = '';
        }
        // Own profile hasn't come back from relays yet: fall back to the
        // locally-cached picture from config (same source the sidebar
        // avatar already uses) instead of leaving this blank until the
        // relay round-trip completes.
        var fallbackPicture = viewingOwn ? state.config?.picture : (cache && cache.picture);
        if (pictureEl && placeholderEl) {
            if (fallbackPicture) {
                pictureEl.src = torImgUrl(fallbackPicture);
                pictureEl.style.display = 'block';
                placeholderEl.style.display = 'none';
                pictureEl.onerror = function() {
                    pictureEl.style.display = 'none';
                    placeholderEl.style.display = 'flex';
                };
            } else {
                pictureEl.style.display = 'none';
                placeholderEl.style.display = 'flex';
            }
        }
        if (bannerEl) {
            bannerEl.style.backgroundImage = viewingOwn && state.config?.banner
                ? `url('${torImgUrl(state.config.banner)}')`
                : '';
        }
        if (nip05El) {
            nip05El.style.display = 'none';
        }
        if (websiteEl) {
            websiteEl.style.display = 'none';
        }
        if (lightningEl) {
            lightningEl.style.display = 'none';
        }
        var joinedEl = document.getElementById('profile-joined');
        if (joinedEl) {
            joinedEl.textContent = '—';
        }
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

    if (editProfileBtn) {
        editProfileBtn.style.display = viewingOwn ? 'block' : 'none';
    }
    if (followBtn) {
        followBtn.style.display = viewingOwn ? 'none' : 'block';
    }
    if (messageUserBtn) {
        messageUserBtn.style.display = viewingOwn ? 'none' : 'flex';
    }
    if (muteBtn) {
        muteBtn.style.display = viewingOwn ? 'none' : 'block';
        muteBtn.textContent = (state.viewedProfilePubkey && isUserMuted(state.viewedProfilePubkey)) ? t('profile.unmute') : t('profile.mute');
    }

    if (sidebarAvatar && sidebarPlaceholder) {
        const pic = state.profile?.picture || state.config?.picture;
        if (pic) {
            sidebarAvatar.src = torImgUrl(pic);
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
