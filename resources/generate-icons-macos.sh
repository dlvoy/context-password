#!/bin/bash
# Regenerates resources/AppIcon.icns from development/logo.png.
#
# Not run by `cargo build`/`cargo packager` — a one-time (or as-needed) asset
# step, the macOS counterpart of generate-icons.ps1 (same source image, same
# "pad onto a transparent square canvas, then resize" approach), documented
# in BUILDING.md. Uses only tools Xcode Command Line Tools already provide
# (sips, iconutil) — no ImageMagick, no Rust image crate, matching this
# project's no-extra-dependency-for-a-one-time-conversion stance.
#
# Usage: bash resources/generate-icons-macos.sh

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_png="$root/development/logo.png"
iconset="$root/resources/AppIcon.iconset"
icns_out="$root/resources/AppIcon.icns"

if [[ ! -f "$source_png" ]]; then
    echo "source logo not found: $source_png" >&2
    exit 1
fi

rm -rf "$iconset"
mkdir -p "$iconset"

# .icns wants a fixed set of square PNGs at specific names/sizes. The source
# isn't square (1337x1176), so each size is produced in two steps: resample
# the longest side down to the target (aspect preserved, no stretching), then
# pad onto a transparent square canvas — confirmed empirically that `sips
# --padToHeightWidth` preserves the source's alpha channel (pads with fully
# transparent pixels, not a solid color) rather than needing --padColor.
render() {
    local size="$1" name="$2"
    local tmp
    tmp="$(mktemp -t iconresize).png"
    sips -s format png -Z "$size" "$source_png" --out "$tmp" >/dev/null
    sips -p "$size" "$size" "$tmp" --out "$iconset/$name" >/dev/null
    rm -f "$tmp"
}

for size in 16 32 128 256 512; do
    render "$size" "icon_${size}x${size}.png"
    render "$((size * 2))" "icon_${size}x${size}@2x.png"
done

iconutil -c icns "$iconset" -o "$icns_out"
rm -rf "$iconset"

echo "wrote $icns_out"
