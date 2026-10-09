#!/bin/sh
# Builds a release binary and wraps it into target/tapcode.app (no signing, no icon yet).
set -eu
cd "$(dirname "$0")/.."
cargo build --release
APP=target/tapcode.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/tapcode "$APP/Contents/MacOS/tapcode"
cp assets/icon/tapcode.icns "$APP/Contents/Resources/tapcode.icns"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>tapcode</string>
  <key>CFBundleDisplayName</key><string>tapcode</string>
  <key>CFBundleIdentifier</key><string>dev.tapcode</string>
  <key>CFBundleVersion</key><string>$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)</string>
  <key>CFBundleShortVersionString</key><string>$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)</string>
  <key>CFBundleExecutable</key><string>tapcode</string>
  <key>CFBundleIconFile</key><string>tapcode</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSEnvironment</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string></dict>
</dict></plist>
EOF
echo "built $APP ($(du -sh "$APP" | cut -f1)); open with: open $APP"
