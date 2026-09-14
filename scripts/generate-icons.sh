#!/bin/bash

set -euo pipefail

project_root="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

command -v magick >/dev/null
command -v iconutil >/dev/null
mkdir -p "$project_root/build/icons" "$scratch/app.iconset"

# Render the rounded tile at 4x for smooth alpha edges. At 1024px the
# artwork is 824px wide, inset 100px, with a 184px corner radius.
magick "$project_root/assets/logo.png" -resize 3296x3296! \
  \( -size 3296x3296 xc:none -fill white \
     -draw 'roundrectangle 0,0 3295,3295 736,736' \) \
  -alpha off -compose CopyOpacity -composite \
  -compose Over -bordercolor none -border 400 -resize 1024x1024 \
  -depth 8 -define png:color-type=6 \
  "$project_root/build/icons/1024x1024.png"

for size in 16 32 64 128 256 512; do
  magick "$project_root/build/icons/1024x1024.png" -filter Lanczos \
    -resize "${size}x${size}" -depth 8 -define png:color-type=6 \
    "$project_root/build/icons/${size}x${size}.png"
done

for size in 16 32 128 256 512; do
  cp "$project_root/build/icons/${size}x${size}.png" \
    "$scratch/app.iconset/icon_${size}x${size}.png"
  retina=$((size * 2))
  cp "$project_root/build/icons/${retina}x${retina}.png" \
    "$scratch/app.iconset/icon_${size}x${size}@2x.png"
done

iconutil -c icns "$scratch/app.iconset" -o "$project_root/build/icon.icns"

# Both appearance variants intentionally use the same approved artwork.
cp "$project_root/assets/logo.png" "$project_root/assets/logo-theme-light.png"
cp "$project_root/assets/logo.png" "$project_root/assets/logo-theme-dark.png"
