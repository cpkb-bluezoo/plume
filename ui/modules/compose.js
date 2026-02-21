/*
 * modules/compose.js
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
import { pollForNewNotes, fetchNotesFirehoseOnHomeClick } from './feed.js';

let isPosting = false;
let composePendingUploads = [];
let composeUploadCounter = 0;
let uploadProgressUnlisten = null;

// Open the compose modal (optionally as a reply: openCompose({ id, pubkey, name }))
export function openCompose(replyingTo) {
    state.replyingTo = replyingTo || null;
    const modal = document.getElementById('compose-modal');
    const replyCtx = document.getElementById('compose-reply-context');
    const replyName = document.getElementById('compose-reply-name');
    if (replyCtx) {
        replyCtx.style.display = state.replyingTo ? 'block' : 'none';
    }
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (replyName && state.replyingTo) {
        replyName.textContent = state.replyingTo.name ? `@${state.replyingTo.name}` : t('note.replyLabel');
    }
    modal.classList.add('active');
    const content = document.getElementById('compose-content');
    if (content) {
        content.value = '';
    }
    const charCountEl = document.getElementById('compose-char-count');
    if (charCountEl) {
        charCountEl.textContent = t('composeModal.charCount', { count: 0 });
    }
    composePendingUploads = [];
    clearUploadUI('compose-uploads');
    hideComposeError();
    hideComposeStatus();
    enableComposeButton();
    setTimeout(() => content && content.focus(), 100);
}

// Close the compose modal and delete any pending uploads
export function closeCompose() {
    state.replyingTo = null;
    deleteComposeUploads();
    document.getElementById('compose-modal').classList.remove('active');
}

// Show error in compose modal
export function showComposeError(message) {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = message;
    errorEl.classList.add('visible');
}

// Hide compose error
export function hideComposeError() {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = '';
    errorEl.classList.remove('visible');
}

// Show status in compose modal
export function showComposeStatus(message, isSuccess = false) {
    const statusEl = document.getElementById('compose-status');
    statusEl.textContent = message;
    statusEl.classList.add('visible');
    if (isSuccess) {
        statusEl.classList.add('success');
    } else {
        statusEl.classList.remove('success');
    }
}

// Hide compose status
export function hideComposeStatus() {
    const statusEl = document.getElementById('compose-status');
    statusEl.textContent = '';
    statusEl.classList.remove('visible');
    statusEl.classList.remove('success');
}

// Disable compose button during posting
export function disableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = true;
        const text = document.getElementById('compose-btn-text');
        if (text) {
            text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.posting') : 'Posting…');
        }
    }
}

// Enable compose button
export function enableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = false;
        const text = document.getElementById('compose-btn-text');
        if (text) {
            text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.post') : 'Post');
        }
    }
}

// Update character count
export function updateCharCount() {
    const textarea = document.getElementById('compose-content');
    const count = textarea ? textarea.value.length : 0;
    const el = document.getElementById('compose-char-count');
    if (el && window.PlumeI18n && window.PlumeI18n.t) {
        el.textContent = window.PlumeI18n.t('composeModal.charCount', { count: count });
    } else if (el) {
        el.textContent = count + ' / 10000';
    }
}

// Handle compose form submission
export async function handleComposeSubmit(event) {
    event.preventDefault();

    if (isPosting) {
        return;
    }

    const content = document.getElementById('compose-content').value.trim();

    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    // Validate content
    if (!content) {
        showComposeError(t('composeModal.contentRequired'));
        return;
    }

    if (content.length > 10000) {
        showComposeError(t('composeModal.tooLong'));
        return;
    }

    // Check if we have a private key
    if (!state.config || !state.config.private_key) {
        showComposeError(t('composeModal.noPrivateKey'));
        return;
    }

    isPosting = true;
    hideComposeError();
    showComposeStatus(t('composeModal.signingPublishing'));
    disableComposeButton();

    const replyTo = state.replyingTo ? { event_id: state.replyingTo.id, pubkey: state.replyingTo.pubkey } : null;
    try {
        const resultJson = await invoke('post_note', {
            content,
            replyToEventId: replyTo ? replyTo.event_id : null,
            replyToPubkey: replyTo ? replyTo.pubkey : null
        });
        const result = JSON.parse(resultJson);

        console.log('Post result:', result);

        if (result.success_count > 0) {
            const msg = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishedSuccess', { success: result.success_count, total: result.total_count }) : `Published to ${result.success_count} of ${result.total_count} relay(s)`);
            showComposeStatus(msg, true);

            composePendingUploads = [];
            state.replyingTo = null;
            setTimeout(() => {
                closeCompose();
                if (state.homeFeedMode === 'follows') {
                    pollForNewNotes();
                } else {
                    fetchNotesFirehoseOnHomeClick();
                }
            }, 1500);
        } else {
            // All relays failed
            let errorMessage = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishFailed') : 'Failed to publish to any relay');
            if (result.results && result.results.length > 0) {
                const firstError = result.results[0].message;
                if (firstError) {
                    errorMessage += ': ' + firstError;
                }
            }
            showComposeError(errorMessage);
            hideComposeStatus();
            enableComposeButton();
        }
    } catch (error) {
        console.error('Failed to post note:', error);
        showComposeError((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.postFailed') : 'Failed to post note') + ': ' + error);
        hideComposeStatus();
        enableComposeButton();
    } finally {
        isPosting = false;
    }
}

// ============================================================
// Media Upload Functions
// ============================================================

function generateUploadId() {
    composeUploadCounter++;
    return 'upload_' + Date.now() + '_' + composeUploadCounter;
}

/// Initialize upload event listeners. Called once from app.js.
export function initComposeUploads() {
    var attachBtn = document.getElementById('compose-attach-btn');
    if (attachBtn) {
        attachBtn.addEventListener('click', function() {
            pickAndUpload('compose-content', 'compose-uploads', composePendingUploads);
        });
    }

    if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
        window.__TAURI__.event.listen('upload-progress', function(ev) {
            var payload = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            handleUploadProgress(payload);
        }).then(function(unlisten) {
            uploadProgressUnlisten = unlisten;
        });
        window.__TAURI__.event.listen('upload-complete', function(ev) {
            var payload = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            handleUploadComplete(payload);
        });
        window.__TAURI__.event.listen('upload-failed', function(ev) {
            var payload = typeof ev.payload === 'string' ? JSON.parse(ev.payload) : ev.payload;
            handleUploadFailed(payload);
        });
    }
}

/// Open native file dialog, then start upload. Shared by compose and reply.
/// Fire-and-forget: results arrive via upload-complete / upload-failed events.
function pickAndUpload(textareaId, uploadsContainerId, pendingUploads) {
    invoke('pick_media_file').then(function(result) {
        if (!result) return;
        var picked = typeof result === 'string' ? JSON.parse(result) : result;
        if (!picked || !picked.path) return;

        var uploadId = generateUploadId();
        var entry = {
            uploadId: uploadId, fileName: picked.name, textareaId: textareaId,
            containerId: uploadsContainerId, url: null, fileHash: null, status: 'uploading'
        };
        pendingUploads.push(entry);
        renderUploadItem(uploadsContainerId, entry);

        invoke('upload_media', {
            file_path: picked.path,
            file_name: picked.name,
            upload_id: uploadId
        }).catch(function(err) {
            console.error('Upload failed:', err);
            entry.status = 'failed';
            entry.error = String(err);
            renderUploadItem(uploadsContainerId, entry);
        });
    }).catch(function(err) {
        console.error('File picker failed:', err);
    });
}

function insertUrlIntoTextarea(textareaId, url) {
    var textarea = document.getElementById(textareaId);
    if (!textarea) return;

    var text = textarea.value;
    var insertion = url;
    if (text.length > 0 && !text.endsWith('\n') && !text.endsWith(' ')) {
        insertion = '\n' + insertion;
    }
    var pos = textarea.selectionStart || text.length;
    textarea.value = text.slice(0, pos) + insertion + text.slice(pos);
    textarea.selectionStart = textarea.selectionEnd = pos + insertion.length;
    textarea.focus();

    // Trigger char count update
    if (textareaId === 'compose-content') {
        updateCharCount();
    }
}

function renderUploadItem(containerId, entry) {
    var container = document.getElementById(containerId);
    if (!container) return;

    var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    var existing = container.querySelector('[data-upload-id="' + entry.uploadId + '"]');

    if (entry.status === 'complete') {
        if (existing) existing.remove();
        return;
    }

    var html = '<div class="upload-progress-item" data-upload-id="' + entry.uploadId + '">';
    html += '<span class="upload-file-name">' + escapeHtml(entry.fileName) + '</span>';

    if (entry.status === 'uploading') {
        var pct = entry.percent || 0;
        html += '<div class="upload-progress-bar-wrap"><div class="upload-progress-bar" style="width: ' + pct + '%"></div></div>';
        html += '<span class="upload-progress-pct">' + pct + '%</span>';
    } else if (entry.status === 'failed') {
        var reason = entry.error ? ': ' + escapeHtml(entry.error) : '';
        html += '<span class="upload-failed">' + t('composeModal.uploadFailed') + reason + '</span>';
    }
    html += '</div>';

    if (existing) {
        existing.outerHTML = html;
    } else {
        container.insertAdjacentHTML('beforeend', html);
    }
}

function findUploadEntry(uploadId) {
    var entry = composePendingUploads.find(function(u) { return u.uploadId === uploadId; });
    if (entry) return entry;
    return replyPendingUploads.find(function(u) { return u.uploadId === uploadId; });
}

function handleUploadProgress(payload) {
    if (!payload || !payload.upload_id) return;

    var entry = findUploadEntry(payload.upload_id);
    if (!entry) return;

    if (payload.status === 'uploading' && payload.total_bytes > 0) {
        entry.percent = Math.round((payload.bytes_sent / payload.total_bytes) * 100);
        renderUploadItem(entry.containerId || 'compose-uploads', entry);
    }
}

function handleUploadComplete(payload) {
    if (!payload || !payload.upload_id) return;

    var entry = findUploadEntry(payload.upload_id);
    if (!entry) return;

    entry.url = payload.url;
    entry.fileHash = payload.file_hash;
    entry.status = 'complete';
    renderUploadItem(entry.containerId || 'compose-uploads', entry);
    insertUrlIntoTextarea(entry.textareaId || 'compose-content', payload.url);
}

function handleUploadFailed(payload) {
    if (!payload || !payload.upload_id) return;

    var entry = findUploadEntry(payload.upload_id);
    if (!entry) return;

    entry.status = 'failed';
    entry.error = payload.error || 'Upload failed';
    renderUploadItem(entry.containerId || 'compose-uploads', entry);
}

function clearUploadUI(containerId) {
    var container = document.getElementById(containerId);
    if (container) container.innerHTML = '';
}

function deleteComposeUploads() {
    for (var i = 0; i < composePendingUploads.length; i++) {
        var entry = composePendingUploads[i];
        if (entry.fileHash && entry.status === 'complete') {
            invoke('delete_media', { fileHash: entry.fileHash }).catch(function(err) {
                console.warn('Failed to delete upload:', err);
            });
        }
    }
    composePendingUploads = [];
    clearUploadUI('compose-uploads');
}

function escapeHtml(str) {
    return str.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

// ============================================================
// Reply Upload Support (used from app.js)
// ============================================================

export let replyPendingUploads = [];

export function initReplyUploads() {
    var attachBtn = document.getElementById('reply-attach-btn');
    if (attachBtn) {
        attachBtn.addEventListener('click', function() {
            pickAndUpload('note-detail-reply-content', 'reply-uploads', replyPendingUploads);
        });
    }
}

export function clearReplyUploads() {
    replyPendingUploads = [];
    clearUploadUI('reply-uploads');
}

export function deleteReplyUploads() {
    for (var i = 0; i < replyPendingUploads.length; i++) {
        var entry = replyPendingUploads[i];
        if (entry.fileHash && entry.status === 'complete') {
            invoke('delete_media', { fileHash: entry.fileHash }).catch(function(err) {
                console.warn('Failed to delete reply upload:', err);
            });
        }
    }
    replyPendingUploads = [];
    clearUploadUI('reply-uploads');
}
