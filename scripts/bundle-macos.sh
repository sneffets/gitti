#!/usr/bin/env bash
# Builds gitti in release mode and wraps it into target/release/gitti.app (icon, bundle id),
# so it can be started from Finder / Dock / Spotlight. Copy the .app to /Applications.
#   scripts/bundle-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."
# Icon files from assets/icon/*.svg (rewrites only what changed, so usually no rebuild).
cargo run --release -q -p gitti-app --example make_icons
cargo build --release -p gitti-app
VERSION=$(cargo pkgid -p gitti-app | sed 's/.*[#@]//')
APP=target/release/gitti.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/gitti "$APP/Contents/MacOS/gitti"
cp assets/icon/gitti.icns "$APP/Contents/Resources/gitti.icns"
# Licences shipped with the binary: our own, and every third-party component's
# (`cargo xtask licenses` keeps the list current; the app also shows it).
mkdir -p "$APP/Contents/Resources/licenses"
cp COPYING LICENSE-MIT LICENSE-APACHE licenses/THIRD-PARTY-LICENSES.txt licenses/LUCIDE.txt \
  "$APP/Contents/Resources/licenses/"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>gitti</string>
  <key>CFBundleDisplayName</key><string>gitti</string>
  <key>CFBundleIdentifier</key><string>io.github.sneffets.gitti</string>
  <key>CFBundleExecutable</key><string>gitti</string>
  <key>CFBundleIconFile</key><string>gitti</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST
# Ad-hoc signature, so Gatekeeper accepts the locally built bundle.
codesign --force --sign - "$APP" >/dev/null 2>&1
echo "$APP"
