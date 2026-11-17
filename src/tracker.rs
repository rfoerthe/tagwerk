//! Zeiterfassungs-Logik: automatische Perioden starten/stoppen, manuelle Entscheidungen
//! (Arbeit unterbrechen, Arbeitsbeginn, Feierabend) eintragen und alles auf Tage verteilen.
//!
//! Pausen entstehen durch Sperren des Macs oder über „Arbeit unterbrechen“. Eine Unterbrechung, die
//! mindestens so lange dauert wie die eingestellte Mindestpause, ist eine Pause; kürzere
//! zählen zur Arbeitszeit. Eine Pause über „Arbeit unterbrechen“ endet mit „Weiter arbeiten“ oder
//! beim nächsten Entsperren bzw. Aufwachen des Macs.

use crate::day::{self, Day, Mode, DAY_END};
use crate::rules::{self, Settings, Warning};
use crate::store::{self, Status};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};

/// Warum gerade nicht automatisch erfasst wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    Startup,
    /// Beenden der App, Feierabend o. Ä. (überschreibt keinen anderen Grund).
    Manual,
    /// Der Benutzer hat „Arbeit unterbrechen“ gewählt.
    Break,
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
    pub settings: Settings,
}

/// Kennzahlen für heute (inklusive laufender Periode).
pub struct TodayInfo {
    pub work_secs: i64,
    pub pause_secs: i64,
    /// Effektiver Beginn (manuell oder erstes Entsperren).
    pub first_start: Option<i64>,
    /// Beginn wurde manuell festgelegt.
    pub start_manual: bool,
    /// Erstes automatisches Entsperren heute im Arbeitszeitrahmen.
    pub auto_first: Option<i64>,
    pub mode: Mode,
    /// Sollarbeitszeit heute.
    pub target_secs: i64,
    /// Saldo dieser Woche (Montag bis heute, nur Tage mit Arbeitszeit).
    pub week_balance: i64,
    /// Hinweise zu Verstößen gegen die Regeln.
    pub warnings: Vec<Warning>,
}

fn secs(t: NaiveDateTime) -> i64 {
    day::secs_of(t.time())
}

impl Tracker {
    pub fn new(days: Vec<Day>, settings: Settings) -> Self {
        let saved = days.clone();
        Self {
            days,
            saved,
            running_since: None,
            paused_since: None,
            pause_reason: PauseReason::Startup,
            settings,
        }
    }

    /// Liegt `now` im Arbeitszeitrahmen? Außerhalb wird nichts gezählt.
    pub fn in_frame(&self, now: NaiveDateTime) -> bool {
        self.settings.in_frame(secs(now))
    }

    pub fn is_running(&self) -> bool {
        self.running_since.is_some()
    }

    /// Pause über „Arbeit unterbrechen“ (endet nicht von selbst, nur durch „Weiter arbeiten“ oder
    /// Entsperren/Aufwachen).
    pub fn on_break(&self) -> bool {
        self.running_since.is_none() && self.pause_reason == PauseReason::Break
    }

    /// „Arbeit unterbrechen“: laufende Erfassung anhalten.
    pub fn take_break(&mut self, now: NaiveDateTime) -> bool {
        self.running_since.is_some() && self.stop(now, PauseReason::Break)
    }

    /// Eine Pause über „Arbeit unterbrechen“ fortsetzen, die vor einem Neustart der App begonnen hat.
    pub fn restore_break(&mut self, since: NaiveDateTime) {
        self.running_since = None;
        self.paused_since = Some(since);
        self.pause_reason = PauseReason::Break;
    }

    /// Zustand für die Statusdatei und die Anzeige.
    pub fn status(&self, now: NaiveDateTime) -> Status {
        match self.mode(now) {
            Mode::Closed(_) => Status::Closed,
            Mode::Auto => match self.paused_since {
                Some(since) if self.on_break() => Status::Break(since),
                _ if !self.in_frame(now) => Status::Outside,
                _ if self.is_running() => Status::Running,
                _ => Status::Paused,
            },
        }
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

    /// Modus von heute (Feierabend oder automatisch).
    pub fn mode(&self, now: NaiveDateTime) -> Mode {
        self.day(now.date()).map(Day::mode).unwrap_or(Mode::Auto)
    }

    /// Automatische Erfassung starten. Nach Feierabend passiert nichts.
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

    /// Tageswechsel: laufende Periode an Mitternacht teilen.
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
        let s = &self.settings;
        let date = now.date();
        let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
        let (week_work, week_target) = rules::balance(&snap, monday, date, s);
        let mut info = TodayInfo {
            work_secs: 0,
            pause_secs: 0,
            first_start: None,
            start_manual: false,
            auto_first: None,
            mode: Mode::Auto,
            target_secs: s.daily_target(date),
            week_balance: week_work - week_target,
            warnings: Vec::new(),
        };
        let Some(d) = snap.iter().find(|d| d.date == date) else { return info };
        if let Some(rec) = d.record(s) {
            info.work_secs = rec.work_secs;
            info.pause_secs = rec.pause_secs();
            info.first_start = (rec.work_secs > 0 || d.start.is_some()).then_some(rec.first_start);
        }
        info.start_manual = d.start.is_some();
        info.auto_first = d.auto_bounds_in_frame(s).map(|(first, _)| first);
        info.mode = d.mode();
        info.warnings = rules::day_warnings(&snap, date, s);
        info
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

    /// Feierabend um `at` (höchstens jetzt). Stoppt die Erfassung.
    pub fn close_day(&mut self, now: NaiveDateTime, at: i64) {
        let t = secs(now);
        self.stop(now, PauseReason::Manual);
        self.day_mut(now.date()).end = Some(at.min(t));
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
    pub fn replace_days(&mut self, days: Vec<Day>, settings: Settings, now: NaiveDateTime) {
        self.saved = days.clone();
        self.days = days;
        self.settings = settings;
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
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 30));
        let info = t.today(dt(16, 13, 0));
        assert_eq!(info.work_secs, 4 * 3600 + 30 * 60);
        assert_eq!(info.pause_secs, 30 * 60);
        assert_eq!(info.first_start, Some(hm(8, 0)));
    }

    #[test]
    fn manual_break_until_resumed() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        assert_eq!(t.status(dt(16, 9, 0)), Status::Running);
        assert!(t.take_break(dt(16, 12, 0)));
        assert!(t.on_break());
        assert_eq!(t.status(dt(16, 12, 30)), Status::Break(dt(16, 12, 0)));
        // während der Pause wird nichts gezählt
        assert_eq!(t.today(dt(16, 12, 30)).work_secs, hm(4, 0));
        // App beendet: Grund bleibt „Pause“
        t.stop(dt(16, 12, 31), PauseReason::Manual);
        assert!(t.on_break());
        assert!(t.start(dt(16, 12, 45)));
        assert!(!t.on_break());
        let info = t.today(dt(16, 13, 0));
        assert_eq!(info.work_secs, hm(4, 15));
        assert_eq!(info.pause_secs, hm(0, 45));
        // kürzer als die Mindestpause: zählt als Arbeitszeit
        t.take_break(dt(16, 14, 0));
        t.start(dt(16, 14, 10));
        assert_eq!(t.today(dt(16, 15, 0)).work_secs, hm(6, 15));
    }

    #[test]
    fn lock_during_break_ends_it_on_unlock() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.take_break(dt(16, 12, 0));
        t.stop(dt(16, 12, 5), PauseReason::Locked);
        assert!(!t.on_break());
        assert_eq!(t.status(dt(16, 12, 10)), Status::Paused);
        // beim Entsperren läuft die Erfassung wieder; Pause ab 12:00
        assert!(t.start(dt(16, 12, 40)));
        assert_eq!(t.today(dt(16, 13, 0)).pause_secs, hm(0, 40));
        // Neustart der App während einer Pause
        let mut t = Tracker::new(t.snapshot(dt(16, 13, 0)), Settings::default());
        t.restore_break(dt(16, 13, 0));
        assert_eq!(t.status(dt(16, 13, 5)), Status::Break(dt(16, 13, 0)));
    }

    #[test]
    fn midnight_split() {
        let all_day = Settings { frame_from: 0, frame_to: DAY_END, ..Settings::default() };
        let mut t = Tracker::new(vec![], all_day);
        t.start(dt(16, 23, 0));
        t.roll_over(dt(17, 0, 30));
        t.stop(dt(17, 1, 0), PauseReason::Manual);
        assert_eq!(t.days.len(), 2);
        assert_eq!(t.days[0].record(&all_day).unwrap().work_secs, 3600);
        assert_eq!(t.days[1].record(&all_day).unwrap().work_secs, 3600);
        // im Standard-Rahmen (07:00–22:00) zählt davon nichts
        assert_eq!(t.days[0].record(&Settings::default()).unwrap().work_secs, 0);
    }

    #[test]
    fn short_lock_counts_as_work_long_lock_as_pause() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 10, 0), PauseReason::Locked);
        t.start(dt(16, 10, 10)); // 10 Min. gesperrt
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 40)); // 40 Min. gesperrt
        let info = t.today(dt(16, 13, 0));
        assert_eq!(info.work_secs, hm(4, 20));
        assert_eq!(info.pause_secs, hm(0, 40));
        assert_eq!(info.target_secs, hm(7, 48));
        assert_eq!(info.week_balance, hm(4, 20) - hm(7, 48));
    }

    #[test]
    fn nothing_counted_outside_frame() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 6, 30));
        assert!(!t.in_frame(dt(16, 6, 59)));
        assert!(t.in_frame(dt(16, 7, 0)));
        let info = t.today(dt(16, 6, 50));
        assert_eq!((info.work_secs, info.first_start), (0, None));
        let info = t.today(dt(16, 8, 0));
        assert_eq!((info.work_secs, info.first_start), (hm(1, 0), Some(hm(7, 0))));
        assert!(matches!(&info.warnings[..], [Warning::OutsideFrame { spans, .. }] if spans.len() == 1));
        assert_eq!(info.auto_first, Some(hm(7, 0)));
    }

    #[test]
    fn closed_day_blocks_auto_start_until_reopened() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.close_day(dt(16, 17, 20), hm(17, 0));
        assert!(!t.is_running());
        assert!(!t.start(dt(16, 20, 0)));
        assert_eq!(t.today(dt(16, 20, 0)).work_secs, hm(9, 0));
        t.reopen_day(dt(16, 20, 0));
        assert!(t.start(dt(16, 20, 0)));
        // nächster Tag: wieder automatisch
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.close_day(dt(16, 17, 0), hm(17, 0));
        assert!(t.start(dt(17, 8, 0)));
    }

    #[test]
    fn manual_start_earlier_and_reset() {
        let mut t = Tracker::new(vec![], Settings::default());
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
        let mut t = Tracker::new(vec![old.clone()], Settings::default());
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
}
