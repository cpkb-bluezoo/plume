/*
 * app.js
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

console.log('[Plume] app.js module loading');

// ============================================================
// Module imports
// ============================================================

import { state, getEffectiveRelays } from './modules/state.js';
import { invoke } from './modules/tauri.js';
import { debugLog, escapeHtml, showConfirm } from './modules/utils.js';
import { validatePublicKey } from './modules/keys.js';
import { loadConfig, saveConfig, updateUIFromConfig, setSavingState, updateFeedInitialState } from './modules/config.js';
import { isNoteMuted, isContentUnreadable, ensureMutedConfig, handleMuteClick, loadMutedPanel, renderMutedPanels, saveMutedFromPanel } from './modules/muting.js';
import {
    getAuthorDisplay, ensureProfilesForNotes, getParentEventId, getReplyToPubkey,
    setCardAvatar, isNoteLiked, performZap, performLike,
    openLikeEmojiModal, closeLikeEmojiModal, handleLikeMouseDown, handleLikeMouseUp, handleLikeMouseLeave,
    createNoteCard, createRepostCard, verifyNote, resolveNostrEmbeds, displayNotes,
    isNoteBookmarked
} from './modules/notes.js';
import {
    startInitialFeedFetch, pollForNewNotes, fetchNotesFirehoseOnHomeClick,
    showMessage, appendNoteCardToFeed
} from './modules/feed.js';
import {
    updateSidebarAvatar, fetchProfile, openProfileForUser,
    openProfileQRModal, closeProfileQRModal, openEditProfileInSettings,
    handleEditProfileSubmit, loadProfileFeed, generateNewKeyPair, updateProfileDisplay,
    setSwitchView as profileSetSwitchView, setUpdateUIFromConfig as profileSetUpdateUIFromConfig
} from './modules/profile.js';
import {
    fetchFollowingAndFollowers, handleFollowClick, switchFollowTab,
    loadFollowsPanel, renderFollowsPanel, saveFollowsPanel
} from './modules/follows.js';
import {
    loadMessagesView, selectConversation, loadConversationMessages,
    sendMessage, updateMessagesNavUnread
} from './modules/messages.js';
import {
    openCompose, closeCompose, updateCharCount, handleComposeSubmit
} from './modules/compose.js';
import {
    openSettings, closeSettings, showSettingsPanel,
    populateKeysPanel, copyNsecToClipboard, saveKeysPanel,
    saveHomeFeedModeFromPanel, saveZapsFromPanel, saveMediaServerFromPanel,
    handleSettingsSubmit, setUpdateUIFromConfig as settingsSetUpdateUIFromConfig
} from './modules/settings.js';
import { updateRelayList, bindRelayPanelHandlers, runRelayTests } from './modules/relays.js';
import {
    updateSidebarAuthState, showMutedTooltip, populateWelcomeProfiles,
    handleWelcomeLogin, handleWelcomeGenerate, handleProfileSelect, handleLogout,
    setSwitchView as authSetSwitchView
} from './modules/auth.js';
import {
    switchView, loadBookmarksView, openNoteDetail, renderNoteDetailPage, buildReplyThread
} from './modules/views.js';

// ============================================================
// Wire lazy references to break circular dependencies
// ============================================================

profileSetSwitchView(switchView);
profileSetUpdateUIFromConfig(updateUIFromConfig);
authSetSwitchView(switchView);
settingsSetUpdateUIFromConfig(updateUIFromConfig);

// ============================================================
// Global error handlers
// ============================================================

window.onerror = function(message, source, lineno, colno, error) {
    console.error('[Plume] ERROR:', message, 'at', source, lineno, colno);
    return false;
};

window.onunhandledrejection = function(event) {
    console.error('[Plume] PROMISE ERROR:', event.reason);
};

// ============================================================
// Initialization
// ============================================================

async function init() {
    try {
        console.log('[Plume] init() entered');

        // Wire the UI first with no awaits – so nothing can block before buttons work
        updateMessagesNavUnread();
        var navItems = document.querySelectorAll('.nav-item[data-view]');
        console.log('[Plume] nav items found: ' + navItems.length);
        document.querySelector('.sidebar-logo')?.addEventListener('click', function(e) {
            e.preventDefault();
            switchView('feed');
        });
        navItems.forEach(function(item) {
            item.addEventListener('click', function(e) {
                e.preventDefault();
                var view = item.dataset.view;
                if (!view) {
                    return;
                }
                // Muted nav items show a tooltip instead of navigating
                if (item.classList.contains('nav-muted')) {
                    showMutedTooltip(item);
                    return;
                }
                // Profile icon: show welcome when logged out, profile when logged in
                if (view === 'profile') {
                    if (!state.config || !state.config.public_key) {
                        switchView('welcome');
                        return;
                    }
                    state.viewedProfilePubkey = null;
                    state.viewedProfile = state.profile;
                }
                switchView(view);
            });
        });
        // Compose button also respects muted state
        var composeBtnEl = document.getElementById('compose-btn');
        if (composeBtnEl) {
            composeBtnEl.addEventListener('click', function(e) {
                if (composeBtnEl.classList.contains('nav-muted')) {
                    e.stopImmediatePropagation();
                    showMutedTooltip(composeBtnEl);
                }
            }, true); // capture phase so it fires before the existing handler
        }

        // Wire welcome screen buttons (sync, before any awaits)
        document.getElementById('welcome-login-btn')?.addEventListener('click', handleWelcomeLogin);
        document.getElementById('welcome-generate-btn')?.addEventListener('click', handleWelcomeGenerate);

        // Wire logout button (sync, before any awaits)
        var logoutBtnEl = document.getElementById('logout-btn');
        if (logoutBtnEl) {
            logoutBtnEl.addEventListener('click', handleLogout);
            console.log('[Plume] logout button wired');
        } else {
            console.warn('[Plume] logout-btn element NOT found');
        }

        console.log('[Plume] nav + logo listeners attached');

        // Settings modal (Account) – open from Settings menu
        const closeSettingsBtn = document.getElementById('close-settings');
        const settingsModal = document.getElementById('settings-modal');
        if (closeSettingsBtn) {
            closeSettingsBtn.addEventListener('click', closeSettings);
        }
        if (settingsModal) {
            settingsModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) {
                    closeSettings();
                }
            });
        }

        // Settings page menu – show corresponding panel on the right
        var settingsMenuBtns = document.querySelectorAll('.settings-menu-item');
        console.log('[Plume] settings menu items: ' + settingsMenuBtns.length);
        settingsMenuBtns.forEach(function(btn) {
            btn.addEventListener('click', function() {
                console.log('[Plume] settings panel clicked: ' + (btn.dataset.settings || '?'));
                showSettingsPanel(btn.dataset.settings);
            });
        });

        document.getElementById('home-feed-panel-save')?.addEventListener('click', saveHomeFeedModeFromPanel);
        document.getElementById('settings-media-save')?.addEventListener('click', saveMediaServerFromPanel);
        document.getElementById('settings-muted-save')?.addEventListener('click', saveMutedFromPanel);
        document.getElementById('settings-follows-save')?.addEventListener('click', saveFollowsPanel);
        document.getElementById('settings-zaps-save')?.addEventListener('click', saveZapsFromPanel);
        var settingsKeysForm = document.getElementById('settings-keys-form');
        if (settingsKeysForm) {
            settingsKeysForm.addEventListener('submit', function(e) {
                e.preventDefault();
                saveKeysPanel(e);
            });
        }
        document.getElementById('settings-keys-copy-nsec')?.addEventListener('click', copyNsecToClipboard);

        // Messages view: conversation list, send button, Message from profile
        var messagesListEl = document.querySelector('.messages-list');
        if (messagesListEl) {
            messagesListEl.addEventListener('click', function(e) {
                var item = e.target.closest('.conversation-item');
                if (item) {
                    var other = item.getAttribute('data-other-pubkey');
                    if (other) {
                        selectConversation(other);
                    }
                }
            });
        }
        document.getElementById('message-send-btn')?.addEventListener('click', sendMessage);
        var messageInput = document.getElementById('message-input');
        if (messageInput) {
            messageInput.addEventListener('keydown', function(e) {
                if (e.key === 'Enter') {
                    e.preventDefault();
                    sendMessage();
                }
            });
        }
        document.getElementById('message-user-btn')?.addEventListener('click', function() {
            var pk = state.viewedProfilePubkey;
            if (pk) {
                state.openConversationWith = pk;
                switchView('messages');
            }
        });

        if (window.__TAURI__ && window.__TAURI__.event && typeof window.__TAURI__.event.listen === 'function') {
            window.__TAURI__.event.listen('dm-received', function(ev) {
                var payload = ev.payload;
                var otherPubkey = Array.isArray(payload) ? payload[0] : (payload && payload.other_pubkey);
                if (!otherPubkey) {
                    return;
                }
                var norm = (state.selectedConversation || '').toLowerCase();
                var otherNorm = (otherPubkey || '').toLowerCase();
                if (state.currentView === 'messages' && norm === otherNorm) {
                    loadConversationMessages(state.selectedConversation);
                } else {
                    state.unreadMessageCount = (state.unreadMessageCount || 0) + 1;
                    updateMessagesNavUnread();
                }
            });
        }

        // Follows panel: sort buttons, Add, Save (Save bound above)
        document.querySelectorAll('.follows-sort-btn').forEach(function(btn) {
            btn.addEventListener('click', function() {
                document.querySelectorAll('.follows-sort-btn').forEach(function(b) { b.classList.remove('active'); });
                this.classList.add('active');
                state.followsPanelSort = this.dataset.followsSort || 'name';
                renderFollowsPanel();
            });
        });
        document.getElementById('follows-add-btn')?.addEventListener('click', function() {
            var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
            var input = document.getElementById('follows-add-input');
            if (!input) {
                return;
            }
            var raw = (input.value || '').trim();
            if (!raw) {
                return;
            }
            if (state.followsPanelLoading) {
                alert(t('settings.followsStillLoading') || 'Follow list is still loading, please wait.');
                return;
            }
            validatePublicKey(raw).then(function(r) {
                if (!r.valid || !r.hex) {
                    alert(t('settings.followsInvalidKey') || 'Invalid public key. Please enter a valid npub or 64-character hex key.');
                    return;
                }
                var hex = r.hex;
                if (!state.followsPanelList) {
                    state.followsPanelList = [];
                }
                var exists = state.followsPanelList.some(function(x) { return (x.pubkey || '').toLowerCase() === hex.toLowerCase(); });
                if (exists) {
                    alert(t('settings.followsAlreadyExists') || 'This key is already in your follow list.');
                    input.value = '';
                    return;
                }
                state.followsPanelList.push({ pubkey: hex, checked: true, listOrder: state.followsPanelList.length });
                input.value = '';
                renderFollowsPanel();
                // Fetch profile for the new entry so it shows name/avatar
                ensureProfilesForNotes([{ pubkey: hex }]).then(function() {
                    renderFollowsPanel();
                });
            }).catch(function(err) {
                console.error('Follow add validation error:', err);
                alert(t('settings.followsInvalidKey') || 'Invalid public key. Please enter a valid npub or 64-character hex key.');
            });
        });
        document.getElementById('muted-user-add-btn')?.addEventListener('click', function() {
            var input = document.getElementById('muted-user-add-input');
            if (!input) {
                return;
            }
            var raw = (input.value || '').trim();
            if (!raw) {
                return;
            }
            validatePublicKey(raw).then(function(r) {
                if (!r.valid || !r.hex) {
                    return;
                }
                var hex = r.hex;
                if (!state.mutedUsersPanelList) {
                    state.mutedUsersPanelList = [];
                }
                var exists = state.mutedUsersPanelList.some(function(x) { return (x.pubkey || '').toLowerCase() === hex.toLowerCase(); });
                if (exists) {
                    return;
                }
                state.mutedUsersPanelList.push({ pubkey: hex, checked: true });
                input.value = '';
                renderMutedPanels();
            });
        });

        // Muted tabs
        document.querySelectorAll('.muted-tab').forEach(function(tab) {
            tab.addEventListener('click', function() {
                var tabKey = this.dataset.mutedTab;
                document.querySelectorAll('.muted-tab').forEach(function(x) { x.classList.remove('active'); });
                document.querySelectorAll('.muted-tab-panel').forEach(function(x) { x.style.display = 'none'; });
                var panel = document.getElementById('muted-tab-' + tabKey);
                if (panel) {
                    panel.style.display = 'block';
                }
                this.classList.add('active');
            });
        });
        document.getElementById('muted-word-add')?.addEventListener('click', function() {
            var input = document.getElementById('muted-word-input');
            if (!input || !state.config) {
                return;
            }
            var w = (input.value && input.value.trim()) || '';
            if (!w) {
                return;
            }
            ensureMutedConfig();
            if (state.config.muted_words.indexOf(w) === -1) {
                state.config.muted_words.push(w);
            }
            input.value = '';
            renderMutedPanels();
        });
        document.getElementById('muted-hashtag-add')?.addEventListener('click', function() {
            var input = document.getElementById('muted-hashtag-input');
            if (!input || !state.config) {
                return;
            }
            var h = (input.value && input.value.trim()).replace(/^#/, '') || '';
            if (!h) {
                return;
            }
            ensureMutedConfig();
            if (state.config.muted_hashtags.indexOf(h) === -1) {
                state.config.muted_hashtags.push(h);
            }
            input.value = '';
            renderMutedPanels();
        });
        document.getElementById('settings-detail')?.addEventListener('click', function(e) {
            var remove = e.target.closest('.muted-item-remove');
            if (!remove || !state.config) {
                return;
            }
            var li = remove.closest('li');
            if (!li) {
                return;
            }
            ensureMutedConfig();
            if (li.dataset.word) {
                state.config.muted_words = state.config.muted_words.filter(function(w) { return w !== li.dataset.word; });
            } else if (li.dataset.hashtag) {
                state.config.muted_hashtags = state.config.muted_hashtags.filter(function(h) { return h !== li.dataset.hashtag; });
            }
            renderMutedPanels();
        });

        // Set up settings form
        const settingsForm = document.getElementById('settings-form');
        if (settingsForm) {
            settingsForm.addEventListener('submit', handleSettingsSubmit);
        }

        // Set up compose modal
        const composeBtn = document.getElementById('compose-btn');
        const closeComposeBtn = document.getElementById('close-compose');
        const cancelComposeBtn = document.getElementById('cancel-compose');
        const composeModal = document.getElementById('compose-modal');
        if (composeBtn) {
            composeBtn.addEventListener('click', function() { console.log('[Plume] compose btn clicked'); openCompose(); });
        }
        if (closeComposeBtn) {
            closeComposeBtn.addEventListener('click', closeCompose);
        }
        if (cancelComposeBtn) {
            cancelComposeBtn.addEventListener('click', closeCompose);
        }
        if (composeModal) {
            composeModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) {
                    closeCompose();
                }
            });
        }

        var profileQrBtn = document.getElementById('profile-qr-btn');
        var closeProfileQrBtn = document.getElementById('close-profile-qr');
        var profileQrModal = document.getElementById('profile-qr-modal');
        if (profileQrBtn) {
            profileQrBtn.addEventListener('click', openProfileQRModal);
        }
        if (closeProfileQrBtn) {
            closeProfileQrBtn.addEventListener('click', closeProfileQRModal);
        }
        if (profileQrModal) {
            profileQrModal.addEventListener('click', function(e) {
                if (e.target === e.currentTarget) {
                    closeProfileQRModal();
                }
            });
        }

        var editProfileBtn = document.getElementById('edit-profile-btn');
        var editProfileForm = document.getElementById('edit-profile-form');
        if (editProfileBtn) {
            editProfileBtn.addEventListener('click', openEditProfileInSettings);
        }
        if (editProfileForm) {
            editProfileForm.addEventListener('submit', function(e) {
                e.preventDefault();
                handleEditProfileSubmit(e);
            });
        }

        var followBtn = document.getElementById('follow-btn');
        if (followBtn) {
            followBtn.addEventListener('click', handleFollowClick);
        }
        var muteBtn = document.getElementById('mute-btn');
        if (muteBtn) {
            muteBtn.addEventListener('click', handleMuteClick);
        }

        // Set up compose form
        const composeForm = document.getElementById('compose-form');
        const composeContent = document.getElementById('compose-content');
        if (composeForm) {
            composeForm.addEventListener('submit', handleComposeSubmit);
        }
        if (composeContent) {
            composeContent.addEventListener('input', updateCharCount);
        }

        // Generate keys button (on profile when no key)
        debugLog('Looking for generate-keys-btn...');
        const generateKeysBtn = document.getElementById('generate-keys-btn');
        debugLog('generateKeysBtn found: ' + (generateKeysBtn ? 'YES' : 'NO'));
        if (generateKeysBtn) {
            generateKeysBtn.addEventListener('click', function(e) {
                debugLog('=== GENERATE KEYS BUTTON CLICKED ===');
                generateNewKeyPair();
            });
            debugLog('Generate keys button listener attached');
        } else {
            debugLog('ERROR: Generate keys button not found in DOM!');
        }

        document.querySelectorAll('.notif-tab').forEach(function(tab) {
            tab.addEventListener('click', function() {
                document.querySelectorAll('.notif-tab').forEach(function(el) { el.classList.remove('active'); });
                tab.classList.add('active');
                state.notifFilter = tab.dataset.filter;
                // TODO: filter notifications by state.notifFilter
            });
        });

        document.querySelectorAll('.profile-tab').forEach(function(tabEl) {
            tabEl.addEventListener('click', function() {
                document.querySelectorAll('.profile-tab').forEach(function(t) { t.classList.remove('active'); });
                tabEl.classList.add('active');
                state.profileTab = tabEl.dataset.tab;
                loadProfileFeed();
            });
        });

        // Note card: reply button and author link (avatar/name -> profile). Same behavior for home feed and profile feed.
        function handleNoteCardClick(e) {
            // Nostr profile link (from nostr:npub or nostr:nprofile in note content)
            var profileLink = e.target.closest('.nostr-profile-link');
            if (profileLink && profileLink.dataset.pubkey) {
                e.preventDefault();
                openProfileForUser(profileLink.dataset.pubkey);
                return;
            }
            // Embedded note card (from nostr:nevent or nostr:note in note content)
            var embedCard = e.target.closest('.note-card-embed');
            if (embedCard && embedCard.dataset.noteId) {
                // Don't navigate if user clicked a profile link inside the embed
                if (e.target.closest('.note-author-link') || e.target.closest('.nostr-profile-link')) {
                    // let it fall through to the author link handler below
                } else {
                    e.preventDefault();
                    openNoteDetail(embedCard.dataset.noteId);
                    return;
                }
            }
            var authorLink = e.target.closest('.note-author-link');
            if (authorLink && authorLink.dataset.pubkey) {
                e.preventDefault();
                openProfileForUser(authorLink.dataset.pubkey);
                return;
            }
            var replyBtn = e.target.closest('.note-action[data-action="reply"]');
            if (replyBtn) {
                e.preventDefault();
                var card = replyBtn.closest('.note-card');
                var noteId = (replyBtn.dataset.noteId || (card && card.dataset.noteId)) || '';
                if (!noteId) {
                    return;
                }
                var note = (state.notes && state.notes.find(function(n) { return n.id === noteId; })) ||
                    (state.profileNotes && state.profileNotes.find(function(n) { return n.id === noteId; })) ||
                    (state.bookmarkNotes && state.bookmarkNotes.find(function(n) { return n.id === noteId; }));
                if (!note && state.noteDetailReplies) {
                    var found = state.noteDetailReplies.find(function(x) { return x.note.id === noteId; });
                    if (found) {
                        note = found.note;
                    }
                }
                if (!note && state.noteDetailAncestors) {
                    note = state.noteDetailAncestors.find(function(n) { return n.id === noteId; });
                }
                if (!note && state.noteDetailSubject && state.noteDetailSubject.id === noteId) {
                    note = state.noteDetailSubject;
                }
                openNoteDetail(note || noteId);
                return;
            }
            var zapBtn = e.target.closest('.note-action[data-action="zap"]');
            if (zapBtn && !zapBtn.disabled) {
                e.preventDefault();
                e.stopPropagation();
                var targetPubkey = zapBtn.getAttribute('data-zap-target-pubkey');
                var eventId = zapBtn.getAttribute('data-zap-event-id') || (zapBtn.closest('.note-card') && zapBtn.closest('.note-card').dataset.noteId);
                if (targetPubkey) {
                    performZap(targetPubkey, eventId, zapBtn);
                }
                return;
            }
            var likeBtn = e.target.closest('.note-action[data-action="like"]');
            if (likeBtn) {
                e.preventDefault();
                e.stopPropagation();
                return;
            }
            var repostBtn = e.target.closest('.note-action[data-action="repost"]');
            if (repostBtn) {
                e.preventDefault();
                var card = repostBtn.closest('.note-card');
                var noteId = (repostBtn.dataset.noteId || (card && card.dataset.noteId)) || '';
                var pubkey = (repostBtn.dataset.pubkey || (card && card.dataset.pubkey)) || '';
                if (!noteId || !pubkey) {
                    return;
                }
                var note = (state.notes && state.notes.find(function(n) { return n.id === noteId; })) ||
                    (state.profileNotes && state.profileNotes.find(function(n) { return n.id === noteId; })) ||
                    (state.bookmarkNotes && state.bookmarkNotes.find(function(n) { return n.id === noteId; }));
                var contentOpt = note ? JSON.stringify(note) : null;
                repostBtn.disabled = true;
                invoke('post_repost', { eventId: noteId, authorPubkey: pubkey, contentOptional: contentOpt })
                    .then(function() {
                        repostBtn.classList.add('reposted');
                    })
                    .catch(function(err) {
                        console.error('Repost failed:', err);
                        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToPublish') : 'Failed to publish repost') + ': ' + err);
                    })
                    .finally(function() { repostBtn.disabled = false; });
                return;
            }
            var bookmarkBtn = e.target.closest('.note-action[data-action="bookmark"]');
            if (bookmarkBtn) {
                e.preventDefault();
                var noteId = bookmarkBtn.dataset.noteId || (bookmarkBtn.closest('.note-card') && bookmarkBtn.closest('.note-card').dataset.noteId);
                if (!noteId || !state.config) {
                    return;
                }
                if (!Array.isArray(state.config.bookmarks)) {
                    state.config.bookmarks = [];
                }
                var idx = state.config.bookmarks.indexOf(noteId);
                if (idx === -1) {
                    state.config.bookmarks.push(noteId);
                } else {
                    state.config.bookmarks.splice(idx, 1);
                }
                saveConfig();
                var img = bookmarkBtn.querySelector('img');
                var nowBookmarked = idx === -1;
                if (img) {
                    img.src = nowBookmarked ? 'icons/bookmark-filled.svg' : 'icons/bookmark.svg';
                }
                var t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
                var label = nowBookmarked ? (t('note.unbookmark') || 'Unbookmark') : (t('note.bookmark') || 'Bookmark');
                bookmarkBtn.setAttribute('title', label);
                bookmarkBtn.setAttribute('aria-label', label);
                if (img) {
                    img.setAttribute('alt', label);
                }
                // In bookmarks view we do not remove the card on unbookmark; list refreshes when user leaves and returns.
                return;
            }
        }
        var notesContainer = document.getElementById('notes-container');
        if (notesContainer) {
            notesContainer.addEventListener('click', handleNoteCardClick);
            notesContainer.addEventListener('mousedown', handleLikeMouseDown);
            notesContainer.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var profileFeed = document.getElementById('profile-feed');
        if (profileFeed) {
            profileFeed.addEventListener('click', handleNoteCardClick);
            profileFeed.addEventListener('mousedown', handleLikeMouseDown);
            profileFeed.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var bookmarksContainer = document.getElementById('bookmarks-container');
        if (bookmarksContainer) {
            bookmarksContainer.addEventListener('click', handleNoteCardClick);
            bookmarksContainer.addEventListener('mousedown', handleLikeMouseDown);
            bookmarksContainer.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        document.addEventListener('mouseup', handleLikeMouseUp);

        var noteDetailRepliesEl = document.getElementById('note-detail-replies');
        if (noteDetailRepliesEl) {
            noteDetailRepliesEl.addEventListener('click', handleNoteCardClick);
            noteDetailRepliesEl.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailRepliesEl.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var noteDetailAncestorsEl = document.getElementById('note-detail-ancestors');
        if (noteDetailAncestorsEl) {
            noteDetailAncestorsEl.addEventListener('click', handleNoteCardClick);
            noteDetailAncestorsEl.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailAncestorsEl.addEventListener('mouseleave', handleLikeMouseLeave);
        }
        var noteDetailSubjectWrap = document.getElementById('note-detail-subject-wrap');
        if (noteDetailSubjectWrap) {
            noteDetailSubjectWrap.addEventListener('click', handleNoteCardClick);
            noteDetailSubjectWrap.addEventListener('mousedown', handleLikeMouseDown);
            noteDetailSubjectWrap.addEventListener('mouseleave', handleLikeMouseLeave);
        }

        document.getElementById('close-like-emoji-modal')?.addEventListener('click', closeLikeEmojiModal);
        var likeEmojiModal = document.getElementById('like-emoji-modal');
        if (likeEmojiModal) {
            likeEmojiModal.addEventListener('click', function(e) {
                if (e.target === likeEmojiModal) {
                    closeLikeEmojiModal();
                }
            });
        }

        var noteDetailBack = document.getElementById('note-detail-back');
        if (noteDetailBack) {
            noteDetailBack.addEventListener('click', function() {
                switchView(state.noteDetailPreviousView || 'feed');
            });
        }

        console.log('[Plume] all sync listeners attached, about to await i18n...');
        await (window.PlumeI18n && window.PlumeI18n.init ? window.PlumeI18n.init() : Promise.resolve());
        console.log('[Plume] i18n done');

        // Load app-level config to determine active profile
        var appConfig = null;
        try {
            var appConfigJson = await invoke('get_app_config');
            appConfig = JSON.parse(appConfigJson);
        } catch (e) {
            console.log('[Plume] No app config, fresh start');
            appConfig = { active_profile: null, known_profiles: [] };
        }
        state.appConfig = appConfig;
        console.log('[Plume] App config loaded, active_profile:', appConfig.active_profile || 'none');

        if (appConfig.active_profile) {
            // Logged in – load profile config and go to feed
            await loadConfig();
            updateSidebarAuthState();
            switchView('feed');
            // Fetch profile from relays in background to update sidebar avatar and local config
            fetchProfile();
        } else {
            // Not logged in – show welcome screen
            updateSidebarAuthState();
            await populateWelcomeProfiles();
            switchView('welcome');
        }
        console.log('[Plume] loadConfig done');

        var noteDetailReplyBtn = document.getElementById('note-detail-reply-btn');
        var noteDetailReplyContent = document.getElementById('note-detail-reply-content');
        if (noteDetailReplyBtn && noteDetailReplyContent) {
            noteDetailReplyBtn.addEventListener('click', async function() {
                var content = noteDetailReplyContent.value.trim();
                if (!content) {
                    return;
                }
                if (!state.noteDetailSubject || !state.noteDetailSubjectId) {
                    return;
                }
                var sub = state.noteDetailSubject;
                noteDetailReplyBtn.disabled = true;
                try {
                    var resultJson = await invoke('post_note', {
                        content: content,
                        replyToEventId: state.noteDetailSubjectId,
                        replyToPubkey: sub.pubkey || null
                    });
                    var result = JSON.parse(resultJson);
                    if (result.success_count > 0) {
                        noteDetailReplyContent.value = '';
                        var replyJson = await invoke('fetch_replies_to_event', {
                            relay_urls: getEffectiveRelays(),
                            event_id: state.noteDetailSubjectId,
                            limit: 500
                        });
                        var repliesRaw = replyJson ? JSON.parse(replyJson) : [];
                        state.noteDetailReplies = buildReplyThread(repliesRaw, state.noteDetailSubjectId);
                        renderNoteDetailPage();
                    } else {
                        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.publishFailed') : 'Failed to publish'));
                    }
                } catch (e) {
                    console.error('Reply failed:', e);
                    alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.postFailed') : 'Failed to post') + ': ' + e);
                } finally {
                    noteDetailReplyBtn.disabled = false;
                }
            });
        }

        if (state.appConfig && state.appConfig.active_profile) {
            console.log('[Plume] calling startInitialFeedFetch...');
            startInitialFeedFetch();
        }
        console.log('[Plume] init() completed successfully');
    } catch (error) {
        console.error('[Plume] init() FAILED:', error);
        alert('Initialization error: ' + (error && error.message ? error.message : String(error)));
    }
}

// Run initialization when the page is ready (DOMContentLoaded may have already fired when script runs at end of body)
function runInit() {
    console.log('[Plume] runInit() called, readyState=' + document.readyState);
    init();
}
console.log('[Plume] app.js loaded, readyState=' + document.readyState);
if (document.readyState === 'loading') {
    console.log('[Plume] Waiting for DOMContentLoaded...');
    document.addEventListener('DOMContentLoaded', runInit);
} else {
    console.log('[Plume] Document already ready, calling runInit() now');
    runInit();
}
