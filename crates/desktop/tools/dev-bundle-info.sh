#!/usr/bin/env bash
set -euo pipefail

desktop_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
client_root="$(cd -- "$desktop_dir/../.." && pwd)"
cd "$client_root"

version="$(kache cargo -- metadata --locked --offline --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "listenbox-desktop") | .version')"
bundle_dir="$desktop_dir/dist/dev-bundle"
mkdir -p "$bundle_dir"
bundle_info="$(mktemp "$bundle_dir/Info.plist.XXXXXX")"
trap 'rm -f "$bundle_info"' EXIT

cat > "$bundle_info" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>Listenbox Dev</string>
<key>CFBundleDisplayName</key><string>Listenbox Dev</string>
<key>CFBundleIdentifier</key><string>app.listenbox.client.dev</string>
<key>CFBundleExecutable</key><string>listenbox-desktop</string>
<key>CFBundleIconFile</key><string>icon.icns</string>
<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$version</string>
<key>LSMinimumSystemVersion</key><string>13.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

plutil -lint "$bundle_info"
mv "$bundle_info" "$bundle_dir/Info.plist"
