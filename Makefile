# Plume - Nostr Desktop Client
# Build using: make

# Default target
all: build

# Development build (faster compilation, includes debug symbols)
build:
	cargo build

# Release build (optimized, smaller binary)
release:
	cargo build --release

# Run the development version
run:
	cargo tauri dev

# Run the release version
run-release:
	cargo run --release

# Build macOS .app bundle (generates icons if missing, then builds)
app: icons-ensure
	cargo tauri build

# Build distributable packages for the current platform (alias for app)
bundle: app

# Generate icons only if the .icns file is missing
icons-ensure:
	@if [ ! -f icons/icon.icns ]; then \
		echo "App icons not found, generating from icons/app-icon.svg..."; \
		$(MAKE) icons; \
	fi

# Clean build artifacts
clean:
	cargo clean

# Check code without building (faster feedback)
check:
	cargo check

# Format code using rustfmt
fmt:
	cargo fmt

# Run clippy linter
lint:
	cargo clippy -- -W warnings

# Run tests
test:
	cargo test

# Regenerate app icons (32x32, 128x128, icon.ico, icon.icns) from icons/app-icon.svg
# Requires: ImageMagick (convert), librsvg (rsvg-convert), or Python cairosvg
icons:
	@chmod +x scripts/gen-icons.sh 2>/dev/null || true
	./scripts/gen-icons.sh

# Install required tools (run once during setup)
setup:
	@echo "Installing required tools..."
	cargo install tauri-cli
	@echo "Setup complete!"

# Show help
help:
	@echo "Plume - Nostr Desktop Client"
	@echo ""
	@echo "Available targets:"
	@echo "  make build      - Build debug version"
	@echo "  make release    - Build optimized release version"
	@echo "  make app        - Build macOS .app bundle (generates icons if needed)"
	@echo "  make run        - Run in development mode (with hot reload)"
	@echo "  make clean      - Remove build artifacts"
	@echo "  make check      - Quick syntax/type check"
	@echo "  make fmt        - Format source code"
	@echo "  make lint       - Run clippy linter"
	@echo "  make test       - Run tests"
	@echo "  make icons      - Regenerate app icons from icons/app-icon.svg"
	@echo "  make setup      - Install required tools (tauri-cli)"
	@echo ""

.PHONY: all build release run run-release app bundle icons-ensure clean check fmt lint test setup icons help

