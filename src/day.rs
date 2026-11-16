//! Tagesmodell: automatisch erfasste Perioden plus manuelle Korrekturen.
//!
//! Die automatisch erfassten Perioden (Mac entsperrt) bleiben unverändert erhalten.
//! Manuelle Entscheidungen – Arbeitsbeginn, Pausen, Feierabend – liegen als eigene
//! Schicht darüber; die Arbeitszeit wird aus beidem berechnet. So lässt sich jede
//! Korrektur nachvollziehen und wieder zurücknehmen.
//!
//! Alle Uhrzeiten sind Sekunden seit Mitternacht (`0..=86_400`).

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

/// Eine manuell eingetragene Pause; `to == None` heißt: läuft noch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    pub from: i64,
    pub to: Option<i64>,
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
    /// Pause läuft seit der angegebenen Uhrzeit.
    Break(i64),
    /// Feierabend um die angegebene Uhrzeit.
    Closed(i64),
}

/// Kennzahlen eines Tages (das, was in der Tabelle steht).
#[derive(Clone, Debug, PartialEq)]
pub struct DayRecord {
    pub date: NaiveDate,
    /// Beginn (erstes Entsperren oder manuell festgelegt).
    pub first_start: i64,
    /// Ende (letztes Sperren oder manuell festgelegt).
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

    pub fn open_break(&self) -> Option<&Break> {
        self.breaks.iter().find(|b| b.to.is_none())
    }

    pub fn mode(&self) -> Mode {
        if let Some(e) = self.end {
            return Mode::Closed(e);
        }
        match self.open_break() {
            Some(b) => Mode::Break(b.from),
            None => Mode::Auto,
        }
    }

    pub fn sort_breaks(&mut self) {
        self.breaks.sort_by_key(|b| (b.from, b.to.unwrap_or(DAY_END)));
    }

    /// Effektive Arbeitsabschnitte. `open_until` ist das Ende für noch laufende Pausen
    /// (heute: jetzt, vergangene Tage: Mitternacht).
    pub fn spans(&self, open_until: i64) -> Vec<Span> {
        let mut spans = self.periods.clone();
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
            let to = b.to.unwrap_or(open_until.max(b.from));
            spans = subtract(spans, Span::new(b.from, to));
        }
        spans
    }

    /// Tabellenzeile für diesen Tag (`None`, wenn es nichts anzuzeigen gibt).
    pub fn record(&self, open_until: i64) -> Option<DayRecord> {
        if self.is_empty() {
            return None;
        }
        let spans = self.spans(open_until);
        let work: i64 = spans.iter().map(Span::len).sum();
        let (first, last) = match (spans.first(), spans.last()) {
            // Eine laufende Pause zählt bis jetzt als Pause (nicht erst, wenn wieder gearbeitet wird).
            (Some(f), Some(l)) if self.open_break().is_some() && self.end.is_none() => {
                (f.from, l.to.max(open_until))
            }
            (Some(f), Some(l)) => (f.from, l.to),
            _ => {
                let first = self
                    .start
                    .or(self.periods.first().map(|p| p.from))
                    .or(self.end)
                    .or(self.breaks.first().map(|b| b.from))
                    .unwrap_or(0);
                let last = self
                    .end
                    .or(self.periods.last().map(|p| p.to))
                    .unwrap_or(first)
                    .max(first);
                (first, last)
            }
        };
        Some(DayRecord { date: self.date, first_start: first, last_end: last, work_secs: work })
    }

    /// Summe der Pausen von mindestens 15 Minuten zwischen Arbeitsabschnitten
    /// (so zählt das Arbeitszeitgesetz).
    pub fn long_pause_secs(spans: &[Span]) -> i64 {
        spans
            .windows(2)
            .map(|w| w[1].from - w[0].to)
            .filter(|g| *g >= 15 * 60)
            .sum()
    }

    /// Hinweis nach § 4 ArbZG, falls die Pausen nicht reichen.
    pub fn legal_hint(spans: &[Span]) -> Option<&'static str> {
        let work: i64 = spans.iter().map(Span::len).sum();
        let pause = Self::long_pause_secs(spans);
        if work > 9 * 3600 && pause < 45 * 60 {
            Some("Ab 9 h Arbeit sind 45 Min. Pause vorgeschrieben")
        } else if work > 6 * 3600 && pause < 30 * 60 {
            Some("Ab 6 h Arbeit sind 30 Min. Pause vorgeschrieben")
        } else {
            None
        }
    }

    /// Längste Lücke um die Mittagszeit (11:00–14:30, mindestens 20 Min.), die noch
    /// nicht als Pause eingetragen ist – Vorschlag „als Mittagspause eintragen“.
    pub fn lunch_gap(&self, spans: &[Span]) -> Option<Span> {
        if !self.breaks.is_empty() {
            return None;
        }
        spans
            .windows(2)
            .map(|w| Span::new(w[0].to, w[1].from))
            .filter(|g| g.len() >= 20 * 60 && g.from < 14 * 3600 + 1800 && g.to > 11 * 3600)
            .max_by_key(Span::len)
    }
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

    fn day() -> Day {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d
    }

    fn work(d: &Day) -> i64 {
        d.record(DAY_END).unwrap().work_secs
    }

    #[test]
    fn automatic_only() {
        let d = day();
        assert_eq!(work(&d), h(3, 46) + h(4, 49));
        let r = d.record(DAY_END).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(8, 12), h(17, 30)));
        assert_eq!(r.pause_secs(), h(0, 43));
    }

    #[test]
    fn earlier_start_adds_time() {
        let mut d = day();
        d.start = Some(h(7, 30));
        assert_eq!(work(&d), h(3, 46) + h(4, 49) + h(0, 42));
        assert_eq!(d.record(DAY_END).unwrap().first_start, h(7, 30));
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
        let r = d.record(DAY_END).unwrap();
        assert_eq!((r.first_start, r.work_secs), (h(12, 41), h(4, 49)));
        // Feierabend in der Mittagslücke: die gesperrte Zeit zählt nicht als Arbeit
        let mut d = day();
        d.end = Some(h(12, 20));
        let r = d.record(DAY_END).unwrap();
        assert_eq!((r.last_end, r.work_secs), (h(11, 58), h(3, 46)));
    }

    #[test]
    fn break_overlapping_lock_is_not_counted_twice() {
        let mut d = day();
        d.breaks.push(Break { from: h(12, 0), to: Some(h(12, 45)) });
        // gesperrt war ohnehin 11:58–12:41, zusätzlich entfällt nur 12:41–12:45
        assert_eq!(work(&d), h(3, 46) + h(4, 45));
    }

    #[test]
    fn open_break_runs_until_now() {
        let mut d = day();
        d.breaks.push(Break { from: h(17, 0), to: None });
        assert_eq!(d.mode(), Mode::Break(h(17, 0)));
        let r = d.record(h(17, 30)).unwrap(); // jetzt = 17:30 (Ende der letzten Periode)
        assert_eq!(r.work_secs, h(3, 46) + h(4, 19));
        // die laufende Pause zählt schon jetzt bei den Pausen mit
        let r = d.record(h(17, 50)).unwrap();
        assert_eq!(r.last_end, h(17, 50));
        assert_eq!(r.pause_secs(), h(0, 43) + h(0, 50));
    }

    #[test]
    fn end_clips_and_extends() {
        let mut d = day();
        d.end = Some(h(17, 0));
        assert_eq!(d.mode(), Mode::Closed(h(17, 0)));
        assert_eq!(work(&d), h(3, 46) + h(4, 19));
        d.end = Some(h(18, 0));
        assert_eq!(work(&d), h(3, 46) + h(5, 19));
        assert_eq!(d.record(DAY_END).unwrap().last_end, h(18, 0));
    }

    #[test]
    fn manual_day_without_periods() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap());
        d.start = Some(h(8, 0));
        d.end = Some(h(16, 0));
        d.breaks.push(Break { from: h(12, 0), to: Some(h(12, 30)) });
        let r = d.record(DAY_END).unwrap();
        assert_eq!(r.work_secs, h(7, 30));
        assert_eq!(r.pause_secs(), h(0, 30));
    }

    #[test]
    fn legal_hint_and_lunch_gap() {
        let d = day();
        let spans = d.spans(DAY_END);
        assert_eq!(Day::legal_hint(&spans), None); // 8:35 h Arbeit, 43 Min. Pause
        assert_eq!(d.lunch_gap(&spans), Some(Span::new(h(11, 58), h(12, 41))));
        let mut long = Day::new(d.date);
        long.add_period(h(8, 0), h(15, 0));
        assert!(Day::legal_hint(&long.spans(DAY_END)).is_some());
        assert_eq!(long.lunch_gap(&long.spans(DAY_END)), None);
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
