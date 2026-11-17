#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Tagwerk – baut die App, packt sie in ein DMG, signiert und notarisiert es.
#
#   ./build.sh                 sauberer Build (cargo clean) für Apple Silicon (arm64),
#                              signieren, notarisieren
#   ./build.sh --no-notarize   nur bauen und signieren
#   ./build.sh --no-sign       nur bauen (ad-hoc signiert, ohne Notarisierung)
#   ./build.sh --install       zusätzlich nach /Applications kopieren (eine laufende
#                              Tagwerk-App wird beendet und danach neu gestartet);
#                              lässt sich mit den anderen Optionen kombinieren
#
# Einstellungen über Umgebungsvariablen:
#   SIGN_IDENTITY   "Developer ID Application: …" (Standard: erste gefundene Developer-ID)
#   NOTARY_PROFILE  Schlüsselbund-Profil für notarytool (Standard: general-notary)
#   BUNDLE_ID       Bundle-Kennung (Standard: com.tagwerk.app)
#   INSTALL_DIR     Ziel für --install (Standard: /Applications)
#
# Das Notary-Profil wird einmalig angelegt mit:
#   xcrun notarytool store-credentials general-notary --apple-id <id> --team-id <team>
# ---------------------------------------------------------------------------
set -euo pipefail

APP_NAME="Tagwerk"
BUNDLE_ID="${BUNDLE_ID:-com.tagwerk.app}"
NOTARY_PROFILE="${NOTARY_PROFILE:-general-notary}"
SIGN_IDENTITY="${SIGN_IDENTITY:-}"
MIN_MACOS="13.0"

DO_SIGN=1
DO_NOTARIZE=1
DO_INSTALL=0
INSTALL_DIR="${INSTALL_DIR:-/Applications}"
TARGET="aarch64-apple-darwin"

for arg in "$@"; do
  case "$arg" in
    --no-notarize) DO_NOTARIZE=0 ;;
    --no-sign)     DO_SIGN=0; DO_NOTARIZE=0 ;;
    --install)     DO_INSTALL=1 ;;
    -h|--help)     sed -n '2,21p' "$0"; exit 0 ;;
    *) echo "Unbekannte Option: $arg" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")"
VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
DIST="target/dist"
WORK="$DIST/work"
APP="$WORK/$APP_NAME.app"
DMG="$DIST/$APP_NAME-$VERSION.dmg"

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31mFehler:\033[0m %s\n' "$*" >&2; exit 1; }

[[ "$(uname)" == "Darwin" ]] || fail "build.sh läuft nur auf macOS."
command -v cargo >/dev/null || fail "cargo (Rust) wurde nicht gefunden – siehe https://rustup.rs"

export MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS"

# 1) Kompilieren ---------------------------------------------------------------
BIN="$WORK/$APP_NAME"
step "Räume alte Build-Ergebnisse auf (cargo clean)"
cargo clean
# erst nach cargo clean anlegen – das löscht target/ und damit auch $DIST
rm -rf "$WORK"
mkdir -p "$WORK"
if command -v rustup >/dev/null; then
  rustup target add "$TARGET" >/dev/null 2>&1
fi
step "Kompiliere für Apple Silicon (arm64)"
cargo build --release --target "$TARGET"
cp "target/$TARGET/release/$APP_NAME" "$BIN"
lipo -archs "$BIN" | grep -qx "arm64" || fail "Das Programm ist nicht für arm64 gebaut."

# 2) App-Symbol ----------------------------------------------------------------
step "Erzeuge App-Symbol"
ICONSET="$WORK/AppIcon.iconset"
ICON_SRC="assets/AppIcon.png"   # 1024 × 1024, Quelle: assets/AppIcon.svg
[ -f "$ICON_SRC" ] || fail "$ICON_SRC fehlt."
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$ICON_SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$WORK/AppIcon.icns"

# 3) App-Bundle ----------------------------------------------------------------
step "Baue $APP_NAME.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$APP_NAME"
cp "$WORK/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
printf 'APPL????' > "$APP/Contents/PkgInfo"
# Plattform-Angaben, damit macOS (z. B. Systeminformation) die App als Mac-App erkennt
SDK_VERSION="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null || sw_vers -productVersion)"
SDK_BUILD="$(xcrun --sdk macosx --show-sdk-build-version 2>/dev/null || sw_vers -buildVersion)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>                 <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>          <string>$APP_NAME</string>
  <key>CFBundleExecutable</key>           <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>           <string>$BUNDLE_ID</string>
  <key>CFBundleIconFile</key>             <string>AppIcon</string>
  <key>CFBundlePackageType</key>          <string>APPL</string>
  <key>CFBundleShortVersionString</key>   <string>$VERSION</string>
  <key>CFBundleVersion</key>              <string>$VERSION</string>
  <key>CFBundleDevelopmentRegion</key>    <string>de</string>
  <key>CFBundleLocalizations</key>        <array><string>de</string></array>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleSupportedPlatforms</key>   <array><string>MacOSX</string></array>
  <key>DTPlatformName</key>               <string>macosx</string>
  <key>DTPlatformVersion</key>            <string>$SDK_VERSION</string>
  <key>DTPlatformBuild</key>              <string>$SDK_BUILD</string>
  <key>DTSDKName</key>                    <string>macosx$SDK_VERSION</string>
  <key>DTSDKBuild</key>                   <string>$SDK_BUILD</string>
  <key>LSMinimumSystemVersion</key>       <string>$MIN_MACOS</string>
  <key>LSApplicationCategoryType</key>    <string>public.app-category.productivity</string>
  <key>LSUIElement</key>                  <true/>
  <key>NSHighResolutionCapable</key>      <true/>
  <key>NSHumanReadableCopyright</key>     <string>Tagwerk</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# 4) Signieren -----------------------------------------------------------------
if [[ "$DO_SIGN" == 1 ]]; then
  if [[ -z "$SIGN_IDENTITY" ]]; then
    SIGN_IDENTITY="$(security find-identity -v -p codesigning \
      | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' | head -n1)"
  fi
  [[ -n "$SIGN_IDENTITY" ]] || fail "Kein Zertifikat \"Developer ID Application\" gefunden. SIGN_IDENTITY setzen oder --no-sign verwenden."
  step "Signiere App mit: $SIGN_IDENTITY"
  codesign --force --options runtime --timestamp --sign "$SIGN_IDENTITY" "$APP"
  codesign --verify --strict --verbose=2 "$APP"
else
  step "Signiere ad-hoc (nur für diesen Mac)"
  codesign --force --sign - "$APP"
fi

# 5) DMG -----------------------------------------------------------------------
step "Erzeuge $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Programme"
rm -f "$DMG"
if diskutil image create from -h >/dev/null 2>&1; then
  if ! out="$(diskutil image create from --format UDZO --volumeName "$APP_NAME" "$STAGE" "$DMG" 2>&1)"; then
    echo "$out" >&2
    fail "DMG konnte nicht erstellt werden."
  fi
else
  # ältere macOS-Versionen ohne "diskutil image"
  hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
fi
if [[ "$DO_SIGN" == 1 ]]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
fi

# 6) Notarisieren --------------------------------------------------------------
if [[ "$DO_NOTARIZE" == 1 ]]; then
  step "Notarisiere bei Apple (Profil: $NOTARY_PROFILE) – das kann einige Minuten dauern"
  LOG="$WORK/notary.log"
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait | tee "$LOG"
  if ! grep -q "status: Accepted" "$LOG"; then
    SUB_ID="$(sed -n 's/^ *id: \(.*\)$/\1/p' "$LOG" | head -n1)"
    fail "Notarisierung nicht akzeptiert. Details: xcrun notarytool log ${SUB_ID:-<id>} --keychain-profile $NOTARY_PROFILE"
  fi
  step "Hefte Notarisierungs-Ticket an"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature --verbose "$DMG" || true
fi

# Prüfsumme erst nach dem Anheften des Tickets – das verändert das DMG.
step "Erzeuge Prüfsumme $(basename "$DMG").sha256"
( cd "$DIST" && shasum -a 256 "$(basename "$DMG")" > "$(basename "$DMG").sha256" )

# 7) Installieren --------------------------------------------------------------
if [[ "$DO_INSTALL" == 1 ]]; then
  TARGET_APP="$INSTALL_DIR/$APP_NAME.app"
  TARGET_BIN="$TARGET_APP/Contents/MacOS/$APP_NAME"
  [[ -w "$INSTALL_DIR" ]] || fail "Keine Schreibrechte für $INSTALL_DIR."
  if [[ "$DO_NOTARIZE" == 1 ]]; then
    # Das Ticket der Notarisierung gilt auch für die App im DMG.
    xcrun stapler staple "$APP" >/dev/null || echo "Hinweis: Ticket konnte nicht an die App geheftet werden." >&2
  fi

  WAS_RUNNING=0
  if pgrep -xf "$TARGET_BIN" >/dev/null; then
    WAS_RUNNING=1
    step "Beende laufendes $APP_NAME"
    # Regulär beenden, damit die laufende Erfassung gespeichert wird.
    osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
    for _ in $(seq 1 20); do
      pgrep -f "$TARGET_BIN" >/dev/null || break
      sleep 0.5
    done
    if pgrep -f "$TARGET_BIN" >/dev/null; then
      pkill -TERM -f "$TARGET_BIN" || true
      sleep 1
    fi
    pgrep -f "$TARGET_BIN" >/dev/null && fail "$APP_NAME läuft noch – bitte von Hand beenden."
  fi

  step "Installiere nach $TARGET_APP"
  rm -rf "$TARGET_APP"
  ditto "$APP" "$TARGET_APP"
  codesign --verify --strict "$TARGET_APP" || fail "Die Signatur der installierten App ist ungültig."

  if [[ "$WAS_RUNNING" == 1 ]]; then
    step "Starte $APP_NAME neu"
    open "$TARGET_APP"
  fi
fi

step "Fertig: $DMG"
if [[ "$DO_INSTALL" == 1 ]]; then
  echo "Installiert: $INSTALL_DIR/$APP_NAME.app"
fi
