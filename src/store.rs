//! Speicherung der Arbeitszeiten in einer lokalen SQLite-Datenbank plus tägliche Backups,
//! dazu Export (und Import) als Markdown.
//!
//! Tabellen (Datum immer als `JJJJ-MM-TT`, Uhrzeiten als Sekunden seit Mitternacht):
//! - `periods`   – automatisch erfasste Zeiten (Mac entsperrt),
//! - `breaks`    – manuell eingetragene Pausen (`to_s` NULL = läuft noch),
//! - `day_marks` – manuell festgelegter Arbeitsbeginn und Feierabend.
//!
//! Die Markdown-Datei früherer Versionen (`tagwerk.md`) wird beim ersten Öffnen der
//! Datenbank einmalig übernommen und danach in `tagwerk.md.importiert` umbenannt.

use crate::day::{self, fmt_clock_s, parse_clock, Break, Day, Mode, Span, DAY_END};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Weekday};
use rusqlite::{params, Connection, TransactionBehavior};
use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Wie viele tägliche Backups aufbewahrt werden.
pub const MAX_BACKUPS: usize = 15;
/// Standard-Zeitraum des Exports: die letzten vier Wochen (28 Tage inklusive heute).
pub const EXPORT_DAYS: i64 = 28;

const DB_FILE: &str = "tagwerk.db";
const LEGACY_FILE: &str = "tagwerk.md";
const LEGACY_IMPORTED: &str = "tagwerk.md.importiert";
const SCHEMA_VERSION: i32 = 1;
const SCHEMA: &str = "
    CREATE TABLE periods (
        date TEXT NOT NULL,
        idx INTEGER NOT NULL,
        from_s INTEGER NOT NULL,
        to_s INTEGER NOT NULL,
        PRIMARY KEY (date, idx)
    ) WITHOUT ROWID;
    CREATE TABLE breaks (
        date TEXT NOT NULL,
        idx INTEGER NOT NULL,
        from_s INTEGER NOT NULL,
        to_s INTEGER,
        PRIMARY KEY (date, idx)
    ) WITHOUT ROWID;
    CREATE TABLE day_marks (
        date TEXT PRIMARY KEY,
        start_s INTEGER,
        end_s INTEGER
    ) WITHOUT ROWID;
";

const HEAD_CORRECTIONS: &str = "## Korrekturen";
const HEAD_PERIODS: &str = "## Erfasste Perioden";
const KIND_START: &str = "Arbeitsbeginn";
const KIND_BREAK: &str = "Pause";
const KIND_END: &str = "Feierabend";
const OPEN: &str = "läuft";

/// `$HOME/Library/Application Support/Tagwerk` (zum Testen per `TAGWERK_DIR` änderbar).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TAGWERK_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library")
        .join("Application Support")
        .join("Tagwerk")
}

pub fn db_file() -> PathBuf {
    data_dir().join(DB_FILE)
}

/// Kleine Statusdatei, damit das Fenster "Aufzeichnungen" weiß, ob gerade erfasst wird.
pub fn status_file() -> PathBuf {
    data_dir().join(".status")
}

/// Markierung: Das Fenster „Aufzeichnungen“ soll den Export-Dialog öffnen.
pub fn export_request_file() -> PathBuf {
    data_dir().join(".export-request")
}

fn date_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Verbindung zur Datenbank. Menüleisten-App und Fenster „Aufzeichnungen“ haben je eine
/// eigene; Änderungen der anderen erkennt man mit [`Db::changed_externally`].
pub struct Db {
    conn: Connection,
    dir: PathBuf,
    data_version: i64,
}

impl Db {
    /// Öffnet (oder erstellt) `tagwerk.db` im Datenordner und übernimmt beim ersten Mal
    /// vorhandene Daten (`tagwerk.md`, sonst „TrackMyWork“).
    pub fn open() -> Result<Db> {
        Self::open_in(&data_dir(), std::env::var_os("TAGWERK_DIR").is_none())
    }

    fn open_in(dir: &Path, import_trackmywork: bool) -> Result<Db> {
        fs::create_dir_all(dir)?;
        let mut conn = Connection::open(dir.join(DB_FILE))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        init_schema(&mut conn, dir, import_trackmywork)?;
        let mut db = Db { conn, dir: dir.to_path_buf(), data_version: 0 };
        db.data_version = db.read_data_version()?;
        Ok(db)
    }

    pub fn backup_dir(&self) -> PathBuf {
        self.dir.join("backups")
    }

    fn read_data_version(&self) -> Result<i64> {
        Ok(self.conn.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    /// `true`, wenn ein anderer Prozess seit der letzten Abfrage etwas gespeichert hat.
    pub fn changed_externally(&mut self) -> bool {
        match self.read_data_version() {
            Ok(v) if v != self.data_version => {
                self.data_version = v;
                true
            }
            _ => false,
        }
    }

    /// Alle Tage, nach Datum sortiert.
    pub fn load(&self) -> Result<Vec<Day>> {
        load_between(&self.conn, "", "~")
    }

    /// Alle Tage von `from` bis einschließlich `to`.
    pub fn load_range(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Day>> {
        load_between(&self.conn, &date_key(from), &date_key(to))
    }

    /// Schreibt die übergebenen Tage (leere Tage werden gelöscht) und löscht `removed`.
    /// Andere Tage bleiben unberührt.
    pub fn save_days(&mut self, days: &[Day], removed: &[NaiveDate], now: NaiveDateTime) -> Result<()> {
        if days.is_empty() && removed.is_empty() {
            return Ok(());
        }
        self.daily_backup(now.date());
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for d in days {
            write_day(&tx, d)?;
        }
        for &date in removed {
            delete_day(&tx, date)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Liest einen Tag, ändert ihn und schreibt ihn in derselben Transaktion zurück
    /// (für das Fenster „Aufzeichnungen“).
    pub fn edit_day(&mut self, date: NaiveDate, now: NaiveDateTime, f: impl FnOnce(&mut Day)) -> Result<()> {
        self.daily_backup(now.date());
        let tx = self.conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let key = date_key(date);
        let mut days = load_between(&tx, &key, &key)?;
        let d = day::day_mut(&mut days, date);
        f(d);
        write_day(&tx, d)?;
        tx.commit()?;
        Ok(())
    }

    /// Sichert die Datenbank einmal am Tag (vor der ersten Änderung) als
    /// `backups/tagwerk-JJJJ-MM-TT.db`; es bleiben die letzten [`MAX_BACKUPS`].
    /// Ein Fehler beim Sichern verhindert das Speichern nicht.
    fn daily_backup(&self, today: NaiveDate) {
        let bdir = self.backup_dir();
        let target = bdir.join(format!("tagwerk-{}.db", date_key(today)));
        if target.exists() {
            return;
        }
        let result = (|| -> Result<()> {
            let rows: i64 = self.conn.query_row(
                "SELECT (SELECT count(*) FROM periods) + (SELECT count(*) FROM breaks)
                      + (SELECT count(*) FROM day_marks)",
                [],
                |r| r.get(0),
            )?;
            if rows == 0 {
                return Ok(());
            }
            fs::create_dir_all(&bdir)?;
            let path = target.to_str().ok_or("Pfad des Backups ist kein gültiger Text")?;
            self.conn.execute("VACUUM INTO ?1", [path])?;
            prune_backups(&bdir)?;
            Ok(())
        })();
        if let Err(e) = result {
            // Ein anderer Prozess kann das Backup gerade angelegt haben.
            if !target.exists() {
                eprintln!("Tagwerk: Backup fehlgeschlagen: {e}");
            }
        }
    }
}

fn init_schema(conn: &mut Connection, dir: &Path, import_trackmywork: bool) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        return Err(format!(
            "Die Datenbank stammt von einer neueren Tagwerk-Version (Schema {version})."
        )
        .into());
    }
    let mut imported_file = None;
    if version == 0 {
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        let legacy = dir.join(LEGACY_FILE);
        let days = if legacy.exists() {
            imported_file = Some(legacy.clone());
            parse_file(&fs::read_to_string(&legacy)?)
        } else if import_trackmywork {
            trackmywork_days(dir)?
        } else {
            Vec::new()
        };
        for d in &days {
            write_day(&tx, d)?;
        }
    }
    tx.commit()?;
    if let Some(file) = imported_file {
        if let Err(e) = fs::rename(&file, dir.join(LEGACY_IMPORTED)) {
            eprintln!("Tagwerk: {} konnte nicht umbenannt werden: {e}", file.display());
        }
    }
    Ok(())
}

/// Daten der früheren Version „TrackMyWork“ (`Application Support/TrackMyWork/track-my-work.md`).
/// Deren Backups werden in den Backup-Ordner kopiert; der alte Ordner bleibt unverändert.
fn trackmywork_days(dir: &Path) -> Result<Vec<Day>> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Ok(Vec::new()) };
    let old_dir = home.join("Library").join("Application Support").join("TrackMyWork");
    let old_file = old_dir.join("track-my-work.md");
    if !old_file.exists() {
        return Ok(Vec::new());
    }
    let bdir = dir.join("backups");
    if let Ok(entries) = fs::read_dir(old_dir.join("backups")) {
        fs::create_dir_all(&bdir)?;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("track-my-work-") {
                let target = bdir.join(format!("tagwerk-{rest}"));
                if !target.exists() {
                    fs::copy(e.path(), target)?;
                }
            }
        }
    }
    Ok(parse_file(&fs::read_to_string(&old_file)?))
}

fn load_between(conn: &Connection, from: &str, to: &str) -> Result<Vec<Day>> {
    let mut days: Vec<Day> = Vec::new();
    let parse = |s: String| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok();

    let mut stmt = conn.prepare(
        "SELECT date, from_s, to_s FROM periods WHERE date BETWEEN ?1 AND ?2 ORDER BY date, idx",
    )?;
    let rows = stmt.query_map([from, to], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
    for row in rows {
        let (date, f, t): (String, i64, i64) = row?;
        if let Some(date) = parse(date) {
            day::day_mut(&mut days, date).periods.push(Span::new(f, t));
        }
    }

    let mut stmt = conn.prepare(
        "SELECT date, from_s, to_s FROM breaks WHERE date BETWEEN ?1 AND ?2 ORDER BY date, idx",
    )?;
    let rows = stmt.query_map([from, to], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
    for row in rows {
        let (date, f, t): (String, i64, Option<i64>) = row?;
        if let Some(date) = parse(date) {
            day::day_mut(&mut days, date).breaks.push(Break { from: f, to: t });
        }
    }

    let mut stmt = conn.prepare(
        "SELECT date, start_s, end_s FROM day_marks WHERE date BETWEEN ?1 AND ?2 ORDER BY date",
    )?;
    let rows = stmt.query_map([from, to], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?;
    for row in rows {
        let (date, s, e): (String, Option<i64>, Option<i64>) = row?;
        if let Some(date) = parse(date) {
            let d = day::day_mut(&mut days, date);
            d.start = s;
            d.end = e;
        }
    }

    days.retain(|d| !d.is_empty());
    Ok(days)
}

fn delete_day(conn: &Connection, date: NaiveDate) -> Result<()> {
    let key = date_key(date);
    conn.execute("DELETE FROM periods WHERE date = ?1", [&key])?;
    conn.execute("DELETE FROM breaks WHERE date = ?1", [&key])?;
    conn.execute("DELETE FROM day_marks WHERE date = ?1", [&key])?;
    Ok(())
}

/// Ersetzt alle Zeilen eines Tages (ein leerer Tag wird nur gelöscht).
fn write_day(conn: &Connection, d: &Day) -> Result<()> {
    delete_day(conn, d.date)?;
    if d.is_empty() {
        return Ok(());
    }
    let key = date_key(d.date);
    for (i, p) in d.periods.iter().enumerate() {
        conn.execute(
            "INSERT INTO periods (date, idx, from_s, to_s) VALUES (?1, ?2, ?3, ?4)",
            params![key, i as i64, p.from, p.to],
        )?;
    }
    for (i, b) in d.breaks.iter().enumerate() {
        conn.execute(
            "INSERT INTO breaks (date, idx, from_s, to_s) VALUES (?1, ?2, ?3, ?4)",
            params![key, i as i64, b.from, b.to],
        )?;
    }
    if d.start.is_some() || d.end.is_some() {
        conn.execute(
            "INSERT INTO day_marks (date, start_s, end_s) VALUES (?1, ?2, ?3)",
            params![key, d.start, d.end],
        )?;
    }
    Ok(())
}

/// Löscht alle bis auf die neuesten [`MAX_BACKUPS`] Backups (`.db` und ältere `.md`).
fn prune_backups(bdir: &Path) -> Result<()> {
    let mut names: Vec<PathBuf> = fs::read_dir(bdir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tagwerk-") && (n.ends_with(".db") || n.ends_with(".md")))
        })
        .collect();
    names.sort();
    if names.len() > MAX_BACKUPS {
        let excess = names.len() - MAX_BACKUPS;
        for p in names.into_iter().take(excess) {
            let _ = fs::remove_file(p);
        }
    }
    Ok(())
}

// ---------- Formatieren ----------

pub fn weekday_short(d: NaiveDate) -> &'static str {
    match d.weekday() {
        Weekday::Mon => "Mo",
        Weekday::Tue => "Di",
        Weekday::Wed => "Mi",
        Weekday::Thu => "Do",
        Weekday::Fri => "Fr",
        Weekday::Sat => "Sa",
        Weekday::Sun => "So",
    }
}

/// `Mo, 16.11.2026`
pub fn fmt_date(d: NaiveDate) -> String {
    format!("{}, {}", weekday_short(d), d.format("%d.%m.%Y"))
}

/// `16.11.2026`
pub fn fmt_date_short(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

/// `HH:MM:SS` für Dauern (Stunden dürfen > 24 sein).
pub fn fmt_hms(secs: i64) -> String {
    let s = secs.max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// `7:05 h` – kurz für Menü und Fenster.
pub fn fmt_hm(secs: i64) -> String {
    let m = secs.max(0) / 60;
    format!("{}:{:02} h", m / 60, m % 60)
}

fn parse_duration(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let nums: Option<Vec<i64>> = parts.iter().map(|p| p.trim().parse::<i64>().ok()).collect();
    let nums = nums?;
    match nums.as_slice() {
        [h, m, s] => Some(h * 3600 + m * 60 + s),
        [h, m] => Some(h * 3600 + m * 60),
        _ => None,
    }
}

/// "Mo, 16.11.2026" oder "16.11.2026" oder "2026-11-16"
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    let token = s.trim().rsplit(' ').next()?.trim();
    NaiveDate::parse_from_str(token, "%d.%m.%Y")
        .or_else(|_| NaiveDate::parse_from_str(token, "%Y-%m-%d"))
        .ok()
}

/// `open_until` für einen Tag: heute die aktuelle Uhrzeit, sonst Mitternacht.
pub fn open_until(date: NaiveDate, now: NaiveDateTime) -> i64 {
    if date == now.date() {
        day::secs_of(now.time())
    } else {
        DAY_END
    }
}

// ---------- Markdown: Import ----------

#[derive(PartialEq)]
enum Section {
    Summary,
    Corrections,
    Periods,
    Other,
}

/// Liest alle Tage aus einem Markdown-Text (Export oder Datei früherer Versionen;
/// unbekannte Zeilen werden ignoriert).
pub fn parse_file(text: &str) -> Vec<Day> {
    let mut days: Vec<Day> = Vec::new();
    let mut legacy: Vec<(NaiveDate, i64, i64, i64)> = Vec::new();
    let mut has_periods_section = false;
    let mut section = Section::Summary;

    for line in text.lines() {
        let line = line.trim();
        if let Some(head) = line.strip_prefix("## ") {
            let head = head.to_lowercase();
            section = if head.contains("korrektur") {
                Section::Corrections
            } else if head.contains("periode") {
                has_periods_section = true;
                Section::Periods
            } else {
                Section::Other
            };
            continue;
        }
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        let Some(date) = cells.first().and_then(|c| parse_date(c)) else { continue };
        match section {
            Section::Summary if cells.len() >= 4 => {
                if let (Some(first), Some(last), Some(work)) =
                    (parse_clock(cells[1]), parse_clock(cells[2]), parse_duration(cells[3]))
                {
                    legacy.retain(|l| l.0 != date);
                    legacy.push((date, first, last.max(first), work));
                }
            }
            Section::Corrections if cells.len() >= 3 => {
                let kind = cells[1].to_lowercase();
                let Some(from) = parse_clock(cells[2]) else { continue };
                let to = cells.get(3).and_then(|c| parse_clock(c));
                let d = day::day_mut(&mut days, date);
                if kind.contains("beginn") || kind.contains("start") {
                    d.start = Some(from);
                } else if kind.contains("feierabend") || kind.contains("ende") {
                    d.end = Some(from);
                } else if kind.contains("pause") {
                    d.breaks.push(Break { from, to: to.map(|t| t.max(from)) });
                    d.sort_breaks();
                }
            }
            Section::Periods if cells.len() >= 3 => {
                if let (Some(from), Some(to)) = (parse_clock(cells[1]), parse_clock(cells[2])) {
                    day::day_mut(&mut days, date).add_period(from, to);
                }
            }
            _ => {}
        }
    }

    // Ältere Datei ohne Perioden: aus der Übersicht ableiten.
    if !has_periods_section {
        for (date, first, last, work) in legacy {
            let d = day::day_mut(&mut days, date);
            if !d.periods.is_empty() {
                continue;
            }
            let span = last - first;
            if work >= span || work <= 0 {
                d.add_period(first, (first + work.max(0)).min(DAY_END));
            } else {
                // Pause in die Mitte legen, damit Beginn, Ende und Arbeitszeit stimmen.
                let half = work / 2;
                d.add_period(first, first + half);
                d.add_period(last - (work - half), last);
            }
        }
    }
    days.retain(|d| !d.is_empty());
    days
}

// ---------- Markdown: Export ----------

/// Standard-Zeitraum des Exports: die letzten 28 Tage bis einschließlich `today`.
pub fn default_export_range(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    (today - Duration::days(EXPORT_DAYS - 1), today)
}

/// Vorschlag für den Dateinamen: `tagwerk-2026-10-21_2026-11-17.md`.
pub fn export_file_name(from: NaiveDate, to: NaiveDate) -> String {
    format!("tagwerk-{}_{}.md", date_key(from), date_key(to))
}

/// Markdown mit Übersicht (samt Summe), Korrekturen und erfassten Perioden für die Tage
/// von `from` bis einschließlich `to`. Lässt sich mit [`parse_file`] wieder einlesen.
pub fn export_markdown(days: &[Day], from: NaiveDate, to: NaiveDate, now: NaiveDateTime) -> String {
    let days: Vec<&Day> = days.iter().filter(|d| d.date >= from && d.date <= to).collect();
    let mut s = String::new();
    s.push_str(&format!(
        "# Tagwerk – Arbeitszeiten {} bis {}\n\n",
        fmt_date_short(from),
        fmt_date_short(to)
    ));
    s.push_str(&format!(
        "Exportiert am {} um {} Uhr. Arbeitszeit = Zeit, in der der Mac entsperrt war, verrechnet \
         mit den manuellen Korrekturen. Pausenzeit = Zeit zwischen Beginn und Ende, in der nicht \
         gearbeitet wurde.\n\n",
        fmt_date_short(now.date()),
        now.format("%H:%M")
    ));
    s.push_str("| Datum | Beginn | Ende | Arbeitszeit | Pausenzeit |\n");
    s.push_str("|:------|:------:|:----:|------------:|-----------:|\n");
    let (mut work, mut pause, mut count) = (0, 0, 0);
    for d in &days {
        let Some(r) = d.record(open_until(d.date, now)) else { continue };
        work += r.work_secs;
        pause += r.pause_secs();
        count += 1;
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            fmt_date(r.date),
            fmt_clock_s(r.first_start.min(DAY_END - 1)),
            fmt_clock_s(r.last_end.min(DAY_END - 1)),
            fmt_hms(r.work_secs),
            fmt_hms(r.pause_secs()),
        ));
    }
    let label = if count == 1 { "Arbeitstag" } else { "Arbeitstage" };
    s.push_str(&format!(
        "| **Summe: {count} {label}** | | | **{}** | **{}** |\n",
        fmt_hms(work),
        fmt_hms(pause)
    ));

    s.push_str(&format!("\n{HEAD_CORRECTIONS}\n\n"));
    s.push_str("Manuell festgelegt. Art: Arbeitsbeginn, Pause oder Feierabend; „läuft“ = Pause noch nicht beendet.\n\n");
    s.push_str("| Datum | Art | Von | Bis |\n");
    s.push_str("|:------|:----|:---:|:---:|\n");
    for d in &days {
        let date = fmt_date(d.date);
        if let Some(st) = d.start {
            s.push_str(&format!("| {date} | {KIND_START} | {} | – |\n", fmt_clock_s(st)));
        }
        for b in &d.breaks {
            let to = b.to.map(fmt_clock_s).unwrap_or_else(|| OPEN.to_string());
            s.push_str(&format!("| {date} | {KIND_BREAK} | {} | {to} |\n", fmt_clock_s(b.from)));
        }
        if let Some(e) = d.end {
            s.push_str(&format!("| {date} | {KIND_END} | {} | – |\n", fmt_clock_s(e)));
        }
    }

    s.push_str(&format!("\n{HEAD_PERIODS}\n\n"));
    s.push_str("Automatisch erfasst (Mac entsperrt, Tagwerk aktiv).\n\n");
    s.push_str("| Datum | Von | Bis |\n");
    s.push_str("|:------|:---:|:---:|\n");
    for d in &days {
        let date = fmt_date(d.date);
        for p in &d.periods {
            s.push_str(&format!("| {date} | {} | {} |\n", fmt_clock_s(p.from), fmt_clock_s(p.to)));
        }
    }
    s
}

// ---------- Status der Menüleisten-App ----------

/// Was die Menüleisten-App gerade tut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Paused,
    Break,
    Closed,
}

pub fn write_status(running: bool, mode: Mode) {
    let text = match (mode, running) {
        (Mode::Closed(_), _) => "closed",
        (Mode::Break(_), _) => "break",
        (Mode::Auto, true) => "running",
        (Mode::Auto, false) => "paused",
    };
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(status_file(), format!("{text}\n"));
}

/// `None` = unbekannt (App läuft nicht).
pub fn read_status() -> Option<Status> {
    let text = fs::read_to_string(status_file()).ok()?;
    match text.trim() {
        "running" => Some(Status::Running),
        "paused" => Some(Status::Paused),
        "break" => Some(Status::Break),
        "closed" => Some(Status::Closed),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn now() -> NaiveDateTime {
        date(17).and_hms_opt(10, 0, 0).unwrap()
    }

    /// Eigener, leerer Ordner je Test (Tests laufen parallel).
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tagwerk-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_days() -> Vec<Day> {
        let mut d = Day::new(date(16));
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d.start = Some(h(8, 0));
        d.breaks.push(Break { from: h(12, 0), to: Some(h(12, 45)) });
        d.end = Some(h(17, 15));
        let mut open = Day::new(date(17));
        open.add_period(h(8, 0), h(9, 30));
        open.breaks.push(Break { from: h(9, 30), to: None });
        vec![d, open]
    }

    #[test]
    fn db_roundtrip_and_partial_save() {
        let dir = temp_dir("roundtrip");
        let mut db = Db::open_in(&dir, false).unwrap();
        let days = sample_days();
        db.save_days(&days, &[], now()).unwrap();
        assert_eq!(db.load().unwrap(), days);
        assert_eq!(db.load_range(date(17), date(17)).unwrap(), vec![days[1].clone()]);

        // Nur einen Tag schreiben: der andere bleibt unberührt.
        let mut changed = days[1].clone();
        changed.end = Some(h(9, 45));
        db.save_days(&[changed.clone()], &[], now()).unwrap();
        assert_eq!(db.load().unwrap(), vec![days[0].clone(), changed]);

        // Löschen und leere Tage
        db.save_days(&[Day::new(date(17))], &[date(16)], now()).unwrap();
        assert!(db.load().unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_day_and_external_change() {
        let dir = temp_dir("edit");
        let mut a = Db::open_in(&dir, false).unwrap();
        let mut b = Db::open_in(&dir, false).unwrap();
        a.save_days(&sample_days(), &[], now()).unwrap();
        assert!(!a.changed_externally(), "eigene Änderung zählt nicht");
        assert!(b.changed_externally());
        b.edit_day(date(16), now(), |d| d.breaks.clear()).unwrap();
        assert!(a.changed_externally());
        assert!(a.load().unwrap()[0].breaks.is_empty());
        assert!(!a.changed_externally());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_legacy_markdown_once() {
        let dir = temp_dir("import");
        let days = sample_days();
        fs::write(dir.join(LEGACY_FILE), export_markdown(&days, date(1), date(30), now())).unwrap();
        let db = Db::open_in(&dir, false).unwrap();
        assert_eq!(db.load().unwrap(), days);
        assert!(!dir.join(LEGACY_FILE).exists());
        assert!(dir.join(LEGACY_IMPORTED).exists());
        drop(db);
        // Erneutes Öffnen importiert nichts doppelt, auch wenn wieder eine Datei daliegt.
        fs::write(dir.join(LEGACY_FILE), "| Mo, 02.11.2026 | 08:00:00 | 09:00:00 | 01:00:00 | 0 |\n").unwrap();
        let db = Db::open_in(&dir, false).unwrap();
        assert_eq!(db.load().unwrap(), days);
        assert!(dir.join(LEGACY_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_range_sum_and_roundtrip() {
        let days = sample_days();
        let md = export_markdown(&days, date(16), date(17), now());
        assert!(md.contains("# Tagwerk – Arbeitszeiten 16.11.2026 bis 17.11.2026"), "{md}");
        assert!(md.contains("| Mo, 16.11.2026 | 08:00:00 | 17:15:00 | 08:28:00 | 00:47:00 |"), "{md}");
        assert!(md.contains("| Mo, 16.11.2026 | Pause | 12:00:00 | 12:45:00 |"));
        assert!(md.contains("| Di, 17.11.2026 | Pause | 09:30:00 | läuft |"));
        // 17.11. um 10:00: 1:30 h Arbeit bis zur laufenden Pause, 0:30 h Pause
        assert!(md.contains("| **Summe: 2 Arbeitstage** | | | **09:58:00** | **01:17:00** |"), "{md}");
        assert_eq!(parse_file(&md), days);

        let only_16 = export_markdown(&days, date(10), date(16), now());
        assert!(!only_16.contains("17.11.2026 |"));
        assert!(only_16.contains("**Summe: 1 Arbeitstag**"));
        assert_eq!(parse_file(&only_16), vec![days[0].clone()]);
    }

    #[test]
    fn default_range_is_28_days() {
        let (from, to) = default_export_range(date(17));
        assert_eq!((from, to), (NaiveDate::from_ymd_opt(2026, 10, 21).unwrap(), date(17)));
        assert_eq!(export_file_name(from, to), "tagwerk-2026-10-21_2026-11-17.md");
    }

    #[test]
    fn legacy_file_becomes_periods() {
        let md = "| Datum | Erster Login | Letzter Logout | Arbeitszeit | Pausenzeit |\n\
                  |---|---|---|---|---|\n\
                  | Mo, 09.11.2026 | 08:00:00 | 17:00:00 | 08:00:00 | 01:00:00 |\n\
                  | Di, 10.11.2026 | 08:00:00 | 12:00:00 | 04:00:00 | 00:00:00 |\n";
        let days = parse_file(md);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].periods, vec![Span::new(h(8, 0), h(12, 0)), Span::new(h(13, 0), h(17, 0))]);
        assert_eq!(days[1].periods, vec![Span::new(h(8, 0), h(12, 0))]);
        let r = days[0].record(DAY_END).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(8, 0), h(1, 0)));
        // nach Export und erneutem Einlesen bleiben die Werte gleich
        assert_eq!(parse_file(&export_markdown(&days, date(1), date(30), now())), days);
    }

    #[test]
    fn daily_backup_keeps_last_15() {
        let dir = temp_dir("backup");
        let mut db = Db::open_in(&dir, false).unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        for i in 0..20 {
            let day = start + Duration::days(i);
            let mut d = Day::new(day);
            d.add_period(h(8, 0), h(9, 0));
            let t = day.and_hms_opt(12, 0, 0).unwrap();
            db.save_days(&[d.clone()], &[], t).unwrap();
            db.save_days(&[d], &[], t).unwrap(); // zweites Speichern am selben Tag: kein neues Backup
        }
        let bdir = db.backup_dir();
        // Erstes Speichern (leere DB) erzeugt kein Backup: 19 Tage, davon 15 übrig.
        assert_eq!(fs::read_dir(&bdir).unwrap().count(), MAX_BACKUPS);
        assert!(bdir.join("tagwerk-2026-10-20.db").exists());
        assert!(!bdir.join("tagwerk-2026-10-05.db").exists());
        // Ein Backup ist eine vollständige Datenbank mit dem Stand vor der Änderung.
        let backup = Connection::open(bdir.join("tagwerk-2026-10-20.db")).unwrap();
        let n: i64 = backup.query_row("SELECT count(*) FROM periods", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 19);
        let _ = fs::remove_dir_all(&dir);
    }
}
