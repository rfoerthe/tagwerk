//! Zeiterfassungs-Logik: automatische Perioden starten/stoppen, manuelle Entscheidungen
//! (Arbeit unterbrechen, Arbeitsbeginn, Feierabend) eintragen und alles auf Tage verteilen.
//!
//! Pausen entstehen durch Sperren des Macs oder über „Arbeit unterbrechen“. Eine Unterbrechung, die
//! mindestens so lange dauert wie die eingestellte Mindestpause, ist eine Pause; kürzere
//! zählen zur Arbeitszeit. Eine Pause über „Arbeit unterbrechen“ endet mit „Weiter arbeiten“ oder
//! beim nächsten Entsperren bzw. Aufwachen des Macs.
//!
//! „Ohne Mac weiterarbeiten“ (Meeting, Telefonat …) zählt die Zeit bis zum nächsten Entsperren
//! bzw. bis „Zurück am Mac“ als Arbeit ohne Mac – auch wenn der Mac dazwischen gesperrt ist.
//! Spätestens um Mitternacht oder mit dem Feierabend endet es, damit ein vergessenes „Zurück am
//! Mac“ nicht tagelang Arbeitszeit einträgt.
//!
//! An einem Tag mit ganztägiger Abwesenheit (Urlaub, Krank, Feiertag, Zeitausgleich) wird
//! nichts erfasst, und die manuellen Entscheidungen greifen nicht – bis die Abwesenheit im
//! Fenster „Aufzeichnungen“ wieder auf „Keine“ steht.

use crate::day::{self, Absence, Activity, Day, Mode, Span, WorkBlock, DAY_END};
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
    /// Der Benutzer arbeitet ohne Mac weiter (zählt als Arbeitszeit).
    Away,
    /// Der Mac ist entsperrt, aber seit längerer Zeit ohne Eingabe (Inaktivitätserkennung).
    Idle,
}

pub struct Tracker {
    pub days: Vec<Day>,
    /// Stand der Datenbank nach dem letzten Laden/Speichern (um nur Änderungen zu schreiben).
    saved: Vec<Day>,
    /// Beginn der laufenden automatischen Periode.
    pub running_since: Option<NaiveDateTime>,
    pub paused_since: Option<NaiveDateTime>,
    pub pause_reason: PauseReason,
    /// Art der Arbeit ohne Mac (nur bei [`PauseReason::Away`]).
    pub away: Activity,
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
    /// Stand des Zeitkontos (siehe [`rules::account`]).
    pub account: i64,
    /// Arbeit am Stück seit der letzten Pause von mindestens 15 Minuten.
    pub stretch_secs: i64,
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
            away: Activity::Other,
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

    /// Angehalten, weil am entsperrten Mac länger nichts eingegeben wurde?
    pub fn on_idle(&self) -> bool {
        self.running_since.is_none() && self.pause_reason == PauseReason::Idle
    }

    /// Inaktivität: Die Erfassung endet rückwirkend bei der letzten Eingabe `last_input`
    /// (frühestens am Beginn der laufenden Periode) – wie eine Sperre ab diesem Zeitpunkt.
    /// Erst ab der Mindestpause wird daraus eine Pause.
    pub fn go_idle(&mut self, last_input: NaiveDateTime) -> bool {
        let Some(since) = self.running_since else { return false };
        self.stop(last_input.max(since), PauseReason::Idle)
    }

    /// Seit wann der Mac gesperrt war bzw. geschlafen hat (`None`, wenn die Erfassung läuft
    /// oder aus einem anderen Grund angehalten ist) – für die Nachfrage beim Entsperren.
    pub fn locked_since(&self) -> Option<NaiveDateTime> {
        let locked = matches!(self.pause_reason, PauseReason::Locked | PauseReason::Sleep | PauseReason::SessionInactive);
        (self.running_since.is_none() && locked).then_some(self.paused_since).flatten()
    }

    /// Arbeitet der Benutzer gerade ohne Mac weiter (Meeting, Telefonat …)?
    pub fn on_away(&self) -> bool {
        self.running_since.is_none() && self.pause_reason == PauseReason::Away
    }

    /// „Ohne Mac weiterarbeiten“: Ab jetzt zählt die Zeit als Arbeit ohne Mac, auch wenn der Mac
    /// gesperrt wird – bis zum nächsten Entsperren bzw. Aufwachen oder „Zurück am Mac“.
    pub fn go_away(&mut self, now: NaiveDateTime, activity: Activity) -> bool {
        if self.on_away() || matches!(self.mode(now), Mode::Closed(_)) || self.absent(now.date()).is_some() {
            return false;
        }
        if let Some(since) = self.running_since.take() {
            Self::commit(&mut self.days, since, now);
        }
        self.paused_since = Some(now);
        self.pause_reason = PauseReason::Away;
        self.away = activity;
        true
    }

    /// „Ohne Mac weiterarbeiten“ fortsetzen, das vor einem Neustart der App begonnen hat.
    pub fn restore_away(&mut self, since: NaiveDateTime, activity: Activity) {
        self.running_since = None;
        self.paused_since = Some(since);
        self.pause_reason = PauseReason::Away;
        self.away = activity;
    }

    /// Arbeit ohne Mac bis `until` eintragen und beenden (die Erfassung startet der Aufrufer).
    pub fn end_away(&mut self, until: NaiveDateTime) {
        if !self.on_away() {
            return;
        }
        if let Some(since) = self.paused_since {
            let until = until.max(since);
            // Zwischendurch gespeichert wurde die laufende Arbeit ohne Mac bis „jetzt“; endet sie
            // früher (z. B. zum Feierabend), entfällt der Rest.
            if until.date() == since.date() {
                if let Some(d) = self.days.iter_mut().find(|d| d.date == until.date()) {
                    d.cut_work(secs(until), DAY_END);
                }
            }
            Self::commit_work(&mut self.days, since, until, self.away.note());
        }
        self.paused_since = Some(until);
        self.pause_reason = PauseReason::Manual;
    }

    /// Ganztägige Abwesenheit am Tag `date` (siehe [`Day::locks_entries`]): Dann wird nichts
    /// erfasst und nichts von Hand eingetragen.
    pub fn absent(&self, date: NaiveDate) -> Option<Absence> {
        self.day(date).filter(|d| d.locks_entries()).and_then(|d| d.absence)
    }

    /// Zustand für die Statusdatei und die Anzeige.
    pub fn status(&self, now: NaiveDateTime) -> Status {
        if self.absent(now.date()).is_some() {
            return Status::Absent;
        }
        match self.mode(now) {
            Mode::Closed(_) => Status::Closed,
            Mode::Auto => match self.paused_since {
                Some(since) if self.on_away() => Status::Away(since, self.away),
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

    /// Automatische Erfassung starten. Nach Feierabend und an einem ganztägigen
    /// Abwesenheitstag passiert nichts. Beendet „Ohne Mac weiterarbeiten“ (die Zeit bis jetzt
    /// zählt als Arbeit ohne Mac).
    pub fn start(&mut self, now: NaiveDateTime) -> bool {
        if self.running_since.is_some()
            || matches!(self.mode(now), Mode::Closed(_))
            || self.absent(now.date()).is_some()
        {
            return false;
        }
        self.end_away(now);
        self.running_since = Some(now);
        self.paused_since = None;
        true
    }

    pub fn stop(&mut self, now: NaiveDateTime, reason: PauseReason) -> bool {
        let Some(since) = self.running_since.take() else {
            // Ohne Mac weiterarbeiten: Sperren, Ruhezustand oder Beenden der App ändern nichts.
            if self.on_away() {
                return false;
            }
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
                // Heute ganztägig abwesend: ab Mitternacht wird nichts erfasst.
                if self.absent(now.date()).is_some() {
                    self.running_since = None;
                    self.paused_since = Some(midnight);
                    self.pause_reason = PauseReason::Manual;
                }
                changed = true;
            }
        }
        if let (true, Some(since)) = (self.on_away(), self.paused_since) {
            if now.date() > since.date() {
                // Arbeit ohne Mac endet um Mitternacht (danach wird erst nach dem Entsperren erfasst).
                let midnight = (since.date() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap();
                self.end_away(midnight);
                changed = true;
            }
        }
        changed
    }

    /// Alle Tage inklusive der laufenden Periode bzw. der laufenden Arbeit ohne Mac bis `now`
    /// (für Anzeige und Speichern).
    pub fn snapshot(&self, now: NaiveDateTime) -> Vec<Day> {
        let mut days = self.days.clone();
        if let Some(since) = self.running_since {
            Self::commit(&mut days, since, now);
        }
        if let (true, Some(since)) = (self.on_away(), self.paused_since) {
            Self::commit_work(&mut days, since, now, self.away.note());
        }
        days
    }

    /// Pausen von heute (Lücken zwischen den Arbeitsabschnitten).
    pub fn pauses_today(&self, now: NaiveDateTime) -> Vec<Span> {
        let snap = self.snapshot(now);
        snap.iter().find(|d| d.date == now.date()).map(|d| d.pauses(&self.settings)).unwrap_or_default()
    }

    /// Arbeit ohne Mac nachtragen (z. B. eine Pause als Arbeitszeit zählen).
    pub fn add_work(&mut self, date: NaiveDate, from: i64, to: i64, note: &str) {
        if self.absent(date).is_some() {
            return;
        }
        self.day_mut(date).add_work(from, to, note);
        self.cleanup();
    }

    /// Arbeit ohne Mac von `from` bis `to` löschen – die Zeit zählt wieder wie erfasst (eine
    /// Sperre ab der Mindestpause wird wieder zur Pause). Liegt darin die gerade laufende
    /// Arbeit ohne Mac, endet sie ohne Eintrag; dann liefert die Funktion `true`, und der
    /// Aufrufer startet ggf. die Erfassung.
    pub fn remove_work(&mut self, date: NaiveDate, from: i64, mut to: i64, now: NaiveDateTime) -> bool {
        if self.absent(date).is_some() {
            return false;
        }
        let away = self.on_away()
            && self.paused_since.is_some_and(|since| since.date() == date && (from..=to).contains(&secs(since)));
        if away {
            // Zwischendurch gespeichert wurde die laufende Arbeit ohne Mac bis „jetzt“.
            if now.date() == date {
                to = to.max(secs(now));
            }
            self.pause_reason = PauseReason::Manual;
        }
        if let Some(d) = self.days.iter_mut().find(|d| d.date == date) {
            d.cut_work(from, to);
        }
        self.cleanup();
        away
    }

    pub fn today(&self, now: NaiveDateTime) -> TodayInfo {
        let snap = self.snapshot(now);
        let s = &self.settings;
        let date = now.date();
        let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
        let (week_work, week_target) = rules::balance(&snap, monday, date, date, s);
        let mut info = TodayInfo {
            work_secs: 0,
            pause_secs: 0,
            first_start: None,
            start_manual: false,
            auto_first: None,
            mode: Mode::Auto,
            target_secs: s.daily_target(date),
            week_balance: week_work - week_target,
            account: rules::account(&snap, date, s),
            stretch_secs: 0,
            warnings: Vec::new(),
        };
        let Some(d) = snap.iter().find(|d| d.date == date) else { return info };
        // Mit Abwesenheit gilt deren Soll (halber Urlaubstag: die Hälfte).
        info.target_secs = s.day_target(date, d.absence);
        if let Some(rec) = d.record(s) {
            info.work_secs = rec.shown_work_secs;
            info.pause_secs = rec.shown_pause_secs;
            info.first_start = (rec.work_secs > 0 || d.start.is_some()).then_some(rec.first_start);
        }
        info.stretch_secs = Day::current_stretch_secs(&d.spans(s));
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
    ///
    /// Arbeit ohne Mac vor dem neuen Beginn entfällt (die letzte Angabe gilt); zurück kommt,
    /// was entfallen ist. Läuft gerade Arbeit ohne Mac, zählt sie erst ab dem Beginn.
    pub fn set_start(&mut self, now: NaiveDateTime, at: Option<i64>) -> Vec<WorkBlock> {
        if self.absent(now.date()).is_some() {
            return Vec::new();
        }
        let at = at.map(|a| a.min(secs(now)));
        if let (Some(at), true, Some(since)) = (at, self.on_away(), self.paused_since) {
            if since.date() == now.date() && secs(since) < at {
                self.paused_since = Some(now.date().and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(at));
            }
        }
        let cut = self.day_mut(now.date()).set_start(at);
        self.cleanup();
        cut
    }

    /// Feierabend um `at` (höchstens jetzt). Stoppt die Erfassung. Arbeit ohne Mac danach
    /// entfällt (die letzte Angabe gilt); zurück kommt, was entfallen ist.
    pub fn close_day(&mut self, now: NaiveDateTime, at: i64) -> Vec<WorkBlock> {
        if self.absent(now.date()).is_some() {
            return Vec::new();
        }
        let t = secs(now);
        if self.on_away() {
            let until = now.date().and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(at.clamp(0, t));
            self.end_away(until);
        }
        self.stop(now, PauseReason::Manual);
        self.day_mut(now.date()).set_end(Some(at.min(t)))
    }

    /// Feierabend zurücknehmen. Die Erfassung startet der Aufrufer (falls entsperrt).
    pub fn reopen_day(&mut self, now: NaiveDateTime) {
        if self.absent(now.date()).is_some() {
            return;
        }
        if let Some(d) = self.days.iter_mut().find(|d| d.date == now.date()) {
            d.end = None;
        }
        self.cleanup();
    }

    /// Nach einer Änderung der Datenbank von außen (Fenster „Aufzeichnungen“) neu einlesen.
    /// Die laufende Periode bleibt erhalten; doppelt gespeicherte Zeiten werden zusammengefasst.
    pub fn replace_days(&mut self, days: Vec<Day>, settings: Settings, now: NaiveDateTime) {
        // Tag im Fenster „Aufzeichnungen“ gelöscht: Was gerade läuft, zählt erst ab jetzt –
        // sonst brächte das nächste Speichern die laufende Periode bzw. Arbeit ohne Mac seit
        // ihrem Beginn zurück.
        let deleted = |since: NaiveDateTime| {
            self.saved.iter().any(|d| d.date == since.date()) && !days.iter().any(|d| d.date == since.date())
        };
        if self.running_since.is_some_and(deleted) {
            self.running_since = Some(now);
        }
        if self.on_away() && self.paused_since.is_some_and(deleted) {
            self.paused_since = Some(now);
        }
        self.saved = days.clone();
        self.days = days;
        self.settings = settings;
        if let Mode::Closed(at) = self.mode(now) {
            // Feierabend im Fenster „Aufzeichnungen“: Arbeit ohne Mac endet damit.
            if self.on_away() {
                let until = now.date().and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(at.clamp(0, secs(now)));
                self.end_away(until);
            }
            if self.running_since.is_some() {
                self.stop(now, PauseReason::Manual);
            }
        }
        // Heute ganztägig abwesend (eingetragen in „Aufzeichnungen“): Was läuft, endet ohne
        // Eintrag – der Tag nimmt keine Zeiten an.
        if self.absent(now.date()).is_some()
            && (self.running_since.is_some_and(|since| since.date() == now.date()) || self.on_away())
        {
            self.running_since = None;
            self.paused_since = Some(now);
            self.pause_reason = PauseReason::Manual;
        }
    }

    /// Arbeit ohne Mac von `start` bis `end` eintragen (über Mitternacht auf die Tage verteilt).
    fn commit_work(days: &mut Vec<Day>, start: NaiveDateTime, end: NaiveDateTime, note: &str) {
        if end <= start {
            return;
        }
        let mut s = start;
        while s.date() < end.date() {
            day::day_mut(days, s.date()).add_work(secs(s), DAY_END, note);
            s = (s.date() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap();
        }
        day::day_mut(days, s.date()).add_work(secs(s), secs(end), note);
        days.retain(|d| !d.is_empty());
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
        // Endet die Periode genau um Mitternacht, bekommt der neue Tag keinen leeren Abschnitt
        // (sonst hätte z. B. ein Urlaubstag plötzlich einen Eintrag).
        if s < end || s == start {
            day::day_mut(days, s.date()).add_period(secs(s), secs(end));
        }
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
    fn away_counts_while_locked_until_unlock() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        assert!(t.go_away(dt(16, 12, 0), Activity::Meeting));
        assert!(t.on_away());
        assert!(!t.go_away(dt(16, 12, 1), Activity::Call), "läuft schon");
        assert_eq!(t.status(dt(16, 12, 5)), Status::Away(dt(16, 12, 0), Activity::Meeting));
        // Sperren ändert nichts, die Zeit zählt weiter.
        assert!(!t.stop(dt(16, 12, 5), PauseReason::Locked));
        assert!(t.on_away());
        let info = t.today(dt(16, 13, 0));
        assert_eq!((info.work_secs, info.pause_secs), (hm(5, 0), 0));
        // Beenden der App: bleibt bestehen
        t.stop(dt(16, 13, 0), PauseReason::Manual);
        assert!(t.on_away());
        // Entsperren beendet es; die Zeit ist als Arbeit ohne Mac eingetragen.
        assert!(t.start(dt(16, 13, 30)));
        assert!(!t.on_away());
        assert_eq!(t.status(dt(16, 13, 31)), Status::Running);
        let d = &t.days[0];
        assert_eq!(d.work.len(), 1);
        assert_eq!((d.work[0].from, d.work[0].to, d.work[0].note.as_str()), (hm(12, 0), hm(13, 30), "Meeting"));
        assert_eq!(t.today(dt(16, 14, 0)).work_secs, hm(6, 0));
        // Neustart der App während der Arbeit ohne Mac
        let mut t2 = Tracker::new(t.snapshot(dt(16, 14, 0)), Settings::default());
        t2.restore_away(dt(16, 14, 0), Activity::Call);
        assert_eq!(t2.today(dt(16, 14, 30)).work_secs, hm(6, 30));
        // Feierabend während der Arbeit ohne Mac: endet zum Feierabend
        t2.close_day(dt(16, 15, 0), hm(14, 45));
        assert!(!t2.on_away());
        assert_eq!(t2.today(dt(16, 16, 0)).work_secs, hm(6, 45));
        assert!(!t2.go_away(dt(16, 16, 0), Activity::Other), "nach Feierabend nicht möglich");
    }

    #[test]
    fn away_over_midnight_and_from_break() {
        let all_day = Settings { frame_from: 0, frame_to: DAY_END, ..Settings::default() };
        let mut t = Tracker::new(vec![], all_day);
        t.start(dt(16, 22, 0));
        t.take_break(dt(16, 22, 30));
        assert!(t.go_away(dt(16, 23, 0), Activity::Other));
        // Mitternacht beendet die Arbeit ohne Mac (auch wenn das Wochenende dazwischen liegt).
        assert!(t.roll_over(dt(18, 0, 10)));
        assert!(!t.on_away());
        assert_eq!(t.days.len(), 1);
        assert_eq!(t.days[0].work[0].to, DAY_END);
        assert_eq!(t.days[0].record(&all_day).unwrap().pause_secs(), hm(0, 30));
        assert!(t.start(dt(18, 8, 0)));
        assert_eq!(t.today(dt(18, 9, 0)).work_secs, hm(1, 0));
        // Feierabend von außen (Fenster „Aufzeichnungen“) beendet sie ebenfalls.
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.go_away(dt(16, 16, 0), Activity::Meeting);
        let mut days = t.snapshot(dt(16, 17, 0));
        days[0].end = Some(hm(16, 30));
        t.replace_days(days, Settings::default(), dt(16, 17, 0));
        assert!(!t.on_away());
        assert_eq!(t.today(dt(16, 17, 0)).work_secs, hm(8, 30));
    }

    #[test]
    fn deleted_day_restarts_running_capture() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        // Gespeichert, dann im Fenster „Aufzeichnungen“ alle Einträge von heute gelöscht.
        let mut db_days = t.snapshot(dt(16, 13, 0));
        t.replace_days(db_days.clone(), Settings::default(), dt(16, 13, 0));
        db_days.retain(|d| d.date != dt(16, 0, 0).date());
        t.replace_days(db_days, Settings::default(), dt(16, 13, 30));
        assert!(t.is_running(), "die Erfassung läuft weiter");
        assert_eq!(t.today(dt(16, 14, 0)).work_secs, hm(0, 30), "erst ab dem Löschen");
        // Ohne Löschen bleibt die laufende Periode, wie sie war.
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        let days = t.snapshot(dt(16, 13, 0));
        t.replace_days(days, Settings::default(), dt(16, 13, 0));
        assert_eq!(t.today(dt(16, 14, 0)).work_secs, hm(6, 0));
        // Laufende Arbeit ohne Mac zählt nach dem Löschen ebenfalls erst ab jetzt.
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        assert!(t.go_away(dt(16, 12, 0), Activity::Meeting));
        let days = t.snapshot(dt(16, 13, 0));
        t.replace_days(days, Settings::default(), dt(16, 13, 0));
        t.replace_days(vec![], Settings::default(), dt(16, 13, 30));
        assert!(t.on_away());
        assert_eq!(t.today(dt(16, 14, 0)).work_secs, hm(0, 30));
    }

    #[test]
    fn full_day_absence_stops_capture() {
        use crate::day::AbsenceKind;
        let vacation = Some(Absence::new(AbsenceKind::Vacation, false));
        let mut off = Day::new(dt(17, 0, 0).date());
        off.absence = vacation;
        // Urlaub am Dienstag im Voraus eingetragen; am Montag wird bis in die Nacht erfasst.
        let mut t = Tracker::new(vec![off.clone()], Settings { frame_to: DAY_END, ..Settings::default() });
        assert!(t.start(dt(16, 22, 0)));
        // Um Mitternacht endet die Erfassung: Der Urlaubstag nimmt keine Zeit an.
        assert!(t.roll_over(dt(17, 0, 5)));
        assert!(!t.is_running());
        assert_eq!(t.status(dt(17, 9, 0)), Status::Absent);
        assert_eq!(t.absent(dt(17, 0, 0).date()), vacation);
        // Entsperren, Befehle und Nachträge tun nichts.
        assert!(!t.start(dt(17, 9, 0)));
        assert!(!t.go_away(dt(17, 9, 0), Activity::Meeting));
        assert!(t.set_start(dt(17, 9, 0), Some(hm(8, 0))).is_empty());
        t.close_day(dt(17, 9, 0), hm(9, 0));
        t.add_work(dt(17, 0, 0).date(), hm(10, 0), hm(11, 0), "Meeting");
        let tue = t.snapshot(dt(17, 12, 0)).into_iter().find(|d| d.date == dt(17, 0, 0).date()).unwrap();
        assert_eq!(tue, off, "nur die Abwesenheit");
        // Halber Tag: Die Erfassung läuft.
        let mut half = off.clone();
        half.absence = Some(Absence::new(AbsenceKind::Vacation, true));
        let mut t = Tracker::new(vec![half], Settings::default());
        assert!(t.start(dt(17, 9, 0)));
        assert_ne!(t.status(dt(17, 10, 0)), Status::Absent);
        // Ganztags in „Aufzeichnungen“ eingetragen, während die Erfassung lief: Sie endet.
        let mut t = Tracker::new(vec![], Settings::default());
        assert!(t.start(dt(17, 9, 0)));
        t.replace_days(vec![off], Settings::default(), dt(17, 9, 30));
        assert!(!t.is_running());
        assert_eq!(t.today(dt(17, 10, 0)).work_secs, 0);
    }

    #[test]
    fn pause_counted_as_work() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 45));
        let pauses = t.pauses_today(dt(16, 13, 0));
        assert_eq!(pauses, vec![Span::new(hm(12, 0), hm(12, 45))]);
        t.add_work(dt(16, 0, 0).date(), pauses[0].from, pauses[0].to, "Telefonat");
        assert!(t.pauses_today(dt(16, 13, 0)).is_empty());
        assert_eq!(t.today(dt(16, 13, 0)).work_secs, hm(5, 0));
        // Wieder löschen: Die Pause ist zurück.
        assert!(!t.remove_work(dt(16, 0, 0).date(), pauses[0].from, pauses[0].to, dt(16, 13, 0)));
        assert!(t.days[0].work.is_empty());
        assert_eq!(t.pauses_today(dt(16, 13, 0)), pauses);
        assert_eq!(t.today(dt(16, 13, 0)).work_secs, hm(4, 15));
    }

    #[test]
    fn remove_running_away_work() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 30));
        assert!(t.go_away(dt(16, 13, 0), Activity::Meeting));
        // Zwischendurch gespeichert und von außen neu eingelesen (wie nach „Aufzeichnungen“).
        let days = t.snapshot(dt(16, 13, 20));
        t.replace_days(days, Settings::default(), dt(16, 13, 20));
        assert!(t.on_away());
        // In der Wochenansicht (Stand 13:10) gelöscht: endet ohne Eintrag.
        assert!(t.remove_work(dt(16, 0, 0).date(), hm(13, 0), hm(13, 10), dt(16, 13, 30)));
        assert!(!t.on_away());
        assert!(t.days[0].work.is_empty());
        assert!(t.start(dt(16, 13, 30)));
        assert_eq!(t.pauses_today(dt(16, 14, 0)), vec![Span::new(hm(12, 0), hm(12, 30)), Span::new(hm(13, 0), hm(13, 30))]);
        // Ein anderer Block lässt laufende Arbeit ohne Mac unberührt.
        t.add_work(dt(16, 0, 0).date(), hm(12, 0), hm(12, 30), "Telefonat");
        t.go_away(dt(16, 14, 0), Activity::Call);
        assert!(!t.remove_work(dt(16, 0, 0).date(), hm(12, 0), hm(12, 30), dt(16, 14, 10)));
        assert!(t.on_away());
    }

    #[test]
    fn idle_ends_at_the_last_input() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        // Um 10:12 fällt auf: seit 10:00 keine Eingabe mehr.
        assert!(t.go_idle(dt(16, 10, 0)));
        assert!(t.on_idle() && !t.is_running());
        assert_eq!(t.status(dt(16, 10, 12)), Status::Paused);
        assert_eq!(t.locked_since(), None, "keine Sperre: keine Nachfrage");
        assert!(!t.go_idle(dt(16, 10, 5)), "läuft nicht mehr");
        // Zurück nach 40 Min.: eine Pause ab der letzten Eingabe.
        assert!(t.start(dt(16, 10, 40)));
        assert!(!t.on_idle());
        let info = t.today(dt(16, 11, 0));
        assert_eq!((info.work_secs, info.pause_secs), (hm(2, 20), hm(0, 40)));
        // Nur kurz weg (unter der Mindestpause): zählt weiter als Arbeit.
        t.go_idle(dt(16, 11, 0));
        t.start(dt(16, 11, 10));
        assert_eq!(t.today(dt(16, 11, 30)).work_secs, hm(2, 50));
        // Wird der Mac währenddessen gesperrt, ist es eine Sperre seit der letzten Eingabe.
        t.go_idle(dt(16, 12, 0));
        t.stop(dt(16, 12, 20), PauseReason::Locked);
        assert!(!t.on_idle());
        assert_eq!(t.locked_since(), Some(dt(16, 12, 0)));
        // Die letzte Eingabe liegt nie vor dem Beginn der laufenden Periode.
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.go_idle(dt(16, 7, 30));
        assert_eq!(t.paused_since, Some(dt(16, 8, 0)));
    }

    #[test]
    fn today_reports_stretch_and_account() {
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.stop(dt(16, 10, 0), PauseReason::Locked);
        t.start(dt(16, 10, 5)); // 5 Min.: keine Pause, die Arbeit am Stück läuft weiter
        assert_eq!(t.today(dt(16, 11, 0)).stretch_secs, hm(3, 0));
        t.stop(dt(16, 12, 0), PauseReason::Locked);
        t.start(dt(16, 12, 30)); // 30 Min. Pause: neu
        let info = t.today(dt(16, 13, 0));
        assert_eq!(info.stretch_secs, hm(0, 30));
        // Zeitkonto: heute 4:30 von 7:48, dazu der Übertrag.
        assert_eq!(info.account, hm(4, 30) - hm(7, 48));
        t.settings.account_carry = hm(10, 0);
        assert_eq!(t.today(dt(16, 13, 0)).account, hm(10, 0) + hm(4, 30) - hm(7, 48));
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
    fn start_after_an_appointment_drops_it() {
        // Morgens ein Termin ohne Mac, später soll der Tag erst um 09:20 beginnen.
        let mut t = Tracker::new(vec![], Settings::default());
        t.start(dt(16, 8, 0));
        t.go_away(dt(16, 8, 5), Activity::Meeting);
        t.start(dt(16, 9, 0));
        assert_eq!(t.today(dt(16, 10, 0)).first_start, Some(hm(8, 0)));
        let cut = t.set_start(dt(16, 10, 0), Some(hm(9, 20)));
        assert_eq!(cut.iter().map(WorkBlock::describe).collect::<Vec<_>>(), ["Meeting 08:05–09:00"]);
        let info = t.today(dt(16, 10, 0));
        assert_eq!((info.first_start, info.work_secs), (Some(hm(9, 20)), hm(0, 40)));
        // Feierabend vor einem nachgetragenen Termin: Der Termin entfällt.
        t.add_work(dt(16, 0, 0).date(), hm(9, 40), hm(9, 55), "Telefonat");
        let cut = t.close_day(dt(16, 10, 0), hm(9, 30));
        assert_eq!(cut.len(), 1);
        assert_eq!(t.today(dt(16, 10, 0)).work_secs, hm(0, 10));
        // Läuft gerade Arbeit ohne Mac, zählt sie erst ab dem neuen Beginn.
        let mut t = Tracker::new(vec![], Settings::default());
        t.go_away(dt(16, 8, 0), Activity::Call);
        assert!(t.set_start(dt(16, 9, 0), Some(hm(8, 30))).is_empty());
        assert_eq!(t.today(dt(16, 9, 0)).work_secs, hm(0, 30));
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
