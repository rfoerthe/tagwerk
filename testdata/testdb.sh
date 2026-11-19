#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Tagwerk – Testdatenbank bauen und bei Bedarf als zentrale Datenbank einsetzen.
#
#   testdata/testdb.sh             Testdaten erzeugen und target/testdata/tagwerk.db
#                                  neu bauen
#   testdata/testdb.sh --install   zusätzlich die zentrale Datenbank durch die
#                                  Testdatenbank ersetzen (vorher wird sie gesichert)
#   testdata/testdb.sh --restore   die zuletzt gesicherte zentrale Datenbank zurückholen
#
# Die Testdaten reichen immer bis gestern (18 volle Monate plus der laufende Monat, siehe
# generate_testdata.py) und werden bei jedem Aufruf neu erzeugt – samt Markdown-Quelle
# target/testdata/testdaten.md. Nichts davon liegt im Repository. Sie enthalten Arbeit
# ohne Mac und Regelverstöße, darunter rund 10 % Tage mit mehr als 6 Std. ohne Pause.
#
# Laufende Tagwerk-Instanzen, die die zentrale Datenbank benutzen, werden vor dem
# Austausch beendet und danach wieder gestartet (sofern sie als App-Bundle liefen).
#
# Zentrale Datenbank: $TAGWERK_DIR/tagwerk.db, sonst
#   ~/Library/Application Support/Tagwerk/tagwerk.db
# Sicherungen:        <Datenordner>/vor-testdaten/tagwerk-JJJJ-MM-TT_HHMMSS.db
#
# Die Testdatenbank trägt eine Kennung (SQLite application_id). Enthält die zentrale
# Datenbank schon Testdaten, wird sie nicht noch einmal gesichert, und --restore
# übergeht Sicherungen mit Testdaten – so kommen immer die echten Daten zurück.
# ---------------------------------------------------------------------------
set -euo pipefail

MODE="build"
for arg in "$@"; do
  case "$arg" in
    --install) MODE="install" ;;
    --restore) MODE="restore" ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
    *) echo "Unbekannte Option: $arg" >&2; exit 2 ;;
  esac
done

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
TEST_DIR="$ROOT/target/testdata"
TEST_DB="$TEST_DIR/tagwerk.db"
TEST_MD="$TEST_DIR/testdaten.md"
BIN="$ROOT/target/debug/Tagwerk"
DATA_DIR="${TAGWERK_DIR:-$HOME/Library/Application Support/Tagwerk}"
CENTRAL_DB="$DATA_DIR/tagwerk.db"
SAFE_DIR="$DATA_DIR/vor-testdaten"
# Kennung der Testdatenbank ("TWKT"); Tagwerk selbst ändert die application_id nie.
TEST_MARK=1414990676
# Datenbankschema, für das generate_testdata.py schreibt (src/store.rs, SCHEMA_VERSION)
SCHEMA_VERSION=3

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31mFehler:\033[0m %s\n' "$*" >&2; exit 1; }

command -v sqlite3 >/dev/null || fail "sqlite3 wurde nicht gefunden."

# Macht eine Datenbankdatei eigenständig (ohne -wal/-shm) und prüft sie.
# Tagwerk schaltet beim Öffnen selbst wieder auf WAL um.
seal() {
  local db="$1"
  sqlite3 "$db" "PRAGMA journal_mode=DELETE;" >/dev/null
  rm -f "$db-wal" "$db-shm"
  [[ "$(sqlite3 "$db" 'PRAGMA integrity_check')" == "ok" ]] || return 1
  rm -f "$db-wal" "$db-shm"
}

# Enthält die Datei Testdaten?
is_test_db() {
  [[ "$(sqlite3 "$1" 'PRAGMA application_id' 2>/dev/null)" == "$TEST_MARK" ]]
}

# ---------- Testdatenbank bauen ----------------------------------------------

build_test_db() {
  command -v python3 >/dev/null || fail "python3 wurde nicht gefunden."
  command -v cargo >/dev/null || fail "cargo (Rust) wurde nicht gefunden."
  step "Baue Tagwerk (Debug)"
  (cd "$ROOT" && cargo build --quiet)
  mkdir -p "$TEST_DIR"
  step "Erzeuge Testdaten bis gestern"
  python3 "$HERE/generate_testdata.py" > "$TEST_MD"
  step "Baue $TEST_DB"
  rm -f "$TEST_DB" "$TEST_DB-wal" "$TEST_DB-shm"
  # eigener, leerer Datenordner – nichts aus der zentralen Datenbank landet darin
  local tmp
  tmp="$(mktemp -d)"
  TAGWERK_DIR="$tmp" "$BIN" --import "$TEST_MD"
  local db="$tmp/tagwerk.db" version blocks
  version="$(sqlite3 "$db" 'PRAGMA user_version')"
  [[ "$version" == "$SCHEMA_VERSION" ]] \
    || fail "Tagwerk hat Schema $version angelegt, die Testdaten sind für Schema $SCHEMA_VERSION – generate_testdata.py anpassen."
  blocks="$(sqlite3 "$db" 'SELECT count(*) FROM work_blocks')"
  [[ "$blocks" -gt 0 ]] || fail "Die Testdatenbank enthält keine Arbeit ohne Mac."
  sqlite3 "$db" "PRAGMA application_id = $TEST_MARK;"
  seal "$db" || fail "Die Testdatenbank ist beschädigt."
  cp "$db" "$TEST_DB"
  rm -rf "$tmp"
  echo "$(sqlite3 "$TEST_DB" 'SELECT count(DISTINCT date) FROM periods') Tage," \
    "$(sqlite3 "$TEST_DB" 'SELECT count(DISTINCT date) FROM work_blocks') davon mit Arbeit ohne Mac" \
    "($(sqlite3 "$TEST_DB" 'SELECT min(date) || " bis " || max(date) FROM periods'))"
}

# ---------- Laufende Instanzen ----------------------------------------------

# Prozesse, die die zentrale Datenbank geöffnet haben (Menüleisten-App, Fenster).
users_of_db() {
  local files=()
  for f in "$DATA_DIR/.lock" "$CENTRAL_DB" "$CENTRAL_DB-wal"; do
    [[ -e "$f" ]] && files+=("$f")
  done
  [[ ${#files[@]} -gt 0 ]] || return 0
  lsof -t "${files[@]}" 2>/dev/null | sort -u || true
}

RESTART_APPS=()

stop_instances() {
  local pids
  pids="$(users_of_db)"
  [[ -n "$pids" ]] || return 0
  step "Beende laufende Tagwerk-Instanzen"
  local pid exe app
  for pid in $pids; do
    exe="$(ps -o comm= -p "$pid" 2>/dev/null || true)"
    if [[ "$exe" == *.app/Contents/MacOS/* ]]; then
      app="${exe%%/Contents/MacOS/*}"
      # regulär beenden, damit die laufende Erfassung noch gespeichert wird
      osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || kill -TERM "$pid" 2>/dev/null || true
      [[ " ${RESTART_APPS[*]:-} " == *" $app "* ]] || RESTART_APPS+=("$app")
    else
      kill -TERM "$pid" 2>/dev/null || true
    fi
  done
  for _ in $(seq 1 20); do
    [[ -z "$(users_of_db)" ]] && return 0
    sleep 0.5
  done
  fail "Tagwerk benutzt die Datenbank noch (PID: $(users_of_db | tr '\n' ' ')) – bitte von Hand beenden."
}

restart_instances() {
  local app
  for app in "${RESTART_APPS[@]:-}"; do
    [[ -n "$app" ]] || continue
    step "Starte $app neu"
    open "$app"
  done
}

# ---------- Austausch ---------------------------------------------------------

replace_central_db() {
  local source="$1"
  rm -f "$CENTRAL_DB-wal" "$CENTRAL_DB-shm"
  cp "$source" "$CENTRAL_DB"
}

install_test_db() {
  [[ -f "$TEST_DB" ]] || fail "$TEST_DB fehlt."
  mkdir -p "$DATA_DIR"
  stop_instances
  if [[ -f "$CENTRAL_DB" ]] && is_test_db "$CENTRAL_DB"; then
    step "Die zentrale Datenbank enthält schon Testdaten – keine neue Sicherung"
  elif [[ -f "$CENTRAL_DB" ]]; then
    mkdir -p "$SAFE_DIR"
    local backup
    backup="$SAFE_DIR/tagwerk-$(date +%Y-%m-%d_%H%M%S).db"
    step "Sichere die zentrale Datenbank nach $backup"
    sqlite3 "$CENTRAL_DB" ".backup '$backup'"
    seal "$backup" || fail "Die Sicherung ist unvollständig – die zentrale Datenbank bleibt unverändert."
  fi
  step "Ersetze $CENTRAL_DB durch die Testdatenbank"
  replace_central_db "$TEST_DB"
  restart_instances
  echo
  echo "Die Testdatenbank ist aktiv. Zurück zu den echten Daten: testdata/testdb.sh --restore"
}

restore_central_db() {
  # neueste Sicherung, die keine Testdaten enthält
  local latest="" f
  while IFS= read -r f; do
    if [[ -n "$f" ]] && ! is_test_db "$f"; then
      latest="$f"
      break
    fi
  done < <(ls -1 "$SAFE_DIR"/tagwerk-*.db 2>/dev/null | sort -r)
  [[ -n "$latest" ]] || fail "Keine Sicherung mit echten Daten in $SAFE_DIR gefunden."
  seal "$latest" || fail "$latest ist beschädigt."
  stop_instances
  step "Stelle $CENTRAL_DB aus $(basename "$latest") wieder her"
  replace_central_db "$latest"
  restart_instances
  echo
  echo "Die echten Daten sind wieder aktiv. Die Sicherung bleibt in $SAFE_DIR liegen."
}

case "$MODE" in
  build)   build_test_db ;;
  install) build_test_db; install_test_db ;;
  restore) restore_central_db ;;
esac

step "Fertig"
