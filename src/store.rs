//! Speicherung der Arbeitszeiten als Markdown-Datei plus tägliche Backups.
//!
//! Die Datei hat drei Abschnitte:
//! 1. Übersichtstabelle (berechnet, zum Lesen),
//! 2. `## Korrekturen` – manuelle Entscheidungen (Arbeitsbeginn, Pausen, Feierabend),
//! 3. `## Erfasste Perioden` – automatisch erfasste Zeiten (Mac entsperrt).
//!
//! Dateien früherer Versionen enthalten nur die Übersicht; daraus werden beim Lesen
//! Perioden abgeleitet (Pause in der Mitte des Tages angenommen).

use crate::day::{self, fmt_clock_s, parse_clock, Break, Day, Mode, DAY_END};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

/// Wie viele tägliche Backups aufbewahrt werden.
pub const MAX_BACKUPS: usize = 15;
const FILE_NAME: &str = "tagwerk.md";

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

/// Übernimmt einmalig die Daten der früheren Version „TrackMyWork“
/// (`Application Support/TrackMyWork/track-my-work.md` und deren Backups).
/// Der alte Ordner bleibt unverändert liegen.
pub fn migrate_legacy() {
    if std::env::var_os("TAGWERK_DIR").is_some() || data_file().exists() {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let old_dir = home.join("Library").join("Application Support").join("TrackMyWork");
    let old_file = old_dir.join("track-my-work.md");
    if !old_file.exists() {
        return;
    }
    let result = (|| -> io::Result<()> {
        fs::create_dir_all(backup_dir())?;
        if let Ok(entries) = fs::read_dir(old_dir.join("backups")) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(rest) = name.strip_prefix("track-my-work-") {
                    let target = backup_dir().join(format!("tagwerk-{rest}"));
                    if !target.exists() {
                        fs::copy(e.path(), target)?;
                    }
                }
            }
        }
        let days = parse_file(&fs::read_to_string(&old_file)?);
        let tmp = data_dir().join(format!(".{FILE_NAME}.tmp"));
        fs::write(&tmp, render_file(&days, chrono::Local::now().naive_local()))?;
        fs::rename(&tmp, data_file())?;
        Ok(())
    })();
    if let Err(e) = result {
        eprintln!("Tagwerk: alte Daten konnten nicht übernommen werden: {e}");
    }
}

pub fn data_file() -> PathBuf {
    data_dir().join(FILE_NAME)
}

pub fn backup_dir() -> PathBuf {
    data_dir().join("backups")
}

/// Kleine Statusdatei, damit das Fenster "Aufzeichnungen" weiß, ob gerade erfasst wird.
pub fn status_file() -> PathBuf {
    data_dir().join(".status")
}

/// Änderungszeit der Datendatei (um Änderungen von außen zu erkennen).
pub fn file_mtime() -> Option<SystemTime> {
    fs::metadata(data_file()).and_then(|m| m.modified()).ok()
}

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

#[derive(PartialEq)]
enum Section {
    Summary,
    Corrections,
    Periods,
    Other,
}

/// Liest alle Tage aus dem Markdown-Text (unbekannte Zeilen werden ignoriert).
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

/// `open_until` für einen Tag: heute die aktuelle Uhrzeit, sonst Mitternacht.
pub fn open_until(date: NaiveDate, now: NaiveDateTime) -> i64 {
    if date == now.date() {
        day::secs_of(now.time())
    } else {
        DAY_END
    }
}

pub fn render_file(days: &[Day], now: NaiveDateTime) -> String {
    let mut s = String::new();
    s.push_str("# Tagwerk – Arbeitszeiten\n\n");
    s.push_str(
        "Arbeitszeit = Zeit, in der der Mac entsperrt war, verrechnet mit den manuellen \
         Korrekturen. Pausenzeit = Zeit zwischen Beginn und Ende, in der nicht gearbeitet wurde.\n\
         Diese Übersicht wird berechnet. Änderungen bitte über Tagwerk (Fenster „Aufzeichnungen“) \
         oder im Abschnitt „Korrekturen“ vornehmen.\n\n",
    );
    s.push_str("| Datum | Beginn | Ende | Arbeitszeit | Pausenzeit |\n");
    s.push_str("|:------|:------:|:----:|------------:|-----------:|\n");
    for d in days {
        let Some(r) = d.record(open_until(d.date, now)) else { continue };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            fmt_date(r.date),
            fmt_clock_s(r.first_start.min(DAY_END - 1)),
            fmt_clock_s(r.last_end.min(DAY_END - 1)),
            fmt_hms(r.work_secs),
            fmt_hms(r.pause_secs()),
        ));
    }

    s.push_str(&format!("\n{HEAD_CORRECTIONS}\n\n"));
    s.push_str("Manuell festgelegt. Art: Arbeitsbeginn, Pause oder Feierabend; „läuft“ = Pause noch nicht beendet.\n\n");
    s.push_str("| Datum | Art | Von | Bis |\n");
    s.push_str("|:------|:----|:---:|:---:|\n");
    for d in days {
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
    s.push_str("Automatisch erfasst (Mac entsperrt, Tagwerk aktiv). Bitte nicht von Hand ändern.\n\n");
    s.push_str("| Datum | Von | Bis |\n");
    s.push_str("|:------|:---:|:---:|\n");
    for d in days {
        let date = fmt_date(d.date);
        for p in &d.periods {
            s.push_str(&format!("| {date} | {} | {} |\n", fmt_clock_s(p.from), fmt_clock_s(p.to)));
        }
    }
    s
}

pub fn load() -> io::Result<Vec<Day>> {
    match fs::read_to_string(data_file()) {
        Ok(text) => Ok(parse_file(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Schreibt die Datei. Vor der ersten Änderung eines Tages wird die bisherige Datei
/// als `backups/tagwerk-JJJJ-MM-TT.md` gesichert; es bleiben die letzten 15 Backups.
pub fn save(days: &[Day], now: NaiveDateTime) -> io::Result<()> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let file = data_file();
    if file.exists() {
        let bdir = backup_dir();
        let backup = bdir.join(format!("tagwerk-{}.md", now.date().format("%Y-%m-%d")));
        if !backup.exists() {
            fs::create_dir_all(&bdir)?;
            fs::copy(&file, &backup)?;
            prune_backups()?;
        }
    }
    let tmp = dir.join(format!(".{FILE_NAME}.tmp"));
    fs::write(&tmp, render_file(days, now))?;
    fs::rename(&tmp, &file)?;
    Ok(())
}

/// Liest die Datei, ändert einen Tag und schreibt sie sofort zurück
/// (für das Fenster „Aufzeichnungen“, damit nichts Älteres überschrieben wird).
pub fn edit_day(date: NaiveDate, now: NaiveDateTime, f: impl FnOnce(&mut Day)) -> io::Result<()> {
    let mut days = load()?;
    f(day::day_mut(&mut days, date));
    days.retain(|d| !d.is_empty());
    save(&days, now)
}

/// Löscht alle bis auf die neuesten [`MAX_BACKUPS`] Backups.
pub fn prune_backups() -> io::Result<()> {
    let mut names: Vec<PathBuf> = fs::read_dir(backup_dir())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tagwerk-") && n.ends_with(".md"))
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
    use crate::day::Span;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn now() -> NaiveDateTime {
        date(17).and_hms_opt(10, 0, 0).unwrap()
    }

    #[test]
    fn roundtrip_with_corrections() {
        let mut d = Day::new(date(16));
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d.start = Some(h(8, 0));
        d.breaks.push(Break { from: h(12, 0), to: Some(h(12, 45)) });
        d.end = Some(h(17, 15));
        let mut open = Day::new(date(17));
        open.add_period(h(8, 0), h(9, 30));
        open.breaks.push(Break { from: h(9, 30), to: None });
        let days = vec![d, open];
        let md = render_file(&days, now());
        assert!(md.contains("| Mo, 16.11.2026 | 08:00:00 | 17:15:00 | 08:28:00 | 00:47:00 |"), "{md}");
        assert!(md.contains("| Mo, 16.11.2026 | Pause | 12:00:00 | 12:45:00 |"));
        assert!(md.contains("| Di, 17.11.2026 | Pause | 09:30:00 | läuft |"));
        assert_eq!(parse_file(&md), days);
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
        // nach dem Speichern bleiben die Werte gleich
        assert_eq!(parse_file(&render_file(&days, now())), days);
    }

    #[test]
    fn daily_backup_keeps_last_15() {
        let dir = std::env::temp_dir().join(format!("tagwerk-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("TAGWERK_DIR", &dir);
        let start = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        for i in 0..20 {
            let day = start + chrono::Duration::days(i);
            let mut d = Day::new(day);
            d.add_period(h(8, 0), h(9, 0));
            let t = day.and_hms_opt(12, 0, 0).unwrap();
            save(&[d.clone()], t).unwrap();
            save(&[d], t).unwrap(); // zweites Speichern am selben Tag: kein neues Backup
        }
        let count = fs::read_dir(backup_dir()).unwrap().count();
        assert_eq!(count, MAX_BACKUPS);
        assert!(backup_dir().join("tagwerk-2026-10-20.md").exists());
        assert!(!backup_dir().join("tagwerk-2026-10-05.md").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
