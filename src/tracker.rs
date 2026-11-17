//! Zeiterfassungs-Logik: automatische Perioden starten/stoppen, manuelle Entscheidungen
//! (Arbeitsbeginn, Mittagspause, Feierabend) eintragen und alles auf Tage verteilen.

use crate::day::{self, Break, Day, Mode, Span, DAY_END};
use crate::store;
use chrono::{Duration, NaiveDate, NaiveDateTime};

/// Warum gerade nicht automatisch erfasst wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    Startup,
    Manual,
    Locked,
    Sleep,
    SessionInactive,
}

pub struct Tracker {
    pub days: Vec<Day>,
    /// Stand der Datenbank nach dem letzten Laden/Speichern (um nur Änderungen zu schreiben).
    saved: Vec<Day>,
    /// Beginn der laufenden automatischen Periode.
    pub running_since: Option<NaiveDateTime>,
    pub paused_since: Option<NaiveDateTime>,
    pub pause_reason: PauseReason,
}

/// Kennzahlen für heute (inklusive laufender Periode).
pub struct TodayInfo {
    pub work_secs: i64,
    pub pause_secs: i64,
    /// Effektiver Beginn (manuell oder erstes Entsperren).
    pub first_start: Option<i64>,
    /// Beginn wurde manuell festgelegt.
    pub start_manual: bool,
    /// Erstes automatisches Entsperren heute.
    pub auto_first: Option<i64>,
    pub mode: Mode,
    pub legal_hint: Option<&'static str>,
    pub lunch_gap: Option<Span>,
}

fn secs(t: NaiveDateTime) -> i64 {
    day::secs_of(t.time())
}

impl Tracker {
    pub fn new(days: Vec<Day>) -> Self {
        let saved = days.clone();
        Self { days, saved, running_since: None, paused_since: None, pause_reason: PauseReason::Startup }
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    fn day(&self, date: NaiveDate) -> Option<&Day> {
        self.days.iter().find(|d| d.date == date)
    }

    fn day_mut(&mut self, date: NaiveDate) -> &mut Day {
        day::day_mut(&mut self.days, date)
    }

    fn cleanup(&mut self) {
        self.days.retain(|d| !d.is_empty());
    }

    /// Modus von heute (Feierabend, Mittagspause oder automatisch).
    pub fn mode(&self, now: NaiveDateTime) -> Mode {
        self.day(now.date()).map(Day::mode).unwrap_or(Mode::Auto)
    }

    /// Automatische Erfassung starten. Nach Feierabend passiert nichts.
    /// Während einer Mittagspause wird weiter erfasst, die Pause wird aber abgezogen –
    /// so kann ihr Ende auch nachträglich verschoben werden.
    pub fn start(&mut self, now: NaiveDateTime) -> bool {
        if self.running_since.is_some() || matches!(self.mode(now), Mode::Closed(_)) {
            return false;
        }
        self.running_since = Some(now);
        self.paused_since = None;
        true
    }

    pub fn stop(&mut self, now: NaiveDateTime, reason: PauseReason) -> bool {
        let Some(since) = self.running_since.take() else {
            // schon pausiert: nur den Grund ggf. präzisieren
            if reason != PauseReason::Manual {
                self.pause_reason = reason;
            }
            return false;
        };
        Self::commit(&mut self.days, since, now);
        self.paused_since = Some(now);
        self.pause_reason = reason;
        true
    }

    /// Tageswechsel: laufende Periode an Mitternacht teilen, offene Pausen alter Tage schließen.
    /// Gibt `true` zurück, wenn sich etwas geändert hat (dann speichern).
    pub fn roll_over(&mut self, now: NaiveDateTime) -> bool {
        let mut changed = false;
        if let Some(since) = self.running_since {
            if now.date() > since.date() {
                let midnight = now.date().and_hms_opt(0, 0, 0).unwrap();
                Self::commit(&mut self.days, since, midnight);
                self.running_since = Some(midnight);
                changed = true;
            }
        }
        for d in self.days.iter_mut().filter(|d| d.date < now.date()) {
            for b in d.breaks.iter_mut().filter(|b| b.to.is_none()) {
                b.to = Some(DAY_END);
                changed = true;
            }
        }
        changed
    }

    /// Alle Tage inklusive der laufenden Periode bis `now` (für Anzeige und Speichern).
    pub fn snapshot(&self, now: NaiveDateTime) -> Vec<Day> {
        let mut days = self.days.clone();
        if let Some(since) = self.running_since {
            Self::commit(&mut days, since, now);
        }
        days
    }

    pub fn today(&self, now: NaiveDateTime) -> TodayInfo {
        let snap = self.snapshot(now);
        let Some(d) = snap.iter().find(|d| d.date == now.date()) else {
            return TodayInfo {
                work_secs: 0,
                pause_secs: 0,
                first_start: None,
                start_manual: false,
                auto_first: None,
                mode: Mode::Auto,
                legal_hint: None,
                lunch_gap: None,
            };
        };
        let spans = d.spans(secs(now));
        let rec = d.record(secs(now));
        let mode = d.mode();
        TodayInfo {
            work_secs: rec.as_ref().map(|r| r.work_secs).unwrap_or(0),
            pause_secs: rec.as_ref().map(|r| r.pause_secs()).unwrap_or(0),
            first_start: rec.as_ref().map(|r| r.first_start),
            start_manual: d.start.is_some(),
            auto_first: d.periods.first().map(|p| p.from),
            mode,
            legal_hint: if mode == Mode::Auto { Day::legal_hint(&spans) } else { None },
            lunch_gap: d.lunch_gap(&spans),
        }
    }

    /// Speichert nur die Tage, die sich seit dem letzten Laden/Speichern geändert haben,
    /// damit Korrekturen aus dem Fenster „Aufzeichnungen“ an anderen Tagen erhalten bleiben.
    pub fn save(&mut self, db: &mut store::Db, now: NaiveDateTime) -> store::Result<()> {
        let snap = self.snapshot(now);
        let (changed, removed) = Self::diff(&self.saved, &snap);
        db.save_days(&changed, &removed, now)?;
        self.saved = snap;
        Ok(())
    }

    /// Geänderte bzw. neue Tage und Tage, die es nicht mehr gibt.
    fn diff(old: &[Day], new: &[Day]) -> (Vec<Day>, Vec<NaiveDate>) {
        let changed = new
            .iter()
            .filter(|d| old.iter().find(|o| o.date == d.date) != Some(*d))
            .cloned()
            .collect();
        let removed = old
            .iter()
            .filter(|o| !new.iter().any(|d| d.date == o.date))
            .map(|o| o.date)
            .collect();
        (changed, removed)
    }

    // ---------- manuelle Entscheidungen (immer für heute) ----------

    /// Arbeitsbeginn festlegen (`None` = wieder das erste Entsperren verwenden).
    pub fn set_start(&mut self, now: NaiveDateTime, at: Option<i64>) {
        let at = at.map(|a| a.min(secs(now)));
        self.day_mut(now.date()).start = at;
        self.cleanup();
    }

    /// Mittagspause ab `from` beginnen. Gibt `false` zurück, wenn schon eine läuft
    /// oder Feierabend ist.
    pub fn start_break(&mut self, now: NaiveDateTime, from: i64) -> bool {
        if self.mode(now) != Mode::Auto {
            return false;
        }
        let from = from.min(secs(now));
        let d = self.day_mut(now.date());
        d.breaks.push(Break { from, to: None });
        d.sort_breaks();
        true
    }

    /// Laufende Mittagspause jetzt beenden.
    pub fn end_break(&mut self, now: NaiveDateTime) -> bool {
        let t = secs(now);
        let Some(d) = self.days.iter_mut().find(|d| d.date == now.date()) else { return false };
        match d.breaks.iter_mut().find(|b| b.to.is_none()) {
            Some(b) => {
                b.to = Some(t.max(b.from));
                true
            }
            None => false,
        }
    }

    /// Lücke nachträglich als Pause eintragen (Vorschlag aus dem Menü).
    pub fn add_break(&mut self, now: NaiveDateTime, from: i64, to: i64) {
        let d = self.day_mut(now.date());
        d.breaks.push(Break { from, to: Some(to.max(from)) });
        d.sort_breaks();
    }

    /// Feierabend um `at` (höchstens jetzt). Stoppt die Erfassung und beendet eine laufende Pause.
    pub fn close_day(&mut self, now: NaiveDateTime, at: i64) {
        let t = secs(now);
        self.stop(now, PauseReason::Manual);
        let d = self.day_mut(now.date());
        for b in d.breaks.iter_mut().filter(|b| b.to.is_none()) {
            b.to = Some(t.max(b.from));
        }
        d.end = Some(at.min(t));
    }

    /// Feierabend zurücknehmen. Die Erfassung startet der Aufrufer (falls entsperrt).
    pub fn reopen_day(&mut self, now: NaiveDateTime) {
        if let Some(d) = self.days.iter_mut().find(|d| d.date == now.date()) {
            d.end = None;
        }
        self.cleanup();
    }

    /// Nach einer Änderung der Datenbank von außen (Fenster „Aufzeichnungen“) neu einlesen.
    /// Die laufende Periode bleibt erhalten; doppelt gespeicherte Zeiten werden zusammengefasst.
    pub fn replace_days(&mut self, days: Vec<Day>, now: NaiveDateTime) {
        self.saved = days.clone();
        self.days = days;
        if self.running_since.is_some() && matches!(self.mode(now), Mode::Closed(_)) {
            self.stop(now, PauseReason::Manual);
        }
    }

    fn commit(days: &mut Vec<Day>, start: NaiveDateTime, end: NaiveDateTime) {
        if end < start {
            return;
        }
        let mut s = start;
        while s.date() < end.date() {
            day::day_mut(days, s.date()).add_period(secs(s), DAY_END);
            s = (s.date() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap();
        }
        day::day_mut(days, s.date()).add_period(secs(s), secs(end));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(d: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    fn hm(h: i64, m: i64) -> i64 {
        h * 3600 + m * 60
    }

    #[test]
    fn work_and_pause() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 30));
        let info = t.today(dt(16, 13, 0));
        assert_eq!(info.work_secs, 4 * 3600 + 30 * 60);
        assert_eq!(info.pause_secs, 30 * 60);
        assert_eq!(info.first_start, Some(hm(8, 0)));
    }

    #[test]
    fn midnight_split() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 23, 0));
        t.roll_over(dt(17, 0, 30));
        t.stop(dt(17, 1, 0), PauseReason::Manual);
        assert_eq!(t.days.len(), 2);
        assert_eq!(t.days[0].record(DAY_END).unwrap().work_secs, 3600);
        assert_eq!(t.days[1].record(DAY_END).unwrap().work_secs, 3600);
    }

    #[test]
    fn lunch_break_survives_lock_and_can_end_later() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 8, 0));
        assert!(t.start_break(dt(16, 12, 0), hm(12, 0)));
        assert!(!t.start_break(dt(16, 12, 1), hm(12, 1)));
        t.stop(dt(16, 12, 2), PauseReason::Locked);
        assert!(t.start(dt(16, 12, 40))); // entsperrt: Rohdaten laufen weiter
        assert_eq!(t.mode(dt(16, 12, 40)), Mode::Break(hm(12, 0)));
        assert_eq!(t.today(dt(16, 12, 50)).work_secs, hm(4, 0));
        t.end_break(dt(16, 12, 50));
        assert_eq!(t.mode(dt(16, 12, 50)), Mode::Auto);
        assert_eq!(t.today(dt(16, 13, 0)).work_secs, hm(4, 10));
        assert_eq!(t.today(dt(16, 13, 0)).pause_secs, hm(0, 50));
    }

    #[test]
    fn closed_day_blocks_auto_start_until_reopened() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 8, 0));
        t.close_day(dt(16, 17, 20), hm(17, 0));
        assert!(!t.is_running());
        assert!(!t.start(dt(16, 20, 0)));
        assert_eq!(t.today(dt(16, 20, 0)).work_secs, hm(9, 0));
        t.reopen_day(dt(16, 20, 0));
        assert!(t.start(dt(16, 20, 0)));
        // nächster Tag: wieder automatisch
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 8, 0));
        t.close_day(dt(16, 17, 0), hm(17, 0));
        assert!(t.start(dt(17, 8, 0)));
    }

    #[test]
    fn manual_start_earlier_and_reset() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 8, 12));
        t.set_start(dt(16, 9, 0), Some(hm(7, 30)));
        let info = t.today(dt(16, 9, 0));
        assert_eq!(info.work_secs, hm(1, 30));
        assert!(info.start_manual);
        assert_eq!(info.auto_first, Some(hm(8, 12)));
        t.set_start(dt(16, 9, 0), None);
        assert_eq!(t.today(dt(16, 9, 0)).work_secs, hm(0, 48));
    }

    #[test]
    fn save_writes_only_changed_days() {
        let mut old = Day::new(NaiveDate::from_ymd_opt(2026, 11, 10).unwrap());
        old.add_period(hm(8, 0), hm(9, 0));
        let mut t = Tracker::new(vec![old.clone()]);
        t.start(dt(16, 8, 0));
        let snap = t.snapshot(dt(16, 9, 0));
        let (changed, removed) = Tracker::diff(&t.saved, &snap);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].date, dt(16, 0, 0).date());
        assert!(removed.is_empty());
        let (changed, removed) = Tracker::diff(&snap, &snap[1..]);
        assert!(changed.is_empty());
        assert_eq!(removed, vec![old.date]);
    }

    #[test]
    fn open_break_closed_at_midnight() {
        let mut t = Tracker::new(vec![]);
        t.start(dt(16, 20, 0));
        t.start_break(dt(16, 21, 0), hm(21, 0));
        t.stop(dt(16, 21, 0), PauseReason::Locked);
        t.roll_over(dt(17, 7, 0));
        assert_eq!(t.days[0].breaks[0].to, Some(DAY_END));
        assert_eq!(t.mode(dt(17, 7, 0)), Mode::Auto);
    }
}
