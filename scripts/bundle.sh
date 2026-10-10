#!/bin/sh
# Build a macOS application bundle: target/bundle/Alef.app
# Launch it with `open`, drag it to /Applications, or add it to the Dock.
set -eu
cd "$(dirname "$0")/.."
cargo build --release
cargo build --release --example icon

APP="target/bundle/Alef.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/alef-term "$APP/Contents/MacOS/alef-term"

# Icon: render once at 1024 and let sips make the other sizes.
ICONSET="target/bundle/icon.iconset"
rm -rf "$ICONSET"; mkdir -p "$ICONSET"
./target/release/examples/icon "$ICONSET/icon_512x512@2x.png" 1024 >/dev/null
for s in 16 32 128 256 512; do
  sips -z $s $s "$ICONSET/icon_512x512@2x.png" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  [ $s -ne 512 ] && sips -z $d $d "$ICONSET/icon_512x512@2x.png" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/alef-term.icns"

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Alef</string>
  <key>CFBundleDisplayName</key><string>Alef</string>
  <key>CFBundleIdentifier</key><string>dev.almodhfer.alef-term</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleExecutable</key><string>alef-term</string>
  <key>CFBundleIconFile</key><string>alef-term</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
</dict>
</plist>
PLIST
# Seal the bundle with an ad-hoc signature. It is not an Apple developer
# identity, but it makes the bundle's signature cover its Info.plist and icon
# rather than only the executable the linker signed.
codesign --force --sign - "$APP" >/dev/null 2>&1 || echo "note: could not ad-hoc sign $APP"
echo "built $APP"
