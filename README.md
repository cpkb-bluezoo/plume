# Plume 🪶

A cross-platform Nostr desktop client built with Rust and Tauri.

## Features

- Browse Nostr notes from multiple relays
- View images and videos embedded in notes
- Manage your Nostr identity (public/private keys)
- Configure relay connections
- Cross-platform: MacOS, Linux, Windows

## Prerequisites

### MacOS

```bash
# Install Xcode Command Line Tools
xcode-select --install

# Install Rust (recommended method)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Tauri CLI
cargo install tauri-cli
```

### Linux (Debian/Ubuntu)

```bash
# Install system dependencies
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
    libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install Tauri CLI
cargo install tauri-cli
```

### Windows

1. Install [Microsoft Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
2. Install [Rust](https://rustup.rs/)
3. Run: `cargo install tauri-cli`

## Building

```bash
# Development build (faster, with debug symbols)
make build

# Release build (optimized)
make release

# Create distributable packages
make bundle
```

## Running

```bash
# Development mode with hot reload
make run

# Or directly with cargo
cargo tauri dev
```

## Configuration

Plume stores its configuration in `~/.plume/config.json`. This includes:

- Your Nostr public key (required for following/viewing)
- Your Nostr private key (optional, for posting)
- List of relay URLs to connect to
- Display name and other preferences

## License

GPL-3.0 - See [COPYING](COPYING) for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## Nostr Resources

- [Nostr Protocol](https://github.com/nostr-protocol/nostr) - Protocol specification
- [NIPs](https://github.com/nostr-protocol/nips) - Nostr Implementation Possibilities
- [Awesome Nostr](https://github.com/aljazceru/awesome-nostr) - Curated list of Nostr resources
