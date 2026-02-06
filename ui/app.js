// Plume - Nostr Client Frontend
// JavaScript for the user interface

// Debug logging - outputs to browser console (view with Cmd+Option+I)
function debugLog(message) {
    console.log('[Plume]', message);
}

// Global error handler
window.onerror = function(message, source, lineno, colno, error) {
    console.error('[Plume] ERROR:', message, 'at', source, lineno, colno);
    return false;
};

window.onunhandledrejection = function(event) {
    console.error('[Plume] PROMISE ERROR:', event.reason);
};

// ============================================================
// Global State
// ============================================================

// Application state
const state = {
    config: null,
    currentView: 'feed',
    notes: [],
    loading: false,
    publicKeyHex: null,
    publicKeyNpub: null,
    profile: null,
    profileLoading: false,
    homeFeedMode: 'firehose',
    initialFeedLoadDone: false,
    feedPollIntervalId: null,
    // pubkey (hex) -> { name, nip05, picture } for note authors
    profileCache: {},
    // When set, compose is a reply to this note
    replyingTo: null,
    // Profile page: null = current user, or hex pubkey of the user being viewed
    viewedProfilePubkey: null,
    // Profile data for the profile page (own or other); state.profile is always current user for sidebar
    viewedProfile: null
};

// ============================================================
// Tauri API Helpers
// ============================================================

// Call a Tauri command (function defined in Rust)
async function invoke(command, args = {}) {
    // __TAURI__ is injected by Tauri when the app runs
    if (window.__TAURI__ && window.__TAURI__.core) {
        return await window.__TAURI__.core.invoke(command, args);
    } else {
        console.warn('Tauri API not available - running in browser?');
        return null;
    }
}

// ============================================================
// Key Management
// ============================================================

// Parse a key and get its info (hex, npub, nsec)
async function parseKey(key) {
    if (!key || key.trim() === '') {
        return null;
    }
    
    try {
        const resultJson = await invoke('parse_key', { key: key });
        if (resultJson) {
            return JSON.parse(resultJson);
        }
    } catch (error) {
        console.error('Failed to parse key:', error);
    }
    return null;
}

// Convert public key to hex format
async function publicKeyToHex(key) {
    try {
        return await invoke('convert_public_key_to_hex', { key: key });
    } catch (error) {
        console.error('Failed to convert public key:', error);
        return null;
    }
}

// Convert hex to npub format
async function hexToNpub(hexKey) {
    try {
        return await invoke('convert_hex_to_npub', { hexKey: hexKey });
    } catch (error) {
        console.error('Failed to convert to npub:', error);
        return null;
    }
}

// Convert secret key to hex format
async function secretKeyToHex(key) {
    try {
        return await invoke('convert_secret_key_to_hex', { key: key });
    } catch (error) {
        console.error('Failed to convert secret key:', error);
        return null;
    }
}

// Validate and normalize a public key (returns hex or null)
async function validatePublicKey(key) {
    if (!key || key.trim() === '') {
        return { valid: false, hex: null, npub: null, error: 'Key is empty' };
    }
    
    try {
        const hex = await publicKeyToHex(key);
        if (hex) {
            const npub = await hexToNpub(hex);
            return { valid: true, hex: hex, npub: npub, error: null };
        }
    } catch (error) {
        return { valid: false, hex: null, npub: null, error: error.toString() };
    }
    
    return { valid: false, hex: null, npub: null, error: 'Invalid key format' };
}

// Validate and normalize a secret key (returns hex or null)
async function validateSecretKey(key) {
    if (!key || key.trim() === '') {
        return { valid: true, hex: null, error: null }; // Empty is OK (optional)
    }
    
    try {
        const hex = await secretKeyToHex(key);
        if (hex) {
            return { valid: true, hex: hex, error: null };
        }
    } catch (error) {
        return { valid: false, hex: null, error: error.toString() };
    }
    
    return { valid: false, hex: null, error: 'Invalid key format' };
}

// ============================================================
// Configuration Management
// ============================================================

// Load configuration from the backend
async function loadConfig() {
    try {
        const configJson = await invoke('load_config');
        if (configJson) {
            state.config = JSON.parse(configJson);
            console.log('Config loaded:', state.config);
            
            // Parse the public key to get npub format
            if (state.config.public_key) {
                const keyInfo = await validatePublicKey(state.config.public_key);
                if (keyInfo.valid) {
                    state.publicKeyHex = keyInfo.hex;
                    state.publicKeyNpub = keyInfo.npub;
                }
            }
            
            updateUIFromConfig();
        }
    } catch (error) {
        console.error('Failed to load config:', error);
        // Use default config
        state.config = {
            public_key: '',
            private_key: null,
            relays: ['wss://relay.damus.io', 'wss://nos.lol', 'wss://relay.nostr.band'],
            display_name: 'Anonymous'
        };
        updateUIFromConfig();
    }
}

// Save configuration to the backend
async function saveConfig() {
    try {
        const configJson = JSON.stringify(state.config);
        await invoke('save_config', { configJson: configJson });
        console.log('Config saved');
    } catch (error) {
        console.error('Failed to save config:', error);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToSaveSettings') : 'Failed to save settings') + ': ' + error);
    }
}

// Update UI elements from the current config
function updateUIFromConfig() {
    if (!state.config) return;

    state.homeFeedMode = (state.config.home_feed_mode === 'follows') ? 'follows' : 'firehose';

    const nameEl = document.getElementById('input-display-name');
    const pubEl = document.getElementById('input-public-key');
    const privEl = document.getElementById('input-private-key');
    if (nameEl) nameEl.value = state.config.display_name || '';
    if (pubEl) pubEl.value = state.config.public_key || '';
    if (privEl) privEl.value = state.config.private_key || '';

    updateProfileDisplay();
    updateRelayList();
}

// ============================================================
// Profile Management
// ============================================================

// Fetch profile for the profile page (own or viewed user)
async function fetchProfile() {
    if (state.profileLoading) return;
    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    if (viewingOwn && (!state.config || !state.config.public_key)) {
        state.viewedProfile = null;
        updateProfileDisplay();
        return;
    }

    state.profileLoading = true;
    try {
        if (viewingOwn) {
            const profileJson = await invoke('fetch_own_profile');
            if (profileJson && profileJson !== '{}') {
                state.profile = JSON.parse(profileJson);
                state.viewedProfile = state.profile;
            } else {
                state.profile = null;
                state.viewedProfile = null;
            }
        } else {
            const profileJson = await invoke('fetch_profile', {
                pubkey: state.viewedProfilePubkey,
                relayUrls: state.config.relays
            });
            if (profileJson && profileJson !== '{}') {
                state.viewedProfile = JSON.parse(profileJson);
            } else {
                state.viewedProfile = null;
            }
        }
        updateProfileDisplay();
    } catch (error) {
        console.error('Failed to fetch profile:', error);
        state.viewedProfile = null;
        updateProfileDisplay();
    } finally {
        state.profileLoading = false;
    }
}

// Open profile page for a user (from note card avatar/name click)
function openProfileForUser(pubkey) {
    if (!pubkey) return;
    state.viewedProfilePubkey = pubkey;
    switchView('profile');
}

// Generate a new key pair
async function generateNewKeyPair() {
    debugLog('generateNewKeyPair called');
    const btn = document.getElementById('generate-keys-btn');
    if (!btn) {
        debugLog('ERROR: Button not found in generateNewKeyPair');
        return;
    }
    const originalText = btn.textContent;
    
    // Skip confirmation for now - just generate
    debugLog('Proceeding to generate keys...');
    
    try {
        btn.disabled = true;
        btn.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.generating') : 'Generating...');
        debugLog('Calling invoke generate_keypair...');
        
        const result = await invoke('generate_keypair');
        debugLog('Got result: ' + result);
        const keys = JSON.parse(result);
        
        debugLog('New key pair generated: ' + keys.npub);
        
        // Update the state
        state.config.public_key = keys.public_key_hex;
        state.config.private_key = keys.private_key_hex;
        state.publicKeyNpub = keys.npub;
        
        // Update the UI
        updateUIFromConfig();
        
        // Show the keys to the user
        const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        alert(t('profile.newIdentityCreated') + '\n\nPublic Key (npub):\n' + keys.npub + '\n\nSecret Key (nsec):\n' + keys.nsec + '\n\n' + t('profile.saveNsecWarning'));
        
    } catch (error) {
        debugLog('ERROR generating key pair: ' + error);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.failedToGenerateKeys') : 'Failed to generate key pair') + ': ' + error);
    } finally {
        btn.disabled = false;
        btn.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('profile.generateKeyPair') : originalText);
    }
}

// Update the profile display (profile page from state.viewedProfile; sidebar from state.profile)
function updateProfileDisplay() {
    const nameEl = document.getElementById('profile-name');
    const pubkeyEl = document.getElementById('profile-pubkey');
    const aboutEl = document.getElementById('profile-about');
    const pictureEl = document.getElementById('profile-picture');
    const placeholderEl = document.getElementById('profile-placeholder');
    const bannerEl = document.getElementById('profile-banner');
    const nip05El = document.getElementById('profile-nip05');
    const websiteEl = document.getElementById('profile-website');
    const lightningEl = document.getElementById('profile-lightning');
    const lud16El = document.getElementById('profile-lud16');
    const sidebarAvatar = document.getElementById('sidebar-avatar');
    const sidebarPlaceholder = document.getElementById('sidebar-avatar-placeholder');
    const editProfileBtn = document.getElementById('edit-profile-btn');
    const followBtn = document.getElementById('follow-btn');
    const messageUserBtn = document.getElementById('message-user-btn');
    const muteBtn = document.getElementById('mute-btn');

    const viewingOwn = state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex;
    const profile = state.viewedProfile;

    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (profile) {
        if (nameEl) nameEl.textContent = profile.name || (viewingOwn ? state.config?.display_name : null) || t('profile.anonymous');
        if (aboutEl) aboutEl.textContent = profile.about || '';
        if (pictureEl && placeholderEl) {
            if (profile.picture) {
                pictureEl.src = profile.picture;
                pictureEl.style.display = 'block';
                placeholderEl.style.display = 'none';
                pictureEl.onerror = () => {
                    pictureEl.style.display = 'none';
                    placeholderEl.style.display = 'flex';
                };
            } else {
                pictureEl.style.display = 'none';
                placeholderEl.style.display = 'flex';
            }
        }
        if (bannerEl) {
            bannerEl.style.backgroundImage = profile.banner ? `url('${profile.banner}')` : '';
        }
        if (nip05El) {
            nip05El.textContent = profile.nip05 || '';
            nip05El.style.display = profile.nip05 ? 'block' : 'none';
        }
        if (websiteEl) {
            websiteEl.href = profile.website || '#';
            websiteEl.style.display = profile.website ? 'inline' : 'none';
        }
        if (lightningEl && lud16El) {
            lud16El.textContent = profile.lud16 || '';
            lightningEl.style.display = profile.lud16 ? 'inline' : 'none';
        }
    } else {
        if (nameEl) nameEl.textContent = viewingOwn ? (state.config?.display_name || t('profile.notConfigured')) : '…';
        if (aboutEl) aboutEl.textContent = '';
        if (pictureEl && placeholderEl) {
            pictureEl.style.display = 'none';
            placeholderEl.style.display = 'flex';
        }
        if (bannerEl) bannerEl.style.backgroundImage = '';
        if (nip05El) nip05El.style.display = 'none';
        if (websiteEl) websiteEl.style.display = 'none';
        if (lightningEl) lightningEl.style.display = 'none';
    }

    if (pubkeyEl) {
        if (viewingOwn && (state.publicKeyNpub || state.config?.public_key)) {
            pubkeyEl.textContent = state.publicKeyNpub || state.config.public_key;
            pubkeyEl.style.display = 'block';
        } else if (!viewingOwn && state.viewedProfilePubkey) {
            pubkeyEl.textContent = state.viewedProfilePubkey;
            pubkeyEl.style.display = 'block';
        } else {
            pubkeyEl.style.display = 'none';
        }
    }

    const noKeyNotice = document.getElementById('no-key-notice');
    if (noKeyNotice) {
        noKeyNotice.classList.toggle('hidden', !viewingOwn || !!state.config?.public_key);
    }

    if (editProfileBtn) editProfileBtn.style.display = viewingOwn ? 'block' : 'none';
    if (followBtn) followBtn.style.display = viewingOwn ? 'none' : 'block';
    if (messageUserBtn) messageUserBtn.style.display = viewingOwn ? 'none' : 'block';
    if (muteBtn) muteBtn.style.display = viewingOwn ? 'none' : 'block';

    if (sidebarAvatar && sidebarPlaceholder) {
        const pic = state.profile?.picture;
        if (pic) {
            sidebarAvatar.src = pic;
            sidebarAvatar.style.display = 'block';
            sidebarPlaceholder.style.display = 'none';
            sidebarAvatar.onerror = () => {
                sidebarAvatar.style.display = 'none';
                sidebarPlaceholder.style.display = 'flex';
            };
        } else {
            sidebarAvatar.style.display = 'none';
            sidebarPlaceholder.style.display = 'flex';
        }
    }
}

// ============================================================
// Following / Followers Management
// ============================================================

// Fetch following and followers
async function fetchFollowingAndFollowers() {
    if (!state.config || !state.config.public_key) return;
    const fc = document.getElementById('following-count');
    const fl = document.getElementById('followers-count');
    if (fc) fc.textContent = '…';
    if (fl) fl.textContent = '…';

    // Fetch both in parallel
    const [followingResult, followersResult] = await Promise.allSettled([
        fetchFollowing(),
        fetchFollowers()
    ]);
    
    // Handle following result
    if (followingResult.status === 'fulfilled' && followingResult.value) {
        displayFollowing(followingResult.value);
    } else {
        showFollowingMessage('Failed to fetch following list');
    }
    
    // Handle followers result
    if (followersResult.status === 'fulfilled' && followersResult.value) {
        displayFollowers(followersResult.value);
    } else {
        showFollowersMessage('Failed to fetch followers');
    }
}

// Fetch following (who you follow)
async function fetchFollowing() {
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

// Fetch followers (who follows you)
async function fetchFollowers() {
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
function displayFollowing(data) {
    const countEl = document.getElementById('following-count');
    if (!countEl) return;
    const count = data.contacts ? data.contacts.length : 0;
    countEl.textContent = count.toString();
}

// Display followers list (updates count on profile page)
function displayFollowers(data) {
    const countEl = document.getElementById('followers-count');
    if (!countEl) return;
    const count = data.followers ? data.followers.length : 0;
    countEl.textContent = count.toString();
}

// Create a follow item element
function createFollowItem(pubkey, petname) {
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
function showFollowingMessage(message) {
    const listEl = document.getElementById('following-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Show message in followers list
function showFollowersMessage(message) {
    const listEl = document.getElementById('followers-list');
    listEl.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// Switch between following/followers tabs
function switchFollowTab(tabName) {
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
// View Management
// ============================================================

// Switch to a different view
function switchView(viewName) {
    const viewEl = document.getElementById('view-' + viewName);
    if (!viewEl) return;

    document.querySelectorAll('.nav-item').forEach(item => {
        item.classList.remove('active');
        if (item.dataset.view === viewName) {
            item.classList.add('active');
        }
    });

    document.querySelectorAll('.view').forEach(view => {
        view.classList.remove('active');
    });
    viewEl.classList.add('active');
    state.currentView = viewName;

    if (viewName === 'profile') {
        fetchProfile();
        if (state.viewedProfilePubkey === null || state.viewedProfilePubkey === state.publicKeyHex) {
            fetchFollowingAndFollowers();
        }
    }
    if (viewName === 'feed' && state.initialFeedLoadDone && state.homeFeedMode === 'firehose') {
        fetchNotesFirehoseOnHomeClick();
    }
}

// ============================================================
// Modal Management
// ============================================================

// Open settings modal
function openSettings() {
    // Clear any previous validation errors
    clearValidationErrors();
    document.getElementById('settings-modal').classList.add('active');
}

// Close settings modal
function closeSettings() {
    document.getElementById('settings-modal').classList.remove('active');
}

// Clear validation error displays
function clearValidationErrors() {
    document.querySelectorAll('.validation-error').forEach(el => {
        el.textContent = '';
        el.style.display = 'none';
    });
    document.querySelectorAll('.form-group input').forEach(el => {
        el.classList.remove('invalid');
    });
}

// Show validation error for an input
function showValidationError(inputId, message) {
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

// ============================================================
// Relay Management
// ============================================================

// Update the relay list in the UI
function updateRelayList() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const relayList = document.getElementById('relay-list');
    relayList.innerHTML = '';
    
    if (!state.config || !state.config.relays) return;
    
    const testLabel = t('relays.test');
    const notTestedTitle = t('relays.notTested');
    state.config.relays.forEach((relay, index) => {
        const li = document.createElement('li');
        li.className = 'relay-item';
        li.innerHTML = `
            <span class="relay-url">${escapeHtml(relay)}</span>
            <button class="btn btn-small" onclick="testRelay('${escapeHtml(relay)}')">${escapeHtml(testLabel)}</button>
            <div class="relay-status" id="relay-status-${index}" title="${escapeHtml(notTestedTitle)}"></div>
        `;
        relayList.appendChild(li);
    });
}

// Test a relay connection
async function testRelay(relayUrl) {
    console.log('Testing relay:', relayUrl);
    
    try {
        const result = await invoke('test_relay_connection', { relayUrl: relayUrl });
        alert('✅ ' + result);
    } catch (error) {
        alert('❌ Connection failed: ' + error);
    }
}

// ============================================================
// Note Fetching (async, non-blocking; merge/sort; incremental poll)
// ============================================================

const FEED_LIMIT = 50;
const POLL_INTERVAL_MS = 45000;

// Returns list of hex pubkeys for "follows" mode, or null for firehose.
async function getHomeFeedAuthors() {
    if (state.homeFeedMode !== 'follows') return null;
    if (!state.config || !state.config.public_key) return null;
    try {
        const json = await invoke('fetch_own_following');
        if (!json) return null;
        const data = JSON.parse(json);
        const contacts = data.contacts || [];
        if (contacts.length === 0) return null;
        return contacts.map(c => c.pubkey).filter(Boolean);
    } catch (e) {
        console.error('Failed to get following for feed:', e);
        return null;
    }
}

// Low-level fetch: relayUrls, optional authors (hex), optional since (unix ts).
async function fetchFeedNotes(relayUrls, authors, since) {
    if (!relayUrls || relayUrls.length === 0) return [];
    const notesJson = await invoke('fetch_notes_from_relays', {
        relayUrls,
        limit: FEED_LIMIT,
        authors: authors && authors.length ? authors : null,
        since: since ?? null
    });
    if (!notesJson) return [];
    const notes = JSON.parse(notesJson);
    return Array.isArray(notes) ? notes : [];
}

// Merge new notes into state.notes. isIncremental: true = append new ones below the fold; false = replace and sort.
function mergeNotesIntoState(newNotes, isIncremental) {
    if (!newNotes || newNotes.length === 0 && !isIncremental) return;
    const seen = new Set(state.notes.map(n => n.id));
    if (!isIncremental) {
        state.notes = newNotes.slice();
        state.notes.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
        return;
    }
    const added = newNotes.filter(n => n.id && !seen.has(n.id));
    if (added.length === 0) return;
    added.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    state.notes = state.notes.concat(added);
}

// Start initial feed fetch (non-blocking). Call after loadConfig; UI is already visible.
async function startInitialFeedFetch() {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (!state.config || !state.config.relays || state.config.relays.length === 0) {
        showMessage(t('feed.noRelays'));
        return;
    }
    state.loading = true;
    try {
        let authors = null;
        if (state.homeFeedMode === 'follows') {
            authors = await getHomeFeedAuthors();
            if (!authors || authors.length === 0) {
                authors = null; // no follows => firehose
            }
        }
        const notes = await fetchFeedNotes(state.config.relays, authors, null);
        mergeNotesIntoState(notes, false);
        displayNotes(state.notes);
        state.initialFeedLoadDone = true;
        if (state.homeFeedMode === 'follows' && authors && authors.length > 0) {
            if (state.feedPollIntervalId) clearInterval(state.feedPollIntervalId);
            state.feedPollIntervalId = setInterval(pollForNewNotes, POLL_INTERVAL_MS);
        }
    } catch (error) {
        console.error('Initial feed fetch failed:', error);
        showMessage((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('feed.feedFailed') : 'Failed to load feed. Check relays and try again.'));
    } finally {
        state.loading = false;
    }
}

// Incremental poll (follows mode only). Fetches notes since latest we have; appends below the fold.
async function pollForNewNotes() {
    if (!state.config || !state.config.relays.length || state.loading) return;
    const authors = await getHomeFeedAuthors();
    if (!authors || authors.length === 0) return;
    const since = state.notes.length
        ? Math.max(...state.notes.map(n => n.created_at || 0))
        : 0;
    try {
        const notes = await fetchFeedNotes(state.config.relays, authors, since);
        if (notes.length === 0) return;
        mergeNotesIntoState(notes, true);
        displayNotes(state.notes);
    } catch (e) {
        console.error('Feed poll failed:', e);
    }
}

// Firehose: fetch new notes when user opens Home (no auto-poll).
async function fetchNotesFirehoseOnHomeClick() {
    if (!state.config || !state.config.relays.length || state.loading) return;
    const since = state.notes.length
        ? Math.max(...state.notes.map(n => n.created_at || 0))
        : 0;
    state.loading = true;
    try {
        const notes = await fetchFeedNotes(state.config.relays, null, since);
        if (notes.length > 0) {
            mergeNotesIntoState(notes, true);
            displayNotes(state.notes);
        }
    } catch (e) {
        console.error('Firehose fetch failed:', e);
    } finally {
        state.loading = false;
    }
}

// Show a message in the notes container
function showMessage(message) {
    const container = document.getElementById('notes-container');
    container.innerHTML = `
        <div class="placeholder-message">
            <p>${escapeHtml(message)}</p>
        </div>
    `;
}

// ============================================================
// Note Display
// ============================================================

// Get display name and NIP-05 for a pubkey from profile cache (NIP-05 is the verified identity, e.g. user@domain.com).
function getAuthorDisplay(pubkey) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const c = state.profileCache[pubkey];
    if (c) {
        return { name: c.name || t('profile.anonymous'), nip05: c.nip05 || '' };
    }
    return { name: '…', nip05: '' };
}

// Fetch profiles for note authors and update cache + DOM.
async function ensureProfilesForNotes(notes) {
    if (!state.config || !state.config.relays || state.config.relays.length === 0) return;
    const unique = [...new Set(notes.map(n => n.pubkey).filter(Boolean))];
    const toFetch = unique.filter(p => !state.profileCache[p]);
    if (toFetch.length === 0) return;
    const relays = state.config.relays;
    await Promise.all(toFetch.map(async (pubkey) => {
        try {
            const json = await invoke('fetch_profile', { pubkey, relayUrls: relays });
            if (!json || json === '{}') return;
            const p = JSON.parse(json);
            state.profileCache[pubkey] = {
                name: p.name || null,
                nip05: p.nip05 || null,
                picture: p.picture || null
            };
        } catch (_) { /* ignore */ }
    }));
    // Update cards: find by data-pubkey and set name + nip05
    toFetch.forEach(pubkey => {
        const profile = state.profileCache[pubkey];
        if (!profile) return;
        const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
        const name = profile.name || t('profile.anonymous');
        const nip05 = profile.nip05 || '';
        document.querySelectorAll(`.note-card[data-pubkey="${escapeCssAttr(pubkey)}"]`).forEach(card => {
            const nameEl = card.querySelector('.note-author-name');
            const nip05El = card.querySelector('.note-author-nip05');
            if (nameEl) nameEl.textContent = name;
            if (nip05El) {
                nip05El.textContent = nip05;
                nip05El.style.display = nip05 ? '' : 'none';
            }
            const avatar = card.querySelector('.note-avatar');
            if (avatar && profile.picture) {
                const fallback = avatar.querySelector('.avatar-fallback');
                const img = avatar.querySelector('img');
                if (img) {
                    img.src = profile.picture;
                    img.alt = '';
                    img.style.display = '';
                    if (fallback) fallback.style.display = 'none';
                } else {
                    const newImg = document.createElement('img');
                    newImg.src = profile.picture;
                    newImg.alt = '';
                    newImg.onerror = () => { if (fallback) fallback.style.display = 'flex'; };
                    avatar.insertBefore(newImg, avatar.firstChild);
                    if (fallback) fallback.style.display = 'none';
                }
            }
        });
    });
}

function escapeCssAttr(s) {
    return String(s).replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

// Create HTML for a note card: name, tick, NIP-05, time; content; action bar (icons spaced across width).
function createNoteCard(note, noteIndex) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const time = formatTimestamp(note.created_at);
    const { name: displayName, nip05 } = getAuthorDisplay(note.pubkey);
    const processedContent = processNoteContent(note.content);
    const safePubkey = escapeHtml(note.pubkey || '');
    const safeId = escapeHtml(note.id || '');

    const card = document.createElement('div');
    card.className = 'note-card';
    card.dataset.noteIndex = noteIndex;
    card.dataset.noteId = safeId;
    card.dataset.pubkey = note.pubkey || '';
    const viewProfile = t('note.viewProfile');
    const verifying = t('note.verifying');
    card.innerHTML = `
        <div class="note-top-row">
            <button type="button" class="note-avatar note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}" aria-label="${escapeHtml(viewProfile)}"><span class="avatar-fallback">?</span></button>
            <div class="note-head">
                <div class="note-head-line">
                    <button type="button" class="note-author-name note-author-link" data-pubkey="${safePubkey}" title="${escapeHtml(viewProfile)}">${escapeHtml(displayName)}</button>
                    <span class="note-verification" id="verify-${noteIndex}" title="${escapeHtml(verifying)}"><span class="verify-pending">·</span></span>
                    <span class="note-author-nip05" ${nip05 ? '' : 'style="display:none"'}>${escapeHtml(nip05)}</span>
                    <span class="note-time">${escapeHtml(time)}</span>
                </div>
                <div class="note-content">${processedContent}</div>
                <div class="note-actions">
                    <button type="button" class="note-action" title="${escapeHtml(t('note.reply'))}" aria-label="${escapeHtml(t('note.reply'))}" data-action="reply" data-note-id="${safeId}" data-pubkey="${safePubkey}"><img src="icons/reply.svg" alt="${escapeHtml(t('note.reply'))}" class="icon-reply"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.zap'))}" aria-label="${escapeHtml(t('note.zap'))}" data-action="zap"><img src="icons/zap.svg" alt="${escapeHtml(t('note.zap'))}" class="icon-zap"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.like'))}" aria-label="${escapeHtml(t('note.like'))}" data-action="like"><img src="icons/heart.svg" alt="${escapeHtml(t('note.like'))}" class="icon-heart"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.repost'))}" aria-label="${escapeHtml(t('note.repost'))}" data-action="repost"><img src="icons/repost.svg" alt="${escapeHtml(t('note.repost'))}" class="icon-repost"></button>
                    <button type="button" class="note-action" title="${escapeHtml(t('note.bookmark'))}" aria-label="${escapeHtml(t('note.bookmark'))}" data-action="bookmark"><img src="icons/bookmark.svg" alt="${escapeHtml(t('note.bookmark'))}" class="icon-bookmark"></button>
                </div>
            </div>
        </div>
    `;
    return card;
}

// Verify a note's signature
async function verifyNote(note, noteIndex) {
    try {
        const noteJson = JSON.stringify(note);
        const resultJson = await invoke('verify_event', { eventJson: noteJson });
        
        if (resultJson) {
            const result = JSON.parse(resultJson);
            updateVerificationBadge(noteIndex, result);
        }
    } catch (error) {
        console.error('Verification failed for note', noteIndex, error);
        updateVerificationBadge(noteIndex, { valid: false, error: error.toString() });
    }
}

// Update the verification badge for a note
function updateVerificationBadge(noteIndex, result) {
    const badgeEl = document.getElementById(`verify-${noteIndex}`);
    if (!badgeEl) return;
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

// Shorten a key for display
function shortenKey(key) {
    if (!key || key.length <= 16) {
        return key || '';
    }
    return key.substring(0, 8) + '...' + key.substring(key.length - 4);
}

// Format a Unix timestamp to human-readable relative time (e.g. 1min, 4h, 2 months)
function formatTimestamp(timestamp) {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now - date;
    const diffSec = Math.floor(diffMs / 1000);
    const diffMin = Math.floor(diffSec / 60);
    const diffHour = Math.floor(diffMin / 60);
    const diffDay = Math.floor(diffHour / 24);
    const diffMonth = Math.floor(diffDay / 30);
    const diffYear = Math.floor(diffDay / 365);

    if (diffSec < 60) return 'now';
    if (diffMin < 60) return diffMin === 1 ? '1min' : diffMin + 'min';
    if (diffHour < 24) return diffHour + 'h';
    if (diffDay < 30) return diffDay === 1 ? '1 day' : diffDay + ' days';
    if (diffMonth < 12) return diffMonth === 1 ? '1 month' : diffMonth + ' months';
    return diffYear === 1 ? '1 year' : diffYear + ' years';
}

// Process note content - find and embed images/videos
function processNoteContent(content) {
    // Escape HTML first
    let html = escapeHtml(content);
    
    // Find image URLs and convert to img tags
    const imageAlt = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('content.image') : 'Image');
    const imageRegex = /(https?:\/\/[^\s]+\.(jpg|jpeg|png|gif|webp)(\?[^\s]*)?)/gi;
    html = html.replace(imageRegex, '<img src="$1" alt="' + escapeHtml(imageAlt) + '" loading="lazy">');
    
    // Find video URLs and convert to video tags
    const videoRegex = /(https?:\/\/[^\s]+\.(mp4|webm|mov)(\?[^\s]*)?)/gi;
    html = html.replace(videoRegex, '<video src="$1" controls preload="metadata"></video>');
    
    // Convert plain URLs to links (but not ones we already converted)
    const urlRegex = /(?<!src=")(https?:\/\/[^\s<]+)(?![^<]*>)/gi;
    html = html.replace(urlRegex, '<a href="$1" target="_blank" rel="noopener">$1</a>');
    
    return html;
}

// Display notes in the feed
function displayNotes(notes) {
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    const container = document.getElementById('notes-container');
    container.innerHTML = '';
    
    if (!notes || notes.length === 0) {
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
        // Only show text notes (kind 1)
        if (note.kind === 1) {
            const card = createNoteCard(note, noteIndex);
            container.appendChild(card);
            notesToVerify.push({ note, index: noteIndex });
            noteIndex++;
        }
    });
    
    // If no kind 1 notes were found
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
}

// Verify notes asynchronously
async function verifyNotesAsync(notesToVerify) {
    // Verify in batches to avoid overwhelming the backend
    for (const { note, index } of notesToVerify) {
        // Don't await - let them run in parallel
        verifyNote(note, index);
        
        // Small delay to avoid hammering the backend
        await new Promise(resolve => setTimeout(resolve, 10));
    }
}

// ============================================================
// Utility Functions
// ============================================================

// Escape HTML to prevent XSS
function escapeHtml(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// ============================================================
// Event Handlers
// ============================================================

// Handle settings form submission
async function handleSettingsSubmit(event) {
    event.preventDefault();
    clearValidationErrors();
    
    const displayName = document.getElementById('input-display-name').value.trim();
    const publicKeyInput = document.getElementById('input-public-key').value.trim();
    const privateKeyInput = document.getElementById('input-private-key').value.trim();
    
    // Validate public key (if provided)
    let publicKeyHex = '';
    let publicKeyNpub = '';
    
    if (publicKeyInput) {
        const pubKeyResult = await validatePublicKey(publicKeyInput);
        if (!pubKeyResult.valid) {
            showValidationError('input-public-key', pubKeyResult.error);
            return;
        }
        publicKeyHex = pubKeyResult.hex;
        publicKeyNpub = pubKeyResult.npub;
    }
    
    // Validate private key (if provided)
    let privateKeyHex = null;
    
    if (privateKeyInput) {
        const privKeyResult = await validateSecretKey(privateKeyInput);
        if (!privKeyResult.valid) {
            showValidationError('input-private-key', privKeyResult.error);
            return;
        }
        privateKeyHex = privKeyResult.hex;
    }
    
    // Update config - store hex format internally
    state.config.display_name = displayName;
    state.config.public_key = publicKeyHex;
    state.config.private_key = privateKeyHex;
    
    // Update our display cache
    state.publicKeyHex = publicKeyHex;
    state.publicKeyNpub = publicKeyNpub;
    
    // Save and update UI
    await saveConfig();
    updateUIFromConfig();
    closeSettings();
    
    // Show success feedback
    console.log('Settings saved successfully');
}

// ============================================================
// Compose / Posting
// ============================================================

// State for compose
let isPosting = false;

// Open the compose modal (optionally as a reply: openCompose({ id, pubkey, name }))
function openCompose(replyingTo) {
    state.replyingTo = replyingTo || null;
    const modal = document.getElementById('compose-modal');
    const replyCtx = document.getElementById('compose-reply-context');
    const replyName = document.getElementById('compose-reply-name');
    if (replyCtx) replyCtx.style.display = state.replyingTo ? 'block' : 'none';
    const t = window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t.bind(window.PlumeI18n) : function(k) { return k; };
    if (replyName && state.replyingTo) replyName.textContent = state.replyingTo.name ? `@${state.replyingTo.name}` : t('note.replyLabel');
    modal.classList.add('active');
    const content = document.getElementById('compose-content');
    if (content) content.value = '';
    const charCountEl = document.getElementById('compose-char-count');
    if (charCountEl) charCountEl.textContent = t('composeModal.charCount', { count: 0 });
    hideComposeError();
    hideComposeStatus();
    enableComposeButton();
    setTimeout(() => content && content.focus(), 100);
}

// Close the compose modal
function closeCompose() {
    state.replyingTo = null;
    document.getElementById('compose-modal').classList.remove('active');
}

// Show error in compose modal
function showComposeError(message) {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = message;
    errorEl.classList.add('visible');
}

// Hide compose error
function hideComposeError() {
    const errorEl = document.getElementById('compose-error');
    errorEl.textContent = '';
    errorEl.classList.remove('visible');
}

// Show status in compose modal
function showComposeStatus(message, isSuccess = false) {
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
function hideComposeStatus() {
    const statusEl = document.getElementById('compose-status');
    statusEl.textContent = '';
    statusEl.classList.remove('visible');
    statusEl.classList.remove('success');
}

// Disable compose button during posting
function disableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = true;
        const text = document.getElementById('compose-btn-text');
        if (text) text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.posting') : 'Posting…');
    }
}

// Enable compose button
function enableComposeButton() {
    const btn = document.getElementById('submit-compose');
    if (btn) {
        btn.disabled = false;
        const text = document.getElementById('compose-btn-text');
        if (text) text.textContent = (window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('composeModal.post') : 'Post');
    }
}

// Update character count
function updateCharCount() {
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
async function handleComposeSubmit(event) {
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
            
            state.replyingTo = null;
            setTimeout(() => {
                closeCompose();
                if (state.homeFeedMode === 'follows') pollForNewNotes();
                else fetchNotesFirehoseOnHomeClick();
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
// Initialization
// ============================================================

// Initialize the application
async function init() {
    try {
        debugLog('Plume initializing...');
        await (window.PlumeI18n && window.PlumeI18n.init ? window.PlumeI18n.init() : Promise.resolve());
        
        // Load configuration
        await loadConfig();
        
        document.querySelector('.sidebar-logo')?.addEventListener('click', (e) => {
            e.preventDefault();
            switchView('feed');
        });

        document.querySelectorAll('.nav-item[data-view]').forEach(item => {
            item.addEventListener('click', (e) => {
                e.preventDefault();
                if (item.dataset.view === 'profile') state.viewedProfilePubkey = null;
                if (item.dataset.view) switchView(item.dataset.view);
            });
        });

        // Settings modal (Account) – open from Settings menu
        const closeSettingsBtn = document.getElementById('close-settings');
        const settingsModal = document.getElementById('settings-modal');
        if (closeSettingsBtn) closeSettingsBtn.addEventListener('click', closeSettings);
        if (settingsModal) {
            settingsModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) closeSettings();
            });
        }

        // Settings page menu – Account opens modal, Relays shows relay list
        document.querySelectorAll('.settings-menu-item').forEach(btn => {
            btn.addEventListener('click', () => {
                const key = btn.dataset.settings;
                const detailDefault = document.getElementById('settings-detail-default');
                const relaysContainer = document.getElementById('relays-container');
                if (detailDefault) detailDefault.style.display = key === 'relays' ? 'none' : 'block';
                if (relaysContainer) relaysContainer.style.display = key === 'relays' ? 'block' : 'none';
                if (key === 'account') openSettings();
                if (key === 'relays') updateRelayList();
            });
        });
        
        // Set up settings form
        const settingsForm = document.getElementById('settings-form');
        if (settingsForm) settingsForm.addEventListener('submit', handleSettingsSubmit);
        
        // Set up compose modal
        const composeBtn = document.getElementById('compose-btn');
        const closeComposeBtn = document.getElementById('close-compose');
        const cancelComposeBtn = document.getElementById('cancel-compose');
        const composeModal = document.getElementById('compose-modal');
        if (composeBtn) composeBtn.addEventListener('click', openCompose);
        if (closeComposeBtn) closeComposeBtn.addEventListener('click', closeCompose);
        if (cancelComposeBtn) cancelComposeBtn.addEventListener('click', closeCompose);
        if (composeModal) {
            composeModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) {
                    closeCompose();
                }
            });
        }
        
        // Set up compose form
        const composeForm = document.getElementById('compose-form');
        const composeContent = document.getElementById('compose-content');
        if (composeForm) composeForm.addEventListener('submit', handleComposeSubmit);
        if (composeContent) composeContent.addEventListener('input', updateCharCount);

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
        
        document.querySelectorAll('.notif-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.notif-tab').forEach(t => t.classList.remove('active'));
                tab.classList.add('active');
                state.notifFilter = tab.dataset.filter;
                // TODO: filter notifications by state.notifFilter
            });
        });

        document.querySelectorAll('.profile-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                document.querySelectorAll('.profile-tab').forEach(t => t.classList.remove('active'));
                tab.classList.add('active');
                state.profileTab = tab.dataset.tab;
                // TODO: load profile feed by state.profileTab
            });
        });

        // Note card: reply button and author link (avatar/name -> profile)
        const notesContainer = document.getElementById('notes-container');
        if (notesContainer) {
            notesContainer.addEventListener('click', (e) => {
                const authorLink = e.target.closest('.note-author-link');
                if (authorLink && authorLink.dataset.pubkey) {
                    e.preventDefault();
                    openProfileForUser(authorLink.dataset.pubkey);
                    return;
                }
                const replyBtn = e.target.closest('.note-action[data-action="reply"]');
                if (replyBtn) {
                    e.preventDefault();
                    const card = replyBtn.closest('.note-card');
                    const name = card ? (card.querySelector('.note-author-name')?.textContent || '').trim() : '';
                    openCompose({
                        id: replyBtn.dataset.noteId || '',
                        pubkey: replyBtn.dataset.pubkey || '',
                        name: name || '…'
                    });
                }
            });
        }

        startInitialFeedFetch();
        debugLog('Plume initialized successfully');
    } catch (error) {
        debugLog('Init FAILED: ' + error.message);
        alert((window.PlumeI18n && window.PlumeI18n.t ? window.PlumeI18n.t('errors.initError') : 'Initialization error') + ': ' + error.message);
    }
}

// Run initialization when the page loads
console.log('=== Setting up DOMContentLoaded listener ===');
document.addEventListener('DOMContentLoaded', function() {
    console.log('=== DOMContentLoaded fired ===');
    // Debug area should exist now
    debugLog('DOMContentLoaded fired');
    debugLog('Starting init...');
    init();
});
console.log('=== app.js fully loaded ===');
