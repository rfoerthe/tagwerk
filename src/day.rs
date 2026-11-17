//! Tagesmodell: automatisch erfasste Perioden plus manuelle Korrekturen.
//!
//! Die automatisch erfassten Perioden (Mac entsperrt) bleiben unverändert erhalten.
//! Manuelle Entscheidungen – Arbeitsbeginn, Pausen, Feierabend – liegen als eigene
//! Schicht darüber; die Arbeitszeit wird aus beidem und den Einstellungen berechnet:
//!
//! 1. Lücken zwischen Perioden, die kürzer als die Mindestpause sind, zählen als Arbeit.
//! 2. Manueller Arbeitsbeginn und Feierabend verlängern bzw. kürzen den Tag.
//! 3. Manuell eingetragene Pausen werden abgezogen.
//! 4. Gezählt wird nur, was im Arbeitszeitrahmen liegt.
//!
//! So lässt sich jede Korrektur nachvollziehen und wieder zurücknehmen; geänderte
//! Einstellungen wirken auch auf frühere Tage.
//!
//! Alle Uhrzeiten sind Sekunden seit Mitternacht (`0..=86_400`).

use crate::rules::{Settings, LEGAL_BREAK_SEGMENT};
use chrono::{NaiveDate, NaiveTime, Timelike};

/// Mitternacht am Ende des Tages (24:00:00).
pub const DAY_END: i64 = 86_400;

/// Ein Zeitabschnitt `[from, to)` innerhalb eines Tages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: i64,
    pub to: i64,
}

impl Span {
    pub fn new(from: i64, to: i64) -> Self {
        Self { from, to }
    }
    pub fn len(&self) -> i64 {
        (self.to - self.from).max(0)
    }
}

/// Eine manuell eingetragene Pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    pub from: i64,
    pub to: i64,
}

/// Ein Arbeitstag mit Rohdaten und Korrekturen.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    /// Automatisch erfasst (Mac entsperrt), sortiert und überschneidungsfrei.
    pub periods: Vec<Span>,
    /// Manuell festgelegter Arbeitsbeginn.
    pub start: Option<i64>,
    /// Manuell festgelegter Feierabend.
    pub end: Option<i64>,
    /// Manuell eingetragene Pausen.
    pub breaks: Vec<Break>,
}

/// Zustand eines Tages aus Sicht der manuellen Entscheidungen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Normale, automatische Erfassung.
    Auto,
    /// Feierabend um die angegebene Uhrzeit.
    Closed(i64),
}

/// Kennzahlen eines Tages (das, was in der Tabelle steht).
#[derive(Clone, Debug, PartialEq)]
pub struct DayRecord {
    pub date: NaiveDate,
    /// Beginn (erstes Entsperren oder manuell festgelegt, frühestens Beginn des Rahmens).
    pub first_start: i64,
    /// Ende (letztes Sperren oder manuell festgelegt, spätestens Ende des Rahmens).
    pub last_end: i64,
    /// Arbeitszeit in Sekunden.
    pub work_secs: i64,
}

impl DayRecord {
    /// Pausen = Zeit zwischen Beginn und Ende, in der nicht gearbeitet wurde.
    pub fn pause_secs(&self) -> i64 {
        ((self.last_end - self.first_start) - self.work_secs).max(0)
    }
}

impl Day {
    pub fn new(date: NaiveDate) -> Self {
        Self { date, periods: Vec::new(), start: None, end: None, breaks: Vec::new() }
    }

    pub fn has_corrections(&self) -> bool {
        self.start.is_some() || self.end.is_some() || !self.breaks.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.periods.is_empty() && !self.has_corrections()
    }

    /// Fügt eine automatisch erfasste Periode hinzu (überlappende werden zusammengefasst).
    pub fn add_period(&mut self, from: i64, to: i64) {
        if to < from {
            return;
        }
        self.periods.push(Span::new(from.clamp(0, DAY_END), to.clamp(0, DAY_END)));
        self.periods = union(std::mem::take(&mut self.periods));
    }

    pub fn mode(&self) -> Mode {
        match self.end {
            Some(e) => Mode::Closed(e),
            None => Mode::Auto,
        }
    }

    pub fn sort_breaks(&mut self) {
        self.breaks.sort_by_key(|b| (b.from, b.to));
    }

    /// Arbeitsabschnitte ohne den Arbeitszeitrahmen (Schritte 1–3).
    fn unframed_spans(&self, min_pause: i64) -> Vec<Span> {
        let mut spans = fill_gaps(self.periods.clone(), min_pause);
        // Manuelle Angaben fügen nur vor dem ersten Entsperren bzw. nach dem letzten Sperren
        // Zeit hinzu – nie in Lücken dazwischen (das wäre z. B. die gesperrte Mittagszeit).
        if let Some(s) = self.start {
            // Früherer Beginn: die Zeit bis zum ersten Entsperren zählt mit.
            let first = spans.first().map(|x| x.from).unwrap_or(self.end.unwrap_or(s));
            if s < first {
                spans.insert(0, Span::new(s, first));
            }
            spans = clip(spans, s, DAY_END);
        }
        if let Some(e) = self.end {
            // Späterer Feierabend: die Zeit nach dem letzten Sperren zählt mit.
            let last = spans.last().map(|x| x.to).unwrap_or(self.start.unwrap_or(e));
            if e > last {
                spans.push(Span::new(last, e));
            }
            spans = clip(spans, 0, e);
        }
        let mut spans = union(spans);
        for b in &self.breaks {
            spans = subtract(spans, Span::new(b.from, b.to));
        }
        spans
    }

    /// Effektive Arbeitsabschnitte nach den Einstellungen `s`.
    pub fn spans(&self, s: &Settings) -> Vec<Span> {
        clip(self.unframed_spans(s.min_pause), s.frame_from, s.frame_to)
    }

    /// Arbeitszeit außerhalb des Arbeitszeitrahmens (wird nicht gezählt).
    pub fn outside_frame_spans(&self, s: &Settings) -> Vec<Span> {
        let all = self.unframed_spans(s.min_pause);
        let mut out = clip(all.clone(), 0, s.frame_from);
        out.extend(clip(all, s.frame_to, DAY_END));
        out
    }

    /// Erstes Entsperren und letztes Sperren im Arbeitszeitrahmen (ohne manuelle Angaben;
    /// kurze Sperren zählen wie überall zur Arbeitszeit).
    pub fn auto_bounds_in_frame(&self, s: &Settings) -> Option<(i64, i64)> {
        let spans = clip(fill_gaps(self.periods.clone(), s.min_pause), s.frame_from, s.frame_to);
        Some((spans.first()?.from, spans.last()?.to))
    }

    /// Tabellenzeile für diesen Tag (`None`, wenn es nichts anzuzeigen gibt).
    pub fn record(&self, s: &Settings) -> Option<DayRecord> {
        if self.is_empty() {
            return None;
        }
        let spans = self.spans(s);
        let work: i64 = spans.iter().map(Span::len).sum();
        let (first, last) = match (spans.first(), spans.last()) {
            (Some(f), Some(l)) => (f.from, l.to),
            // Nichts im Rahmen: weder Arbeit noch Pause.
            _ => {
                let first = self
                    .start
                    .or(self.periods.first().map(|p| p.from))
                    .or(self.end)
                    .or(self.breaks.first().map(|b| b.from))
                    .unwrap_or(0)
                    .clamp(s.frame_from, s.frame_to);
                (first, first)
            }
        };
        Some(DayRecord { date: self.date, first_start: first, last_end: last, work_secs: work })
    }

    /// Summe der Pausen von mindestens 15 Minuten zwischen Arbeitsabschnitten
    /// (so zählt das Arbeitszeitgesetz).
    pub fn legal_pause_secs(spans: &[Span]) -> i64 {
        spans
            .windows(2)
            .map(|w| w[1].from - w[0].to)
            .filter(|g| *g >= LEGAL_BREAK_SEGMENT)
            .sum()
    }
}

/// Schließt Lücken, die kürzer als `min_pause` sind: der Mac war nur kurz gesperrt,
/// die Zeit zählt zum laufenden Arbeitsblock.
pub fn fill_gaps(spans: Vec<Span>, min_pause: i64) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in union(spans) {
        match out.last_mut() {
            Some(last) if s.from - last.to < min_pause => last.to = last.to.max(s.to),
            _ => out.push(s),
        }
    }
    out
}

/// Holt (oder legt an) den Tag `date`; die Liste bleibt nach Datum sortiert.
pub fn day_mut(days: &mut Vec<Day>, date: NaiveDate) -> &mut Day {
    let idx = match days.binary_search_by_key(&date, |d| d.date) {
        Ok(i) => i,
        Err(i) => {
            days.insert(i, Day::new(date));
            i
        }
    };
    &mut days[idx]
}

/// Sortiert und fasst überlappende/angrenzende Abschnitte zusammen.
pub fn union(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.from, s.to));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match out.last_mut() {
            Some(last) if s.from <= last.to => last.to = last.to.max(s.to),
            _ => out.push(s),
        }
    }
    out
}

fn clip(spans: Vec<Span>, lo: i64, hi: i64) -> Vec<Span> {
    spans
        .into_iter()
        .filter_map(|s| {
            let c = Span::new(s.from.max(lo), s.to.min(hi));
            (c.to > c.from).then_some(c)
        })
        .collect()
}

fn subtract(spans: Vec<Span>, cut: Span) -> Vec<Span> {
    if cut.to <= cut.from {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len() + 1);
    for s in spans {
        if s.to <= cut.from || s.from >= cut.to {
            out.push(s);
            continue;
        }
        if s.from < cut.from {
            out.push(Span::new(s.from, cut.from));
        }
        if s.to > cut.to {
            out.push(Span::new(cut.to, s.to));
        }
    }
    out
}

// ---------- Uhrzeiten ----------

pub fn secs_of(t: NaiveTime) -> i64 {
    t.num_seconds_from_midnight() as i64
}

/// `08:05` (24:00 möglich).
pub fn fmt_clock(secs: i64) -> String {
    let s = secs.clamp(0, DAY_END);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// `08:05:30` (24:00:00 möglich).
pub fn fmt_clock_s(secs: i64) -> String {
    let s = secs.clamp(0, DAY_END);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Liest `8:05`, `08:05`, `08:05:30`, `8.05` oder `24:00`.
pub fn parse_clock(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches("Uhr").trim();
    let parts: Vec<&str> = s.split([':', '.']).collect();
    let nums: Vec<i64> = parts.iter().map(|p| p.trim().parse::<i64>().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = match nums.as_slice() {
        [h, m] => (*h, *m, 0),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    if !(0..=24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&sec) {
        return None;
    }
    let total = h * 3600 + m * 60 + sec;
    (total <= DAY_END).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn st() -> Settings {
        Settings::default()
    }

    fn day() -> Day {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d
    }

    fn work(d: &Day) -> i64 {
        d.record(&st()).unwrap().work_secs
    }

    #[test]
    fn automatic_only() {
        let d = day();
        assert_eq!(work(&d), h(3, 46) + h(4, 49));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(8, 12), h(17, 30)));
        assert_eq!(r.pause_secs(), h(0, 43));
    }

    #[test]
    fn short_locks_count_as_work() {
        let mut d = day();
        d.periods = vec![Span::new(h(8, 0), h(10, 0)), Span::new(h(10, 14), h(12, 0)), Span::new(h(12, 15), h(16, 0))];
        // 14 Min. gesperrt: Arbeit; 15 Min.: Pause
        let r = d.record(&st()).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(7, 45), h(0, 15)));
        // mit 20 Min. Mindestpause zählen beide Lücken als Arbeit
        let s = Settings { min_pause: 20 * 60, ..st() };
        let r = d.record(&s).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(8, 0), 0));
        // Rohdaten bleiben unverändert
        assert_eq!(d.periods.len(), 3);
    }

    #[test]
    fn frame_clips_work_and_pause() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(6, 0), h(8, 0));
        d.add_period(h(21, 0), h(23, 30));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(7, 0), h(22, 0)));
        assert_eq!(r.work_secs, h(2, 0));
        assert_eq!(
            d.outside_frame_spans(&st()),
            vec![Span::new(h(6, 0), h(7, 0)), Span::new(h(22, 0), h(23, 30))]
        );
        assert_eq!(d.auto_bounds_in_frame(&st()), Some((h(7, 0), h(22, 0))));
        // nur außerhalb: weder Arbeit noch Pause
        let mut late = Day::new(d.date);
        late.add_period(h(22, 30), h(23, 30));
        let r = late.record(&st()).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (0, 0));
        assert_eq!(late.auto_bounds_in_frame(&st()), None);
        // manueller Beginn vor dem Rahmen zählt erst ab Rahmenbeginn
        let mut early = day();
        early.start = Some(h(6, 30));
        assert_eq!(early.record(&st()).unwrap().first_start, h(7, 0));
        assert_eq!(early.outside_frame_spans(&st()), vec![Span::new(h(6, 30), h(7, 0))]);
    }

    #[test]
    fn earlier_start_adds_time() {
        let mut d = day();
        d.start = Some(h(7, 30));
        assert_eq!(work(&d), h(3, 46) + h(4, 49) + h(0, 42));
        assert_eq!(d.record(&st()).unwrap().first_start, h(7, 30));
    }

    #[test]
    fn later_start_cuts_time() {
        let mut d = day();
        d.start = Some(h(8, 30));
        assert_eq!(work(&d), h(3, 28) + h(4, 49));
    }

    #[test]
    fn start_and_end_inside_gap_add_no_time() {
        // Beginn in der Mittagslücke (gesperrt 11:58–12:41): Arbeit ab 12:41
        let mut d = day();
        d.start = Some(h(12, 30));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.work_secs), (h(12, 41), h(4, 49)));
        // Feierabend in der Mittagslücke: die gesperrte Zeit zählt nicht als Arbeit
        let mut d = day();
        d.end = Some(h(12, 20));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.last_end, r.work_secs), (h(11, 58), h(3, 46)));
    }

    #[test]
    fn break_overlapping_lock_is_not_counted_twice() {
        let mut d = day();
        d.breaks.push(Break { from: h(12, 0), to: h(12, 45) });
        // gesperrt war ohnehin 11:58–12:41, zusätzlich entfällt nur 12:41–12:45
        assert_eq!(work(&d), h(3, 46) + h(4, 45));
    }

    #[test]
    fn end_clips_and_extends() {
        let mut d = day();
        d.end = Some(h(17, 0));
        assert_eq!(d.mode(), Mode::Closed(h(17, 0)));
        assert_eq!(work(&d), h(3, 46) + h(4, 19));
        d.end = Some(h(18, 0));
        assert_eq!(work(&d), h(3, 46) + h(5, 19));
        assert_eq!(d.record(&st()).unwrap().last_end, h(18, 0));
    }

    #[test]
    fn manual_day_without_periods() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap());
        d.start = Some(h(8, 0));
        d.end = Some(h(16, 0));
        d.breaks.push(Break { from: h(12, 0), to: h(12, 30) });
        let r = d.record(&st()).unwrap();
        assert_eq!(r.work_secs, h(7, 30));
        assert_eq!(r.pause_secs(), h(0, 30));
    }

    #[test]
    fn legal_pause_counts_segments_from_15_minutes() {
        let spans = vec![Span::new(h(8, 0), h(10, 0)), Span::new(h(10, 14), h(12, 0)), Span::new(h(12, 30), h(15, 0))];
        assert_eq!(Day::legal_pause_secs(&spans), h(0, 30));
    }

    #[test]
    fn periods_merge() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(8, 0), h(9, 0));
        d.add_period(h(8, 0), h(9, 30)); // erneut gespeicherte laufende Periode
        d.add_period(h(10, 0), h(11, 0));
        assert_eq!(d.periods, vec![Span::new(h(8, 0), h(9, 30)), Span::new(h(10, 0), h(11, 0))]);
    }

    #[test]
    fn clock_parsing() {
        assert_eq!(parse_clock("8:05"), Some(h(8, 5)));
        assert_eq!(parse_clock("08.05 Uhr"), Some(h(8, 5)));
        assert_eq!(parse_clock("24:00:00"), Some(DAY_END));
        assert_eq!(parse_clock("24:01"), None);
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(fmt_clock_s(DAY_END), "24:00:00");
    }
}
