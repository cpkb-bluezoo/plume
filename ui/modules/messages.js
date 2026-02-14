/*
 * modules/messages.js
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

// Guard against concurrent sendMessage calls
var sendInProgress = false;

/// Start the DM stream (background sync of messages from relays).
/// Safe to call multiple times — only starts once.
export function startDmStream() {
    if (!state.dmStreamStarted && getEffectiveRelays().length > 0) {
        state.dmStreamStarted = true;
        invoke('start_dm_stream').catch(function(e) { console.warn('start_dm_stream:', e); });
    }
}

function shortenPubkey(pubkey) {
    if (!pubkey || pubkey.length < 20) {
        return pubkey || '';
    }
    return pubkey.slice(0, 8) + '…' + pubkey.slice(-8);
}

export async function loadMessagesView() {
    const listEl = document.querySelector('.messages-list');
    const emptyEl = document.querySelector('.messages-chat-empty');
    const paneEl = document.querySelector('.messages-chat-pane');
    if (!listEl) {
        return;
    }

    // Reset stale state from previous profile / session
    state.selectedConversation = null;
    if (paneEl) {
        paneEl.style.display = 'none';
        var msgContainer = document.getElementById('messages-chat-messages');
        if (msgContainer) {
            msgContainer.innerHTML = '';
        }
    }
    if (emptyEl) {
        emptyEl.style.display = 'flex';
    }

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
            var uncachedPubkeys = [];
            for (let i = 0; i < conversations.length; i++) {
                const c = conversations[i];
                const other = c.other_pubkey || '';
                const cached = state.profileCache && state.profileCache[other] ? state.profileCache[other] : null;
                const name = (cached && cached.name) ? escapeHtml(cached.name) : shortenPubkey(other);
                const picture = cached && cached.picture ? cached.picture : null;
                const ts = c.last_created_at ? new Date(c.last_created_at * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';
                var avatarHtml = picture
                    ? '<img src="' + escapeHtml(picture) + '" alt="" class="conversation-avatar" onerror="this.style.display=\'none\';this.nextElementSibling.style.display=\'flex\'">'
                      + '<span class="conversation-avatar conversation-avatar-placeholder" style="display:none"><img src="icons/user.svg" alt="" class="icon-sm"></span>'
                    : '<span class="conversation-avatar conversation-avatar-placeholder"><img src="icons/user.svg" alt="" class="icon-sm"></span>';
                html += '<div class="conversation-item" role="button" tabindex="0" data-other-pubkey="' + escapeHtml(other) + '" title="' + escapeHtml(other) + '">'
                    + avatarHtml
                    + '<div class="conversation-item-info"><span class="conversation-item-name">' + name + '</span>'
                    + (ts ? '<span class="conversation-item-meta">' + escapeHtml(ts) + '</span>' : '')
                    + '</div></div>';
                if (!cached && other) {
                    uncachedPubkeys.push(other);
                }
            }
            listEl.innerHTML = html;

            // Fetch profiles for conversation partners not yet in cache,
            // then update the DOM items in-place once they arrive.
            if (uncachedPubkeys.length > 0) {
                var relays = getEffectiveRelays();
                uncachedPubkeys.forEach(function(pubkey) {
                    invoke('fetch_profile', { pubkey: pubkey, relay_urls: relays })
                        .then(function(profileJson) {
                            if (!profileJson || profileJson === '{}') {
                                return;
                            }
                            var profile = JSON.parse(profileJson);
                            state.profileCache[pubkey] = {
                                name: profile.name || null,
                                nip05: profile.nip05 || null,
                                picture: profile.picture || null,
                                lud16: profile.lud16 || null
                            };
                            // Update the conversation item in the DOM
                            var item = listEl.querySelector('[data-other-pubkey="' + pubkey.replace(/"/g, '\\"') + '"]');
                            if (item) {
                                var nameEl = item.querySelector('.conversation-item-name');
                                if (nameEl && profile.name) {
                                    nameEl.textContent = profile.name;
                                }
                                if (profile.picture) {
                                    // Remove all existing avatar elements and insert fresh img + fallback
                                    var oldAvatars = item.querySelectorAll('.conversation-avatar');
                                    var insertBefore = oldAvatars.length > 0 ? oldAvatars[0] : null;
                                    var newHtml = '<img src="' + escapeHtml(profile.picture) + '" alt="" class="conversation-avatar" onerror="this.style.display=\'none\';this.nextElementSibling.style.display=\'flex\'">'
                                        + '<span class="conversation-avatar conversation-avatar-placeholder" style="display:none"><img src="icons/user.svg" alt="" class="icon-sm"></span>';
                                    if (insertBefore) {
                                        insertBefore.insertAdjacentHTML('beforebegin', newHtml);
                                    }
                                    oldAvatars.forEach(function(el) { el.remove(); });
                                }
                            }
                        })
                        .catch(function(e) {
                            console.warn('Failed to fetch profile for conversation partner:', pubkey, e);
                        });
                });
            }
        }

        // Ensure DM stream is running (in case it wasn't started at app init)
        startDmStream();

        state.openConversationWith = null;
        if (openWith) {
            selectConversation(openWith);
        }
    } catch (e) {
        console.error('get_conversations failed:', e);
        listEl.innerHTML = '<div class="placeholder-message"><p>' + escapeHtml(String(e && e.message ? e.message : e)) + '</p></div>';
    }
}

export function selectConversation(otherPubkeyHex) {
    state.selectedConversation = otherPubkeyHex;
    // User is actively reading messages — clear the unread badge and persist the read timestamp
    if (otherPubkeyHex) {
        state.unreadMessageCount = 0;
        updateMessagesNavUnread();
        invoke('mark_dms_read').catch(function(e) {
            console.warn('mark_dms_read:', e);
        });
    }
    const paneEl = document.querySelector('.messages-chat-pane');
    const emptyEl = document.querySelector('.messages-chat-empty');
    document.querySelectorAll('.conversation-item').forEach(function(el) {
        el.classList.toggle('active', (el.getAttribute('data-other-pubkey') || '').toLowerCase() === (otherPubkeyHex || '').toLowerCase());
    });
    if (!otherPubkeyHex) {
        if (paneEl) {
            paneEl.style.display = 'none';
        }
        if (emptyEl) {
            emptyEl.style.display = 'flex';
        }
        return;
    }
    if (emptyEl) {
        emptyEl.style.display = 'none';
    }
    if (paneEl) {
        paneEl.style.display = 'flex';
    }
    // Clear any leftover text and enforce disabled state for the send button
    var msgInput = document.getElementById('message-input');
    if (msgInput) {
        msgInput.value = '';
    }
    updateSendButtonState();
    loadConversationMessages(otherPubkeyHex);
}

export async function loadConversationMessages(otherPubkeyHex) {
    const container = document.getElementById('messages-chat-messages');
    if (!container) {
        return;
    }
    container.innerHTML = '<p class="placeholder-message">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('noteDetail.loading') : 'Loading…') + '</p>';
    try {
        const json = await invoke('get_messages', { other_pubkey_hex: otherPubkeyHex });
        const messages = json ? JSON.parse(json) : [];
        renderMessages(container, messages);
    } catch (e) {
        console.error('get_messages failed:', e);
        container.innerHTML = '<p class="placeholder-message">' + escapeHtml(String(e && e.message ? e.message : e)) + '</p>';
    }
}

export function renderMessages(container, messages) {
    if (!container) {
        return;
    }
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

export async function sendMessage() {
    if (sendInProgress) {
        return;
    }
    const other = state.selectedConversation;
    const input = document.getElementById('message-input');
    if (!other || !input) {
        return;
    }
    const text = (input.value || '').trim();
    if (!text) {
        return;
    }

    // Lock immediately — disable button and clear field before the async send
    // so a second click/Enter cannot produce a duplicate.
    sendInProgress = true;
    var sendBtn = document.getElementById('message-send-btn');
    if (sendBtn) {
        sendBtn.disabled = true;
    }
    input.value = '';
    updateSendButtonState();

    try {
        await invoke('send_dm', { recipient_pubkey: other, plaintext: text });
        // Reload the full conversation from the local store so the new message
        // appears exactly once (no optimistic append that would duplicate with
        // the relay echo arriving via the DM stream).
        await loadConversationMessages(other);
    } catch (e) {
        console.error('send_dm failed:', e);
        // Put the text back so the user can retry
        input.value = text;
        updateSendButtonState();
        var errMsg = e && e.message ? e.message : String(e);
        var container = document.getElementById('messages-chat-messages');
        if (container) {
            var errDiv = document.createElement('div');
            errDiv.className = 'message-error';
            errDiv.textContent = errMsg;
            container.appendChild(errDiv);
            container.scrollTop = container.scrollHeight;
        }
    } finally {
        sendInProgress = false;
        if (sendBtn) {
            sendBtn.disabled = !input.value.trim();
        }
    }
}

/// Enable/disable the send button based on whether the input has text.
export function updateSendButtonState() {
    var input = document.getElementById('message-input');
    var sendBtn = document.getElementById('message-send-btn');
    if (sendBtn && !sendInProgress) {
        sendBtn.disabled = !input || !input.value.trim();
    }
}

/// Check for unread DMs on startup by comparing conversation timestamps
/// against the persisted dm_last_read_at.  Sets the badge accordingly.
export function checkUnreadDmsOnStartup() {
    invoke('count_unread_dms')
        .then(function(count) {
            if (count > 0) {
                state.unreadMessageCount = count;
                updateMessagesNavUnread();
            }
        })
        .catch(function(e) {
            console.warn('count_unread_dms:', e);
        });
}

// Update Messages nav item: filled icon and unread badge when state.unreadMessageCount > 0.
export function updateMessagesNavUnread() {
    const wrap = document.getElementById('messages-nav-icon-wrap');
    const icon = document.getElementById('messages-nav-icon');
    const badge = document.getElementById('messages-unread-badge');
    if (!wrap || !icon || !badge) {
        return;
    }
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
