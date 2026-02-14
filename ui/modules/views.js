/*
 * modules/views.js
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
import { escapeHtml } from './utils.js';
import { updateFeedInitialState } from './config.js';
import { isNoteMuted } from './muting.js';
import { createNoteCard, getReplyToPubkey, getParentEventId, verifyNote, ensureProfilesForNotes, resolveNostrEmbeds, displayNotes, setCardAvatar } from './notes.js';
import { startInitialFeedFetch, pollForNewNotes, fetchNotesFirehoseOnHomeClick } from './feed.js';
import { fetchProfile, updateProfileDisplay, loadProfileFeed } from './profile.js';
import { fetchFollowingAndFollowers } from './follows.js';
import { loadMessagesView, updateMessagesNavUnread } from './messages.js';
import { showSettingsPanel } from './settings.js';

// ============================================================
// View Management
// ============================================================

// Load and render the bookmarks view (fetch events by ids from config)
export async function loadBookmarksView() {
    const container = document.getElementById('bookmarks-container');
    if (!container) {
        return;
    }
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
                if (cardEl) {
                    setCardAvatar(cardEl, profile.picture);
                }
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
export function buildReplyThread(replies, subjectId) {
    var byParent = {};
    byParent[subjectId] = [];
    replies.forEach(function(note) {
        var pid = getParentEventId(note) || subjectId;
        if (!byParent[pid]) {
            byParent[pid] = [];
        }
        byParent[pid].push(note);
    });
    var out = [];
    function addChildren(parentId, indent) {
        var list = byParent[parentId];
        if (!list) {
            return;
        }
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
export async function openNoteDetail(noteIdOrNote) {
    var noteId = typeof noteIdOrNote === 'string' ? noteIdOrNote : (noteIdOrNote && noteIdOrNote.id);
    if (!noteId) {
        return;
    }
    if (state.currentView === 'note-detail' && state.noteDetailSubjectId === noteId) {
        return;
    }
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
    if (ancestorsEl) {
        ancestorsEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('noteDetail.loading')) + '</p></div>';
    }
    if (subjectWrap) {
        subjectWrap.innerHTML = '';
    }
    if (repliesEl) {
        repliesEl.innerHTML = '';
    }

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
        if (ancestorsEl) {
            ancestorsEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('feed.feedFailed')) + '</p></div>';
        }
        return;
    }
    state.noteDetailSubject = subject;

    var ancestors = [];
    var current = subject;
    var seen = {};
    while (current) {
        var parentId = getParentEventId(current);
        if (!parentId || seen[parentId]) {
            break;
        }
        seen[parentId] = true;
        try {
            var r = await invoke('fetch_events_by_ids', { relay_urls: relays, ids: [parentId] });
            var a = r ? JSON.parse(r) : [];
            if (!a.length) {
                break;
            }
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

export function renderNoteDetailPage() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var ancestorsEl = document.getElementById('note-detail-ancestors');
    var subjectWrap = document.getElementById('note-detail-subject-wrap');
    var repliesEl = document.getElementById('note-detail-replies');
    var replyContent = document.getElementById('note-detail-reply-content');
    if (!ancestorsEl || !subjectWrap || !repliesEl) {
        return;
    }

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

    if (replyContent) {
        replyContent.value = '';
    }

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
export function switchView(viewName) {
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
        // Don't clear unread badge here — it is cleared when the user actually
        // opens a conversation (selectConversation), not just by clicking the icon.
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
