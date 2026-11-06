/*
 * modules/follows.js
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

import { state, getEffectiveRelays } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml, shortenKey, torImgUrl } from './utils.js';
import { setSavingState, saveConfig, updateFeedInitialState } from './config.js';
import { getAuthorDisplay, ensureProfilesForNotes } from './notes.js';
import { validatePublicKey } from './keys.js';
import { startInitialFeedFetch } from './feed.js';

// ============================================================
// Following / Followers Management
// ============================================================

// Fetch following and followers (for own profile). Doesn't just delegate to
// fetchFollowingAndFollowersForUser: that function displays whatever the
// live fetch returns with no fallback, which is correct for another user
// (there's no local copy of their contact list to fall back to) but wrong
// here -- the kind:3 event behind "following" might simply not be on
// whichever relays answered this particular query, exactly like the kind:0
// profile fetch, and showing "0" in that case would misrepresent a locally
// cached, non-empty following list as if it had been lost.
export async function fetchFollowingAndFollowers() {
    if (!state.config || !state.config.public_key) {
        return;
    }
    var relays = getEffectiveRelays();
    if (!relays.length) {
        return;
    }
    var cachedCount = Array.isArray(state.config.following) ? state.config.following.length : 0;
    var fc = document.getElementById('following-count');
    var fl = document.getElementById('followers-count');
    if (fc) {
        fc.textContent = cachedCount > 0 ? String(cachedCount) : '…';
    }
    if (fl) {
        fl.textContent = '…';
    }

    var followingResult = null;
    var followersResult = null;
    try {
        followingResult = await invoke('fetch_following', { pubkey: state.config.public_key, relayUrls: relays });
        followersResult = await invoke('fetch_followers', { pubkey: state.config.public_key, relayUrls: relays });
    } catch (e) {
        console.error('Failed to fetch following/followers:', e);
        if (fl) {
            fl.textContent = '0';
        }
        return;
    }
    if (followingResult) {
        try {
            var data = JSON.parse(followingResult);
            if ((!data.contacts || data.contacts.length === 0) && cachedCount > 0) {
                // Nothing came back this time; keep showing what's already
                // known locally rather than a misleading zero.
                if (fc) {
                    fc.textContent = String(cachedCount);
                }
            } else {
                displayFollowing(data);
            }
        } catch (_) {
            if (fc && cachedCount === 0) {
                fc.textContent = '0';
            }
        }
    }
    if (followersResult) {
        try {
            var data = JSON.parse(followersResult);
            displayFollowers(data);
        } catch (_) {
            if (fl) {
                fl.textContent = '0';
            }
        }
    }
}

// Fetch following and followers for any user (profile page counts). pubkey can be hex or npub.
export async function fetchFollowingAndFollowersForUser(pubkey) {
    var relays = getEffectiveRelays();
    if (!relays.length || !pubkey) {
        return;
    }
    var fc = document.getElementById('following-count');
    var fl = document.getElementById('followers-count');
    if (fc) {
        fc.textContent = '…';
    }
    if (fl) {
        fl.textContent = '…';
    }

    var followingResult = null;
    var followersResult = null;
    try {
        followingResult = await invoke('fetch_following', { pubkey: pubkey, relayUrls: relays });
        followersResult = await invoke('fetch_followers', { pubkey: pubkey, relayUrls: relays });
    } catch (e) {
        console.error('Failed to fetch following/followers:', e);
        if (fc) {
            fc.textContent = '0';
        }
        if (fl) {
            fl.textContent = '0';
        }
        return;
    }
    if (followingResult) {
        try {
            var data = JSON.parse(followingResult);
            displayFollowing(data);
        } catch (_) {
            if (fc) {
                fc.textContent = '0';
            }
        }
    }
    if (followersResult) {
        try {
            var data = JSON.parse(followersResult);
            displayFollowers(data);
        } catch (_) {
            if (fl) {
                fl.textContent = '0';
            }
        }
    }
}

// Fetch following (who you follow)
export async function fetchFollowing() {
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
export function updateFollowButtonState() {
    const followBtn = document.getElementById('follow-btn');
    if (!followBtn) {
        return;
    }
    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn) {
        return;
    }
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const pk = (state.viewedProfilePubkey || '').toLowerCase();
    const isFollowing = !!(state.ownFollowingPubkeys && pk && state.ownFollowingPubkeys.some(function(p) { return String(p).toLowerCase() === pk; }));
    followBtn.textContent = isFollowing ? (t('profile.unfollow') || 'Unfollow') : (t('profile.follow') || 'Follow');
    followBtn.dataset.following = isFollowing ? '1' : '0';
}

// Add or remove `pubkey` from the local follow-list cache, kept in sync
// with `update_contact_list`'s own local-first update on the Rust side.
function setLocallyFollowing(pubkey, following) {
    var pk = String(pubkey).toLowerCase();
    if (following) {
        if (!state.ownFollowingPubkeys) {
            state.ownFollowingPubkeys = [];
        }
        if (!state.ownFollowingPubkeys.some(function(p) { return String(p).toLowerCase() === pk; })) {
            state.ownFollowingPubkeys.push(pubkey);
        }
    } else if (state.ownFollowingPubkeys) {
        state.ownFollowingPubkeys = state.ownFollowingPubkeys.filter(function(p) { return String(p).toLowerCase() !== pk; });
    }
}

// Follow or unfollow the currently viewed profile user: updates the local
// follow-list cache and the button immediately (no relay round trip on
// this path at all -- see `update_contact_list` on the Rust side), then
// publishes to relays in the background. `handleContactListSyncFailed`
// below reverts this if that background publish never reaches any relay.
export async function handleFollowClick() {
    if (!state.viewedProfilePubkey || state.viewedProfilePubkey === state.publicKeyHex) {
        return;
    }
    const followBtn = document.getElementById('follow-btn');
    const currentlyFollowing = followBtn && followBtn.dataset.following === '1';
    const add = !currentlyFollowing;
    followBtn && (followBtn.disabled = true);
    try {
        await invoke('update_contact_list', { add: add, targetPubkey: state.viewedProfilePubkey });
        setLocallyFollowing(state.viewedProfilePubkey, add);
        updateFollowButtonState();
    } catch (e) {
        console.error('Follow/unfollow failed:', e);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to update follow') + ': ' + e);
    } finally {
        if (followBtn) {
            followBtn.disabled = false;
        }
    }
}

// `contact-list-sync-failed`: the background publish that follows a
// Follow/Unfollow click never reached any relay. The local cache (and the
// button, if still showing this profile) already reflect the change the
// user asked for, so revert both and let them know rather than silently
// drifting from what relays actually have.
export function handleContactListSyncFailed(payload) {
    var data = typeof payload === 'string' ? JSON.parse(payload) : payload;
    if (!data || !data.pubkey) {
        return;
    }
    setLocallyFollowing(data.pubkey, !data.add);
    if (state.viewedProfilePubkey && String(state.viewedProfilePubkey).toLowerCase() === String(data.pubkey).toLowerCase()) {
        updateFollowButtonState();
    }
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    alert((t('errors.failedToPublish') || 'Failed to update follow') + ': no relay accepted the update');
}

// Fetch followers (who follows you)
export async function fetchFollowers() {
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
export function displayFollowing(data) {
    const countEl = document.getElementById('following-count');
    if (!countEl) {
        return;
    }
    const count = data.contacts ? data.contacts.length : 0;
    countEl.textContent = count.toString();
}

// Display followers list (updates count and avatar strip on profile page)
export function displayFollowers(data) {
    const countEl = document.getElementById('followers-count');
    if (!countEl) {
        return;
    }
    const followers = data.followers || [];
    const count = followers.length;
    countEl.textContent = count.toString();
    renderFollowerAvatars(followers, count);
}

var FOLLOWER_AVATAR_LIMIT = 10;

async function renderFollowerAvatars(followers, totalCount) {
    var container = document.getElementById('follower-avatars');
    if (!container) {
        return;
    }
    container.innerHTML = '';
    if (!followers || followers.length === 0) {
        return;
    }

    var topFollowers = followers.slice(0, FOLLOWER_AVATAR_LIMIT);
    var pubkeys = topFollowers.map(function(f) { return f.pubkey; }).filter(Boolean);

    await ensureProfilesForNotes(pubkeys.map(function(pk) { return { pubkey: pk }; }));

    var stack = document.createElement('div');
    stack.className = 'follower-avatar-stack';

    pubkeys.forEach(function(pubkey) {
        var profile = state.profileCache[pubkey];
        var picture = profile && profile.picture;
        var name = (profile && profile.name) || '';
        var initial = name ? name.charAt(0).toUpperCase() : '?';

        if (picture) {
            var img = document.createElement('img');
            img.className = 'follower-avatar-item';
            img.src = torImgUrl(picture);
            img.alt = name || '';
            img.title = name || pubkey.substring(0, 12) + '…';
            img.dataset.pubkey = pubkey;
            img.addEventListener('error', function() {
                var fallback = document.createElement('span');
                fallback.className = 'follower-avatar-fallback';
                fallback.textContent = initial;
                fallback.title = img.title;
                fallback.dataset.pubkey = pubkey;
                img.replaceWith(fallback);
            });
            stack.appendChild(img);
        } else {
            var fallback = document.createElement('span');
            fallback.className = 'follower-avatar-fallback';
            fallback.textContent = initial;
            fallback.title = name || pubkey.substring(0, 12) + '…';
            fallback.dataset.pubkey = pubkey;
            stack.appendChild(fallback);
        }
    });

    container.appendChild(stack);

    if (totalCount > FOLLOWER_AVATAR_LIMIT) {
        var overflow = document.createElement('span');
        overflow.className = 'follower-avatar-overflow';
        overflow.textContent = '+' + (totalCount - FOLLOWER_AVATAR_LIMIT);
        container.appendChild(overflow);
    }
}

// Create a follow item element
export function createFollowItem(pubkey, petname) {
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
export function showFollowingMessage(message) {
    const listEl = document.getElementById('following-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Show message in followers list
export function showFollowersMessage(message) {
    const listEl = document.getElementById('followers-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Switch between following/followers tabs
export function switchFollowTab(tabName) {
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
// Follows settings panel
// ============================================================

// Populates the panel from the in-memory, already-synced follows list
// (state.config.following -- kept current by adoptOwnFollowingFromNetwork()
// at startup/login, same as the home feed's own follows-mode filtering
// reads it) rather than making its own network round trip every time the
// panel opens. Profile names/avatars for anyone not yet in the cache are
// resolved in the background and the list re-rendered once they arrive --
// a pubkey whose profile genuinely isn't reachable on any configured relay
// still falls back to its shortened hex form, same as everywhere else.
export function loadFollowsPanel() {
    var listEl = document.getElementById('follows-list');
    var addInput = document.getElementById('follows-add-input');
    if (!listEl) {
        return;
    }
    if (addInput) {
        addInput.value = '';
    }
    var pubkeys = Array.isArray(state.config && state.config.following) ? state.config.following : [];
    state.followsPanelList = pubkeys.map(function(pubkey, i) {
        return { pubkey: pubkey, checked: true, listOrder: i };
    });
    state.followsPanelSort = getFollowsPanelSort();
    renderFollowsPanel();
    if (pubkeys.length > 0) {
        ensureProfilesForNotes(pubkeys.map(function(p) { return { pubkey: p }; })).then(function() {
            renderFollowsPanel();
        });
    }
}

// The locally saved following list may be stale or empty (a fresh login,
// or a follow/unfollow made on another client) -- the kind:3 contact list
// published to the network is the real list, same reasoning as
// adoptOwnRelayListFromNetwork() in profile.js. Safe to call on every
// login/startup: a no-op once the local list already matches, and
// harmless if nothing has been published yet. This is what keeps
// state.config.following current for both the home feed's follows-mode
// filtering (see getHomeFeedAuthors in feed.js) and this settings panel,
// instead of either having to fetch it themselves.
export async function adoptOwnFollowingFromNetwork() {
    if (!state.config || !state.config.public_key) {
        return;
    }
    try {
        var json = await invoke('fetch_own_following');
        var data = json ? JSON.parse(json) : null;
        var contacts = (data && data.contacts) ? data.contacts : [];
        var pubkeys = contacts.map(function(c) { return c.pubkey; }).filter(Boolean);
        if (pubkeys.length === 0) {
            return;
        }
        var current = Array.isArray(state.config.following) ? state.config.following : [];
        var changed = pubkeys.length !== current.length || pubkeys.some(function(p) { return current.indexOf(p) === -1; });
        if (!changed) {
            return;
        }
        state.config.following = pubkeys;
        await saveConfig();

        var followsPanel = document.getElementById('settings-panel-follows');
        if (followsPanel && followsPanel.style.display !== 'none') {
            loadFollowsPanel();
        }

        // Follows-mode feed filtering reads state.config.following directly
        // -- restart it so a changed list takes effect now rather than at
        // the next poll.
        if (state.homeFeedMode === 'follows') {
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
        }
    } catch (e) {
        console.warn('[Plume] Failed to fetch own following list:', e);
    }
}

export function renderFollowsPanel() {
    var listEl = document.getElementById('follows-list');
    if (!listEl) {
        return;
    }
    var list = state.followsPanelList || [];
    var sort = state.followsPanelSort || 'name';
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    var sorted = list.slice().sort(function(a, b) {
        if (sort === 'order') {
            return (a.listOrder !== undefined ? a.listOrder : 0) - (b.listOrder !== undefined ? b.listOrder : 0);
        }
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
                    ? '<img src="' + escapeHtml(torImgUrl(picture)) + '" alt="" class="follows-item-avatar">'
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
        if (img) {
            img.addEventListener('error', function() {
                this.style.display = 'none';
                this.nextElementSibling.style.display = 'inline-flex';
            });
        }
    });

    listEl.querySelectorAll('.follows-item-checkbox').forEach(function(cb) {
        cb.addEventListener('change', function() {
            var pubkey = cb.dataset.pubkey;
            var item = state.followsPanelList.find(function(x) { return x.pubkey === pubkey; });
            if (item) {
                item.checked = cb.checked;
            }
        });
    });
}

export function getFollowsPanelSort() {
    var btn = document.querySelector('.follows-sort-btn.active');
    return (btn && btn.dataset.followsSort) ? btn.dataset.followsSort : 'name';
}

export function saveFollowsPanel() {
    var pubkeys = (state.followsPanelList || []).filter(function(x) { return x.checked; }).map(function(x) { return x.pubkey; });
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var restoreBtn = setSavingState(document.getElementById('settings-follows-save'));
    invoke('set_contact_list', { pubkeys: pubkeys })
        .then(function() {
            state.ownFollowingPubkeys = pubkeys;
            // Keep local config in sync so follows-mode feed works immediately
            if (state.config) {
                state.config.following = pubkeys.slice();
            }
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
