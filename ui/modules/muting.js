/*
 * modules/muting.js
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
import { escapeHtml, shortenKey, torImgUrl } from './utils.js';
import { saveConfig } from './config.js';
import { updateProfileDisplay } from './profile.js';
import { ensureProfilesForNotes } from './notes.js';
import { publishWithProgress } from './publish.js';

// Muted: ensure config arrays exist
export function ensureMutedConfig() {
    if (!state.config) {
        return;
    }
    if (!Array.isArray(state.config.muted_users)) {
        state.config.muted_users = [];
    }
    if (!Array.isArray(state.config.muted_words)) {
        state.config.muted_words = [];
    }
    if (!Array.isArray(state.config.muted_hashtags)) {
        state.config.muted_hashtags = [];
    }
}

// Fetches the user's own NIP-51 mute list (kind 10000) from the network and
// merges it into the locally cached one. Mutes were purely local before
// this -- never published, never fetched -- so another client's mute list
// (public or private) was invisible to Plume even when it had hundreds of
// entries. A union, not a replace: something muted in Plume this session
// but not yet published must survive this merge too.
export async function fetchAndMergeOwnMuteList() {
    if (!state.config || !state.config.public_key) {
        return;
    }
    if (!(window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function')) {
        return;
    }
    ensureMutedConfig();
    try {
        var unlisten = await window.__TAURI__.event.listen('own-mute-list-fetch-done', function(ev) {
            unlisten();
            var data = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            if (!data || !data.any_confirmed || !data.adopted) {
                return;
            }
            var mergeInto = function(localList, networkList) {
                var merged = localList.slice();
                (networkList || []).forEach(function(v) {
                    if (merged.indexOf(v) === -1) {
                        merged.push(v);
                    }
                });
                return merged;
            };
            var changed = false;
            ['pubkeys', 'words', 'hashtags'].forEach(function(key) {
                var localKey = key === 'pubkeys' ? 'muted_users' : (key === 'words' ? 'muted_words' : 'muted_hashtags');
                var merged = mergeInto(state.config[localKey], data.adopted[key]);
                if (merged.length !== state.config[localKey].length) {
                    changed = true;
                }
                state.config[localKey] = merged;
            });
            if (changed) {
                saveConfig();
                renderMutedPanels();
            }
        });
        await invoke('start_own_mute_list_fetch');
    } catch (e) {
        console.error('Failed to fetch own mute list:', e);
    }
}

export function isUserMuted(pubkey) {
    if (!pubkey || !state.config || !Array.isArray(state.config.muted_users)) {
        return false;
    }
    var pk = String(pubkey).toLowerCase();
    return state.config.muted_users.some(function(p) { return String(p).toLowerCase() === pk; });
}

// True if the note should be hidden by mute filters (muted user, muted word in content, or muted hashtag in tags).
export function isNoteMuted(note) {
    if (!note || !state.config) {
        return false;
    }
    ensureMutedConfig();
    var pubkey = (note.pubkey || '').toLowerCase();
    if (state.config.muted_users.some(function(p) { return String(p).toLowerCase() === pubkey; })) {
        return true;
    }
    if (note.kind === 1 || note.kind === 30023) {
        var content = (note.content || '').toLowerCase();
        var words = state.config.muted_words || [];
        for (var w = 0; w < words.length; w++) {
            if (content.indexOf(String(words[w]).toLowerCase()) !== -1) {
                return true;
            }
        }
        var tags = note.tags || [];
        var mutedHashtags = (state.config.muted_hashtags || []).map(function(h) { return String(h).toLowerCase().replace(/^#/, ''); });
        for (var t = 0; t < tags.length; t++) {
            var tag = tags[t];
            if (Array.isArray(tag) && tag[0] === 't' && tag[1]) {
                var tagVal = String(tag[1]).toLowerCase().replace(/^#/, '');
                if (mutedHashtags.indexOf(tagVal) !== -1) {
                    return true;
                }
            }
        }
    }
    return false;
}

// Detect notes whose content is unreadable encrypted data (base64 blobs).
// These are kind 1 notes with encrypted payloads not intended for public display.
export function isContentUnreadable(content) {
    if (!content || content.length < 20) {
        return false;
    }
    var trimmed = content.trim();
    // Pure base64 blob: only base64 characters with no spaces or readable structure
    if (/^[A-Za-z0-9+\/]+=*$/.test(trimmed)) {
        return true;
    }
    // NIP-04 ciphertext: "<base64 ciphertext>?iv=<base64 iv>" -- the most
    // common form of leaked encrypted content, but the literal "?iv=" in
    // the middle means it never matches the pure-base64 check above.
    if (/^[A-Za-z0-9+\/]+=*\?iv=[A-Za-z0-9+\/]+=*$/.test(trimmed)) {
        return true;
    }
    // Raw JSON blob (WebRTC signaling, SDP offers, machine-to-machine payloads)
    if ((trimmed[0] === '{' && trimmed[trimmed.length - 1] === '}') ||
        (trimmed[0] === '[' && trimmed[trimmed.length - 1] === ']')) {
        try {
            JSON.parse(trimmed);
            return true;
        }
        catch (e) {
            // Not valid JSON — fall through
        }
    }
    // Tagged machine payload: "[broadcast:[#id]] {json...}" or similar bracketed-prefix protocols
    var taggedMatch = trimmed.match(/^\[[\w:.#\[\]-]+\]\s*(\{[\s\S]*\})$/);
    if (taggedMatch) {
        try {
            JSON.parse(taggedMatch[1]);
            return true;
        }
        catch (e) {
            // Not valid JSON after tag — fall through
        }
    }
    // General case: one long, unbroken token (no whitespace at all) made
    // up of nothing but encoded-data characters -- hex, base64, and the
    // handful of separators formats like these commonly mix in (., _, -,
    // :, =, ?, &). Rather than recognising every exact shape a hash, key,
    // or ciphertext dump can take (the checks above only ever cover the
    // ones already seen), this catches the one thing they all have in
    // common: nobody types a 40+ character word with no spaces in it.
    // Explicitly excluded: a bare URL is unbroken too, but it's still
    // meaningfully readable as-is, so it's let through even at this length.
    if (!/\s/.test(trimmed) && trimmed.length >= 50 && /^[A-Za-z0-9+/=_.:?&-]+$/.test(trimmed)) {
        var isUrl = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed);
        if (!isUrl) {
            return true;
        }
    }
    return false;
}

// Mute or unmute the currently viewed profile user. Updates local config immediately and saves to disk.
export function handleMuteClick() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var pubkey = state.viewedProfilePubkey;
    if (!pubkey || !state.config) {
        return;
    }
    var muteBtn = document.getElementById('mute-btn');
    ensureMutedConfig();
    var pk = String(pubkey).toLowerCase();
    var idx = state.config.muted_users.findIndex(function(p) { return String(p).toLowerCase() === pk; });
    var desiredPubkeys = state.config.muted_users.slice();
    if (idx === -1) {
        desiredPubkeys.push(pubkey);
    } else {
        desiredPubkeys.splice(idx, 1);
    }
    // Mute list changes go through the same publish-and-confirm pipeline
    // as profile/relay-list saves: this is still local-only data loss
    // waiting to happen otherwise -- see fetchAndMergeOwnMuteList's
    // comment for why that was the actual bug behind "Settings says I
    // have no muted users."
    publishWithProgress('mute_list', muteBtn, function() {
        return invoke('start_save_mute_list', {
            pubkeys: desiredPubkeys,
            words: state.config.muted_words || [],
            hashtags: state.config.muted_hashtags || []
        });
    }).then(function(result) {
        if (!result.anyConfirmed || !result.adopted) {
            alert(t('profile.muteFailed') || 'Could not confirm the mute list update on any relay. Please try again.');
            return;
        }
        state.config.muted_users = result.adopted.pubkeys || [];
        state.config.muted_words = result.adopted.words || [];
        state.config.muted_hashtags = result.adopted.hashtags || [];
        return saveConfig();
    }).then(function() {
        updateProfileDisplay();
    }).catch(function(err) {
        console.error('Failed to save mute:', err);
        alert((t('errors.failedToSaveSettings') || 'Failed to save') + ': ' + err);
    });
}

export async function loadMutedPanel() {
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

export function renderMutedPanels() {
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
                    ? '<img src="' + escapeHtml(torImgUrl(picture)) + '" alt="" class="follows-item-avatar">'
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
                if (img) {
                    img.addEventListener('error', function() {
                        this.style.display = 'none';
                        this.nextElementSibling.style.display = 'inline-flex';
                    });
                }
                ulUsers.appendChild(li);
            });
        }
        ulUsers.querySelectorAll('.muted-user-checkbox').forEach(function(cb) {
            cb.addEventListener('change', function() {
                var pubkey = cb.dataset.pubkey;
                var item = state.mutedUsersPanelList.find(function(x) { return x.pubkey === pubkey; });
                if (item) {
                    item.checked = cb.checked;
                }
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

export function saveMutedFromPanel() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    ensureMutedConfig();
    var desiredPubkeys = state.mutedUsersPanelList
        ? state.mutedUsersPanelList.filter(function(x) { return x.checked; }).map(function(x) { return x.pubkey; })
        : state.config.muted_users.slice();
    var desiredWords = state.config.muted_words.slice();
    var desiredHashtags = state.config.muted_hashtags.slice();
    var saveBtn = document.getElementById('settings-muted-save');

    publishWithProgress('mute_list', saveBtn, function() {
        return invoke('start_save_mute_list', {
            pubkeys: desiredPubkeys,
            words: desiredWords,
            hashtags: desiredHashtags
        });
    }).then(function(result) {
        if (!result.anyConfirmed || !result.adopted) {
            alert(t('profile.muteFailed') || 'Could not confirm the mute list update on any relay. Please try again.');
            return;
        }
        state.config.muted_users = result.adopted.pubkeys || [];
        state.config.muted_words = result.adopted.words || [];
        state.config.muted_hashtags = result.adopted.hashtags || [];
        loadMutedPanel();
        alert(t('settings.mutedSaved') || 'Muted lists saved.');
    }).catch(function(err) {
        console.error('Failed to save muted lists:', err);
    });
}
