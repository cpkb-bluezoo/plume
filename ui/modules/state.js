/*
 * modules/state.js
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

// Default relays used for anonymous firehose browsing when no user config is
// loaded. Onion addresses, same as Config::new()'s bootstrap list in
// config.rs and for the same reason: reached through Tor's own hidden-service
// rendezvous rather than a public exit node, so they're unaffected by relays
// that block or rate-limit Tor exit traffic.
export const DEFAULT_RELAYS = [
    'ws://oxtrdevav64z64yb7x6rjg4ntzqjhedm5b5zjqulugknhzr46ny2qbad.onion', // nostr.oxtr.dev
    'ws://gnostr2jnapk72mnagq3cuykfon73temzp77hcbncn4silgt77boruid.onion', // nostr.girino.org
    'ws://35vr3xigzjv2xyzfyif6o2gksmkioppy4rmwag7d4bqmwuccs2u4jaid.onion'  // Noderunners relay
];

// Returns the relay list to use: user's configured relays if available, otherwise defaults
export function getEffectiveRelays() {
    if (state.config && Array.isArray(state.config.relays) && state.config.relays.length > 0) {
        return state.config.relays;
    }
    return DEFAULT_RELAYS;
}

// Feed constants
export const FEED_LIMIT = 50;
export const POLL_INTERVAL_MS = 45000;

// Default emoji for quick like (heart). Common social/media emojis for long-press picker.
export const DEFAULT_LIKE_EMOJI = '❤️';
export const LIKE_EMOJI_LIST = ['❤️','🤙','👍','😂','😢','😡','🎉','🔥','👀','💯','❤️‍🔥','😍','🤔','👏','🙏','😭','🤣','💀','✨','💪'];

// Maximum recursion depth for embedded nostr: note references
export const NOSTR_EMBED_MAX_DEPTH = 5;

// Application state
export const state = {
    appConfig: null,
    config: {
        public_key: '',
        private_key: null,
        key_storage: 'plaintext',
        relays: DEFAULT_RELAYS.slice(),
        name: 'Anonymous',
        about: null, picture: null, nip05: null, banner: null, website: null, lud16: null,
        home_feed_mode: 'firehose',
        media_server_url: 'https://blossom.primal.net',
        following: [], muted_users: [], muted_words: [], muted_hashtags: [], bookmarks: [],
        default_zap_amount: 42,
        hide_encrypted_notes: true
    },
    currentView: 'feed',
    notes: [],
    loading: false,
    // Whether a usable OS keyring backend exists on this machine; null
    // until the one-time startup probe (invoke('is_keyring_available'))
    // resolves. Cached for the session -- see maybeOfferKeyringStorage().
    keyringAvailable: null,
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
    viewedProfile: null,
    // Profile feed: notes for the currently viewed user (streamed or batch)
    profileNotes: [],
    profileFeedStreamNoteIndex: 0,
    profileNotesForPubkey: null, // pubkey for which profileNotes was loaded (so tab switch can reuse)
    viewedProfileRelays: null,   // relay URLs for the currently displayed user (NIP-65); null = not loaded
    viewedProfileRelaysForPubkey: null,
    bookmarkNotes: [],  // Notes currently shown on bookmarks page (for repost/like lookup)
    likedNoteIds: {},   // noteId -> true (notes we've liked this session; shows filled heart)
    ownFollowingPubkeys: [],  // Hex pubkeys we follow (for Follow/Unfollow button state)
    // Note detail page
    noteDetailSubjectId: null,
    noteDetailSubject: null,
    noteDetailAncestors: [],
    noteDetailReplies: [],   // [{ note, indent }, ...]
    noteDetailPreviousView: 'feed',
    // Unread DMs count for sidebar Messages icon (filled icon + badge). Set by DM sync when implemented.
    unreadMessageCount: 0,
    // Messages view
    selectedConversation: null,   // other_pubkey (hex) or null
    openConversationWith: null,   // when opening Messages from Profile "Message", set to that pubkey
    dmStreamStarted: false,
    // Follows settings panel: working copy [{ pubkey (hex), checked, listOrder }], sort key
    followsPanelList: [],
    followsPanelSort: 'name',
    // Track where user was before entering edit-profile in settings (so Save navigates back)
    editProfilePreviousView: null,
    // Muted users panel: working copy [{ pubkey, checked }], no config change until Save
    mutedUsersPanelList: []
};
