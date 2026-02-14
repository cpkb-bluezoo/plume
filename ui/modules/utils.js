/*
 * modules/utils.js
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

// Debug logging - outputs to browser console (view with Cmd+Option+I)
export function debugLog(message) {
    console.log('[Plume]', message);
}

// Escape HTML to prevent XSS
export function escapeHtml(text) {
    if (!text) {
        return '';
    }
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

export function escapeCssAttr(s) {
    return String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

// Custom confirm dialog (native confirm() is blocked in Tauri 2.0 webviews)
export function showConfirm(message) {
    return new Promise(function(resolve) {
        var modal = document.getElementById('confirm-dialog');
        var msgEl = document.getElementById('confirm-dialog-message');
        var okBtn = document.getElementById('confirm-dialog-ok');
        var cancelBtn = document.getElementById('confirm-dialog-cancel');
        if (!modal || !msgEl || !okBtn || !cancelBtn) {
            // Fallback: just resolve true if DOM elements are missing
            resolve(true);
            return;
        }
        msgEl.textContent = message;
        modal.classList.add('active');
        modal.setAttribute('aria-hidden', 'false');

        function cleanup() {
            modal.classList.remove('active');
            modal.setAttribute('aria-hidden', 'true');
            okBtn.removeEventListener('click', onOk);
            cancelBtn.removeEventListener('click', onCancel);
            modal.removeEventListener('click', onBackdrop);
        }
        function onOk() { cleanup(); resolve(true); }
        function onCancel() { cleanup(); resolve(false); }
        function onBackdrop(e) {
            if (e.target === modal) {
                cleanup();
                resolve(false);
            }
        }

        okBtn.addEventListener('click', onOk);
        cancelBtn.addEventListener('click', onCancel);
        modal.addEventListener('click', onBackdrop);
        okBtn.focus();
    });
}

// Shorten a key for display
export function shortenKey(key) {
    if (!key || key.length <= 16) {
        return key || '';
    }
    return key.substring(0, 8) + '...' + key.substring(key.length - 4);
}

// Format a Unix timestamp to human-readable relative time (e.g. 1min, 4h, 2 months)
export function formatTimestamp(timestamp) {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now - date;
    const diffSec = Math.floor(diffMs / 1000);
    const diffMin = Math.floor(diffSec / 60);
    const diffHour = Math.floor(diffMin / 60);
    const diffDay = Math.floor(diffHour / 24);
    const diffMonth = Math.floor(diffDay / 30);
    const diffYear = Math.floor(diffDay / 365);

    if (diffSec < 60) {
        return 'now';
    }
    if (diffMin < 60) {
        return diffMin === 1 ? '1min' : diffMin + 'min';
    }
    if (diffHour < 24) {
        return diffHour + 'h';
    }
    if (diffDay < 30) {
        return diffDay === 1 ? '1 day' : diffDay + ' days';
    }
    if (diffMonth < 12) {
        return diffMonth === 1 ? '1 month' : diffMonth + ' months';
    }
    return diffYear === 1 ? '1 year' : diffYear + ' years';
}

// Validate and sanitize a URL: only allow http/https schemes, strip control characters.
// Returns the sanitized URL or null if unsafe.
export function sanitizeUrl(url) {
    if (!url) {
        return null;
    }
    var trimmed = url.trim();
    // Only allow http: and https: schemes
    if (!/^https?:\/\//i.test(trimmed)) {
        return null;
    }
    // Block URLs containing control characters, quotes, or angle brackets that could break attributes
    if (/[\x00-\x1f"'<>`]/.test(trimmed)) {
        return null;
    }
    // Block javascript: in any encoding (e.g., via entity or percent-encoding in the already-escaped output)
    if (/javascript\s*:/i.test(trimmed)) {
        return null;
    }
    if (/data\s*:/i.test(trimmed)) {
        return null;
    }
    if (/vbscript\s*:/i.test(trimmed)) {
        return null;
    }
    return trimmed;
}

// Clear validation error displays
export function clearValidationErrors() {
    document.querySelectorAll('.validation-error').forEach(el => {
        el.textContent = '';
        el.style.display = 'none';
    });
    document.querySelectorAll('.form-group input').forEach(el => {
        el.classList.remove('invalid');
    });
}

// Show validation error for an input
export function showValidationError(inputId, message) {
    const input = document.getElementById(inputId);
    const errorEl = document.getElementById(inputId + '-error');

    if (input) {
        input.classList.add('invalid');
    }
    if (errorEl) {
        errorEl.textContent = message;
        errorEl.style.display = 'block';
    }
}
