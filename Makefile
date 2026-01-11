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

# Build distributable packages for the current platform
bundle:
	cargo tauri build

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
	@echo "  make run        - Run in development mode (with hot reload)"
	@echo "  make bundle     - Create distributable packages"
	@echo "  make clean      - Remove build artifacts"
	@echo "  make check      - Quick syntax/type check"
	@echo "  make fmt        - Format source code"
	@echo "  make lint       - Run clippy linter"
	@echo "  make test       - Run tests"
	@echo "  make setup      - Install required tools"
	@echo ""

.PHONY: all build release run run-release bundle clean check fmt lint test setup help

