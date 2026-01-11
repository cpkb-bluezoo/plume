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
    // Parsed key info
    publicKeyHex: null,
    publicKeyNpub: null,
    // Profile data fetched from relays
    profile: null,
    profileLoading: false
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
        alert('Failed to save settings: ' + error);
    }
}

// Update UI elements from the current config
function updateUIFromConfig() {
    if (!state.config) return;
    
    // Update settings form - show the original value (could be npub or hex)
    document.getElementById('input-display-name').value = state.config.display_name || '';
    document.getElementById('input-public-key').value = state.config.public_key || '';
    document.getElementById('input-private-key').value = state.config.private_key || '';
    
    // Update profile view with config data (will be overwritten if profile is fetched)
    updateProfileDisplay();
    
    // Update relay list
    updateRelayList();
}

// ============================================================
// Profile Management
// ============================================================

// Fetch profile from relays
async function fetchProfile() {
    if (state.profileLoading) {
        console.log('Already loading profile...');
        return;
    }
    
    if (!state.config || !state.config.public_key) {
        console.log('No public key configured');
        return;
    }
    
    state.profileLoading = true;
    console.log('Fetching profile from relays...');
    
    try {
        const profileJson = await invoke('fetch_own_profile');
        
        if (profileJson && profileJson !== '{}') {
            state.profile = JSON.parse(profileJson);
            console.log('Profile fetched:', state.profile);
        } else {
            console.log('No profile found on relays');
            state.profile = null;
        }
        
        updateProfileDisplay();
    } catch (error) {
        console.error('Failed to fetch profile:', error);
    } finally {
        state.profileLoading = false;
    }
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
        btn.textContent = 'Generating...';
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
        alert(`New identity created!\n\nPublic Key (npub):\n${keys.npub}\n\nSecret Key (nsec):\n${keys.nsec}\n\nIMPORTANT: Save your nsec in a safe place. You will need it to recover your identity!`);
        
    } catch (error) {
        debugLog('ERROR generating key pair: ' + error);
        alert('Failed to generate key pair: ' + error);
    } finally {
        btn.disabled = false;
        btn.textContent = originalText;
    }
}

// Update the profile display
function updateProfileDisplay() {
    // Get elements
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
    
    // If we have profile data from relays, use it
    if (state.profile) {
        // Name (prefer profile name, fall back to config display_name)
        nameEl.textContent = state.profile.name || state.config?.display_name || 'Anonymous';
        
        // About
        aboutEl.textContent = state.profile.about || '';
        
        // Picture
        if (state.profile.picture) {
            pictureEl.src = state.profile.picture;
            pictureEl.style.display = 'block';
            placeholderEl.style.display = 'none';
            
            // Handle image load errors
            pictureEl.onerror = () => {
                pictureEl.style.display = 'none';
                placeholderEl.style.display = 'flex';
            };
        } else {
            pictureEl.style.display = 'none';
            placeholderEl.style.display = 'flex';
        }
        
        // Banner
        if (state.profile.banner) {
            bannerEl.style.backgroundImage = `url('${state.profile.banner}')`;
        } else {
            bannerEl.style.backgroundImage = '';
        }
        
        // NIP-05
        if (state.profile.nip05) {
            nip05El.textContent = state.profile.nip05;
            nip05El.style.display = 'block';
        } else {
            nip05El.style.display = 'none';
        }
        
        // Website
        if (state.profile.website) {
            websiteEl.href = state.profile.website;
            websiteEl.style.display = 'inline';
        } else {
            websiteEl.style.display = 'none';
        }
        
        // Lightning address
        if (state.profile.lud16) {
            lud16El.textContent = state.profile.lud16;
            lightningEl.style.display = 'inline';
        } else {
            lightningEl.style.display = 'none';
        }
    } else {
        // No profile data - use config values
        nameEl.textContent = state.config?.display_name || 'Not configured';
        aboutEl.textContent = '';
        pictureEl.style.display = 'none';
        placeholderEl.style.display = 'flex';
        bannerEl.style.backgroundImage = '';
        nip05El.style.display = 'none';
        websiteEl.style.display = 'none';
        lightningEl.style.display = 'none';
    }
    
    // Public key (always show from config/state)
    if (state.publicKeyNpub) {
        pubkeyEl.textContent = state.publicKeyNpub;
    } else if (state.config?.public_key) {
        pubkeyEl.textContent = state.config.public_key;
    } else {
        pubkeyEl.textContent = 'No public key set';
    }
    
    // Show/hide the "generate key pair" notice
    const noKeyNotice = document.getElementById('no-key-notice');
    if (noKeyNotice) {
        if (state.config?.public_key) {
            noKeyNotice.classList.add('hidden');
        } else {
            noKeyNotice.classList.remove('hidden');
        }
    }
}

// ============================================================
// Following / Followers Management
// ============================================================

// Fetch following and followers
async function fetchFollowingAndFollowers() {
    if (!state.config || !state.config.public_key) {
        showFollowingMessage('No public key configured. Set your key in Settings.');
        return;
    }
    
    // Show loading state
    document.getElementById('following-count').textContent = '...';
    document.getElementById('followers-count').textContent = '...';
    showFollowingMessage('Fetching following list...');
    showFollowersMessage('Fetching followers...');
    
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

// Display following list
function displayFollowing(data) {
    const countEl = document.getElementById('following-count');
    const listEl = document.getElementById('following-list');
    
    const count = data.contacts ? data.contacts.length : 0;
    countEl.textContent = count.toString();
    
    if (count === 0) {
        listEl.innerHTML = `
            <div class="placeholder-message">
                <p>Not following anyone yet</p>
            </div>
        `;
        return;
    }
    
    listEl.innerHTML = '';
    
    for (const contact of data.contacts) {
        const item = createFollowItem(contact.pubkey, contact.petname);
        listEl.appendChild(item);
    }
}

// Display followers list
function displayFollowers(data) {
    const countEl = document.getElementById('followers-count');
    const listEl = document.getElementById('followers-list');
    
    const count = data.followers ? data.followers.length : 0;
    countEl.textContent = count.toString();
    
    if (count === 0) {
        listEl.innerHTML = `
            <div class="placeholder-message">
                <p>No followers found</p>
                <p class="text-muted">Note: Finding followers requires scanning relays</p>
            </div>
        `;
        return;
    }
    
    listEl.innerHTML = '';
    
    for (const follower of data.followers) {
        const item = createFollowItem(follower.pubkey, null);
        listEl.appendChild(item);
    }
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
    // Update navigation
    document.querySelectorAll('.nav-item').forEach(item => {
        item.classList.remove('active');
        if (item.dataset.view === viewName) {
            item.classList.add('active');
        }
    });
    
    // Update views
    document.querySelectorAll('.view').forEach(view => {
        view.classList.remove('active');
    });
    document.getElementById('view-' + viewName).classList.add('active');
    
    state.currentView = viewName;
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
    const relayList = document.getElementById('relay-list');
    relayList.innerHTML = '';
    
    if (!state.config || !state.config.relays) return;
    
    state.config.relays.forEach((relay, index) => {
        const li = document.createElement('li');
        li.className = 'relay-item';
        li.innerHTML = `
            <span class="relay-url">${escapeHtml(relay)}</span>
            <button class="btn btn-small" onclick="testRelay('${escapeHtml(relay)}')">Test</button>
            <div class="relay-status" id="relay-status-${index}" title="Not tested"></div>
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
// Note Fetching
// ============================================================

// Fetch notes from configured relays
async function fetchNotes() {
    if (state.loading) {
        console.log('Already loading notes...');
        return;
    }
    
    if (!state.config || !state.config.relays || state.config.relays.length === 0) {
        showMessage('No relays configured. Add relays in Settings.');
        return;
    }
    
    state.loading = true;
    updateConnectionStatus('connecting');
    showMessage('Fetching notes from relays...');
    
    try {
        // Fetch from all configured relays
        const notesJson = await invoke('fetch_notes_from_relays', {
            relayUrls: state.config.relays,
            limit: 50
        });
        
        if (notesJson) {
            const notes = JSON.parse(notesJson);
            console.log('Received notes:', notes.length);
            
            state.notes = notes;
            displayNotes(notes);
            updateConnectionStatus('connected');
        } else {
            showMessage('No notes received from relays.');
            updateConnectionStatus('disconnected');
        }
    } catch (error) {
        console.error('Failed to fetch notes:', error);
        showMessage('Failed to fetch notes: ' + error);
        updateConnectionStatus('disconnected');
    } finally {
        state.loading = false;
    }
}

// Update the connection status indicator
function updateConnectionStatus(status) {
    const statusEl = document.getElementById('connection-status');
    
    switch (status) {
        case 'connected':
            statusEl.textContent = 'Connected';
            statusEl.className = 'status connected';
            break;
        case 'connecting':
            statusEl.textContent = 'Connecting...';
            statusEl.className = 'status connecting';
            break;
        case 'disconnected':
        default:
            statusEl.textContent = 'Disconnected';
            statusEl.className = 'status disconnected';
            break;
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

// Create HTML for a note card
function createNoteCard(note, noteIndex) {
    const time = formatTimestamp(note.created_at);
    const shortPubkey = shortenKey(note.pubkey);
    
    // Process content for media
    const processedContent = processNoteContent(note.content);
    
    const card = document.createElement('div');
    card.className = 'note-card';
    card.dataset.noteIndex = noteIndex;
    card.innerHTML = `
        <div class="note-header">
            <div class="note-avatar">👤</div>
            <div class="note-author">
                <div class="note-author-name">${escapeHtml(shortPubkey)}</div>
                <div class="note-author-pubkey">${escapeHtml(shortPubkey)}</div>
            </div>
            <div class="note-verification" id="verify-${noteIndex}" title="Verifying signature...">
                <span class="verify-pending">⏳</span>
            </div>
            <div class="note-time">${time}</div>
        </div>
        <div class="note-content">${processedContent}</div>
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
    
    if (result.valid) {
        badgeEl.innerHTML = '<span class="verify-valid" title="Signature verified ✓">✓</span>';
        badgeEl.title = 'Signature verified';
    } else {
        const errorMsg = result.error || 'Invalid signature';
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

// Format a Unix timestamp to a readable string
function formatTimestamp(timestamp) {
    const date = new Date(timestamp * 1000);
    const now = new Date();
    const diffMs = now - date;
    const diffSec = Math.floor(diffMs / 1000);
    const diffMin = Math.floor(diffSec / 60);
    const diffHour = Math.floor(diffMin / 60);
    const diffDay = Math.floor(diffHour / 24);
    
    if (diffSec < 60) {
        return 'just now';
    } else if (diffMin < 60) {
        return diffMin + 'm ago';
    } else if (diffHour < 24) {
        return diffHour + 'h ago';
    } else if (diffDay < 7) {
        return diffDay + 'd ago';
    } else {
        return date.toLocaleDateString();
    }
}

// Process note content - find and embed images/videos
function processNoteContent(content) {
    // Escape HTML first
    let html = escapeHtml(content);
    
    // Find image URLs and convert to img tags
    // Common image extensions
    const imageRegex = /(https?:\/\/[^\s]+\.(jpg|jpeg|png|gif|webp)(\?[^\s]*)?)/gi;
    html = html.replace(imageRegex, '<img src="$1" alt="Image" loading="lazy">');
    
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
    const container = document.getElementById('notes-container');
    container.innerHTML = '';
    
    if (!notes || notes.length === 0) {
        container.innerHTML = `
            <div class="placeholder-message">
                <p>No notes to display</p>
                <p>Click Refresh to fetch notes from relays</p>
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
                <p>No text notes found</p>
                <p>Try connecting to different relays</p>
            </div>
        `;
        return;
    }
    
    // Verify notes asynchronously (don't block UI)
    verifyNotesAsync(notesToVerify);
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

// Open the compose modal
function openCompose() {
    const modal = document.getElementById('compose-modal');
    modal.classList.add('active');
    
    // Clear form
    document.getElementById('compose-content').value = '';
    document.getElementById('char-count').textContent = '0';
    hideComposeError();
    hideComposeStatus();
    enableComposeButton();
    
    // Focus the textarea
    setTimeout(() => {
        document.getElementById('compose-content').focus();
    }, 100);
}

// Close the compose modal
function closeCompose() {
    const modal = document.getElementById('compose-modal');
    modal.classList.remove('active');
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
    btn.disabled = true;
    document.getElementById('compose-btn-text').textContent = 'Posting...';
}

// Enable compose button
function enableComposeButton() {
    const btn = document.getElementById('submit-compose');
    btn.disabled = false;
    document.getElementById('compose-btn-text').textContent = 'Post Note';
}

// Update character count
function updateCharCount() {
    const textarea = document.getElementById('compose-content');
    const count = textarea.value.length;
    document.getElementById('char-count').textContent = count;
}

// Handle compose form submission
async function handleComposeSubmit(event) {
    event.preventDefault();
    
    if (isPosting) {
        return;
    }
    
    const content = document.getElementById('compose-content').value.trim();
    
    // Validate content
    if (!content) {
        showComposeError('Please enter some text for your note');
        return;
    }
    
    if (content.length > 10000) {
        showComposeError('Note is too long (max 10000 characters)');
        return;
    }
    
    // Check if we have a private key
    if (!state.config || !state.config.private_key) {
        showComposeError('No private key configured. Add your nsec in Settings to post notes.');
        return;
    }
    
    isPosting = true;
    hideComposeError();
    showComposeStatus('Signing and publishing note...');
    disableComposeButton();
    
    try {
        // Call the backend to post the note
        const resultJson = await invoke('post_note', { content: content });
        const result = JSON.parse(resultJson);
        
        console.log('Post result:', result);
        
        if (result.success_count > 0) {
            showComposeStatus(
                `Published to ${result.success_count} of ${result.total_count} relay(s)`,
                true
            );
            
            // Close modal after a short delay
            setTimeout(() => {
                closeCompose();
                // Refresh the feed to show the new note
                fetchNotes();
            }, 1500);
        } else {
            // All relays failed
            let errorMessage = 'Failed to publish to any relay';
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
        showComposeError('Failed to post note: ' + error);
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
        
        // Load configuration
        await loadConfig();
        
        // Set up navigation
        document.querySelectorAll('.nav-item').forEach(item => {
            item.addEventListener('click', (e) => {
                e.preventDefault();
                switchView(item.dataset.view);
            });
        });
        
        // Set up settings modal
        const settingsBtn = document.getElementById('settings-btn');
        const closeSettingsBtn = document.getElementById('close-settings');
        const settingsModal = document.getElementById('settings-modal');
        if (settingsBtn) settingsBtn.addEventListener('click', openSettings);
        if (closeSettingsBtn) closeSettingsBtn.addEventListener('click', closeSettings);
        if (settingsModal) {
            settingsModal.addEventListener('click', (e) => {
                if (e.target === e.currentTarget) {
                    closeSettings();
                }
            });
        }
        
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
        
        // Set up refresh buttons
        const refreshBtn = document.getElementById('refresh-btn');
        const refreshProfileBtn = document.getElementById('refresh-profile-btn');
        const refreshFollowingBtn = document.getElementById('refresh-following-btn');
        if (refreshBtn) refreshBtn.addEventListener('click', fetchNotes);
        if (refreshProfileBtn) refreshProfileBtn.addEventListener('click', fetchProfile);
        if (refreshFollowingBtn) refreshFollowingBtn.addEventListener('click', fetchFollowingAndFollowers);
        
        // Set up generate keys button
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
        
        // Set up following/followers tabs
        document.querySelectorAll('.follow-tab').forEach(tab => {
            tab.addEventListener('click', () => {
                switchFollowTab(tab.dataset.tab);
            });
        });
        
        // Show initial message
        showMessage('Click Refresh to fetch notes from Nostr relays');
        
        debugLog('Plume initialized successfully');
    } catch (error) {
        debugLog('Init FAILED: ' + error.message);
        alert('Initialization error: ' + error.message);
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
