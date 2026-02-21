/*
 * modules/notes.js
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

import { state, DEFAULT_LIKE_EMOJI, LIKE_EMOJI_LIST, NOSTR_EMBED_MAX_DEPTH, getEffectiveRelays } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml, escapeCssAttr, shortenKey, formatTimestamp, sanitizeUrl } from './utils.js';
import { isNoteMuted, isContentUnreadable } from './muting.js';

var likeLongPressTimer = null;
var likeLongPressTriggered = false;
var likeButtonMouseDown = null;

// Whether a note (by event id) is in the user's bookmarks
export function isNoteBookmarked(noteId) {
    return !!(state.config && Array.isArray(state.config.bookmarks) && state.config.bookmarks.indexOf(noteId) !== -1);
}

// Get display name and NIP-05 for a pubkey from profile cache (NIP-05 is the verified identity, e.g. user@domain.com).
export function getAuthorDisplay(pubkey) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const c = state.profileCache[pubkey];
    if (c) {
        return { name: c.name || t('profile.anonymous'), nip05: c.nip05 || '' };
    }
    return { name: '…', nip05: '' };
}

// Fetch profiles for note authors (and reply-to targets) and update cache + DOM.
export async function ensureProfilesForNotes(notes) {
    const relays = getEffectiveRelays();
    if (relays.length === 0) {
        return;
    }
    var pubkeys = notes.map(n => n.pubkey).filter(Boolean);
    notes.forEach(function(n) {
        var p = getReplyToPubkey(n);
        if (p) {
            pubkeys.push(p);
        }
        if (n.kind === 6 && n.content && n.content.trim()) {
            try {
                var parsed = JSON.parse(n.content);
                if (parsed && parsed.pubkey) {
                    pubkeys.push(parsed.pubkey);
                }
            } catch (_) {}
        }
    });
    const unique = [...new Set(pubkeys)];
    const toFetch = unique.filter(p => !state.profileCache[p]);
    if (toFetch.length === 0) {
        return;
    }
    await Promise.all(toFetch.map(async (pubkey) => {
        try {
            const json = await invoke('fetch_profile', { pubkey, relay_urls: relays });
            if (!json || json === '{}') {
                return;
            }
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
        if (!profile) {
            return;
        }
        var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        var name = profile.name || t('profile.anonymous');
        var nip05 = profile.nip05 || '';
        document.querySelectorAll('.note-card[data-pubkey="' + escapeCssAttr(pubkey) + '"]').forEach(function(card) {
            var isRepost = card.classList.contains('note-card-repost');
            if (isRepost) {
                var reposterNameEl = card.querySelector('.note-repost-header .note-reposter-name');
                if (reposterNameEl) {
                    reposterNameEl.textContent = name;
                }
            } else {
                var nameEl = card.querySelector('.note-head .note-author-name');
                var nip05El = card.querySelector('.note-head .note-author-nip05');
                if (nameEl) {
                    nameEl.textContent = name;
                }
                if (nip05El) {
                    nip05El.textContent = nip05;
                    nip05El.style.display = nip05 ? '' : 'none';
                }
                var avatar = card.querySelector('.note-avatar');
                if (avatar && profile.picture) {
                    setCardAvatar(card, profile.picture);
                }
            }
            var replyToLink = card.querySelector('.note-reply-to-link[data-pubkey="' + escapeCssAttr(pubkey) + '"]');
            if (replyToLink) {
                replyToLink.textContent = name;
            }
        });
        updateZapButtons();
        document.querySelectorAll('.note-card[data-original-pubkey="' + escapeCssAttr(pubkey) + '"]').forEach(function(card) {
            var nameEl = card.querySelector('.note-original-row .note-author-name');
            var nip05El = card.querySelector('.note-original-row .note-author-nip05');
            if (nameEl) {
                nameEl.textContent = name;
            }
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
                    if (fallback) {
                        fallback.style.display = 'none';
                    }
                } else {
                    var newImg = document.createElement('img');
                    newImg.src = profile.picture;
                    newImg.alt = '';
                    newImg.onerror = function() {
                        if (fallback) {
                            fallback.style.display = 'flex';
                        }
                    };
                    avatar.insertBefore(newImg, avatar.firstChild);
                    if (fallback) {
                        fallback.style.display = 'none';
                    }
                }
            }
        });
    });
}

// NIP-10: get the direct parent event id from a note's "e" tags. Returns null if not a reply.
export function getParentEventId(note) {
    if (!note.tags || !note.tags.length) {
        return null;
    }
    for (var i = 0; i < note.tags.length; i++) {
        var tag = note.tags[i];
        if (Array.isArray(tag) && tag[0] === 'e' && tag[1]) {
            var marker = tag[3] || '';
            if (marker === 'reply') {
                return tag[1];
            }
        }
    }
    // Some clients omit the marker; last "e" is often the reply target
    var lastE = null;
    for (var j = 0; j < note.tags.length; j++) {
        var tagItem = note.tags[j];
        if (Array.isArray(tagItem) && tagItem[0] === 'e' && tagItem[1]) {
            lastE = tagItem[1];
        }
    }
    return lastE;
}

// Get the pubkey of the user being replied to (first "p" tag) when note is a reply (has "e" tag). Returns null if not a reply.
export function getReplyToPubkey(note) {
    if (!note.tags || !note.tags.length) {
        return null;
    }
    var hasE = note.tags.some(function(tag) { return Array.isArray(tag) && tag[0] === 'e'; });
    if (!hasE) {
        return null;
    }
    for (var i = 0; i < note.tags.length; i++) {
        var tag = note.tags[i];
        if (Array.isArray(tag) && tag[0] === 'p' && tag[1]) {
            return tag[1];
        }
    }
    return null;
}

export function setCardAvatar(card, pictureUrl) {
    if (!card || !pictureUrl) {
        return;
    }
    var avatar = card.querySelector('.note-avatar');
    if (!avatar) {
        return;
    }
    var fallback = avatar.querySelector('.avatar-fallback');
    var img = avatar.querySelector('img');
    if (img) {
        img.src = pictureUrl;
        img.alt = '';
        img.style.display = '';
        if (fallback) {
            fallback.style.display = 'none';
        }
    } else {
        img = document.createElement('img');
        img.src = pictureUrl;
        img.alt = '';
        img.onerror = function() {
            if (fallback) {
                fallback.style.display = 'flex';
            }
        };
        avatar.insertBefore(img, avatar.firstChild);
        if (fallback) {
            fallback.style.display = 'none';
        }
    }
}

// Whether the current user has liked this note (we only know from this session).
export function isNoteLiked(noteId) {
    return state.likedNoteIds && state.likedNoteIds[noteId];
}

// Whether the current user has a Lightning address (for sending/receiving zaps).
export function selfHasLud16() {
    if (state.profile && state.profile.lud16 && state.profile.lud16.trim()) {
        return true;
    }
    if (state.publicKeyHex && state.profileCache[state.publicKeyHex] && state.profileCache[state.publicKeyHex].lud16) {
        return true;
    }
    return false;
}

// Whether the given pubkey's profile has a Lightning address (for zapping them).
export function targetHasLud16(pubkey) {
    if (!pubkey || !state.profileCache) {
        return false;
    }
    var p = state.profileCache[pubkey];
    return !!(p && p.lud16 && p.lud16.trim());
}

// Update zap buttons: muted + disabled when self or target lack LUD16.
export function updateZapButtons() {
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

/// Fetch zap totals for all visible note cards and update the zap-count spans.
export function fetchAndDisplayZapTotals() {
    var spans = document.querySelectorAll('.zap-count[data-event-id]');
    var ids = [];
    var seen = {};
    spans.forEach(function(span) {
        var eid = span.getAttribute('data-event-id');
        if (eid && !seen[eid]) {
            seen[eid] = true;
            ids.push(eid);
        }
    });
    if (ids.length === 0) return;
    var relays = state.config && state.config.relays ? state.config.relays : [];
    if (relays.length === 0) return;
    invoke('fetch_note_zap_totals', { event_ids: ids, relay_urls: relays })
        .then(function(json) {
            var totals = {};
            try { totals = JSON.parse(json); } catch (e) { return; }
            document.querySelectorAll('.zap-count[data-event-id]').forEach(function(span) {
                var eid = span.getAttribute('data-event-id');
                var sats = totals[eid];
                if (sats && sats > 0) {
                    var str = sats >= 1000000 ? (sats / 1000000).toFixed(1).replace(/\.0$/, '') + 'M'
                            : sats >= 1000 ? (sats / 1000).toFixed(1).replace(/\.0$/, '') + 'k'
                            : String(sats);
                    span.textContent = str;
                }
            });
        })
        .catch(function() {});
}

// Fire-and-forget: request a zap invoice. Results arrive via events.
export function performZap(targetPubkey, eventId, zapBtn) {
    if (!targetPubkey || !state.config || !state.profileCache) {
        return;
    }
    var profile = state.profileCache[targetPubkey];
    if (!profile || !profile.lud16 || !profile.lud16.trim()) {
        return;
    }
    var amount = (state.config.default_zap_amount != null && state.config.default_zap_amount >= 1)
        ? state.config.default_zap_amount
        : 42;
    if (zapBtn) {
        zapBtn.disabled = true;
    }

    // Store ref so event handler can re-enable
    window._pendingZapBtn = zapBtn;

    invoke('request_zap_invoice', {
        target_lud16: profile.lud16.trim(),
        amount_sats: amount,
        event_id: eventId || '',
        target_pubkey: targetPubkey
    }).catch(function(err) {
        console.error('Zap failed:', err);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to get zap invoice') + ': ' + err);
        if (zapBtn) {
            zapBtn.disabled = false;
        }
    });
}

export function initZapEventListeners() {
    if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
        window.__TAURI__.event.listen('zap-invoice-ready', function(ev) {
            var data = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            if (data && data.pr) {
                var url = data.pr.indexOf('ln') === 0 ? 'lightning:' + data.pr : data.pr;
                window.open(url, '_blank');
            }
            if (window._pendingZapBtn) {
                window._pendingZapBtn.disabled = false;
                window._pendingZapBtn = null;
            }
        });
        window.__TAURI__.event.listen('zap-invoice-failed', function(ev) {
            var data = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            var msg = (data && data.error) ? data.error : 'Failed to get zap invoice';
            console.error('Zap failed:', msg);
            alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to get zap invoice') + ': ' + msg);
            if (window._pendingZapBtn) {
                window._pendingZapBtn.disabled = false;
                window._pendingZapBtn = null;
            }
        });
    }
}

// Perform a like (reaction) and update UI on success
export function performLike(noteId, pubkey, emoji, likeBtn) {
    if (!noteId || !pubkey) {
        return;
    }
    var btn = likeBtn;
    if (btn) {
        btn.disabled = true;
    }
    invoke('post_reaction', { eventId: noteId, authorPubkey: pubkey, emoji: emoji || DEFAULT_LIKE_EMOJI })
        .then(function() {
            if (!state.likedNoteIds) {
                state.likedNoteIds = {};
            }
            state.likedNoteIds[noteId] = true;
            if (btn) {
                var img = btn.querySelector('img');
                if (img) {
                    img.src = 'icons/heart-filled.svg';
                }
                btn.classList.add('liked');
            }
        })
        .catch(function(err) {
            console.error('Reaction failed:', err);
            alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to publish reaction') + ': ' + err);
        })
        .finally(function() {
            if (btn) {
                btn.disabled = false;
            }
        });
}

export function openLikeEmojiModal(noteId, pubkey, likeBtn) {
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

export function closeLikeEmojiModal() {
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

export function likeEmojiModalEscapeHandler(e) {
    if (e.key === 'Escape') {
        e.preventDefault();
        closeLikeEmojiModal();
    }
}

export function handleLikeMouseDown(e) {
    var likeBtn = e.target.closest('.note-action[data-action="like"]');
    if (!likeBtn) {
        return;
    }
    var noteId = likeBtn.dataset.noteId;
    var pubkey = likeBtn.dataset.pubkey;
    if (!noteId || !pubkey) {
        return;
    }
    if (likeLongPressTimer) {
        clearTimeout(likeLongPressTimer);
    }
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

export function handleLikeMouseUp(e) {
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

export function handleLikeMouseLeave(e) {
    var likeBtn = e.target.closest('.note-action[data-action="like"]');
    if (likeBtn && likeButtonMouseDown && likeButtonMouseDown.button === likeBtn) {
        var related = e.relatedTarget;
        if (!related || !likeBtn.contains(related)) {
            if (likeLongPressTimer) {
                clearTimeout(likeLongPressTimer);
            }
            likeLongPressTimer = null;
            likeButtonMouseDown = null;
        }
    }
}

// Create HTML for a note card: name, tick, NIP-05, time; content; action bar. idPrefix avoids id clashes. replyToPubkey adds "Replying to [name]" when set. isBookmarked toggles bookmark icon.
export function createNoteCard(note, noteIndex, idPrefix, replyToPubkey, isBookmarked) {
    if (idPrefix === undefined) {
        idPrefix = '';
    }
    if (isBookmarked === undefined) {
        isBookmarked = isNoteBookmarked(note.id);
    }
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
                    <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safePubkey}" data-zap-event-id="${safeId}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"><span class="zap-count" data-event-id="${safeId}"></span></button>
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
export function createRepostCard(repostEvent, noteIndex, idPrefix) {
    if (idPrefix === undefined) {
        idPrefix = '';
    }
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const { name: reposterName } = getAuthorDisplay(repostEvent.pubkey);
    const safePubkey = escapeHtml(repostEvent.pubkey || '');
    const safeId = escapeHtml(repostEvent.id || '');
    var parsed = null;
    var innerContent = '';
    if (repostEvent.content && repostEvent.content.trim()) {
        try {
            parsed = JSON.parse(repostEvent.content);
            if (parsed && typeof parsed.content === 'string') {
                innerContent = processNoteContent(parsed.content);
            } else {
                innerContent = escapeHtml(t('note.repostedNote') || 'Reposted a note');
            }
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
    if (dataOriginalPubkey) {
        card.dataset.originalPubkey = dataOriginalPubkey;
    }
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
            <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safeOrigPubkey || ''}" data-zap-event-id="${parsed && parsed.id ? escapeHtml(parsed.id) : ''}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"><span class="zap-count" data-event-id="${parsed && parsed.id ? escapeHtml(parsed.id) : ''}"></span></button>
            <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
        </div>
    `;
    return card;
}

// Extract a tag value by name from a note's tags array. Returns the first match or ''.
function getTagValue(note, tagName) {
    if (!note || !note.tags) return '';
    var tag = note.tags.find(function(t) { return Array.isArray(t) && t[0] === tagName && t.length >= 2; });
    return tag ? tag[1] : '';
}

// Create a card for a kind 30023 long-form article.
export function createArticleCard(note, noteIndex, idPrefix) {
    if (idPrefix === undefined) idPrefix = '';
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var safeId = escapeHtml(note.id || '');
    var safePubkey = escapeHtml(note.pubkey || '');
    var verifyId = idPrefix + 'verify-' + noteIndex;
    var viewProfile = t('note.viewProfile') || 'View profile';
    var { name: displayName, nip05 } = getAuthorDisplay(note.pubkey);
    var title = getTagValue(note, 'title') || t('note.untitledArticle') || 'Untitled';
    var summary = getTagValue(note, 'summary') || '';
    var image = getTagValue(note, 'image') || '';
    var publishedAt = getTagValue(note, 'published_at');
    var time = formatTimestamp(publishedAt ? parseInt(publishedAt, 10) : note.created_at);
    var liked = isNoteLiked(note.id);
    var isBookmarked = isNoteBookmarked(note.id);

    var imageHtml = image
        ? '<div class="article-image"><img src="' + escapeHtml(image) + '" alt="" onerror="this.parentNode.style.display=\'none\'"></div>'
        : '';
    var summaryHtml = summary
        ? '<p class="article-summary">' + escapeHtml(summary) + '</p>'
        : '';

    var card = document.createElement('div');
    card.className = 'note-card note-card-article';
    card.dataset.noteIndex = noteIndex;
    card.dataset.noteId = safeId;
    card.dataset.pubkey = note.pubkey || '';
    card.innerHTML = `
        <div class="note-top-row">
            <button type="button" class="note-avatar note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}" aria-label="${escapeHtml(viewProfile)}"><span class="avatar-fallback">?</span></button>
            <div class="note-head">
                <div class="note-head-line">
                    <button type="button" class="note-author-name note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(displayName)}</button>
                    <span class="note-verification" id="${escapeHtml(verifyId)}" title=""><span class="verify-pending">·</span></span>
                    <span class="note-author-nip05" ${nip05 ? '' : 'style="display:none"'}>${escapeHtml(nip05)}</span>
                    <span class="note-time">${escapeHtml(time)}</span>
                </div>
                ${imageHtml}
                <h3 class="article-title">${escapeHtml(title)}</h3>
                ${summaryHtml}
                <div class="note-actions">
                    <button type="button" class="note-action" title="${escapeHtml(t('note.reply'))}" aria-label="${escapeHtml(t('note.reply'))}" data-action="reply" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/reply.svg" alt="${escapeHtml(t('note.reply'))}" class="icon-reply"></button>
                    <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safePubkey}" data-zap-event-id="${safeId}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"><span class="zap-count" data-event-id="${safeId}"></span></button>
                    <button type="button" class="note-action${liked ? ' liked' : ''}" title="${escapeHtml(t('note.like'))}" aria-label="${escapeHtml(t('note.like'))}" data-action="like" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/${liked ? 'heart-filled' : 'heart'}.svg" alt="${escapeHtml(t('note.like'))}" class="icon-heart"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
                    <button type="button" class="note-action" title="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" aria-label="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" data-action="bookmark" data-note-id="${safeId}"><img src="icons/${isBookmarked ? 'bookmark-filled' : 'bookmark'}.svg" alt="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" class="icon-bookmark"></button>
                </div>
            </div>
        </div>
    `;
    return card;
}

// Render Markdown to safe HTML. Supports headings, bold, italic, code, links, images, lists, blockquotes, and horizontal rules.
function renderMarkdown(md) {
    var html = escapeHtml(md);

    // Code blocks (``` ... ```)
    html = html.replace(/```([a-z]*)\n([\s\S]*?)```/g, function(_m, _lang, code) {
        return '<pre><code>' + code + '</code></pre>';
    });

    // Inline code
    html = html.replace(/`([^`\n]+)`/g, '<code>$1</code>');

    // Images: ![alt](url)
    html = html.replace(/!\[([^\]]*)\]\((https?:\/\/[^\s)]+)\)/g, function(_m, alt, url) {
        var safeUrl = sanitizeUrl(url);
        return safeUrl ? '<img src="' + safeUrl + '" alt="' + alt + '" loading="lazy">' : alt;
    });

    // Links: [text](url)
    html = html.replace(/\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g, function(_m, text, url) {
        var safeUrl = sanitizeUrl(url);
        return safeUrl ? '<a href="' + safeUrl + '" target="_blank" rel="noopener noreferrer">' + text + '</a>' : text;
    });

    // Headings (# to ######)
    html = html.replace(/^(#{1,6})\s+(.+)$/gm, function(_m, hashes, text) {
        var level = hashes.length;
        return '<h' + level + '>' + text + '</h' + level + '>';
    });

    // Bold **text** or __text__
    html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>');
    html = html.replace(/__(.+?)__/g, '<strong>$1</strong>');

    // Italic *text* or _text_ (not inside words)
    html = html.replace(/(?<!\w)\*([^*\n]+)\*(?!\w)/g, '<em>$1</em>');
    html = html.replace(/(?<!\w)_([^_\n]+)_(?!\w)/g, '<em>$1</em>');

    // Blockquotes
    html = html.replace(/^&gt;\s?(.*)$/gm, '<blockquote>$1</blockquote>');
    html = html.replace(/<\/blockquote>\n<blockquote>/g, '\n');

    // Horizontal rules
    html = html.replace(/^---+$/gm, '<hr>');

    // Unordered lists
    html = html.replace(/^[\*\-]\s+(.+)$/gm, '<li>$1</li>');
    html = html.replace(/(<li>.*<\/li>)/s, '<ul>$1</ul>');

    // Ordered lists
    html = html.replace(/^\d+\.\s+(.+)$/gm, '<li>$1</li>');

    // Bare URLs
    var urlRegex = /(?<!src="|href=")(https?:\/\/[^\s<]+)(?![^<]*>)/gi;
    html = html.replace(urlRegex, function(url) {
        var safeUrl = sanitizeUrl(url);
        return safeUrl ? '<a href="' + safeUrl + '" target="_blank" rel="noopener noreferrer">' + url + '</a>' : url;
    });

    // Paragraphs: convert double newlines into paragraph breaks
    html = html.replace(/\n\n+/g, '</p><p>');
    html = '<p>' + html + '</p>';
    html = html.replace(/<p>\s*<\/p>/g, '');
    html = html.replace(/<p>(<h[1-6]>)/g, '$1');
    html = html.replace(/(<\/h[1-6]>)<\/p>/g, '$1');
    html = html.replace(/<p>(<pre>)/g, '$1');
    html = html.replace(/(<\/pre>)<\/p>/g, '$1');
    html = html.replace(/<p>(<hr>)<\/p>/g, '$1');
    html = html.replace(/<p>(<ul>)/g, '$1');
    html = html.replace(/(<\/ul>)<\/p>/g, '$1');
    html = html.replace(/<p>(<blockquote>)/g, '$1');
    html = html.replace(/(<\/blockquote>)<\/p>/g, '$1');

    return html;
}

// Render a full article detail view (kind 30023). Returns a DOM element with the full rendered Markdown content.
export function renderArticleDetail(note, noteIndex, idPrefix) {
    if (idPrefix === undefined) idPrefix = '';
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var safeId = escapeHtml(note.id || '');
    var safePubkey = escapeHtml(note.pubkey || '');
    var verifyId = idPrefix + 'verify-' + noteIndex;
    var viewProfile = t('note.viewProfile') || 'View profile';
    var { name: displayName, nip05 } = getAuthorDisplay(note.pubkey);
    var title = getTagValue(note, 'title') || t('note.untitledArticle') || 'Untitled';
    var image = getTagValue(note, 'image') || '';
    var publishedAt = getTagValue(note, 'published_at');
    var time = formatTimestamp(publishedAt ? parseInt(publishedAt, 10) : note.created_at);
    var liked = isNoteLiked(note.id);
    var isBookmarked = isNoteBookmarked(note.id);

    var imageHtml = image
        ? '<div class="article-hero-image"><img src="' + escapeHtml(image) + '" alt="" onerror="this.parentNode.style.display=\'none\'"></div>'
        : '';

    var renderedContent = renderMarkdown(note.content || '');

    var card = document.createElement('div');
    card.className = 'note-card note-card-article article-detail';
    card.dataset.noteIndex = noteIndex;
    card.dataset.noteId = safeId;
    card.dataset.pubkey = note.pubkey || '';
    card.innerHTML = `
        <div class="note-top-row">
            <button type="button" class="note-avatar note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}" aria-label="${escapeHtml(viewProfile)}"><span class="avatar-fallback">?</span></button>
            <div class="note-head">
                <div class="note-head-line">
                    <button type="button" class="note-author-name note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(displayName)}</button>
                    <span class="note-verification" id="${escapeHtml(verifyId)}" title=""><span class="verify-pending">·</span></span>
                    <span class="note-author-nip05" ${nip05 ? '' : 'style="display:none"'}>${escapeHtml(nip05)}</span>
                    <span class="note-time">${escapeHtml(time)}</span>
                </div>
            </div>
        </div>
        ${imageHtml}
        <h2 class="article-detail-title">${escapeHtml(title)}</h2>
        <div class="article-content">${renderedContent}</div>
        <div class="note-actions">
            <button type="button" class="note-action" title="${escapeHtml(t('note.reply'))}" aria-label="${escapeHtml(t('note.reply'))}" data-action="reply" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/reply.svg" alt="${escapeHtml(t('note.reply'))}" class="icon-reply"></button>
            <button type="button" class="note-action zap-muted" title="${escapeHtml(t('note.zapNoWallet'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap" data-zap-target-pubkey="${safePubkey}" data-zap-event-id="${safeId}" disabled><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"><span class="zap-count" data-event-id="${safeId}"></span></button>
            <button type="button" class="note-action${liked ? ' liked' : ''}" title="${escapeHtml(t('note.like'))}" aria-label="${escapeHtml(t('note.like'))}" data-action="like" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/${liked ? 'heart-filled' : 'heart'}.svg" alt="${escapeHtml(t('note.like'))}" class="icon-heart"></button>
            <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
            <button type="button" class="note-action" title="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" aria-label="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" data-action="bookmark" data-note-id="${safeId}"><img src="icons/${isBookmarked ? 'bookmark-filled' : 'bookmark'}.svg" alt="${escapeHtml(isBookmarked ? (t('note.unbookmark') || 'Unbookmark') : t('note.bookmark'))}" class="icon-bookmark"></button>
        </div>
    `;
    return card;
}

// Verify a note's signature. idPrefix optional (e.g. 'profile-' for profile feed).
export async function verifyNote(note, noteIndex, idPrefix) {
    if (idPrefix === undefined) {
        idPrefix = '';
    }
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
export function updateVerificationBadge(noteIndex, result, idPrefix, badgeSuffix) {
    if (idPrefix === undefined) {
        idPrefix = '';
    }
    var suffix = badgeSuffix !== undefined ? badgeSuffix : 'verify-';
    const badgeEl = document.getElementById(idPrefix + suffix + noteIndex);
    if (!badgeEl) {
        return;
    }
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
export async function verifyRepostOriginal(repostEvent, noteIndex, idPrefix) {
    if (idPrefix === undefined) {
        idPrefix = '';
    }
    if (!repostEvent.content || !repostEvent.content.trim()) {
        return;
    }
    try {
        var parsed = JSON.parse(repostEvent.content);
        if (!parsed || !parsed.pubkey) {
            return;
        }
        var noteJson = JSON.stringify(parsed);
        var resultJson = await invoke('verify_event', { eventJson: noteJson });
        if (resultJson) {
            var result = JSON.parse(resultJson);
            updateVerificationBadge(noteIndex, result, idPrefix, 'repost-orig-verify-');
        }
    } catch (_) {}
}

// Process note content - find and embed images/videos and nostr: URIs.
// Content is HTML-escaped first to neutralize any injected tags/scripts,
// then safe URLs are converted to media elements and links.
// depth: recursion depth for nested nostr: note embeds (0 = top-level)
export function processNoteContent(content, depth) {
    if (depth === undefined) {
        depth = 0;
    }

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

    // Standalone URLs (sole content of a line) are rendered as media via
    // progressive probing: try <img> first (safe, sanitises SVG, checks
    // Content-Type).  On error, try <video>.  On error again, fall back to
    // a plain link.  No file-extension guessing, no script execution risk.
    var lines = html.split('\n');
    for (var li = 0; li < lines.length; li++) {
        var trimmed = lines[li].trim();
        var urlOnly = trimmed.match(/^(https?:\/\/[^\s<]+)$/);
        if (urlOnly) {
            var safe = sanitizeUrl(urlOnly[1]);
            if (safe) {
                lines[li] = '<img src="' + safe + '" class="note-media-probe" loading="lazy"'
                    + ' onerror="this.onerror=null;var v=document.createElement(\'video\');v.src=this.src;v.controls=true;v.preload=\'metadata\';v.className=\'note-media-probe\';v.onerror=function(){var a=document.createElement(\'a\');a.href=v.src;a.target=\'_blank\';a.rel=\'noopener noreferrer\';a.textContent=v.src;v.replaceWith(a)};this.replaceWith(v)">';
            }
        }
    }
    html = lines.join('\n');

    // Convert remaining plain URLs to links (skip ones already in tags)
    const urlRegex = /(?<!src="|href="|data=")(https?:\/\/[^\s<]+)(?![^<]*>)/gi;
    html = html.replace(urlRegex, function(match) {
        var safe = sanitizeUrl(match);
        return safe ? '<a href="' + safe + '" target="_blank" rel="noopener noreferrer">' + escapeHtml(match) + '</a>' : escapeHtml(match);
    });

    return html;
}

// Create a compact embedded note card for nostr: URI references.
// No action bar, clickable to open full note detail.
export function createEmbeddedNoteCard(note, depth) {
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
export async function resolveNostrEmbeds(container) {
    if (!container) {
        return;
    }

    // --- 1. Collect all placeholders and profile links ---
    var embedPlaceholders = container.querySelectorAll('.nostr-embed-placeholder[data-nostr-ref]');
    var profileLinks = container.querySelectorAll('.nostr-profile-link[data-nostr-ref]');
    if (embedPlaceholders.length === 0 && profileLinks.length === 0) {
        return;
    }

    // --- 2. Decode all bech32 references in parallel ---
    var decoded = {};  // bech32 -> decoded JSON object
    var allRefs = new Set();
    embedPlaceholders.forEach(function(el) { allRefs.add(el.dataset.nostrRef); });
    profileLinks.forEach(function(el) { allRefs.add(el.dataset.nostrRef); });

    await Promise.all(Array.from(allRefs).map(async function(ref) {
        try {
            var json = await invoke('decode_nostr_uri', { bech32_str: ref });
            if (json) {
                decoded[ref] = JSON.parse(json);
            }
        } catch (e) {
            console.warn('[Plume] Failed to decode nostr URI:', ref, e);
        }
    }));

    // --- 3. Resolve profile links (npub / nprofile) ---
    var profilePubkeys = new Set();
    profileLinks.forEach(function(link) {
        var d = decoded[link.dataset.nostrRef];
        if (!d) {
            return;
        }
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
        if (!pk) {
            return;
        }
        var cached = state.profileCache[pk];
        var name = (cached && cached.name) ? cached.name : shortenKey(pk);
        link.textContent = '@' + name;
    });

    // --- 4. Resolve note/event embeds ---
    var eventIdsToFetch = new Set();
    var embedInfo = {};  // bech32 -> { eventId, relayHints }
    embedPlaceholders.forEach(function(el) {
        var d = decoded[el.dataset.nostrRef];
        if (!d) {
            return;
        }
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
        if (!info) {
            el.remove();
            return;
        }
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
export function displayNotes(notes) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const container = document.getElementById('notes-container');
    container.innerHTML = '';
    var hideEncrypted = !state.config || state.config.hide_encrypted_notes !== false;
    notes = (notes || []).filter(function(n) {
        if (isNoteMuted(n)) {
            return false;
        }
        if (hideEncrypted && n.kind === 1 && isContentUnreadable(n.content)) {
            return false;
        }
        return true;
    });
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
        if (note.kind === 1) {
            const card = createNoteCard(note, noteIndex);
            container.appendChild(card);
            notesToVerify.push({ note, index: noteIndex });
            noteIndex++;
        } else if (note.kind === 30023) {
            const card = createArticleCard(note, noteIndex);
            container.appendChild(card);
            notesToVerify.push({ note, index: noteIndex });
            noteIndex++;
        }
    });

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
    fetchAndDisplayZapTotals();
}

// Verify notes asynchronously
export async function verifyNotesAsync(notesToVerify) {
    // Verify in batches to avoid overwhelming the backend
    for (const { note, index } of notesToVerify) {
        // Don't await - let them run in parallel
        verifyNote(note, index);

        // Small delay to avoid hammering the backend
        await new Promise(resolve => setTimeout(resolve, 10));
    }
}
