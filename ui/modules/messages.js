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
            for (let i = 0; i < conversations.length; i++) {
                const c = conversations[i];
                const other = c.other_pubkey || '';
                const name = (state.profileCache && state.profileCache[other] && state.profileCache[other].name) ? escapeHtml(state.profileCache[other].name) : shortenPubkey(other);
                const ts = c.last_created_at ? new Date(c.last_created_at * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) : '';
                html += '<div class="conversation-item" role="button" tabindex="0" data-other-pubkey="' + escapeHtml(other) + '" title="' + escapeHtml(other) + '"><span class="conversation-item-name">' + name + '</span><span class="conversation-item-meta">' + escapeHtml(ts) + '</span></div>';
            }
            listEl.innerHTML = html;
        }

        if (!state.dmStreamStarted && getEffectiveRelays().length > 0) {
            state.dmStreamStarted = true;
            invoke('start_dm_stream').catch(function(e) { console.warn('start_dm_stream:', e); });
        }

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
    loadConversationMessages(otherPubkeyHex);
}

export async function loadConversationMessages(otherPubkeyHex) {
    const container = document.getElementById('messages-chat-messages');
    if (!container) {
        return;
    }
    container.innerHTML = '<p class="placeholder-message">' + (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('noteDetail.loading') : 'Loading…') + '</p>';
    try {
        const json = await invoke('get_messages', { otherPubkeyHex: otherPubkeyHex });
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
    const other = state.selectedConversation;
    const input = document.getElementById('message-input');
    if (!other || !input) {
        return;
    }
    const text = (input.value || '').trim();
    if (!text) {
        return;
    }
    try {
        await invoke('send_dm', { recipientPubkey: other, plaintext: text });
        input.value = '';
        const container = document.getElementById('messages-chat-messages');
        if (container && container.querySelector('.message-bubble')) {
            const m = { id: '', content: text, created_at: Math.floor(Date.now() / 1000), is_outgoing: true };
            const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            const html = '<div class="message-bubble message-outgoing" data-id=""><div class="message-content">' + escapeHtml(text) + '</div><div class="message-meta">' + escapeHtml(new Date().toLocaleString()) + '</div></div>';
            container.insertAdjacentHTML('beforeend', html);
            container.scrollTop = container.scrollHeight;
        } else {
            loadConversationMessages(other);
        }
    } catch (e) {
        console.error('send_dm failed:', e);
        alert(e && e.message ? e.message : String(e));
    }
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
