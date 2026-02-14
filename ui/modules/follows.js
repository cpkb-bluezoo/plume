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
import { escapeHtml, shortenKey } from './utils.js';
import { setSavingState } from './config.js';
import { getAuthorDisplay, ensureProfilesForNotes } from './notes.js';
import { validatePublicKey } from './keys.js';

// ============================================================
// Following / Followers Management
// ============================================================

// Fetch following and followers (for own profile)
export async function fetchFollowingAndFollowers() {
    if (!state.config || !state.config.public_key) {
        return;
    }
    return fetchFollowingAndFollowersForUser(state.config.public_key);
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

// Follow or unfollow the currently viewed profile user. Updates contact list and publishes to relays immediately.
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
        var pk = String(state.viewedProfilePubkey).toLowerCase();
        if (add) {
            if (!state.ownFollowingPubkeys) {
                state.ownFollowingPubkeys = [];
            }
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
        if (followBtn) {
            followBtn.disabled = false;
        }
    }
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

// Display followers list (updates count on profile page)
export function displayFollowers(data) {
    const countEl = document.getElementById('followers-count');
    if (!countEl) {
        return;
    }
    const count = data.followers ? data.followers.length : 0;
    countEl.textContent = count.toString();
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

export async function loadFollowsPanel() {
    var listEl = document.getElementById('follows-list');
    var addInput = document.getElementById('follows-add-input');
    if (!listEl) {
        return;
    }
    state.followsPanelLoading = true;
    listEl.innerHTML = '<li class="follows-list-placeholder">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('settings.followsLoading') || 'Loading…' : 'Loading…') + '</li>';
    if (addInput) {
        addInput.value = '';
    }
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
