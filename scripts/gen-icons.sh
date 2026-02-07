#!/usr/bin/env bash
# Generate Plume app icons from icons/app-icon.svg
# Requires: one of ImageMagick (convert), librsvg (rsvg-convert), or Python cairosvg
# Then: cargo tauri icon (via make icons or cargo tauri icon)

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ICONS_DIR="$ROOT_DIR/icons"
SVG="$ICONS_DIR/app-icon.svg"
PNG="$ICONS_DIR/app-icon.png"

cd "$ROOT_DIR"

if [[ ! -f "$SVG" ]]; then
  echo "Missing $SVG" >&2
  exit 1
fi

# Generate 1024x1024 PNG from SVG
if command -v convert &>/dev/null; then
  echo "Using ImageMagick..."
  convert -background none -resize 1024x1024 "$SVG" "$PNG"
elif command -v rsvg-convert &>/dev/null; then
  echo "Using rsvg-convert (librsvg)..."
  rsvg-convert -w 1024 -h 1024 "$SVG" -o "$PNG"
elif python3 -c "import cairosvg" 2>/dev/null; then
  echo "Using Python cairosvg..."
  SVG_PATH="$SVG" PNG_PATH="$PNG" python3 << 'PY'
import cairosvg
import os
cairosvg.svg2png(
    url=os.environ["SVG_PATH"],
    write_to=os.environ["PNG_PATH"],
    output_width=1024,
    output_height=1024
)
PY
else
  echo "No SVG→PNG converter found. Install one of:" >&2
  echo "  brew install imagemagick    # for convert" >&2
  echo "  brew install librsvg        # for rsvg-convert" >&2
  echo "  pip install cairosvg        # for Python" >&2
  echo "Then create $PNG (1024x1024) from $SVG and run: cargo tauri icon $PNG -o icons" >&2
  exit 1
fi

echo "Generated $PNG"
echo "Running: cargo tauri icon $PNG -o icons"
cargo tauri icon "$PNG" -o icons
echo "Icons generated in $ICONS_DIR"
