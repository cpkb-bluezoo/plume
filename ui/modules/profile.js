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
import { escapeHtml, debugLog } from './utils.js';
import { saveConfig, setSavingState } from './config.js';
import { isNoteMuted, isUserMuted } from './muting.js';
import { createNoteCard, createRepostCard, verifyNote, verifyRepostOriginal, ensureProfilesForNotes, setCardAvatar, getReplyToPubkey, resolveNostrEmbeds } from './notes.js';
import { fetchFollowingAndFollowers, fetchFollowingAndFollowersForUser, fetchFollowing, updateFollowButtonState } from './follows.js';

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
    invoke('set_profile_metadata', { profileJson: profileJson })
        .then(function() {
            if (state.config) {
                if (name) {
                    state.config.name = name;
                }
                if (about) {
                    state.config.about = about;
                }
                if (picture) {
                    state.config.picture = picture;
                }
                if (nip05) {
                    state.config.nip05 = nip05;
                }
                if (banner) {
                    state.config.banner = banner;
                }
                if (website) {
                    state.config.website = website;
                }
                if (lud16) {
                    state.config.lud16 = lud16;
                }
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
            if (prevView && prevView !== 'settings' && _switchView) {
                _switchView(prevView);
            }
        })
        .catch(function(err) {
            console.error('Failed to save profile:', err);
            alert(typeof err === 'string' ? err : (err?.message || 'Failed to save profile'));
        })
        .finally(restoreBtn);
}

// Whether the note should be shown on the current profile tab (notes / replies / zaps).
export function profileNoteMatchesTab(note, tab) {
    if (tab === 'zaps') {
        return false;
    }
    if (tab === 'notes') {
        return note.kind === 1 || note.kind === 6;
    }
    if (tab === 'replies') {
        return note.kind === 1 && note.tags && note.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
    }
    return true;
}

// Append a single note to #profile-feed (streaming). Dedupes by id; inserts in sorted position. Returns true if appended.
export function appendProfileNoteCardSync(note) {
    if (!note || (note.kind !== 1 && note.kind !== 6)) {
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
    var card = note.kind === 6 ? createRepostCard(note, noteIndex, 'profile-') : (function() {
        var replyToPubkey = getReplyToPubkey(note);
        return createNoteCard(note, noteIndex, 'profile-', replyToPubkey);
    })();
    var viewedPubkey = effectivePubkey ? String(effectivePubkey).toLowerCase() : '';
    if (viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
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
    if (note.kind === 1) {
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
                    if ((note.kind !== 1 && note.kind !== 6) || (note.pubkey && String(note.pubkey).toLowerCase() !== String(viewedPubkeyAtStart).toLowerCase())) {
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
        if (note.kind !== 1 && note.kind !== 6) {
            return;
        }
        var card = note.kind === 6 ? createRepostCard(note, noteIndex, prefix) : (function() {
            var replyToPubkey = getReplyToPubkey(note);
            return createNoteCard(note, noteIndex, prefix, replyToPubkey);
        })();
        container.appendChild(card);
        if (viewedProfile && viewedPubkey && String((note.pubkey || '')).toLowerCase() === viewedPubkey) {
            setCardAvatar(card, viewedProfile.picture);
        }
        if (note.kind === 1) {
            verifyNote(note, noteIndex, prefix);
        }
        if (note.kind === 6) {
            verifyRepostOriginal(note, noteIndex, prefix);
        }
        noteIndex++;
    });
    ensureProfilesForNotes(notes);
    resolveNostrEmbeds(container);
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
        if (nameEl) {
            nameEl.textContent = displayName;
        }
        if (aboutEl) {
            aboutEl.textContent = '';
        }
        if (pictureEl && placeholderEl) {
            if (cache && cache.picture) {
                pictureEl.src = cache.picture;
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
            bannerEl.style.backgroundImage = '';
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
