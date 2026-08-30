#!/usr/bin/env bash
# Shared icon helper for make-app.sh and package.sh
# Usage: make_icns <png_path> <output_icns_path>
make_icns() {
  local png="$1"
  local out_icns="$2"
  if [ -f "$png" ] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
    local iconset
    iconset="$(mktemp -d)/kiri.iconset"
    mkdir -p "$iconset"
    for size in 16 32 64 128 256 512; do
      sips -z "$size" "$size" "$png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
      local double=$((size * 2))
      sips -z "$double" "$double" "$png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    done
    iconutil -c icns "$iconset" -o "$out_icns"
    rm -rf "$(dirname "$iconset")"
    return 0
  elif [ -f "${png%.png}.icns" ]; then
    cp "${png%.png}.icns" "$out_icns"
    return 0
  elif [ -f "assets/kiri.icns" ]; then
    cp "assets/kiri.icns" "$out_icns"
    return 0
  fi
  return 1
}
