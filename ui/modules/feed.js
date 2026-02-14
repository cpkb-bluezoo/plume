/*
 * modules/feed.js
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

import { state, getEffectiveRelays, FEED_LIMIT, POLL_INTERVAL_MS } from './state.js';
import { invoke } from './tauri.js';
import { escapeHtml, showRelayHealthBanner } from './utils.js';
import { isNoteMuted, isContentUnreadable } from './muting.js';
import { createNoteCard, getReplyToPubkey, verifyNote, ensureProfilesForNotes, resolveNostrEmbeds, displayNotes } from './notes.js';
import { updateFeedInitialState } from './config.js';

let feedStreamNoteIndex = 0;
var feedNoteQueue = [];
var feedNoteDrainScheduled = false;

// Returns list of hex pubkeys for "follows" mode, or null for firehose.
// Uses the locally cached following list from config first for instant results,
// then falls back to fetching from relays (which also updates the local cache).
export async function getHomeFeedAuthors() {
    if (state.homeFeedMode !== 'follows') {
        return null;
    }
    if (!state.config || !state.config.public_key) {
        return null;
    }
    // Use locally cached following list if available
    if (state.config.following && state.config.following.length > 0) {
        return state.config.following.slice();
    }
    // Fall back to fetching from relays (also caches locally via backend)
    try {
        const json = await invoke('fetch_own_following');
        if (!json) {
            return null;
        }
        const data = JSON.parse(json);
        const contacts = data.contacts || [];
        if (contacts.length === 0) {
            return null;
        }
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
export async function fetchFeedNotes(relayUrls, authors, since, profileFeed) {
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

// Merge new notes into state.notes. isIncremental: true = append new ones below the fold; false = replace and sort.
export function mergeNotesIntoState(newNotes, isIncremental) {
    if (!newNotes || newNotes.length === 0 && !isIncremental) {
        return;
    }
    const seen = new Set(state.notes.map(n => n.id));
    if (!isIncremental) {
        state.notes = newNotes.slice();
        state.notes.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
        return;
    }
    const added = newNotes.filter(n => n.id && !seen.has(n.id));
    if (added.length === 0) {
        return;
    }
    added.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    state.notes = state.notes.concat(added);
}

// Start initial feed fetch: async stream (each note shown as it arrives) when in Tauri; else batch fetch.
export async function startInitialFeedFetch() {
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
                if (state.feedPollIntervalId) {
                    clearInterval(state.feedPollIntervalId);
                }
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
                if (!authors || authors.length === 0) {
                    authors = null;
                }
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
            if (!authors || authors.length === 0) {
                authors = null;
            }
        }
        const notes = await fetchFeedNotes(effectiveRelays, authors, null);
        mergeNotesIntoState(notes, false);
        displayNotes(state.notes);
        state.initialFeedLoadDone = true;
        // Start periodic polling for new notes (both firehose and follows modes)
        if (state.feedPollIntervalId) {
            clearInterval(state.feedPollIntervalId);
        }
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
export async function pollForNewNotes() {
    const relays = getEffectiveRelays();
    if (!relays.length || state.loading) {
        return;
    }
    const authors = await getHomeFeedAuthors();
    if (!authors || authors.length === 0) {
        return;
    }
    const since = state.notes.length
        ? Math.max(...state.notes.map(n => n.created_at || 0))
        : 0;
    try {
        const notes = await fetchFeedNotes(relays, authors, since);
        if (notes.length === 0) {
            return;
        }
        mergeNotesIntoState(notes, true);
        displayNotes(state.notes);
    } catch (e) {
        console.error('Feed poll failed:', e);
        if (typeof e === 'string' && e.indexOf('Could not reach any') !== -1) {
            showRelayHealthBanner();
        }
    }
}

// Firehose: fetch new notes when user opens Home (no auto-poll).
export async function fetchNotesFirehoseOnHomeClick() {
    const relays = getEffectiveRelays();
    if (!relays.length || state.loading) {
        return;
    }
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
        if (typeof e === 'string' && e.indexOf('Could not reach any') !== -1) {
            showRelayHealthBanner();
        }
    } finally {
        state.loading = false;
    }
}

// Show a message in the notes container
export function showMessage(message) {
    const container = document.getElementById('notes-container');
    container.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

export function scheduleFeedNoteDrain() {
    if (feedNoteDrainScheduled) {
        return;
    }
    feedNoteDrainScheduled = true;
    requestAnimationFrame(function drainFeedNoteQueue() {
        feedNoteDrainScheduled = false;
        if (feedNoteQueue.length === 0) {
            return;
        }
        var note = feedNoteQueue.shift();
        var result = appendNoteCardToFeedSync(note);
        if (result.index !== -1) {
            ensureProfilesForNotes([note]);
            verifyNote(note, result.index);
            if (result.card) {
                resolveNostrEmbeds(result.card);
            }
        }
        if (feedNoteQueue.length > 0) {
            scheduleFeedNoteDrain();
        }
    });
}

// Append a single note card to the feed (streaming). Dedupes by id; inserts in sorted position.
// Returns { index, card } where index is -1 if skipped.
export function appendNoteCardToFeedSync(note) {
    if (!note || note.kind !== 1) {
        return { index: -1, card: null };
    }
    if (isNoteMuted(note)) {
        return { index: -1, card: null };
    }
    if ((!state.config || state.config.hide_encrypted_notes !== false) && isContentUnreadable(note.content)) {
        return { index: -1, card: null };
    }
    if (state.notes.some(function(n) { return n.id === note.id; })) {
        return { index: -1, card: null };
    }

    const container = document.getElementById('notes-container');
    if (!container) {
        return { index: -1, card: null };
    }

    state.notes.push(note);
    state.notes.sort(function(a, b) { return (b.created_at || 0) - (a.created_at || 0); });
    const idx = state.notes.findIndex(function(n) { return n.id === note.id; });

    const placeholder = document.getElementById('feed-loading') || document.getElementById('feed-welcome');
    if (placeholder) {
        placeholder.remove();
    }

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

export function appendNoteCardToFeed(note) {
    if (!note || note.kind !== 1) {
        return;
    }
    if ((!state.config || state.config.hide_encrypted_notes !== false) && isContentUnreadable(note.content)) {
        return;
    }
    if (state.notes.some(function(n) { return n.id === note.id; })) {
        return;
    }
    feedNoteQueue.push(note);
    scheduleFeedNoteDrain();
}
