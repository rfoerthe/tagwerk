//! Optionale Verschlüsselung der Datenbank mit SQLCipher.
//!
//! - **Schlüssel:** 32 Zufallsbytes, als Eintrag im Schlüsselbund von macOS (Dienst
//!   [`SERVICE`]). So startet Tagwerk bei der Anmeldung ohne Passwortabfrage; andere Programme
//!   bekommen den Schlüssel nur mit Rückfrage. Als Wiederherstellungsschlüssel zeigt Tagwerk
//!   ihn in acht Gruppen zu acht Hex-Zeichen ([`Key::recovery_text`]).
//! - **Erkennen:** Eine unverschlüsselte SQLite-Datei beginnt mit `SQLite format 3\0`, eine
//!   verschlüsselte mit Zufallsbytes ([`is_encrypted`]) – eine eigene Einstellung gibt es nicht.
//! - **Umstellen:** SQLCipher kann eine Datei nicht an Ort und Stelle ver- oder entschlüsseln.
//!   [`convert_file`] schreibt deshalb mit `sqlcipher_export` eine neue Datei, prüft sie und
//!   ersetzt damit die alte. Das geht nur, solange niemand sonst die Datenbank geöffnet hat:
//!   Die Einstellungen merken sich den Wunsch ([`request`]), Tagwerk startet neu und stellt vor
//!   dem ersten Öffnen um ([`apply_pending`]). Die Backups werden mit umgestellt.
//!
//! Die täglichen Backups (`VACUUM INTO`) einer verschlüsselten Datenbank sind von selbst mit
//! demselben Schlüssel verschlüsselt.

use rusqlite::{params, Connection, ErrorCode};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Dienst des Eintrags im Schlüsselbund (wie die Bundle-Kennung).
pub const SERVICE: &str = "de.foerther.tagwerk";
const DB_FILE: &str = "tagwerk.db";
const BACKUP_DIR: &str = "backups";
/// Merkt sich bis zum Neustart, ob verschlüsselt (`ein`) oder entschlüsselt (`aus`) werden soll.
const PENDING_FILE: &str = ".verschluesselung";
/// Zwischendatei beim Umstellen (neben der Datei, die sie ersetzt).
const CONVERT_SUFFIX: &str = ".umstellung";
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const KEY_LEN: usize = 32;

/// Schlüssel der Datenbank (wird beim Freigeben überschrieben, nie ausgegeben).
#[derive(Clone, PartialEq, Eq)]
pub struct Key([u8; KEY_LEN]);

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(…)")
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            // Nicht wegoptimieren lassen.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

impl Key {
    /// Neuer Zufallsschlüssel.
    pub fn generate() -> Result<Key> {
        let mut bytes = [0u8; KEY_LEN];
        #[cfg(target_os = "macos")]
        security_framework::random::SecRandom::default().copy_bytes(&mut bytes)?;
        #[cfg(not(target_os = "macos"))]
        fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        Ok(Key(bytes))
    }

    fn from_bytes(bytes: &[u8]) -> Option<Key> {
        <[u8; KEY_LEN]>::try_from(bytes).ok().map(Key)
    }

    fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Wiederherstellungsschlüssel: acht Gruppen zu acht Hex-Zeichen, z. B. `1A2B3C4D-…`.
    pub fn recovery_text(&self) -> String {
        let hex = self.hex();
        hex.as_bytes().chunks(8).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
    }

    /// Liest einen Wiederherstellungsschlüssel. Groß- und Kleinschreibung, Leerzeichen,
    /// Bindestriche und Zeilenumbrüche spielen keine Rolle.
    pub fn parse_recovery(text: &str) -> Option<Key> {
        let hex: Vec<u8> =
            text.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'-').collect();
        if hex.len() != KEY_LEN * 2 {
            return None;
        }
        let mut bytes = [0u8; KEY_LEN];
        for (i, pair) in hex.chunks(2).enumerate() {
            let s = std::str::from_utf8(pair).ok()?;
            bytes[i] = u8::from_str_radix(s, 16).ok()?;
        }
        Some(Key(bytes))
    }

    /// Schlüssel für SQLCipher: `x'…'` heißt „Schlüssel direkt verwenden“ (ohne Ableitung
    /// aus einem Passwort).
    fn sql(&self) -> String {
        format!("x'{}'", self.hex())
    }
}

/// Warum sich eine verschlüsselte Datenbank nicht öffnen lässt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Kein Eintrag im Schlüsselbund.
    Missing,
    /// Der Schlüssel passt nicht zur Datenbank.
    Wrong,
    /// Der Schlüsselbund hat den Zugriff verweigert oder einen Fehler gemeldet.
    Keychain(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Missing => f.write_str(
                "Die Datenbank ist verschlüsselt, aber ihr Schlüssel fehlt im Schlüsselbund. \
                 Tagwerk (Menüleiste) starten und den Wiederherstellungsschlüssel eingeben.",
            ),
            KeyError::Wrong => f.write_str("Der Schlüssel passt nicht zur verschlüsselten Datenbank."),
            KeyError::Keychain(e) => write!(f, "Der Schlüssel der Datenbank ließ sich nicht aus dem Schlüsselbund lesen: {e}"),
        }
    }
}

impl std::error::Error for KeyError {}

/// `true`, wenn `path` eine verschlüsselte Datenbank ist. Eine fehlende oder leere Datei
/// (neue Datenbank) gilt als unverschlüsselt.
pub fn is_encrypted(path: &Path) -> std::io::Result<bool> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let mut head = Vec::with_capacity(SQLITE_HEADER.len());
    file.by_ref().take(SQLITE_HEADER.len() as u64).read_to_end(&mut head)?;
    Ok(!head.is_empty() && head != SQLITE_HEADER)
}

/// Setzt den Schlüssel einer frisch geöffneten Verbindung (muss die erste Anweisung sein) und
/// prüft, dass er passt. Ohne Schlüssel: nur die Prüfung.
pub fn unlock(conn: &Connection, key: Option<&Key>) -> Result<()> {
    if let Some(key) = key {
        conn.execute_batch(&format!("PRAGMA key = \"{}\";", key.sql()))?;
    }
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            Err(if key.is_some() { KeyError::Wrong } else { KeyError::Missing }.into())
        }
        Err(e) => Err(e.into()),
    }
}

/// Passt `key` zur verschlüsselten Datei `path`?
pub fn key_fits(path: &Path, key: &Key) -> bool {
    Connection::open(path).is_ok_and(|c| unlock(&c, Some(key)).is_ok())
}

// ---------- Schlüsselbund ----------

/// Schlüssel, die dieser Prozess schon kennt (je Datenordner) – damit nicht jede Verbindung
/// den Schlüsselbund fragt, und für einen eingegebenen Wiederherstellungsschlüssel, auch wenn
/// er sich nicht im Schlüsselbund speichern ließ.
static KNOWN: Mutex<Vec<(PathBuf, Key)>> = Mutex::new(Vec::new());

fn known(dir: &Path) -> Option<Key> {
    KNOWN.lock().ok()?.iter().find(|(d, _)| d == dir).map(|(_, k)| k.clone())
}

/// Merkt sich `key` für den Datenordner `dir` (nur in diesem Prozess).
pub fn remember(dir: &Path, key: &Key) {
    if let Ok(mut known) = KNOWN.lock() {
        known.retain(|(d, _)| d != dir);
        known.push((dir.to_path_buf(), key.clone()));
    }
}

fn forget(dir: &Path) {
    if let Ok(mut known) = KNOWN.lock() {
        known.retain(|(d, _)| d != dir);
    }
}

/// Konto des Eintrags: „Datenbank“ für den üblichen Datenordner, sonst mit dessen Pfad –
/// damit ein Testordner (`TAGWERK_DIR`) nie den Schlüssel der echten Daten überschreibt.
fn account(dir: &Path) -> String {
    if dir == crate::store::default_data_dir() {
        "Datenbank".to_string()
    } else {
        format!("Datenbank {}", dir.display())
    }
}

/// Der Schlüssel der Datenbank im Ordner `dir`.
pub fn stored_key(dir: &Path) -> std::result::Result<Key, KeyError> {
    if let Some(key) = known(dir) {
        return Ok(key);
    }
    let key = keychain::get(&account(dir))?;
    remember(dir, &key);
    Ok(key)
}

/// Legt den Schlüssel für die Datenbank im Ordner `dir` im Schlüsselbund ab (ersetzt einen
/// vorhandenen).
pub fn store_key(dir: &Path, key: &Key) -> Result<()> {
    keychain::set(&account(dir), key)?;
    remember(dir, key);
    Ok(())
}

/// Entfernt den Schlüssel aus dem Schlüsselbund (kein Fehler, wenn es keinen gibt).
pub fn delete_key(dir: &Path) -> Result<()> {
    forget(dir);
    keychain::delete(&account(dir))
}

#[cfg(target_os = "macos")]
mod keychain {
    use super::{Key, KeyError, Result, SERVICE};
    use security_framework::passwords::{self, PasswordOptions};

    /// `errSecItemNotFound`
    const NOT_FOUND: i32 = -25300;

    pub fn get(account: &str) -> std::result::Result<Key, KeyError> {
        match passwords::generic_password(PasswordOptions::new_generic_password(SERVICE, account)) {
            Ok(bytes) => Key::from_bytes(&bytes).ok_or(KeyError::Wrong),
            Err(e) if e.code() == NOT_FOUND => Err(KeyError::Missing),
            Err(e) => Err(KeyError::Keychain(e.to_string())),
        }
    }

    pub fn set(account: &str, key: &Key) -> Result<()> {
        let mut options = PasswordOptions::new_generic_password(SERVICE, account);
        options.set_label("Tagwerk – Schlüssel der Datenbank");
        options.set_comment("Verschlüsselt die Arbeitszeiten von Tagwerk. Ohne ihn lassen sie sich nicht mehr lesen.");
        passwords::set_generic_password_options(&key.0, options)?;
        Ok(())
    }

    pub fn delete(account: &str) -> Result<()> {
        match passwords::delete_generic_password(SERVICE, account) {
            Err(e) if e.code() != NOT_FOUND => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod keychain {
    use super::{Key, KeyError, Result};

    pub fn get(_account: &str) -> std::result::Result<Key, KeyError> {
        Err(KeyError::Missing)
    }

    pub fn set(_account: &str, _key: &Key) -> Result<()> {
        Err("Einen Schlüsselbund gibt es nur unter macOS.".into())
    }

    pub fn delete(_account: &str) -> Result<()> {
        Ok(())
    }
}

// ---------- Umstellen ----------

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Ver- oder entschlüsselt die Datei `path`: `from` ist ihr jetziger Schlüssel (`None`:
/// unverschlüsselt), `to` der künftige. Die neue Fassung entsteht daneben, wird geprüft
/// (Integrität, gleiche Zeilenzahl in jeder Tabelle) und ersetzt erst dann die alte – bei einem
/// Fehler bleibt die alte unverändert. Niemand sonst darf die Datei geöffnet haben.
pub fn convert_file(path: &Path, from: Option<&Key>, to: Option<&Key>) -> Result<()> {
    let tmp = sidecar(path, CONVERT_SUFFIX);
    let result = export(path, &tmp, from, to);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        return result;
    }
    fs::rename(&tmp, path)?;
    // Leer, wenn es sie überhaupt noch gibt (siehe `export`) – aber zur alten Fassung gehörig.
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(sidecar(path, suffix));
    }
    Ok(())
}

fn export(path: &Path, tmp: &Path, from: Option<&Key>, to: Option<&Key>) -> Result<()> {
    let _ = fs::remove_file(tmp);
    let tmp_str = tmp.to_str().ok_or("Der Pfad der Datenbank ist kein gültiger Text.")?;
    let to_sql = to.map(Key::sql).unwrap_or_default();
    let (version, app_id, tables) = {
        let conn = Connection::open(path)?;
        unlock(&conn, from)?;
        // Alles aus dem Journal in die Datei schreiben: Ein übrig gebliebenes Journal darf
        // nie zur neuen Datei gelesen werden.
        let wal: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
        if wal.eq_ignore_ascii_case("wal") {
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        }
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let app_id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        conn.execute("ATTACH DATABASE ?1 AS conv KEY ?2", params![tmp_str, to_sql])?;
        conn.query_row("SELECT sqlcipher_export('conv')", [], |_| Ok(()))?;
        // `sqlcipher_export` übernimmt beides nicht.
        conn.execute_batch(&format!("PRAGMA conv.user_version = {version}; PRAGMA conv.application_id = {app_id};"))?;
        let names: Vec<String> = conn
            .prepare("SELECT name FROM main.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")?
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        let mut tables = Vec::new();
        for name in names {
            let quoted = name.replace('"', "\"\"");
            let (a, b): (i64, i64) = conn.query_row(
                &format!("SELECT (SELECT count(*) FROM main.\"{quoted}\"), (SELECT count(*) FROM conv.\"{quoted}\")"),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if a != b {
                return Err(format!("Tabelle {name}: {b} statt {a} Zeilen nach dem Umstellen.").into());
            }
            tables.push((name, a));
        }
        conn.execute_batch("DETACH DATABASE conv")?;
        (version, app_id, tables)
    };
    // Mit dem Schließen der letzten Verbindung löscht SQLite das Journal. Gibt es es noch, hat
    // ein anderes Programm die Datenbank offen – es schriebe weiter in die alte Datei.
    if sidecar(path, "-wal").exists() {
        return Err("Die Datenbank ist noch in einem anderen Programm geöffnet (etwa „Tagwerk --records“ \
                    oder einem Export im Terminal). Es muss erst beendet werden."
            .into());
    }
    // Die neue Fassung mit dem neuen Schlüssel prüfen.
    let conn = Connection::open(tmp)?;
    unlock(&conn, to)?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(format!("Die umgestellte Datenbank ist beschädigt: {check}").into());
    }
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let a: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    if (v, a) != (version, app_id) {
        return Err("Die umgestellte Datenbank hat eine andere Kennung.".into());
    }
    for (name, rows) in tables {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM \"{}\"", name.replace('"', "\"\"")), [], |r| r.get(0))?;
        if n != rows {
            return Err(format!("Tabelle {name}: {n} statt {rows} Zeilen nach dem Umstellen.").into());
        }
    }
    Ok(())
}

/// Was eine Umstellung bewirkt hat.
#[derive(Debug, Default)]
pub struct Report {
    /// `true`: verschlüsselt, `false`: entschlüsselt.
    pub encrypted: bool,
    /// Umgestellte Backups.
    pub backups: usize,
    /// Backups, die sich nicht umstellen ließen (Datei, Grund).
    pub failed: Vec<(PathBuf, String)>,
    /// Dateien früherer Versionen mit Aufzeichnungen im Klartext (nur beim Verschlüsseln).
    pub plain_leftovers: Vec<PathBuf>,
}

impl Report {
    /// Text für die Meldung nach dem Neustart.
    pub fn text(&self) -> String {
        let mut text = if self.encrypted {
            "Die Datenbank ist jetzt verschlüsselt. Den Wiederherstellungsschlüssel zeigen die \
             Einstellungen unter „Allgemein“."
                .to_string()
        } else {
            "Die Datenbank ist jetzt wieder unverschlüsselt, ihr Schlüssel ist aus dem \
             Schlüsselbund entfernt."
                .to_string()
        };
        match self.backups {
            0 => {}
            1 => text.push_str(" Ein Backup wurde mit umgestellt."),
            n => text.push_str(&format!(" {n} Backups wurden mit umgestellt.")),
        }
        if !self.failed.is_empty() {
            let list: Vec<String> = self
                .failed
                .iter()
                .map(|(p, e)| format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()))
                .collect();
            text.push_str(&format!(
                "\n\nNicht umgestellt ({}):\n{}",
                list.len(),
                list.join("\n")
            ));
            if !self.encrypted {
                text.push_str("\n\nDer Schlüssel bleibt deshalb im Schlüsselbund.");
            }
        }
        if !self.plain_leftovers.is_empty() {
            let names: Vec<String> =
                self.plain_leftovers.iter().map(|p| p.display().to_string()).collect();
            text.push_str(&format!(
                "\n\nAus einer früheren Version liegen noch Aufzeichnungen unverschlüsselt im \
                 Datenordner. Tagwerk braucht sie nicht mehr:\n{}",
                names.join("\n")
            ));
        }
        text
    }
}

/// Stellt die Datenbank im Ordner `dir` und ihre Backups um (`from` → `to`, siehe
/// [`convert_file`]). Lässt sich die Datenbank selbst nicht umstellen, bleibt alles, wie es
/// war; Backups, bei denen es scheitert, stehen im Bericht.
pub fn convert_dir(dir: &Path, from: Option<&Key>, to: Option<&Key>) -> Result<Report> {
    let db = dir.join(DB_FILE);
    if db.exists() && is_encrypted(&db)? == from.is_some() {
        convert_file(&db, from, to)?;
    }
    let mut report = Report { encrypted: to.is_some(), ..Default::default() };
    let mut backups: Vec<PathBuf> = match fs::read_dir(dir.join(BACKUP_DIR)) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "db"))
            .collect(),
        Err(_) => Vec::new(),
    };
    backups.sort();
    for path in backups {
        // Schon im gewünschten Zustand (oder mit einem anderen Schlüssel verschlüsselt, siehe unten).
        match is_encrypted(&path) {
            Ok(enc) if enc != from.is_some() => continue,
            Ok(_) => {}
            Err(e) => {
                report.failed.push((path, e.to_string()));
                continue;
            }
        }
        match convert_file(&path, from, to) {
            Ok(()) => report.backups += 1,
            Err(e) => report.failed.push((path, e.to_string())),
        }
    }
    if to.is_some() {
        report.plain_leftovers = plain_leftovers(dir);
    }
    Ok(report)
}

/// Markdown-Dateien früherer Versionen mit Aufzeichnungen: die übernommene `tagwerk.md` und
/// Backups als Markdown.
fn plain_leftovers(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = ["tagwerk.md", "tagwerk.md.importiert"]
        .iter()
        .map(|n| dir.join(n))
        .filter(|p| p.exists())
        .collect();
    if let Ok(entries) = fs::read_dir(dir.join(BACKUP_DIR)) {
        let mut md: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .collect();
        md.sort();
        files.extend(md);
    }
    files
}

/// Merkt sich bis zum nächsten Start, dass die Datenbank ver- (`on`) oder entschlüsselt werden
/// soll. Beim Verschlüsseln muss der Schlüssel dann schon im Schlüsselbund liegen.
pub fn request(dir: &Path, on: bool) -> std::io::Result<()> {
    fs::write(dir.join(PENDING_FILE), if on { "ein\n" } else { "aus\n" })
}

/// Führt eine vorgemerkte Umstellung aus (vor dem ersten Öffnen der Datenbank, siehe
/// [`request`]). `None`: Es war nichts vorgemerkt.
pub fn apply_pending(dir: &Path) -> Option<std::result::Result<Report, String>> {
    let marker = dir.join(PENDING_FILE);
    let wish = fs::read_to_string(&marker).ok()?;
    // Auch bei einem Fehler nicht bei jedem Start erneut versuchen.
    let _ = fs::remove_file(&marker);
    let result = match wish.trim() {
        "ein" => stored_key(dir)
            .map_err(|e| e.to_string())
            .and_then(|key| convert_dir(dir, None, Some(&key)).map_err(|e| e.to_string())),
        "aus" => decrypt(dir),
        other => Err(format!("Unbekannter Wunsch „{other}“ in {}.", marker.display())),
    };
    Some(result.map_err(|e| {
        format!("Die Datenbank wurde nicht umgestellt und ist unverändert. {e}")
    }))
}

fn decrypt(dir: &Path) -> std::result::Result<Report, String> {
    let db = dir.join(DB_FILE);
    let encrypted = is_encrypted(&db).map_err(|e| e.to_string())?;
    let key = match stored_key(dir) {
        Ok(key) => Some(key),
        // Schon unverschlüsselt und kein Schlüssel mehr: nichts zu tun.
        Err(KeyError::Missing) if !encrypted => None,
        Err(e) => return Err(e.to_string()),
    };
    let Some(key) = key else {
        return Ok(Report::default());
    };
    let report = convert_dir(dir, Some(&key), None).map_err(|e| e.to_string())?;
    if report.failed.is_empty() {
        delete_key(dir).map_err(|e| format!("Die Datenbank ist entschlüsselt, aber der Schlüssel ließ sich nicht aus dem Schlüsselbund entfernen: {e}"))?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tagwerk-crypt-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(BACKUP_DIR)).unwrap();
        dir
    }

    fn sample(path: &Path, key: Option<&Key>, wal: bool) {
        let conn = Connection::open(path).unwrap();
        unlock(&conn, key).unwrap();
        if wal {
            conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(())).unwrap();
        }
        conn.execute_batch(
            "CREATE TABLE periods (date TEXT, idx INTEGER, PRIMARY KEY (date, idx)) WITHOUT ROWID;
             INSERT INTO periods VALUES ('2026-11-17', 0), ('2026-11-17', 1), ('2026-11-18', 0);
             PRAGMA user_version = 3; PRAGMA application_id = 42;",
        )
        .unwrap();
    }

    fn rows(path: &Path, key: Option<&Key>) -> (i64, i64, i64) {
        let conn = Connection::open(path).unwrap();
        unlock(&conn, key).unwrap();
        let n = conn.query_row("SELECT count(*) FROM periods", [], |r| r.get(0)).unwrap();
        let v = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        let a = conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
        (n, v, a)
    }

    #[test]
    fn recovery_text_round_trip() {
        let key = Key::generate().unwrap();
        let text = key.recovery_text();
        assert_eq!(text.len(), 8 * 8 + 7);
        assert_eq!(text.split('-').count(), 8);
        assert_eq!(Key::parse_recovery(&text), Some(key.clone()));
        let sloppy = format!(" {}\n", text.to_lowercase().replace('-', " "));
        assert_eq!(Key::parse_recovery(&sloppy), Some(key));
        assert_eq!(Key::parse_recovery("1234"), None);
        assert_eq!(Key::parse_recovery(&"Z".repeat(64)), None);
        assert_ne!(Key::generate().unwrap(), Key::generate().unwrap());
        assert_eq!(format!("{:?}", Key::generate().unwrap()), "Key(…)");
    }

    #[test]
    fn detects_encryption_by_header() {
        let dir = temp_dir("header");
        let plain = dir.join("plain.db");
        let enc = dir.join("enc.db");
        assert!(!is_encrypted(&plain).unwrap(), "fehlende Datei");
        fs::write(&plain, b"").unwrap();
        assert!(!is_encrypted(&plain).unwrap(), "leere Datei");
        fs::remove_file(&plain).unwrap();
        sample(&plain, None, false);
        let key = Key::generate().unwrap();
        sample(&enc, Some(&key), false);
        assert!(!is_encrypted(&plain).unwrap());
        assert!(is_encrypted(&enc).unwrap());
        assert!(key_fits(&enc, &key));
        assert!(!key_fits(&enc, &Key::generate().unwrap()));
        let conn = Connection::open(&enc).unwrap();
        let err = unlock(&conn, None).unwrap_err();
        assert_eq!(err.downcast_ref::<KeyError>(), Some(&KeyError::Missing));
        let conn = Connection::open(&enc).unwrap();
        let err = unlock(&conn, Some(&Key::generate().unwrap())).unwrap_err();
        assert_eq!(err.downcast_ref::<KeyError>(), Some(&KeyError::Wrong));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn convert_file_both_ways_keeps_data_and_ids() {
        let dir = temp_dir("both-ways");
        let db = dir.join(DB_FILE);
        sample(&db, None, true);
        let key = Key::generate().unwrap();
        convert_file(&db, None, Some(&key)).unwrap();
        assert!(is_encrypted(&db).unwrap());
        assert!(!sidecar(&db, "-wal").exists() && !sidecar(&db, CONVERT_SUFFIX).exists());
        assert_eq!(rows(&db, Some(&key)), (3, 3, 42));
        let raw = fs::read(&db).unwrap();
        assert!(!raw.windows(10).any(|w| w == b"2026-11-17"), "kein Klartext in der Datei");
        convert_file(&db, Some(&key), None).unwrap();
        assert!(!is_encrypted(&db).unwrap());
        assert_eq!(rows(&db, None), (3, 3, 42));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn convert_file_with_wrong_key_leaves_file_alone() {
        let dir = temp_dir("wrong-key");
        let db = dir.join(DB_FILE);
        let key = Key::generate().unwrap();
        sample(&db, Some(&key), false);
        let before = fs::read(&db).unwrap();
        let other = Key::generate().unwrap();
        assert!(convert_file(&db, Some(&other), None).is_err());
        assert_eq!(fs::read(&db).unwrap(), before);
        assert!(!sidecar(&db, CONVERT_SUFFIX).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn convert_refuses_while_another_connection_is_open() {
        let dir = temp_dir("busy");
        let db = dir.join(DB_FILE);
        sample(&db, None, true);
        let other = Connection::open(&db).unwrap();
        other.query_row("SELECT count(*) FROM periods", [], |r| r.get::<_, i64>(0)).unwrap();
        let err = convert_file(&db, None, Some(&Key::generate().unwrap())).unwrap_err();
        assert!(err.to_string().contains("anderen Programm"), "{err}");
        assert!(!is_encrypted(&db).unwrap());
        drop(other);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn convert_dir_includes_backups_and_names_leftovers() {
        let dir = temp_dir("dir");
        sample(&dir.join(DB_FILE), None, true);
        let b1 = dir.join(BACKUP_DIR).join("tagwerk-2026-11-17.db");
        let b2 = dir.join(BACKUP_DIR).join("tagwerk-2026-11-18.db");
        sample(&b1, None, false);
        sample(&b2, None, false);
        fs::write(dir.join(BACKUP_DIR).join("tagwerk-2026-01-01.md"), "# alt").unwrap();
        fs::write(dir.join("tagwerk.md.importiert"), "# alt").unwrap();
        // Kein Backup: bleibt liegen.
        fs::write(dir.join(BACKUP_DIR).join("kaputt.db"), b"SQLite format 3\0kaputt").unwrap();
        let key = Key::generate().unwrap();
        let report = convert_dir(&dir, None, Some(&key)).unwrap();
        assert!(report.encrypted);
        assert_eq!(report.backups, 2);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.plain_leftovers.len(), 2);
        assert!(report.text().contains("2 Backups"));
        assert!(report.text().contains("kaputt.db"));
        assert!(report.text().contains("tagwerk.md.importiert"));
        for p in [dir.join(DB_FILE), b1.clone(), b2.clone()] {
            assert!(is_encrypted(&p).unwrap(), "{}", p.display());
            assert_eq!(rows(&p, Some(&key)), (3, 3, 42));
        }
        // Zurück: alles wieder unverschlüsselt.
        let report = convert_dir(&dir, Some(&key), None).unwrap();
        assert!(!report.encrypted);
        assert_eq!(report.backups, 2);
        assert!(report.plain_leftovers.is_empty());
        for p in [dir.join(DB_FILE), b1, b2] {
            assert!(!is_encrypted(&p).unwrap());
            assert_eq!(rows(&p, None), (3, 3, 42));
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// Echter Schlüsselbund (eigenes Konto für einen Testordner): `cargo test -- --ignored keychain`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let dir = temp_dir("keychain");
        let key = Key::generate().unwrap();
        store_key(&dir, &key).unwrap();
        forget(&dir);
        assert_eq!(stored_key(&dir).unwrap(), key);
        let other = Key::generate().unwrap();
        store_key(&dir, &other).unwrap();
        forget(&dir);
        assert_eq!(stored_key(&dir).unwrap(), other, "ersetzt");
        delete_key(&dir).unwrap();
        assert_eq!(stored_key(&dir).unwrap_err(), KeyError::Missing);
        delete_key(&dir).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pending_switch_uses_known_key_and_runs_once() {
        let dir = temp_dir("pending");
        sample(&dir.join(DB_FILE), None, true);
        assert!(apply_pending(&dir).is_none(), "nichts vorgemerkt");
        let key = Key::generate().unwrap();
        // Anstelle des Schlüsselbunds (Tests fassen ihn nicht an).
        remember(&dir, &key);
        request(&dir, true).unwrap();
        let report = apply_pending(&dir).unwrap().unwrap();
        assert!(report.encrypted);
        assert!(is_encrypted(&dir.join(DB_FILE)).unwrap());
        assert!(apply_pending(&dir).is_none(), "nur einmal");
        forget(&dir);
        let _ = fs::remove_dir_all(dir);
    }
}
