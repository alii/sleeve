#!/bin/sh
# Builds Sleeve.app for Apple Silicon and Intel, signs it with a Developer ID, packs it in a
# DMG and, when a notary profile is set up, has Apple notarize it.
#
#   scripts/release-macos.sh
#
# Settings, all optional:
#   SIGN_IDENTITY   the signing certificate (default: the first "Developer ID Application")
#   NOTARY_PROFILE  a notarytool keychain profile (default: sleeve-notary). Make one once with
#                   xcrun notarytool store-credentials sleeve-notary --apple-id … --team-id …
#   SKIP_NOTARIZE=1 sign only, for a quick local build

set -eu
cd "$(dirname "$0")/.."

NAME=Sleeve
BUNDLE_ID=com.alii.sleeve
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
OUT=target/release-macos
APP="$OUT/$NAME.app"
DMG="$OUT/$NAME-$VERSION.dmg"
NOTARY_PROFILE=${NOTARY_PROFILE:-sleeve-notary}
SIGN_IDENTITY=${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application:[^"]*\)".*/\1/p' | head -1)}

if [ -z "$SIGN_IDENTITY" ]; then
    echo "No Developer ID Application certificate in the keychain." >&2
    exit 1
fi

echo "Building $NAME $VERSION for Apple Silicon and Intel…"
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

rm -rf "$OUT"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/sleeve" \
    target/aarch64-apple-darwin/release/sleeve \
    target/x86_64-apple-darwin/release/sleeve

echo "Compiling the icon…"
xcrun actool assets/AppIcon.icon --compile "$APP/Contents/Resources" \
    --platform macosx --minimum-deployment-target 13.0 --app-icon AppIcon \
    --output-partial-info-plist "$OUT/icon.plist" >/dev/null

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>$NAME</string>
    <key>CFBundleDisplayName</key><string>$NAME</string>
    <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
    <key>CFBundleExecutable</key><string>sleeve</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundleIconName</key><string>AppIcon</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.music</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSHumanReadableCopyright</key><string>GPL-2.0-or-later</string>
</dict>
</plist>
PLIST

echo "Signing with ${SIGN_IDENTITY}…"
codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"

notarize() {
    xcrun notarytool submit "$1" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$1"
}

can_notarize=0
if [ "${SKIP_NOTARIZE:-0}" != 1 ]; then
    if xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
        can_notarize=1
    else
        echo "No notary profile \"$NOTARY_PROFILE\", so this build won't be notarized." >&2
    fi
fi

# The app is notarized and stapled before it goes in the DMG, so it opens cleanly offline
# even after it's copied out.
if [ $can_notarize = 1 ]; then
    echo "Notarizing the app…"
    ditto -c -k --keepParent "$APP" "$OUT/$NAME.zip"
    notarize "$OUT/$NAME.zip"
    xcrun stapler staple "$APP"
    rm "$OUT/$NAME.zip"
fi

echo "Making the DMG…"
STAGE="$OUT/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
if [ $can_notarize = 1 ]; then
    echo "Notarizing the DMG…"
    notarize "$DMG"
    spctl --assess --type open --context context:primary-signature --verbose "$DMG"
fi

echo "Done: $DMG"
