/*
 * modules/search.js
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
import { escapeHtml, formatTimestamp } from './utils.js';
import { createNoteCard, createArticleCard, verifyNote, ensureProfilesForNotes, resolveNostrEmbeds, fetchAndDisplayZapTotals, getReplyToPubkey } from './notes.js';

var searchBound = false;

var defaultHashtags = [
    'nostr', 'bitcoin', 'lightning', 'zap', 'art', 'music',
    'photography', 'tech', 'news', 'dev', 'opensource', 'privacy',
    'freedom', 'meme', 'grownostr', 'plebchain'
];

export function initSearch() {
    if (searchBound) return;
    searchBound = true;

    var input = document.getElementById('search-input');
    var clearBtn = document.getElementById('search-clear-btn');
    var backBtn = document.getElementById('search-results-back');

    if (input) {
        var debounceTimer = null;
        input.addEventListener('input', function() {
            clearBtn.style.display = input.value ? '' : 'none';
            clearTimeout(debounceTimer);
            if (input.value.trim().length >= 2) {
                debounceTimer = setTimeout(function() {
                    performQuickSearch(input.value.trim());
                }, 500);
            }
        });
        input.addEventListener('keydown', function(e) {
            if (e.key === 'Enter') {
                e.preventDefault();
                clearTimeout(debounceTimer);
                var q = input.value.trim();
                if (q.length > 0) {
                    if (q.startsWith('#')) {
                        performHashtagSearch(q);
                    } else {
                        performQuickSearch(q);
                    }
                }
            }
        });
    }

    if (clearBtn) {
        clearBtn.addEventListener('click', function() {
            if (input) {
                input.value = '';
                clearBtn.style.display = 'none';
            }
            showSearchMainPanel();
        });
    }

    if (backBtn) {
        backBtn.addEventListener('click', function() {
            showSearchMainPanel();
        });
    }

    // Tab switching
    document.querySelectorAll('.search-tab[data-search-tab]').forEach(function(tab) {
        tab.addEventListener('click', function() {
            document.querySelectorAll('.search-tab').forEach(function(t) { t.classList.remove('active'); });
            tab.classList.add('active');
            var target = tab.dataset.searchTab;
            document.querySelectorAll('.search-tab-panel').forEach(function(p) { p.style.display = 'none'; });
            var panel = document.getElementById('search-tab-' + target);
            if (panel) panel.style.display = '';
        });
    });

    // Advanced search form
    var advForm = document.getElementById('search-advanced-form');
    if (advForm) {
        advForm.addEventListener('submit', function(e) {
            e.preventDefault();
            performAdvancedSearch();
        });
    }

    populateHashtagGrid();
    populateAuthorDropdowns();
}

export function showSearchView() {
    showSearchMainPanel();
    var input = document.getElementById('search-input');
    if (input) {
        input.value = '';
        document.getElementById('search-clear-btn').style.display = 'none';
        setTimeout(function() { input.focus(); }, 100);
    }
}

function showSearchMainPanel() {
    var main = document.getElementById('search-main-panel');
    var results = document.getElementById('search-results-panel');
    if (main) main.style.display = '';
    if (results) results.style.display = 'none';
}

function showSearchResults(label) {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var main = document.getElementById('search-main-panel');
    var results = document.getElementById('search-results-panel');
    var labelEl = document.getElementById('search-results-label');
    if (main) main.style.display = 'none';
    if (results) results.style.display = '';
    if (labelEl) labelEl.textContent = label || '';
}

function populateHashtagGrid() {
    var grid = document.getElementById('search-hashtag-grid');
    if (!grid) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    grid.innerHTML = '';
    defaultHashtags.forEach(function(tag) {
        var card = document.createElement('div');
        card.className = 'hashtag-card';
        card.dataset.hashtag = tag;
        card.innerHTML = '<div class="hashtag-card-tag">#' + escapeHtml(tag) + '</div>' +
            '<div class="hashtag-card-count"></div>';
        card.addEventListener('click', function() {
            performHashtagSearch('#' + tag);
        });
        grid.appendChild(card);
    });
}

function populateAuthorDropdowns() {
    var authorSelect = document.getElementById('search-adv-author');
    var replySelect = document.getElementById('search-adv-replying-to');
    if (!authorSelect || !replySelect) return;
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };

    // Add "Following" contacts as options
    var contacts = state.config && Array.isArray(state.config.contacts) ? state.config.contacts : [];
    var entries = [];
    contacts.forEach(function(pubkey) {
        var profile = state.profileCache && state.profileCache[pubkey];
        var name = profile && profile.name ? profile.name : pubkey.substring(0, 12) + '...';
        entries.push({ pubkey: pubkey, name: name });
    });
    entries.sort(function(a, b) { return a.name.localeCompare(b.name); });

    [authorSelect, replySelect].forEach(function(select) {
        while (select.options.length > 1) {
            select.remove(1);
        }
        entries.forEach(function(entry) {
            var opt = document.createElement('option');
            opt.value = entry.pubkey;
            opt.textContent = entry.name;
            select.appendChild(opt);
        });
    });
}

function performQuickSearch(query) {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var relays = getEffectiveRelays();
    if (!relays.length) return;

    showSearchResults('"' + query + '"');
    var container = document.getElementById('search-results-container');
    if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.searching') || 'Searching...') + '</p></div>';

    invoke('search_notes', {
        relay_urls: relays,
        query: query,
        limit: FEED_LIMIT,
        authors: null,
        since: null,
        until: null,
        kind_filter: null
    }).then(function(json) {
        var notes = json ? JSON.parse(json) : [];
        displaySearchResults(notes, '"' + query + '"');
    }).catch(function(err) {
        if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.failed') || 'Search failed') + '</p></div>';
        console.error('Search failed:', err);
    });
}

function performHashtagSearch(hashtag) {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var tag = hashtag.trim().replace(/^#/, '');
    var relays = getEffectiveRelays();
    if (!relays.length || !tag) return;

    var input = document.getElementById('search-input');
    if (input) {
        input.value = '#' + tag;
        document.getElementById('search-clear-btn').style.display = '';
    }

    showSearchResults('#' + tag);
    var container = document.getElementById('search-results-container');
    if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.searching') || 'Searching...') + '</p></div>';

    invoke('search_hashtag', {
        relay_urls: relays,
        hashtag: tag,
        limit: FEED_LIMIT
    }).then(function(json) {
        var notes = json ? JSON.parse(json) : [];
        displaySearchResults(notes, '#' + tag);
    }).catch(function(err) {
        if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.failed') || 'Search failed') + '</p></div>';
        console.error('Hashtag search failed:', err);
    });
}

function performAdvancedSearch() {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var relays = getEffectiveRelays();
    if (!relays.length) return;

    var include = (document.getElementById('search-adv-include') || {}).value || '';
    var exclude = (document.getElementById('search-adv-exclude') || {}).value || '';
    var kindFilter = (document.getElementById('search-adv-kind') || {}).value || null;
    var author = (document.getElementById('search-adv-author') || {}).value || null;
    var timeVal = (document.getElementById('search-adv-time') || {}).value || null;

    var query = include.trim();
    if (!query && !author) {
        return;
    }
    if (!query) {
        query = '*';
    }

    var since = null;
    if (timeVal) {
        var now = Math.floor(Date.now() / 1000);
        var offsets = { '1h': 3600, '24h': 86400, '7d': 604800, '30d': 2592000 };
        if (offsets[timeVal]) {
            since = now - offsets[timeVal];
        }
    }

    var authors = author ? [author] : null;
    var excludeWords = exclude.trim().toLowerCase().split(/\s+/).filter(function(w) { return w.length > 0; });

    var labelParts = [query !== '*' ? '"' + query + '"' : ''];
    if (author) {
        var opt = document.querySelector('#search-adv-author option[value="' + author + '"]');
        if (opt) labelParts.push('by ' + opt.textContent);
    }
    var label = labelParts.filter(function(p) { return p; }).join(' ');

    showSearchResults(label);
    var container = document.getElementById('search-results-container');
    if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.searching') || 'Searching...') + '</p></div>';

    invoke('search_notes', {
        relay_urls: relays,
        query: query,
        limit: FEED_LIMIT,
        authors: authors,
        since: since,
        until: null,
        kind_filter: kindFilter || null
    }).then(function(json) {
        var notes = json ? JSON.parse(json) : [];
        if (excludeWords.length > 0) {
            notes = notes.filter(function(n) {
                var content = (n.content || '').toLowerCase();
                return !excludeWords.some(function(w) { return content.indexOf(w) !== -1; });
            });
        }
        displaySearchResults(notes, label);
    }).catch(function(err) {
        if (container) container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.failed') || 'Search failed') + '</p></div>';
        console.error('Advanced search failed:', err);
    });
}

function displaySearchResults(notes, label) {
    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var container = document.getElementById('search-results-container');
    var labelEl = document.getElementById('search-results-label');
    if (!container) return;
    container.innerHTML = '';

    if (labelEl) {
        labelEl.textContent = label + ' (' + notes.length + ')';
    }

    if (notes.length === 0) {
        container.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(t('search.noResults') || 'No results found') + '</p></div>';
        return;
    }

    state.searchNotes = notes;
    var noteIndex = 0;
    notes.forEach(function(note) {
        var card;
        if (note.kind === 30023) {
            card = createArticleCard(note, noteIndex, 'search-');
        } else {
            var replyToPubkey = getReplyToPubkey(note);
            card = createNoteCard(note, noteIndex, 'search-', replyToPubkey);
        }
        container.appendChild(card);
        verifyNote(note, noteIndex, 'search-');
        noteIndex++;
    });
    ensureProfilesForNotes(notes);
    resolveNostrEmbeds(container);
    fetchAndDisplayZapTotals();
}
