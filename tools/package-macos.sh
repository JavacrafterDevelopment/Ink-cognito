#!/usr/bin/env bash
# Assembles the macOS release package from a built ink-cognito binary.
# Run by the release workflow. Adapted from KittyConnect's, so the Javacrafter
# Launcher installs and updates it the same way.
#
#   tools/package-macos.sh <path/to/ink-cognito> <OutDir> [path/to/ink-map-editor]
#
# The assets go in Contents/Resources/assets (the game and the editor look
# there when they run from a bundle): codesign only wants code in
# Contents/MacOS.
#
# Writes:
#   <Out>/Ink-cognito/Ink-cognito.app         the application bundle
#   <Out>/Ink-cognito/launcher-manifest.json  every file with its size and SHA-256
#   <Out>/Ink-cognito-macos.zip               what the Launcher downloads
#   <Out>/Ink-cognito-macos.dmg               for installing by hand (when hdiutil is there)
#   <Out>/release-entry-macos.json            this release's entry for update-macos.json

set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "Usage: $0 <path/to/ink-cognito> <OutDir> [path/to/ink-map-editor]" >&2
  exit 1
fi

EXE="$1"
OUT="$2"
EDITOR="${3:-}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
FOLDER="$OUT/Ink-cognito"
APP="$FOLDER/Ink-cognito.app"
ZIP_PATH="$OUT/Ink-cognito-macos.zip"
DMG_PATH="$OUT/Ink-cognito-macos.dmg"
MANIFEST="launcher-manifest.json"
EXE_REL="Ink-cognito.app/Contents/MacOS/ink-cognito"

rm -rf "$FOLDER" "$ZIP_PATH" "$DMG_PATH"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$EXE" "$APP/Contents/MacOS/ink-cognito"
chmod 755 "$APP/Contents/MacOS/ink-cognito"
if [ -n "$EDITOR" ]; then
  cp "$EDITOR" "$APP/Contents/MacOS/ink-map-editor"
  chmod 755 "$APP/Contents/MacOS/ink-map-editor"
fi

cp -R "$ROOT/assets" "$APP/Contents/Resources/assets"
find "$APP/Contents/Resources/assets" -name '.DS_Store' -delete

# The local network text is what macOS shows when it asks; players on the
# same network connect to each other directly.
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>Ink-cognito</string>
  <key>CFBundleExecutable</key>
  <string>ink-cognito</string>
  <key>CFBundleIdentifier</key>
  <string>com.javacrafter.inkcognito</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Ink-cognito</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${VERSION}</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.games</string>
  <key>LSRequiresNativeExecution</key>
  <true/>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSLocalNetworkUsageDescription</key>
  <string>Ink-cognito connects directly to the players you join when they're on your network.</string>
</dict>
</plist>
PLIST

# Ad-hoc sign BEFORE hashing, so _CodeSignature and the signed binaries match
# the hashes in launcher-manifest.json. The helper binary first, then the app
# (which seals everything in it). Apple Silicon won't run unsigned code at all;
# ad-hoc is enough for that, but it isn't notarized, so a copy downloaded with
# a browser gets Gatekeeper's warning (right-click > Open the first time).
if command -v codesign >/dev/null 2>&1; then
  if [ -n "$EDITOR" ]; then
    codesign --force --sign - "$APP/Contents/MacOS/ink-map-editor"
  fi
  codesign --force --sign - "$APP"
  codesign --verify --strict --verbose=2 "$APP"
fi

python3 - "$FOLDER" "$VERSION" "$EXE_REL" "$MANIFEST" "$ZIP_PATH" "$OUT/release-entry-macos.json" <<'PY'
import hashlib
import json
import os
import stat
import sys
import zipfile

folder, version, exe_rel, manifest_name, zip_path, entry_path = sys.argv[1:7]

files = []
for root, dirs, filenames in os.walk(folder):
    dirs[:] = [d for d in dirs if d.lower() not in ("data", "ink-cognitodata")]
    for name in filenames:
        if name == manifest_name or name == ".DS_Store":
            continue
        full = os.path.join(root, name)
        rel = os.path.relpath(full, folder).replace("\\", "/")
        h = hashlib.sha256()
        with open(full, "rb") as f:
            while chunk := f.read(65536):
                h.update(chunk)
        files.append({"path": rel, "size": os.path.getsize(full), "sha256": h.hexdigest().lower()})
files.sort(key=lambda item: item["path"])

with open(os.path.join(folder, manifest_name), "w", encoding="utf-8") as f:
    json.dump({"version": version, "exe": exe_rel, "files": files}, f, indent=2)

with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as zf:
    for rel in [item["path"] for item in files] + [manifest_name]:
        src = os.path.join(folder, rel)
        st = os.stat(src)
        zi = zipfile.ZipInfo.from_file(src, arcname=rel)
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.create_system = 3  # Unix, so the modes below count
        mode = stat.S_IMODE(st.st_mode)
        if rel == exe_rel or ".app/Contents/MacOS/" in rel:
            mode |= 0o755
        zi.external_attr = (stat.S_IFREG | mode) << 16
        with open(src, "rb") as f:
            zf.writestr(zi, f.read())

zip_hash = hashlib.sha256()
with open(zip_path, "rb") as f:
    while chunk := f.read(65536):
        zip_hash.update(chunk)

with open(entry_path, "w", encoding="utf-8") as f:
    json.dump({
        "version": version,
        "exe": exe_rel,
        "package": {"name": os.path.basename(zip_path), "size": os.path.getsize(zip_path), "sha256": zip_hash.hexdigest().lower()},
        "files": files,
    }, f, indent=2)
PY

# A disk image with an Applications shortcut, for installing without the Launcher.
if command -v hdiutil >/dev/null 2>&1; then
  STAGE="$OUT/dmg-stage"
  rm -rf "$STAGE"
  mkdir -p "$STAGE"
  cp -R "$APP" "$STAGE/Ink-cognito.app"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -volname "Ink-cognito" -fs HFS+ -srcfolder "$STAGE" -ov -format UDZO "$DMG_PATH" >/dev/null || true
  rm -rf "$STAGE"
fi

echo "Packaged Ink-cognito $VERSION for macOS -> $ZIP_PATH"
