#!/bin/zsh
# ---------------------------------------------------------------------------
# Tagwerk – veröffentlicht ein DMG aus target/dist als Release auf GitHub.
#
# Listet die DMGs mit einer .sha256-Datei daneben, lässt genau eines auswählen,
# prüft Prüfsumme, Signatur und Notarisierung, liest die Version aus der App im
# DMG, setzt das Tag v<Version> und legt das Release mit DMG und .sha256 sowie
# den Notizen aus CHANGELOG.md an. Das Release entsteht zuerst als Entwurf und
# wird erst veröffentlicht, wenn GitHub für jede Datei dieselbe SHA-256 meldet –
# das funktioniert auch mit unveränderlichen Releases (immutable releases).
#
# Ablauf:  ./build.sh  →  ./github-release.sh
#
# Voraussetzungen: GitHub CLI (gh), angemeldet mit Schreibrechten auf das Repo,
# und ein signiertes, notarisiertes DMG von ./build.sh.
#
# Gibt es das Tag noch nicht, wird HEAD getaggt: Der Arbeitsbaum muss sauber
# sein, HEAD muss bereits gepusht sein, und Cargo.toml sowie CHANGELOG.md müssen
# die Version des DMGs tragen, der Changelog-Eintrag mit Datum.
#
# Optionen:
#   --dry-run   alle Prüfungen ausführen und zeigen, was passieren würde
#   --yes       ohne Rückfrage veröffentlichen
#
# Umgebungsvariablen:
#   GIT_REMOTE   Remote für Tag und Release (Standard: origin)
#   GITHUB_REPO  owner/name des Repos (Standard: aus GIT_REMOTE gelesen)
# ---------------------------------------------------------------------------
set -euo pipefail
umask 022
cd "$(dirname "$0")"

DRY_RUN=0
ASSUME_YES=0
for arg in "$@"; do
    case "$arg" in
        --dry-run) DRY_RUN=1 ;;
        --yes)     ASSUME_YES=1 ;;
        -h|--help) sed -n '3,27p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "✗ Unbekannte Option: $arg" >&2; exit 2 ;;
    esac
done

APP_NAME="Tagwerk"
DIST="target/dist"
CHANGELOG="CHANGELOG.md"
MIN_MACOS="13"
GIT_REMOTE="${GIT_REMOTE:-origin}"
WORK=""
MOUNT=""

cleanup() {
    if [[ -n "$MOUNT" ]]; then
        hdiutil detach "$MOUNT" -quiet 2>/dev/null || hdiutil detach "$MOUNT" -force -quiet 2>/dev/null || true
        rmdir "$MOUNT" 2>/dev/null || true
    fi
    [[ -n "$WORK" ]] && rm -rf "$WORK"
    return 0
}
trap cleanup EXIT
trap 'exit 1' INT TERM HUP
WORK="$(mktemp -d)"

die() { echo "✗ $*" >&2; exit 1; }

# ── Vorprüfung ───────────────────────────────────────────────────────────────
[[ "$(uname)" == "Darwin" ]] || die "github-release.sh läuft nur auf macOS."
command -v gh >/dev/null 2>&1 || die "Die GitHub CLI ist nicht installiert: brew install gh"
gh auth status >/dev/null 2>&1 || die "Die GitHub CLI ist nicht angemeldet: gh auth login"
git remote get-url "$GIT_REMOTE" >/dev/null 2>&1 || die "Es gibt kein Git-Remote \"$GIT_REMOTE\"."

REPO="${GITHUB_REPO:-}"
if [[ -z "$REPO" ]]; then
    REPO="$(git remote get-url "$GIT_REMOTE" | sed -E 's#^(git@[^:]+:|ssh://[^/]+/|https?://[^/]+/)##; s#\.git$##')"
fi
[[ "$REPO" == */* ]] || die "owner/name lässt sich nicht aus dem Remote \"$GIT_REMOTE\" lesen; GITHUB_REPO setzen."

# ── DMG auswählen ────────────────────────────────────────────────────────────
DMGS=($DIST/*.dmg(N.om))
(( ${#DMGS} )) || die "Kein DMG in $DIST. Zuerst ./build.sh ausführen."

CANDIDATES=()
SKIPPED=()
for dmg in "${DMGS[@]}"; do
    if [[ -f "$dmg.sha256" ]]; then
        CANDIDATES+=("$dmg")
    else
        SKIPPED+=("$dmg")
    fi
done

echo "DMGs in $DIST:"
for (( i = 1; i <= ${#CANDIDATES}; i++ )); do
    dmg="${CANDIDATES[$i]}"
    printf '  %d) %-32s %4d MB  %s\n' "$i" "${dmg:t}" $(( $(stat -f %z "$dmg") / 1048576 )) \
        "$(stat -f '%Sm' -t '%Y-%m-%d %H:%M' "$dmg")"
done
for dmg in "${SKIPPED[@]}"; do
    printf '  -) %-32s ohne %s, übersprungen\n' "${dmg:t}" "${dmg:t}.sha256"
done
(( ${#CANDIDATES} )) || die "Kein DMG in $DIST hat eine .sha256-Datei daneben. Mit ./build.sh neu bauen."

if (( ${#CANDIDATES} == 1 )); then
    PROMPT="Welches DMG? [1, q zum Abbrechen, Enter für 1] "
else
    PROMPT="Welches DMG? [1-${#CANDIDATES}, q zum Abbrechen] "
fi
DMG=""
while [[ -z "$DMG" ]]; do
    printf '%s' "$PROMPT"
    read -r CHOICE || die "Kein DMG ausgewählt."
    [[ -z "$CHOICE" && ${#CANDIDATES} -eq 1 ]] && CHOICE=1
    case "$CHOICE" in
        q|Q) echo "Abgebrochen."; exit 0 ;;
        <->) (( CHOICE >= 1 && CHOICE <= ${#CANDIDATES} )) && DMG="${CANDIDATES[$CHOICE]}" ;;
    esac
    [[ -n "$DMG" ]] || echo "  Bitte eine Nummer aus der Liste eingeben."
done
DMG_NAME="${DMG:t}"
SHA_FILE="$DMG.sha256"

# ── DMG prüfen ───────────────────────────────────────────────────────────────
echo "▸ Prüfe $DMG_NAME …"
SHA_LINE="$(<"$SHA_FILE")"
SHA="${SHA_LINE%% *}"
[[ "$SHA" =~ ^[0-9a-f]{64}$ && "${SHA_LINE##* }" == "$DMG_NAME" ]] \
    || die "${SHA_FILE:t} enthält keine SHA-256 für $DMG_NAME."
( cd "$DIST" && shasum -a 256 -c "${SHA_FILE:t}" >/dev/null ) || die "$DMG_NAME passt nicht zu ${SHA_FILE:t}."

xcrun stapler validate "$DMG" >/dev/null 2>&1 \
    || die "An $DMG_NAME ist kein Notarisierungs-Ticket geheftet. Mit ./build.sh (ohne --no-sign/--no-notarize) bauen."
SPCTL="$(spctl -a -t open --context context:primary-signature -vv "$DMG" 2>&1 || true)"
[[ "$SPCTL" == *"source=Notarized Developer ID"* ]] || die "Gatekeeper lehnt $DMG_NAME ab: $SPCTL"
SIGNER="$(sed -n 's/^origin=//p' <<< "$SPCTL")"

MOUNT="$(mktemp -d)"
MOUNT="${MOUNT:A}"
if diskutil image -h >/dev/null 2>&1; then
    diskutil image attach --plist --nobrowse --readOnly --mountPoint "$MOUNT" "$DMG" > "$WORK/attach.plist"
else
    hdiutil attach -plist -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" 2>/dev/null > "$WORK/attach.plist"
fi
# Ein bereits eingebundenes DMG (z. B. im Finder geöffnet) wird nicht erneut
# eingebunden: Beide Werkzeuge melden dann das vorhandene Volume und ignorieren
# den Einhängepunkt.
VOLUME=""
for (( i = 0; ; i++ )); do
    plutil -extract "system-entities.$i" xml1 -o /dev/null "$WORK/attach.plist" 2>/dev/null || break
    VOLUME="$(plutil -extract "system-entities.$i.mount-point" raw -o - "$WORK/attach.plist" 2>/dev/null)" && break
done
if [[ "${VOLUME:A}" != "$MOUNT" ]]; then
    rmdir "$MOUNT"
    MOUNT=""
    [[ -n "$VOLUME" ]] || die "$DMG_NAME enthält kein einbindbares Volume."
    die "$DMG_NAME ist bereits unter $VOLUME eingebunden; bitte auswerfen und erneut starten."
fi
APP="$MOUNT/$APP_NAME.app"
[[ -d "$APP" ]] || die "$DMG_NAME enthält kein $APP_NAME.app."
codesign --verify --deep --strict "$APP" 2>/dev/null || die "$APP_NAME.app in $DMG_NAME ist nicht gültig signiert."
SPCTL="$(spctl -a -t exec -vv "$APP" 2>&1 || true)"
[[ "$SPCTL" == *"source=Notarized Developer ID"* ]] || die "Gatekeeper lehnt $APP_NAME.app ab: $SPCTL"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP/Contents/Info.plist")"
hdiutil detach "$MOUNT" -quiet
rmdir "$MOUNT"
MOUNT=""
[[ "$DMG_NAME" == "$APP_NAME-$VERSION.dmg" ]] || die "$DMG_NAME enthält Version $VERSION; erwartet war $APP_NAME-$VERSION.dmg."

TAG="v$VERSION"
PRERELEASE=0
[[ "$VERSION" == *-* ]] && PRERELEASE=1   # z. B. 1.1.0-beta.1

# ── Tag ──────────────────────────────────────────────────────────────────────
echo "▸ Prüfe Tag $TAG …"
git fetch -q "$GIT_REMOTE"
REMOTE_TAG_COMMIT="$(git ls-remote --tags "$GIT_REMOTE" "refs/tags/$TAG^{}" | cut -f1)"
[[ -n "$REMOTE_TAG_COMMIT" ]] || REMOTE_TAG_COMMIT="$(git ls-remote --tags "$GIT_REMOTE" "refs/tags/$TAG" | cut -f1)"
LOCAL_TAG_COMMIT="$(git rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null || true)"

CREATE_TAG=0
PUSH_TAG=0
if [[ -n "$REMOTE_TAG_COMMIT" ]]; then
    if [[ -z "$LOCAL_TAG_COMMIT" ]]; then
        git fetch -q "$GIT_REMOTE" "refs/tags/$TAG:refs/tags/$TAG"
        LOCAL_TAG_COMMIT="$(git rev-parse "refs/tags/$TAG^{commit}")"
    fi
    [[ "$LOCAL_TAG_COMMIT" == "$REMOTE_TAG_COMMIT" ]] \
        || die "Das lokale Tag $TAG und das auf $GIT_REMOTE zeigen auf verschiedene Commits."
    TAG_COMMIT="$REMOTE_TAG_COMMIT"
elif [[ -n "$LOCAL_TAG_COMMIT" ]]; then
    TAG_COMMIT="$LOCAL_TAG_COMMIT"
    PUSH_TAG=1
else
    [[ -z "$(git status --porcelain)" ]] || die "Der Arbeitsbaum hat nicht eingecheckte Änderungen; bitte zuerst committen."
    TAG_COMMIT="$(git rev-parse HEAD)"
    CREATE_TAG=1
    PUSH_TAG=1
fi

[[ -n "$(git branch -r --contains "$TAG_COMMIT" --list "$GIT_REMOTE/*")" ]] \
    || die "Commit ${TAG_COMMIT:0:8} liegt auf keinem Branch von $GIT_REMOTE; bitte zuerst pushen."
TAG_VERSION="$(git show "$TAG_COMMIT:Cargo.toml" | sed -n 's/^version *= *"\(.*\)"/\1/p' | head -n1)"
[[ "$TAG_VERSION" == "$VERSION" ]] \
    || die "Commit ${TAG_COMMIT:0:8} hat in Cargo.toml die Version ${TAG_VERSION:-?}, das DMG $VERSION."

git show "$TAG_COMMIT:$CHANGELOG" > "$WORK/changelog.md" 2>/dev/null \
    || die "Commit ${TAG_COMMIT:0:8} enthält kein $CHANGELOG."
grep -Eq "^## \[${VERSION//./\\.}\] - [0-9]{4}-[0-9]{2}-[0-9]{2}" "$WORK/changelog.md" \
    || die "$CHANGELOG in ${TAG_COMMIT:0:8} hat keinen datierten Eintrag \"## [$VERSION] - JJJJ-MM-TT\"."
NOTES_FILE="$WORK/notes.md"
# Der Abschnitt der Version, ohne die Link-Verweise am Ende der Datei.
awk -v hdr="## [$VERSION]" '
    index($0, hdr) == 1 { g = 1; next }
    g && (/^## \[/ || /^\[[^]]+\]: /) { exit }
    g { print }
' "$WORK/changelog.md" > "$NOTES_FILE"
[[ -n "$(tr -d '[:space:]' < "$NOTES_FILE")" ]] || die "Der Eintrag für $VERSION in $CHANGELOG ist leer."

cat >> "$NOTES_FILE" <<EOF

### Installation

Signiert mit einer Apple Developer ID und von Apple notarisiert. Benötigt macOS $MIN_MACOS (Ventura) oder neuer auf Apple Silicon. Das DMG unten öffnen und $APP_NAME in den Ordner „Programme“ ziehen. Die Datei \`$DMG_NAME.sha256\` enthält die SHA-256-Prüfsumme des DMGs.
EOF

# ── Release ──────────────────────────────────────────────────────────────────
EXISTING_DRAFT=0
if RELEASE_STATE="$(gh release view "$TAG" -R "$REPO" --json isDraft --jq '.isDraft' 2>/dev/null)"; then
    [[ "$RELEASE_STATE" == "true" ]] \
        || die "$REPO hat bereits ein veröffentlichtes Release $TAG: https://github.com/$REPO/releases/tag/${TAG//+/%2B}"
    EXISTING_DRAFT=1
fi

echo
echo "  Repository:  $REPO"
echo "  DMG:         $DMG_NAME (signiert von ${SIGNER:-unbekannt}, notarisiert)"
echo "  SHA-256:     $SHA"
echo "  Tag:         $TAG → ${TAG_COMMIT:0:8} $(git log -1 --format=%s "$TAG_COMMIT")$( (( CREATE_TAG )) && echo ' (neu)' || true)"
echo "  Release:     $( (( EXISTING_DRAFT )) && echo 'vorhandenen Entwurf aktualisieren' || echo 'neu'), $( (( PRERELEASE )) && echo 'Vorabversion' || echo 'als neuestes markiert')"
echo "  Notizen:"
sed 's/^/    │ /' "$NOTES_FILE"
echo

if (( DRY_RUN )); then
    echo "✓ Probelauf: Alle Prüfungen bestanden, nichts wurde verändert."
    exit 0
fi
if (( ! ASSUME_YES )); then
    printf '%s auf %s veröffentlichen? [j/N] ' "$TAG" "$REPO"
    read -r ANSWER || ANSWER=""
    [[ "$ANSWER" == [jJyY]* ]] || { echo "Abgebrochen."; exit 0; }
fi

if (( CREATE_TAG )); then
    echo "▸ Tagge ${TAG_COMMIT:0:8} als $TAG …"
    git tag -a "$TAG" -m "$APP_NAME $VERSION" "$TAG_COMMIT"
fi
if (( PUSH_TAG )); then
    echo "▸ Pushe $TAG nach $GIT_REMOTE …"
    git push -q "$GIT_REMOTE" "refs/tags/$TAG"
fi

TITLE="$APP_NAME $VERSION"
FILES=("$DMG" "$SHA_FILE")
if (( EXISTING_DRAFT )); then
    echo "▸ Lade in den vorhandenen Entwurf hoch …"
    gh release upload "$TAG" "${FILES[@]}" -R "$REPO" --clobber
    gh release edit "$TAG" -R "$REPO" --title "$TITLE" --notes-file "$NOTES_FILE" >/dev/null
else
    echo "▸ Lege den Release-Entwurf an und lade hoch …"
    CREATE_ARGS=("$TAG" "${FILES[@]}" -R "$REPO" --verify-tag --draft --title "$TITLE" --notes-file "$NOTES_FILE")
    (( PRERELEASE )) && CREATE_ARGS+=(--prerelease)
    gh release create "${CREATE_ARGS[@]}" >/dev/null
fi

echo "▸ Prüfe die hochgeladenen Dateien …"
RELEASE_ID="$(gh release view "$TAG" -R "$REPO" --json databaseId --jq '.databaseId')"
gh api "repos/$REPO/releases/$RELEASE_ID/assets" --jq '.[] | [.name, .digest] | @tsv' > "$WORK/assets.tsv"
(( $(wc -l < "$WORK/assets.tsv") == ${#FILES} )) \
    || die "Der Entwurf enthält andere Dateien als ${(j:, :)${FILES[@]:t}}; bitte vor dem Veröffentlichen auf GitHub prüfen."
for file in "${FILES[@]}"; do
    expected="sha256:$(shasum -a 256 "$file" | cut -d' ' -f1)"
    actual="$(awk -F '\t' -v name="${file:t}" '$1 == name { print $2 }' "$WORK/assets.tsv")"
    [[ "$actual" == "$expected" ]] \
        || die "GitHub meldet ${actual:-keine Prüfsumme} für ${file:t}, erwartet war $expected. Das Release bleibt ein Entwurf."
done

echo "▸ Veröffentliche …"
if (( PRERELEASE )); then
    gh release edit "$TAG" -R "$REPO" --draft=false --prerelease --latest=false >/dev/null
else
    gh release edit "$TAG" -R "$REPO" --draft=false --prerelease=false --latest >/dev/null
fi

DOWNLOAD="$(gh release view "$TAG" -R "$REPO" --json assets \
    --jq ".assets[] | select(.name == \"$DMG_NAME\") | .url")"
echo "✓ Veröffentlicht: $(gh release view "$TAG" -R "$REPO" --json url --jq '.url')"
echo "  Download:        $DOWNLOAD"
