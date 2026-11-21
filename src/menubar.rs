//! Menüleisten-App (NSStatusItem) mit Erkennung von Sperren/Entsperren des Macs
//! und manuellen Entscheidungen: Arbeit unterbrechen/Weiter arbeiten, Ohne Mac weiterarbeiten,
//! Pause als Arbeitszeit zählen, Arbeitsbeginn, Feierabend.
//! Pausen ergeben sich aus den Sperrzeiten und aus „Arbeit unterbrechen“ (jeweils ab der
//! eingestellten Mindestdauer).

use crate::about;
use crate::day::{self, fmt_clock, Activity, FreeDay, Mode, Span, WorkBlock};
use crate::glyphs;
use crate::hotkey;
use crate::login_item;
use crate::notify;
use crate::palette::{self, green, indigo, orange, red};
use crate::records;
use crate::rules::{self, BreakWatch, IconSet, LimitWatch};
use crate::settings_window;
use crate::snapshot;
use crate::store;
use crate::tracker::{PauseReason, Tracker};
use crate::week_view;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication,
    NSApplicationActivationPolicy, NSAttributedStringAttachmentConveniences, NSColor, NSControlStateValueOff,
    NSControlStateValueOn, NSDatePicker,
    NSDatePickerElementFlags, NSDatePickerStyle, NSFont, NSFontAttributeName, NSFontWeightRegular,
    NSFontWeightSemibold, NSForegroundColorAttributeName, NSImage, NSImageSymbolConfiguration, NSLineBreakMode, NSMenu,
    NSMenuDelegate, NSMenuItem, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStatusBar, NSStatusItem,
    NSTextAttachment, NSTextField, NSVariableStatusItemLength, NSView,
    NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDate, NSDictionary, NSDistributedNotificationCenter, NSMutableAttributedString,
    NSNotification, NSNotificationCenter, NSPoint, NSRange, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString,
    NSTimer,
};
use std::cell::RefCell;
use std::time::Instant;

/// Menü-Tag: Uhrzeit im Dialog wählen.
const TAG_ASK: isize = -1;
/// Menü-Tag: manuellen Arbeitsbeginn entfernen.
const TAG_RESET: isize = -2;

fn now() -> NaiveDateTime {
    Local::now().naive_local()
}

fn secs(t: NaiveDateTime) -> i64 {
    day::secs_of(t.time())
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

struct MenuItems {
    status: InfoLine,
    worked: InfoLine,
    pauses: InfoLine,
    first: InfoLine,
    balance: InfoLine,
    /// Stand des Zeitkontos.
    account: InfoLine,
    /// Hinweise zu Regelverstößen (so viele, wie es Arten von Hinweisen gibt).
    hints: Vec<InfoLine>,
    /// Fehler beim Speichern oder Lesen der Datenbank (sonst ausgeblendet).
    error: InfoLine,
    /// „Heute erfassen“ an einem freien Tag (Häkchen: Die erfasste Zeit zählt).
    count_day: Retained<NSMenuItem>,
    /// „Arbeit unterbrechen“, „Weiter arbeiten“ bzw. „Zurück am Mac“.
    pause: Retained<NSMenuItem>,
    /// „Ohne Mac weiterarbeiten“ (mit Untermenü Meeting, Telefonat, Sonstiges).
    away: Retained<NSMenuItem>,
    /// „Pause als Arbeitszeit zählen“ mit den Pausen von heute im Untermenü; jede Pause
    /// bietet die Gründe an (Meeting, Telefonat …).
    count_pause: Retained<NSMenuItem>,
    count_pause_menu: Retained<NSMenu>,
    /// Pausen, die das Untermenü gerade zeigt.
    count_pause_list: Vec<Span>,
    /// Ziel der Menüeinträge, die zur Laufzeit entstehen.
    controller: Retained<Controller>,
    /// „Arbeitsbeginn festlegen“ (Untermenü).
    start: Retained<NSMenuItem>,
    start_reset: Retained<NSMenuItem>,
    close: Retained<NSMenuItem>,
    reopen: Retained<NSMenuItem>,
    /// Untermenü-Einträge „vor N Min.“, deren Titel die Uhrzeit zeigen.
    relative: Vec<(Retained<NSMenuItem>, i64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconKind {
    Running,
    /// Arbeit ohne Mac (Meeting, Telefonat …).
    Away,
    Paused,
    Break,
    Outside,
    Closed,
    /// Heute ganztägig abwesend (Urlaub, Krank …): Es wird nichts erfasst.
    Absent,
}

struct AppState {
    tracker: Tracker,
    status_item: Retained<NSStatusItem>,
    items: MenuItems,
    last_save: Instant,
    last_icon: Option<IconKind>,
    /// Zuletzt gezeigtes Symbol in der Menüleiste (Zustand, Symbolsatz, Höhe).
    button_icon: Option<(IconKind, IconSet, u32)>,
    /// Zuletzt gezeigter Text neben dem Symbol (leer = keiner).
    button_title: Option<String>,
    /// Zuletzt übernommener Stand der Einstellung „Globale Kurzbefehle“.
    hotkeys: Option<bool>,
    /// Mitteilungen bei 9:45 und 10 Std. Arbeit.
    limit: LimitWatch,
    /// Erinnerungen an die Pause bei 5:45 und 6 Std. Arbeit am Stück.
    breaks: BreakWatch,
    /// Sperre, zu der gerade nachgefragt wird (Tag, von, bis) – darauf bezieht sich die Antwort.
    asked_lock: Option<(NaiveDate, i64, i64)>,
    db: store::Db,
    /// Die Datenbank ließ sich zuletzt nicht schreiben bzw. nicht lesen (mit dem Text für das
    /// Menü). Solange das Schreiben scheitert, ist die Zeit seit dem letzten Speichern nur im
    /// Speicher der App.
    db_error: Option<(DbFault, String)>,
}

/// Wobei die Datenbank versagt hat – ein Fehler gilt, bis genau das wieder gelingt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DbFault {
    Read,
    Write,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Manuelle Aktion: vorher Änderungen von außen übernehmen, danach speichern und anzeigen.
fn act(f: impl FnOnce(&mut AppState, NaiveDateTime)) {
    with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        s.tracker.roll_over(t);
        f(s, t);
        s.persist(t);
        s.refresh();
    });
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkController"]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSMenuDelegate for Controller {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            with_state(|s| {
                let t = now();
                s.sync_from_db(t);
                if s.tracker.roll_over(t) {
                    s.persist(t);
                }
                s.refresh();
            });
        }
    }

    impl Controller {
        /// „Arbeit unterbrechen“ ↔ „Weiter arbeiten“; bei Arbeit ohne Mac „Zurück am Mac“.
        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, _sender: Option<&AnyObject>) {
            toggle_pause();
        }

        /// „Heute erfassen“ an einem freien Tag ein- bzw. ausschalten.
        #[unsafe(method(countDay:))]
        fn count_day(&self, _sender: Option<&AnyObject>) {
            act(|s, t| {
                let on = s.tracker.counting(t.date()) != Some(true);
                s.tracker.set_counting(t.date(), on);
                // Nach dem Löschen aller Einträge war die Erfassung aus: ab jetzt wieder.
                if on && !screen_locked() {
                    s.tracker.start(t);
                }
            });
        }

        /// „Ohne Mac weiterarbeiten“ (Tag = Art der Arbeit).
        #[unsafe(method(goAway:))]
        fn go_away(&self, sender: Option<&NSMenuItem>) {
            let activity = Activity::from_id(sender.map(|s| s.tag()).unwrap_or(0));
            act(|s, t| {
                s.tracker.go_away(t, activity);
            });
        }

        /// Eine Pause von heute als Arbeitszeit zählen (Tag = Beginn der Pause und Grund,
        /// siehe [`count_pause_tag`]).
        #[unsafe(method(countPause:))]
        fn count_pause(&self, sender: Option<&NSMenuItem>) {
            let Some((from, activity)) = sender.map(|s| count_pause_from_tag(s.tag())) else { return };
            act(|s, t| {
                if let Some(p) = s.tracker.pauses_today(t).into_iter().find(|p| p.from == from) {
                    s.tracker.add_work(t.date(), p.from, p.to, activity.note());
                }
            });
        }

        #[unsafe(method(setStart:))]
        fn set_start(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|s| s.tag()).unwrap_or(0);
            if tag == TAG_RESET {
                act(|s, t| {
                    s.tracker.set_start(t, None);
                });
                return;
            }
            let initial = with_state(|s| s.tracker.today(now()).first_start).flatten();
            let Some(at) = chosen_time(
                tag,
                "Arbeitsbeginn festlegen",
                "Ab wann zählt die Arbeitszeit heute? Ein früherer Beginn als das erste Entsperren wird mitgezählt.",
                initial,
            ) else {
                return;
            };
            act(|s, t| {
                let cut = s.tracker.set_start(t, Some(secs(at)));
                notify_cut_work(&cut, "vor dem Arbeitsbeginn");
            });
        }

        #[unsafe(method(closeDay:))]
        fn close_day(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|s| s.tag()).unwrap_or(0);
            let Some(at) = chosen_time(
                tag,
                "Feierabend",
                "Wann war heute Arbeitsende? Danach wird bis morgen nichts mehr erfasst.",
                None,
            ) else {
                return;
            };
            act(|s, t| {
                let cut = s.tracker.close_day(t, secs(at));
                notify_cut_work(&cut, "nach dem Feierabend");
            });
        }

        #[unsafe(method(reopenDay:))]
        fn reopen_day(&self, _sender: Option<&AnyObject>) {
            act(|s, t| {
                s.tracker.reopen_day(t);
                if !screen_locked() {
                    s.tracker.start(t);
                }
            });
        }

        #[unsafe(method(showRecords:))]
        fn show_records(&self, _sender: Option<&AnyObject>) {
            open_records();
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            settings_window::show(settings_changed);
        }

        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) {
            about::show();
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            shutdown();
            let mtm = MainThreadMarker::new().expect("main thread");
            NSApplication::sharedApplication(mtm).terminate(None);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            with_state(|s| {
                let t = now();
                let mut dirty = s.sync_from_db(t);
                dirty |= s.tracker.roll_over(t);
                let locked = screen_locked();
                // Inaktivität (wenn eingeschaltet): so lange keine Eingabe, dass der Mac als
                // verlassen gilt – außer etwas hält den Bildschirm wach (Videokonferenz, Film).
                let limit = s.tracker.settings.idle_secs;
                let idle = if limit > 0 && !locked && !display_kept_awake() { idle_secs() } else { 0 };
                let left_alone = limit > 0 && idle >= limit;
                if s.tracker.is_running() && locked {
                    // Sicherheitsnetz: falls ein Sperr-Ereignis verpasst wurde.
                    dirty |= s.tracker.stop(t, PauseReason::Locked);
                } else if s.tracker.is_running() && left_alone {
                    // Rückwirkend ab der letzten Eingabe, wie eine Sperre.
                    dirty |= s.tracker.go_idle(t - Duration::seconds(idle));
                } else if !s.tracker.is_running()
                    && !locked
                    && !s.tracker.on_break()
                    && !s.tracker.on_away()
                    && !(s.tracker.on_idle() && left_alone)
                    && s.tracker.paused_since.is_some_and(|p| p.date() == t.date())
                {
                    // Entsperrt, aber nicht erfasst (z. B. Feierabend im Fenster zurückgenommen
                    // oder zurück nach Inaktivität).
                    dirty |= s.tracker.start(t);
                }
                if dirty || (s.tracker.is_running() && s.last_save.elapsed().as_secs() >= 60) {
                    s.persist(t);
                }
                s.refresh();
            });
        }

        #[unsafe(method(screenLocked:))]
        fn screen_locked_note(&self, _n: &NSNotification) {
            pause_for(PauseReason::Locked);
        }

        #[unsafe(method(screenUnlocked:))]
        fn screen_unlocked_note(&self, _n: &NSNotification) {
            resume_auto();
        }

        #[unsafe(method(willSleep:))]
        fn will_sleep(&self, _n: &NSNotification) {
            pause_for(PauseReason::Sleep);
        }

        #[unsafe(method(didWake:))]
        fn did_wake(&self, _n: &NSNotification) {
            // Ohne Kennwortabfrage kommt kein "entsperrt"-Ereignis – dann hier fortsetzen.
            if !screen_locked() {
                resume_auto();
            }
        }

        #[unsafe(method(sessionResigned:))]
        fn session_resigned(&self, _n: &NSNotification) {
            pause_for(PauseReason::SessionInactive);
        }

        #[unsafe(method(sessionActive:))]
        fn session_active(&self, _n: &NSNotification) {
            if !screen_locked() {
                resume_auto();
            }
        }

        #[unsafe(method(willTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            shutdown();
        }

        /// Abmelden, Neustart, Ausschalten: Eine Pause über „Arbeit unterbrechen“ und Arbeit ohne Mac
        /// enden damit (nach dem Anmelden wird wieder erfasst). Beim bloßen Beenden der App bleiben
        /// sie bestehen.
        #[unsafe(method(willPowerOff:))]
        fn will_power_off(&self, _n: &NSNotification) {
            with_state(|s| {
                if s.tracker.on_break() {
                    s.tracker.pause_reason = PauseReason::Manual;
                }
                // Arbeit ohne Mac endet ebenfalls (bis jetzt zählt sie).
                s.tracker.end_away(now());
            });
            shutdown();
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn pause_for(reason: PauseReason) {
    with_state(|s| {
        let t = now();
        s.tracker.stop(t, reason);
        s.persist(t);
        s.refresh();
    });
}

/// Fortsetzen nach Entsperren/Aufwachen (nicht nach Feierabend).
fn resume_auto() {
    with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        s.tracker.roll_over(t);
        let locked_since = s.tracker.locked_since();
        if s.tracker.start(t) {
            s.persist(t);
            if let Some(from) = locked_since {
                s.ask_about_lock(from, t);
            }
        }
        s.refresh();
    });
}

/// Arbeitsbeginn oder Feierabend aus dem Menü hat Arbeit ohne Mac gekürzt (die letzte Angabe
/// gilt): Das meldet eine Mitteilung, denn das Menü ist dann schon zu. `where_` sagt, wo die
/// Arbeit lag („vor dem Arbeitsbeginn“).
fn notify_cut_work(cut: &[WorkBlock], where_: &str) {
    if cut.is_empty() {
        return;
    }
    let what: Vec<String> = cut.iter().map(WorkBlock::describe).collect();
    notify::post_text(
        "work-cut",
        "Arbeit ohne Mac entfällt",
        &format!("{} lag {where_} und zählt nicht mehr – die letzte Angabe gilt.", what.join(", ")),
    );
}

/// Antwort auf die Nachfrage nach langer Sperre: Die Zeit war Arbeit ohne Mac – oder schon
/// Feierabend (dann endet der Tag mit dem Sperren, und die Erfassung hält an).
fn lock_answered(answer: notify::LockAnswer) {
    act(|s, t| {
        let Some((date, from, to)) = s.asked_lock.take() else { return };
        match answer {
            notify::LockAnswer::Work(activity) => s.tracker.add_work(date, from, to, activity.note()),
            notify::LockAnswer::CloseDay if date == t.date() => {
                let cut = s.tracker.close_day(t, from);
                notify_cut_work(&cut, "nach dem Feierabend");
            }
            notify::LockAnswer::CloseDay => {}
        }
    });
    records::reload_if_open();
}

/// Stand für die Wochenansicht in „Aufzeichnungen“ (inklusive laufender Periode).
fn week_snapshot() -> Option<week_view::Snapshot> {
    with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        if s.tracker.roll_over(t) {
            s.persist(t);
        }
        s.week_snapshot(t)
    })
}

/// Dialog in der Wochenansicht: Arbeit ohne Mac eintragen.
fn add_work_block(date: chrono::NaiveDate, from: i64, to: i64, activity: Activity) {
    act(|s, _| s.tracker.add_work(date, from, to, activity.note()));
    records::reload_if_open();
}

/// Klick auf Arbeit ohne Mac in der Wochenansicht: löschen (die Zeit wird wieder zur Pause).
/// War es die gerade laufende Arbeit ohne Mac, wird ab jetzt wieder erfasst.
fn remove_work_block(date: chrono::NaiveDate, from: i64, to: i64) {
    act(|s, t| {
        if s.tracker.remove_work(date, from, to, t) && !screen_locked() {
            s.tracker.start(t);
        }
    });
    records::reload_if_open();
}

/// Öffnet das Fenster „Aufzeichnungen“ (oder holt es nach vorn): in der zuletzt benutzten
/// Ansicht oder mit einem Tag. Vorher speichern, damit es den aktuellen Stand zeigt.
fn open_records() {
    let status = with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        s.persist(t);
        s.tracker.status(t)
    });
    // Außerhalb von `with_state`: Das Fenster meldet Änderungen über `records_changed`.
    if let Some(status) = status {
        let embed = records::Embed {
            on_change: records_changed,
            week: week_snapshot,
            add_work: add_work_block,
            remove_work: remove_work_block,
        };
        records::show(status, embed);
    }
}

/// Einstellungen wurden im Fenster „Einstellungen“ geändert.
fn settings_changed() {
    records_changed();
    records::reload_if_open();
}

/// Das Fenster „Aufzeichnungen“ hat die Datenbank geändert: sofort übernehmen.
fn records_changed() {
    with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        s.persist(t);
        s.refresh();
    });
}

fn shutdown() {
    with_state(|s| {
        let t = now();
        s.sync_from_db(t);
        s.tracker.stop(t, PauseReason::Manual);
        s.persist(t);
    });
}

// ---------- Dialoge ----------

fn activate_app(mtm: MainThreadMarker) {
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

fn to_nsdate(t: NaiveDateTime) -> Retained<NSDate> {
    let ts = Local
        .from_local_datetime(&t)
        .earliest()
        .map(|d| d.timestamp())
        .unwrap_or_else(|| Local::now().timestamp());
    NSDate::dateWithTimeIntervalSince1970(ts as f64)
}

fn from_nsdate(d: &NSDate) -> Option<NaiveDateTime> {
    let ts = d.timeIntervalSince1970();
    DateTime::from_timestamp(ts.floor() as i64, 0).map(|u| u.with_timezone(&Local).naive_local())
}

/// Zeitpunkt aus einem Menü-Eintrag: `tag >= 0` = vor so vielen Minuten,
/// [`TAG_ASK`] = Uhrzeit im Dialog wählen. Immer heute und nicht in der Zukunft.
fn chosen_time(tag: isize, message: &str, info: &str, initial: Option<i64>) -> Option<NaiveDateTime> {
    let t = now();
    let midnight = t.date().and_hms_opt(0, 0, 0).unwrap();
    if tag >= 0 {
        return Some((t - Duration::minutes(tag as i64)).max(midnight));
    }
    let initial = initial
        .and_then(|s| t.date().and_hms_opt(0, 0, 0).map(|m| m + Duration::seconds(s)))
        .unwrap_or(t)
        .clamp(midnight, t);
    let mtm = MainThreadMarker::new()?;
    let picker = NSDatePicker::new(mtm);
    picker.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    picker.setDatePickerElements(NSDatePickerElementFlags::HourMinute);
    picker.setMinDate(Some(&to_nsdate(midnight)));
    picker.setMaxDate(Some(&to_nsdate(t)));
    picker.setDateValue(&to_nsdate(initial));
    picker.sizeToFit();

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(message));
    alert.setInformativeText(&ns(info));
    alert.addButtonWithTitle(&ns("Übernehmen"));
    alert.addButtonWithTitle(&ns("Abbrechen"));
    alert.setAccessoryView(Some(&picker));
    activate_app(mtm);
    if alert.runModal() != NSAlertFirstButtonReturn {
        return None;
    }
    let picked = from_nsdate(&picker.dateValue())?;
    // Nur die Uhrzeit zählt; heute, nicht in der Zukunft.
    let picked = t.date().and_time(picked.time());
    Some(picked.clamp(midnight, now()))
}

impl AppState {
    /// Nach dem Entsperren: Bei langer Sperre nachfragen, was die Zeit war (wenn in den
    /// Einstellungen an). Nur für Sperren von heute, die im Arbeitszeitrahmen begonnen haben –
    /// nicht für die Nacht davor.
    fn ask_about_lock(&mut self, from: NaiveDateTime, to: NaiveDateTime) {
        let settings = &self.tracker.settings;
        let long = (to - from).num_seconds() >= settings.ask_after();
        if !settings.ask_after_lock || !long || from.date() != to.date() || !settings.in_frame(secs(from)) {
            return;
        }
        // Freier Tag ohne „Heute erfassen“: Es zählt ohnehin nichts.
        if self.tracker.counting(to.date()) == Some(false) {
            return;
        }
        let span = Span::new(secs(from), secs(to));
        let asked = notify::ask_lock(
            &format!("Gesperrt von {}", pause_title(span)),
            "Das zählt als Pause. War es Arbeit ohne Mac oder schon Feierabend?",
        );
        self.asked_lock = asked.then_some((from.date(), span.from, span.to));
    }

    /// Übernimmt Änderungen, die das Fenster „Aufzeichnungen“ (oder `--import`) in die
    /// Datenbank geschrieben hat. Gibt `true` zurück, wenn neu eingelesen wurde.
    fn sync_from_db(&mut self, t: NaiveDateTime) -> bool {
        if !self.db.changed_externally() {
            return false;
        }
        match self.db.load().and_then(|days| Ok((days, self.db.load_settings()?))) {
            Ok((days, settings)) => {
                self.tracker.replace_days(days, settings, t);
                self.clear_db_error(DbFault::Read);
                true
            }
            Err(e) => {
                // Beim nächsten Mal noch einmal versuchen (z. B. war die Datenbank gesperrt).
                self.db.recheck_external_change();
                self.report_db_error(DbFault::Read, &e.to_string());
                false
            }
        }
    }

    fn persist(&mut self, t: NaiveDateTime) {
        match self.tracker.save(&mut self.db, t) {
            // Gespeichert wird immer der ganze Stand – damit ist nichts verloren gegangen.
            Ok(()) => self.clear_db_error(DbFault::Write),
            Err(e) => self.report_db_error(DbFault::Write, &e.to_string()),
        }
        store::write_status(self.tracker.status(t));
        self.last_save = Instant::now();
    }

    /// Fehler der Datenbank merken (für das Menü) und beim ersten Auftreten melden – sonst
    /// bemerkt ihn niemand: Die App läuft ohne Fenster und ohne Terminal.
    fn report_db_error(&mut self, fault: DbFault, detail: &str) {
        let (what, consequence) = match fault {
            DbFault::Write => (
                "Speichern fehlgeschlagen",
                "Die Arbeitszeit wird weiter gezählt, aber erst gesichert, wenn das Speichern wieder gelingt.",
            ),
            DbFault::Read => (
                "Die Datenbank konnte nicht gelesen werden",
                "Änderungen aus dem Fenster „Aufzeichnungen“ fehlen, bis das Lesen wieder gelingt.",
            ),
        };
        eprintln!("Tagwerk: {what}: {detail}");
        if self.db_error.is_none() {
            notify::post_text("db-error", &format!("Tagwerk: {what}"), &format!("{detail}. {consequence}"));
        }
        self.db_error = Some((fault, db_error_text(what, detail)));
    }

    fn clear_db_error(&mut self, fault: DbFault) {
        if self.db_error.as_ref().is_some_and(|(f, _)| *f == fault) {
            self.db_error = None;
        }
    }

    fn refresh(&mut self) {
        let t = now();
        let running = self.tracker.is_running();
        let status = self.tracker.status(t);
        let info = self.tracker.today(t);
        let mtm = MainThreadMarker::new().expect("main thread");
        let midnight = t.date().and_hms_opt(0, 0, 0).unwrap();

        let (status_text, icon) = status_line(&self.tracker, t);
        self.items.status.set(&status_title(&status_text, icon));
        let worked = store::fmt_hm(info.work_secs);
        let target = format!("  von {}", store::fmt_hm(info.target_secs));
        let done = info.target_secs > 0 && info.work_secs >= info.target_secs;
        let mut parts = vec![("Heute gearbeitet:  ", Tone::Label), (worked.as_str(), if done { Tone::Good } else { Tone::Value })];
        if info.target_secs > 0 {
            parts.push((target.as_str(), Tone::Muted));
        }
        self.items.worked.set(&rich(&parts));
        let balance = rules::fmt_balance(info.week_balance);
        let tone = if palette::is_minus(info.week_balance) { Tone::Bad } else { Tone::Good };
        self.items.balance.set(&rich(&[("Saldo diese Woche:  ", Tone::Label), (&balance, tone)]));
        let account = rules::fmt_balance(info.account);
        let tone = if palette::is_minus(info.account) { Tone::Bad } else { Tone::Good };
        self.items.account.set(&rich(&[("Zeitkonto:  ", Tone::Label), (&account, tone)]));
        let pauses = store::fmt_hm(info.pause_secs);
        self.items.pauses.set(&rich(&[("Pausen heute:  ", Tone::Label), (&pauses, Tone::Value)]));
        let first = info.first_start.map(|f| format!("{} Uhr", fmt_clock(f)));
        let mut parts = vec![("Beginn heute:  ", Tone::Label), (first.as_deref().unwrap_or("–"), Tone::Value)];
        if info.start_manual {
            parts.push(("  (manuell)", Tone::Muted));
        }
        self.items.first.set(&rich(&parts));
        // Fehler und Hinweise brechen um, statt das Menü zu verbreitern: höchstens so breit wie
        // die breiteste Zeile mit Status und Kennzahlen.
        let items = &self.items;
        let wrap = [&items.status, &items.worked, &items.pauses, &items.first, &items.balance, &items.account]
            .iter()
            .map(|line| line.text_width())
            .fold(InfoLine::MIN_WRAP, f64::max);
        match &self.db_error {
            Some((_, text)) => {
                self.items.error.set_wrapped(&error_title(text), wrap);
                self.items.error.item.setHidden(false);
            }
            None => self.items.error.item.setHidden(true),
        }
        for (i, line) in self.items.hints.iter().enumerate() {
            match info.warnings.get(i) {
                Some(w) => {
                    line.set_wrapped(&warning_title(&w.text()), wrap);
                    line.item.setHidden(false);
                }
                None => line.item.setHidden(true),
            }
        }

        // Untermenüs: Uhrzeiten aktualisieren
        for (item, mins) in &self.items.relative {
            let at = (t - Duration::minutes(*mins)).max(midnight);
            let title = if *mins == 0 {
                format!("Jetzt  ({} Uhr)", at.format("%H:%M"))
            } else {
                format!("Vor {mins} Min.  ({} Uhr)", at.format("%H:%M"))
            };
            item.setTitle(&ns(&title));
        }
        self.items.start_reset.setHidden(!info.start_manual);
        let reset = match info.auto_first {
            Some(a) => format!("Erstes Entsperren verwenden  ({} Uhr)", fmt_clock(a)),
            None => "Manuellen Beginn entfernen".to_string(),
        };
        self.items.start_reset.setTitle(&ns(&reset));

        let closed = matches!(info.mode, Mode::Closed(_));
        let away = self.tracker.on_away();
        // Die Stände laufen immer mit; gemeldet wird nur, was in den Einstellungen an ist.
        let (notify_limit, notify_break) = (self.tracker.settings.notify_limit, self.tracker.settings.notify_break);
        if let Some(alert) = self.limit.update(t.date(), info.work_secs, running || away).filter(|_| notify_limit) {
            notify::post(alert);
        }
        if let Some(alert) = self.breaks.update(t.date(), info.stretch_secs, running || away).filter(|_| notify_break) {
            notify::post_text(alert.id(), &alert.title(), &alert.body());
        }
        // Ganztägig abwesend: nichts zu erfassen, also auch nichts zu steuern.
        let absent = matches!(status, store::Status::Absent);
        // Freier Tag: „Heute erfassen“ mit Häkchen, sobald die erfasste Zeit zählt – nach
        // Feierabend nicht mehr (wie die übrigen Steuerpunkte). Solange sie nicht zählt, ist
        // „Heute erfassen“ der einzige Einstieg: Die übrigen Steuerpunkte erscheinen erst danach.
        let counting = self.tracker.counting(t.date());
        self.items.count_day.setHidden(counting.is_none() || closed || absent);
        self.items.count_day.setState(if counting == Some(true) { NSControlStateValueOn } else { NSControlStateValueOff });
        let free = matches!(status, store::Status::FreeDay);
        let uncounted = counting == Some(false);
        self.items.start.setHidden(absent || uncounted);
        self.items.pause.setHidden(closed || absent || uncounted);
        let (title, symbol) = if running {
            ("Arbeit unterbrechen", "pause.fill")
        } else if away {
            ("Zurück am Mac", "desktopcomputer")
        } else {
            ("Weiter arbeiten", "play.fill")
        };
        if self.items.pause.title().to_string() != title {
            self.items.pause.setTitle(&ns(title));
            set_item_symbol(&self.items.pause, symbol, title);
        }
        self.items.away.setHidden(closed || away || absent || uncounted);
        let pauses = self.tracker.pauses_today(t);
        if pauses != self.items.count_pause_list {
            self.items.count_pause_menu.removeAllItems();
            for p in &pauses {
                // Je Pause ein Untermenü mit dem Grund – wie bei „Ohne Mac weiterarbeiten“.
                let reasons = NSMenu::new(mtm);
                reasons.setAutoenablesItems(false);
                for activity in Activity::ALL {
                    let title = activity.label();
                    let it = menu_item(mtm, title, Some(sel!(countPause:)), "", &self.items.controller);
                    it.setTag(count_pause_tag(p.from, activity));
                    set_item_symbol(&it, activity_symbol(activity), title);
                    reasons.addItem(&it);
                }
                let item = menu_item(mtm, &pause_title(*p), None, "", &self.items.controller);
                item.setEnabled(true);
                item.setSubmenu(Some(&reasons));
                self.items.count_pause_menu.addItem(&item);
            }
            self.items.count_pause_list = pauses;
        }
        self.items.count_pause.setHidden(self.items.count_pause_list.is_empty() || absent);
        self.items.close.setHidden(closed || absent || uncounted);
        self.items.reopen.setHidden(!closed || absent);
        if week_view::is_visible() {
            week_view::update(self.week_snapshot(t));
        }
        records::set_status(status);

        if let Some(button) = self.status_item.button(mtm) {
            let state = match icon {
                IconKind::Running => "läuft",
                IconKind::Away => "Arbeit ohne Mac",
                IconKind::Paused => "pausiert",
                IconKind::Break => "Pause",
                IconKind::Outside if free => "freier Tag, Erfassung zählt nicht",
                IconKind::Outside => "außerhalb des Arbeitszeitrahmens",
                IconKind::Closed => "Feierabend",
                IconKind::Absent => "heute abwesend",
            };
            let mut tip = format!(
                "Tagwerk – {state}\nHeute gearbeitet: {}\nPausen: {}",
                store::fmt_hm(info.work_secs),
                store::fmt_hm(info.pause_secs)
            );
            if let Some((_, text)) = &self.db_error {
                tip.push_str(&format!("\n{text}"));
            }
            button.setToolTip(Some(&ns(&tip)));
            let set = self.tracker.settings.icon_set;
            let height = icon_height(&self.status_item);
            if self.button_icon != Some((icon, set, height)) {
                set_icon(&button, icon, set, height);
                self.button_icon = Some((icon, set, height));
            }
            // Auf Wunsch die Arbeitszeit von heute neben dem Symbol.
            let title = if self.tracker.settings.show_time { bar_time(info.work_secs) } else { String::new() };
            if self.button_title.as_ref() != Some(&title) {
                set_bar_title(&button, &title);
                self.button_title = Some(title);
            }
            self.last_icon = Some(icon);
        }

        // Globale Kurzbefehle nach den Einstellungen ein- oder ausschalten.
        let want = self.tracker.settings.hotkeys;
        if self.hotkeys != Some(want) {
            self.hotkeys = Some(want);
            if let Err(e) = hotkey::set_enabled(want, run_hotkey) {
                eprintln!("Tagwerk: {e}");
                notify::post_text("hotkeys", "Tagwerk: Kurzbefehle nicht verfügbar", &e);
            }
        }
    }

    /// Stand für die Wochenansicht (inklusive laufender Periode).
    fn week_snapshot(&self, t: NaiveDateTime) -> week_view::Snapshot {
        week_view::Snapshot {
            days: self.tracker.snapshot(t),
            settings: self.tracker.settings,
            now: t,
            running: self.tracker.is_running() || self.tracker.on_away(),
        }
    }
}

/// „Pause seit 12:03 Uhr“; solange die Mindestpause nicht erreicht ist, mit Hinweis,
/// dass die Zeit noch als Arbeitszeit zählt.
fn break_text(since: NaiveDateTime, t: NaiveDateTime, min_pause: i64) -> String {
    let at = if since.date() == t.date() {
        format!("{} Uhr", since.format("%H:%M"))
    } else {
        format!("{} Uhr", since.format("%d.%m. %H:%M"))
    };
    if (t - since).num_seconds() < min_pause {
        format!("Pause seit {at} (zählt ab {} Min.)", min_pause / 60)
    } else {
        format!("Pause seit {at}")
    }
}

/// Statuszeile und Symbol für das Menü und die Menüleiste.
fn status_line(tracker: &Tracker, t: NaiveDateTime) -> (String, IconKind) {
    let settings = &tracker.settings;
    if let Some(absence) = tracker.absent(t.date()) {
        return (format!("Heute abwesend: {}", absence.label()), IconKind::Absent);
    }
    match (tracker.status(t), tracker.mode(t)) {
        (_, Mode::Closed(at)) => (format!("Feierabend um {} Uhr", fmt_clock(at)), IconKind::Closed),
        // Außerhalb des Rahmens zählt auch Arbeit ohne Mac nicht: Symbol wie bei „außerhalb“.
        (store::Status::Away(since, activity), _) if !tracker.in_frame(t) => (
            format!("{} – außerhalb des Arbeitszeitrahmens", away_text(since, t, activity)),
            IconKind::Outside,
        ),
        (store::Status::Away(since, activity), _) => (away_text(since, t, activity), IconKind::Away),
        (store::Status::Break(since), _) => (break_text(since, t, settings.min_pause), IconKind::Break),
        (store::Status::FreeDay, _) if tracker.free_day(t.date()) == Some(FreeDay::Stopped) => {
            ("Freier Tag – Erfassung aus".to_string(), IconKind::Outside)
        }
        (store::Status::FreeDay, _) => ("Freier Tag – Erfassung zählt nicht".to_string(), IconKind::Outside),
        (store::Status::Outside, _) => {
            (format!("Außerhalb des Arbeitszeitrahmens ({})", settings.frame_text()), IconKind::Outside)
        }
        (store::Status::Running, _) => ("Zeiterfassung läuft".to_string(), IconKind::Running),
        _ => (
            match tracker.paused_since {
                Some(p) if p.date() == t.date() && tracker.on_idle() => {
                    format!("Keine Eingabe seit {} Uhr – pausiert", p.format("%H:%M"))
                }
                Some(p) if p.date() == t.date() => format!("Zeiterfassung pausiert seit {} Uhr", p.format("%H:%M")),
                _ => "Zeiterfassung pausiert".to_string(),
            },
            IconKind::Paused,
        ),
    }
}

/// „Ohne Mac seit 12:03 Uhr – Meeting“
fn away_text(since: NaiveDateTime, t: NaiveDateTime, activity: Activity) -> String {
    let at = if since.date() == t.date() {
        format!("{} Uhr", since.format("%H:%M"))
    } else {
        format!("{} Uhr", since.format("%d.%m. %H:%M"))
    };
    match activity.note() {
        "" => format!("Ohne Mac seit {at}"),
        note => format!("Ohne Mac seit {at} – {note}"),
    }
}

/// Eintrag unter „Pause als Arbeitszeit zählen“: `15:25 – 15:45 Uhr  (20 Min.)`. Die Dauer
/// richtet sich wie in der Wochenansicht nach den angezeigten Uhrzeiten.
fn pause_title(p: Span) -> String {
    let len = records::shown_len(p.from, p.to);
    format!("{} – {} Uhr  ({})", fmt_clock(p.from), fmt_clock(p.to), fmt_minutes(len))
}

/// Menü-Tag unter „Pause als Arbeitszeit zählen“: Beginn der Pause (Sekunden seit
/// Mitternacht) und Grund in einer Zahl.
fn count_pause_tag(from: i64, activity: Activity) -> isize {
    from as isize * COUNT_PAUSE_REASONS + activity.id()
}

fn count_pause_from_tag(tag: isize) -> (i64, Activity) {
    ((tag / COUNT_PAUSE_REASONS) as i64, Activity::from_id(tag % COUNT_PAUSE_REASONS))
}

/// Platz für den Grund im Menü-Tag (mehr als es Gründe gibt).
const COUNT_PAUSE_REASONS: isize = 8;

/// SF-Symbol für den Grund von Arbeit ohne Mac.
fn activity_symbol(activity: Activity) -> &'static str {
    match activity {
        Activity::Meeting => "person.2",
        Activity::Call => "phone",
        Activity::Networking => "person.3",
        Activity::Other => "ellipsis.circle",
    }
}

/// `55 Min.` bzw. `1:20 h`
fn fmt_minutes(secs: i64) -> String {
    if secs < 3600 {
        format!("{} Min.", secs / 60)
    } else {
        store::fmt_hm(secs)
    }
}

impl IconKind {
    /// SF-Symbol (für den Eintrag im Menü) und Beschreibung.
    fn symbol(self) -> (&'static str, &'static str) {
        match self {
            IconKind::Running => ("clock.fill", "Zeiterfassung läuft"),
            IconKind::Away => ("figure.walk", "Arbeit ohne Mac"),
            IconKind::Paused => ("pause.circle", "Zeiterfassung pausiert"),
            IconKind::Break => ("cup.and.saucer.fill", "Pause"),
            IconKind::Outside => ("moon.stars", "Außerhalb des Arbeitszeitrahmens"),
            IconKind::Closed => ("moon.zzz", "Feierabend"),
            IconKind::Absent => ("calendar.badge.minus", "Heute abwesend"),
        }
    }

    /// Zustand des Menüleisten-Symbols.
    fn glyph(self) -> glyphs::State {
        match self {
            // Arbeit ohne Mac zählt wie die laufende Erfassung.
            IconKind::Running | IconKind::Away => glyphs::State::Running,
            // gesperrt oder im Ruhezustand – in der Menüleiste kaum je zu sehen
            IconKind::Paused | IconKind::Break => glyphs::State::Break,
            IconKind::Outside => glyphs::State::Outside,
            // Wie Feierabend: Heute wird nichts mehr erfasst.
            IconKind::Closed | IconKind::Absent => glyphs::State::Closed,
        }
    }
}

/// SF-Symbol vor einem Menüeintrag (wie in den Menüs von macOS).
fn set_item_symbol(item: &NSMenuItem, symbol: &str, desc: &str) {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), Some(&ns(desc)));
    item.setImage(image.as_deref());
}

/// Darstellung eines Textteils in den Status- und Kennzahl-Zeilen des Menüs.
#[derive(Clone, Copy)]
enum Tone {
    /// Beschriftung in normaler Textfarbe
    Label,
    /// Nebensache (Tagessoll, „manuell“)
    Muted,
    /// Wert: halbfett, gleich breite Ziffern
    Value,
    /// Wert im Plus bzw. erreicht (grün) oder im Minus (rot)
    Good,
    Bad,
    /// Statuszeile: halbfett in der Farbe des Zustands
    Status(fn() -> Retained<NSColor>),
}

/// Menütitel aus mehreren Teilen mit eigener Farbe und Schrift.
fn rich(parts: &[(&str, Tone)]) -> Retained<NSAttributedString> {
    let size = NSFont::menuFontOfSize(0.0).pointSize();
    let strong = || NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { NSFontWeightSemibold });
    let title = NSMutableAttributedString::new();
    for &(text, tone) in parts {
        let (color, font) = match tone {
            Tone::Label => (NSColor::labelColor(), NSFont::menuFontOfSize(0.0)),
            Tone::Muted => (NSColor::secondaryLabelColor(), NSFont::menuFontOfSize(0.0)),
            Tone::Value => (NSColor::labelColor(), strong()),
            Tone::Good => (green(), strong()),
            Tone::Bad => (red(), strong()),
            Tone::Status(color) => (color(), NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })),
        };
        let keys = unsafe { [NSForegroundColorAttributeName, NSFontAttributeName] };
        let values: [Retained<AnyObject>; 2] =
            [Retained::into_super(Retained::into_super(color)), Retained::into_super(Retained::into_super(font))];
        text_with(&title, text, &NSDictionary::from_retained_objects(&keys, &values));
    }
    Retained::into_super(title)
}

/// Statuszeile: farbiger Punkt und Text in der Farbe des Zustands (wie das Symbol in
/// der Menüleiste: grün = läuft, orange = Pause; Feierabend indigo).
fn status_title(text: &str, kind: IconKind) -> Retained<NSAttributedString> {
    let color: fn() -> Retained<NSColor> = match kind {
        IconKind::Running | IconKind::Away => green,
        IconKind::Break | IconKind::Paused => orange,
        IconKind::Closed => indigo,
        IconKind::Absent => palette::absence,
        IconKind::Outside => NSColor::labelColor,
    };
    let dot: fn() -> Retained<NSColor> = match kind {
        IconKind::Outside => NSColor::systemGrayColor,
        _ => color,
    };
    rich(&[("●\u{2002}", Tone::Status(dot)), (text, Tone::Status(color))])
}

/// Farbe für Hinweise auf Regelverstöße (wie im Fenster „Aufzeichnungen“).
fn warning_color() -> Retained<NSColor> {
    NSColor::systemOrangeColor()
}

/// Längste Fehlerzeile im Menü (die Zeile bestimmt dessen Breite).
const DB_ERROR_MAX: usize = 90;

/// `Speichern fehlgeschlagen: database is locked` – auf eine Zeile und eine Länge gebracht,
/// die das Menü nicht sprengt.
fn db_error_text(what: &str, detail: &str) -> String {
    let text = format!("{what}: {}", detail.split_whitespace().collect::<Vec<_>>().join(" "));
    if text.chars().count() <= DB_ERROR_MAX {
        return text;
    }
    let cut: String = text.chars().take(DB_ERROR_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Fehlerzeile im Menü: rot mit Warnschild.
fn error_title(text: &str) -> Retained<NSAttributedString> {
    marked_title(text, &red())
}

/// Menütitel in Warnfarbe mit vorangestelltem Warnschild. Das Schild steckt im Text
/// (nicht im Bild des Menüeintrags), weil macOS Bilder in Menüs nicht immer anzeigt.
fn warning_title(text: &str) -> Retained<NSAttributedString> {
    marked_title(text, &warning_color())
}

/// Menütitel in der Farbe `color` mit vorangestelltem Warnschild in derselben Farbe.
fn marked_title(text: &str, color: &Retained<NSColor>) -> Retained<NSAttributedString> {
    let font = NSFont::menuFontOfSize(0.0);
    let keys = unsafe { [NSForegroundColorAttributeName, NSFontAttributeName] };
    let values: [Retained<AnyObject>; 2] = [
        Retained::into_super(Retained::into_super(color.clone())),
        Retained::into_super(Retained::into_super(font.clone())),
    ];
    let attrs = NSDictionary::from_retained_objects(&keys, &values);
    let title = NSMutableAttributedString::new();
    let mut indent = 0.0;
    if let Some(img) = warning_symbol(color, font.pointSize()) {
        let attachment = NSTextAttachment::new();
        let size = img.size();
        // Auf die Grundlinie setzen und mittig zu den Großbuchstaben ausrichten.
        let y = ((font.capHeight() - size.height) / 2.0).round();
        attachment.setBounds(NSRect::new(NSPoint::new(0.0, y), size));
        attachment.setImage(Some(&img));
        title.appendAttributedString(&NSAttributedString::attributedStringWithAttachment(&attachment));
        text_with(&title, "\u{2002}", &attrs);
        // Warnschild und Halbgeviert (halbe Schriftgröße) davor.
        indent = size.width + font.pointSize() / 2.0;
    }
    text_with(&title, text, &attrs);
    // Bricht die Zeile um, stehen die Folgezeilen unter dem Text, nicht unter dem Warnschild.
    let style = NSMutableParagraphStyle::new();
    style.setHeadIndent(indent);
    style.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    unsafe {
        title.addAttribute_value_range(NSParagraphStyleAttributeName, &style, NSRange::new(0, title.length()));
    }
    Retained::into_super(title)
}

fn text_with(title: &NSMutableAttributedString, text: &str, attrs: &NSDictionary<NSString, AnyObject>) {
    // Farbe und Schrift sind gültige Werte für diese Schlüssel.
    let part = unsafe { NSAttributedString::new_with_attributes(&ns(text), attrs) };
    title.appendAttributedString(&part);
}

/// Gefülltes Warnschild in Warnfarbe (kein Vorlagenbild, damit macOS es nicht umfärbt).
fn warning_symbol(color: &NSColor, point_size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &ns("exclamationmark.triangle.fill"),
        Some(&ns("Hinweis")),
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, unsafe {
        NSFontWeightRegular
    })
    // Weißes Ausrufezeichen auf vollflächig orangem Dreieck, wie das Warnschild von macOS.
    .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[NSColor::whiteColor(), color.retain()]),
    ));
    let image = image.imageWithSymbolConfiguration(&config)?;
    image.setTemplate(false);
    Some(image)
}

/// Größe des Menüleisten-Symbols wie bei den Symbolen von macOS (18 pt bei der
/// üblichen Menüleiste von 24 pt).
fn icon_height(item: &NSStatusItem) -> u32 {
    let thickness = item.statusBar().map(|b| b.thickness()).unwrap_or(24.0);
    (thickness - 6.0).round().clamp(12.0, 20.0) as u32
}

fn set_icon(button: &objc2_app_kit::NSStatusBarButton, kind: IconKind, set: IconSet, height: u32) {
    let img = glyphs::image(set, kind.glyph(), height as f64, 0.0, kind.symbol().1);
    button.setImage(Some(&img));
}

/// Arbeitszeit für die Menüleiste: `6:12` (ohne Einheit, damit sie wenig Platz braucht).
fn bar_time(work_secs: i64) -> String {
    let m = work_secs.max(0) / 60;
    format!("{}:{:02}", m / 60, m % 60)
}

/// Text rechts neben dem Symbol (leer = nur das Symbol). Gleich breite Ziffern, damit die
/// Nachbarn in der Menüleiste nicht jede Minute wandern.
fn set_bar_title(button: &objc2_app_kit::NSStatusBarButton, title: &str) {
    let size = NSFont::menuBarFontOfSize(0.0).pointSize();
    button.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(size, unsafe { NSFontWeightRegular })));
    button.setImagePosition(if title.is_empty() { objc2_app_kit::NSCellImagePosition::ImageOnly } else { objc2_app_kit::NSCellImagePosition::ImageLeading });
    button.setTitle(&ns(title));
}

/// Ein globaler Kurzbefehl wurde gedrückt.
fn run_hotkey(action: hotkey::Action) {
    match action {
        hotkey::Action::TogglePause => toggle_pause(),
        hotkey::Action::ShowRecords => open_records(),
    }
}

/// „Arbeit unterbrechen“ ↔ „Weiter arbeiten“; bei Arbeit ohne Mac „Zurück am Mac“.
fn toggle_pause() {
    act(|s, t| {
        // Freier Tag, der nicht zählt: Es gibt nichts zu unterbrechen (auch nicht per Kurzbefehl).
        if s.tracker.counting(t.date()) == Some(false) {
            return;
        }
        if s.tracker.is_running() {
            s.tracker.take_break(t);
        } else {
            s.tracker.start(t);
        }
    });
}

// ---------- Bildschirm gesperrt? ----------

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *mut AnyObject;
}

/// Liest `CGSSessionScreenIsLocked` aus der aktuellen Sitzung.
pub fn screen_locked() -> bool {
    unsafe {
        let raw = CGSessionCopyCurrentDictionary();
        let Some(dict) = Retained::from_raw(raw) else {
            return false;
        };
        let locked_key = NSString::from_str("CGSSessionScreenIsLocked");
        let locked: Option<Retained<AnyObject>> = msg_send![&*dict, objectForKey: &*locked_key];
        let is_locked = locked.map(|v| -> bool { msg_send![&*v, boolValue] }).unwrap_or(false);
        let console_key = NSString::from_str("kCGSSessionOnConsoleKey");
        let on_console: Option<Retained<AnyObject>> = msg_send![&*dict, objectForKey: &*console_key];
        let off_console = on_console.map(|v| -> bool { !msg_send![&*v, boolValue] }).unwrap_or(false);
        is_locked || off_console
    }
}

// ---------- Inaktivität ----------

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMCopyAssertionsStatus(status: *mut *mut AnyObject) -> i32;
}

/// Sekunden seit der letzten Eingabe (Tastatur, Maus, Trackpad) in dieser Sitzung. Braucht
/// keine Freigabe: Abgefragt wird nur die Zeit, keine Eingabe selbst.
fn idle_secs() -> i64 {
    // kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(0, u32::MAX) };
    if secs.is_finite() { secs.max(0.0) as i64 } else { 0 }
}

/// Hält gerade eine App den Bildschirm wach (Videokonferenz, Präsentation, Film)? Dann sitzt
/// vermutlich jemand davor, auch ohne Eingabe – der Mac gilt nicht als verlassen.
fn display_kept_awake() -> bool {
    unsafe {
        let mut raw: *mut AnyObject = std::ptr::null_mut();
        if IOPMCopyAssertionsStatus(&mut raw) != 0 {
            return false;
        }
        let Some(status) = Retained::from_raw(raw) else { return false };
        ["PreventUserIdleDisplaySleep", "NoDisplaySleepAssertion"].iter().any(|name| {
            let level: Option<Retained<AnyObject>> = msg_send![&*status, objectForKey: &*ns(name)];
            level.is_some_and(|v| -> bool {
                let n: i32 = msg_send![&*v, intValue];
                n > 0
            })
        })
    }
}

fn running_from_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

// ---------- Aufbau ----------

/// Anzeigezeile im Menü (Status, Kennzahlen und Hinweise): ein Eintrag mit eigener Ansicht statt
/// Titel. So lässt er sich weder anklicken noch hervorheben, behält aber seine Farben –
/// deaktivierte Einträge mit Titel graut macOS aus. Maße wie bei normalen Einträgen.
struct InfoLine {
    item: Retained<NSMenuItem>,
    label: Retained<NSTextField>,
}

impl InfoLine {
    /// Zeilenhöhe und Textposition normaler Menüeinträge (macOS 26 und neuer).
    const HEIGHT: f64 = 24.0;
    const LEFT: f64 = 14.0;
    const RIGHT: f64 = 14.0;

    fn new(mtm: MainThreadMarker) -> Self {
        let label = NSTextField::labelWithString(&ns(""), mtm);
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(Self::LEFT + Self::RIGHT, Self::HEIGHT)),
        );
        view.addSubview(&label);
        let item = NSMenuItem::new(mtm);
        item.setView(Some(&view));
        item.setEnabled(false);
        Self { item, label }
    }

    /// Schmaler als so bricht keine Zeile um (Punkte) – auch wenn Status und Kennzahlen
    /// einmal kürzer sind. So breit ist das Menü ohnehin durch seine übrigen Einträge.
    const MIN_WRAP: f64 = 280.0;

    /// Zeile, die bei Bedarf umbricht (Hinweise und Fehler, siehe [`InfoLine::set_wrapped`]).
    fn wrapping(mtm: MainThreadMarker) -> Self {
        let line = Self::new(mtm);
        line.label.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        line.label.setMaximumNumberOfLines(0);
        if let Some(cell) = line.label.cell() {
            cell.setWraps(true);
        }
        line
    }

    fn set(&self, title: &NSAttributedString) {
        self.label.setAttributedStringValue(title);
        self.place(self.label.fittingSize());
    }

    /// Wie [`InfoLine::set`], aber der Text ist höchstens `max_width` breit und bricht sonst
    /// um; die Zeile wird entsprechend höher.
    fn set_wrapped(&self, title: &NSAttributedString, max_width: f64) {
        self.label.setAttributedStringValue(title);
        self.label.setPreferredMaxLayoutWidth(max_width);
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(max_width, 10_000.0));
        let size = match self.label.cell() {
            Some(cell) => cell.cellSizeForBounds(bounds),
            None => self.label.fittingSize(),
        };
        self.place(NSSize::new(size.width.ceil().min(max_width), size.height.ceil()));
    }

    /// Text mit der Größe `size` einsetzen; die Zeile hat oben und unten den Abstand
    /// einer einzeiligen.
    fn place(&self, size: NSSize) {
        let height = Self::HEIGHT.max(size.height + Self::HEIGHT - Self::line_height());
        let y = ((height - size.height) / 2.0).round();
        self.label.setFrame(NSRect::new(NSPoint::new(Self::LEFT, y), size));
        if let Some(view) = self.item.view() {
            view.setFrameSize(NSSize::new(Self::LEFT + size.width + Self::RIGHT, height));
        }
    }

    /// Höhe einer Textzeile in der Schrift des Menüs.
    fn line_height() -> f64 {
        let font = NSFont::menuFontOfSize(0.0);
        (font.ascender() - font.descender() + font.leading()).ceil()
    }

    /// Breite des Texts, wie er zuletzt gesetzt wurde.
    fn text_width(&self) -> f64 {
        self.label.frame().size.width
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    target: &Controller,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns(title),
            action,
            &ns(key),
        )
    };
    if action.is_some() {
        unsafe { item.setTarget(Some(target)) };
    } else {
        item.setEnabled(false);
    }
    item
}

/// Menüpunkt mit Untermenü „Jetzt / Vor N Min. / Andere Uhrzeit …“.
fn time_menu(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    minutes: &[i64],
    target: &Controller,
    relative: &mut Vec<(Retained<NSMenuItem>, i64)>,
) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
    let sub = NSMenu::new(mtm);
    sub.setAutoenablesItems(false);
    for &m in minutes {
        let it = menu_item(mtm, "", Some(action), "", target);
        it.setTag(m as isize);
        sub.addItem(&it);
        relative.push((it, m));
    }
    sub.addItem(&NSMenuItem::separatorItem(mtm));
    let ask = menu_item(mtm, "Andere Uhrzeit …", Some(action), "", target);
    ask.setTag(TAG_ASK);
    sub.addItem(&ask);
    let parent = menu_item(mtm, title, None, "", target);
    parent.setEnabled(true);
    parent.setSubmenu(Some(&sub));
    (parent, sub)
}

/// Zeigt einen Fehler, bei dem Tagwerk nicht weiterlaufen kann.
fn show_fatal(mtm: MainThreadMarker, message: &str, info: &str) {
    eprintln!("Tagwerk: {message} {info}");
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(message));
    alert.setInformativeText(&ns(info));
    alert.addButtonWithTitle(&ns("Beenden"));
    activate_app(mtm);
    alert.runModal();
}

/// Bild des geöffneten Menüs für `--menu-screenshot`: Zustand setzen, das Menü öffnen, das
/// Menüfenster vom Fensterserver holen, speichern und beenden. Solange das Menü offen ist,
/// wartet der Hauptthread in dessen Schleife – deshalb laufen beide Schritte über Zeitgeber,
/// die auch dann feuern.
fn schedule_menu_screenshot(path: std::path::PathBuf) {
    match std::env::var("TAGWERK_MENU_STATE").as_deref() {
        Ok("break") => act(|s, t| {
            s.tracker.take_break(t);
        }),
        Ok("away") => act(|s, t| {
            s.tracker.go_away(t, Activity::Meeting);
        }),
        Ok("closed") => act(|s, t| {
            s.tracker.close_day(t, secs(t));
        }),
        _ => {}
    }
    let after = |seconds: f64, block: &block2::DynBlock<dyn Fn(std::ptr::NonNull<NSTimer>)>| unsafe {
        let timer = NSTimer::timerWithTimeInterval_repeats_block(seconds, false, block);
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    };
    // Erst das Bild (während das Menü offen ist), dann schließen und beenden.
    let shoot = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        // Das Menü liegt über allen normalen Fenstern; das größte Fenster dort ist es.
        let menu = snapshot::own_windows()
            .into_iter()
            .filter(|(_, layer, _)| *layer > 20)
            .max_by(|a, b| (a.2.width * a.2.height).total_cmp(&(b.2.width * b.2.height)));
        let result = match menu.and_then(|(id, ..)| snapshot::window_png(id, true)) {
            Some(png) => std::fs::write(&path, png).map_err(|e| e.to_string()),
            None => Err("Das geöffnete Menü ließ sich nicht aufnehmen (Bildschirm gesperrt?).".to_string()),
        };
        let menu = MainThreadMarker::new().and_then(|mtm| with_state(|s| s.status_item.menu(mtm)).flatten());
        if let Some(menu) = menu {
            menu.cancelTracking();
        }
        shutdown();
        if let Err(e) = result {
            eprintln!("Fehler: {e}");
            std::process::exit(1);
        }
        std::process::exit(0);
    });
    after(2.0, &shoot);
    let open = block2::RcBlock::new(|_timer: std::ptr::NonNull<NSTimer>| {
        // Außerhalb von `with_state`: Beim Öffnen fragt das Menü den Zustand selbst ab.
        let button = MainThreadMarker::new().and_then(|mtm| with_state(|s| s.status_item.button(mtm)).flatten());
        if let Some(button) = button {
            unsafe { button.performClick(None) };
        }
    });
    after(0.8, &open);
}

/// Hält eine Sperrdatei, damit nur eine Instanz läuft.
fn single_instance() -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(store::data_dir());
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(store::data_dir().join(".lock"))
        .ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

/// Startet die Menüleisten-App. Mit `screenshot` (zum Testen und für die Bilder im README:
/// `Tagwerk --menu-screenshot <png>`) öffnet sie ihr Menü, speichert ein Bild davon und endet
/// wieder; den Zustand dafür setzt `TAGWERK_MENU_STATE=break|away|closed` (sonst läuft die
/// Erfassung).
pub fn run(screenshot: Option<std::path::PathBuf>) {
    let mtm = MainThreadMarker::new().expect("Tagwerk muss im Hauptthread starten");
    let Some(_lock) = single_instance() else {
        eprintln!("Tagwerk läuft bereits.");
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let controller = Controller::new(mtm);

    // Menü
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*controller)));

    let mut relative = Vec::new();
    let (start_item, start_sub) =
        time_menu(mtm, "Arbeitsbeginn festlegen", sel!(setStart:), &[0, 15, 30], &controller, &mut relative);
    let start_reset = menu_item(mtm, "Erstes Entsperren verwenden", Some(sel!(setStart:)), "", &controller);
    start_reset.setTag(TAG_RESET);
    start_sub.addItem(&NSMenuItem::separatorItem(mtm));
    start_sub.addItem(&start_reset);
    let (close, _) = time_menu(mtm, "Feierabend", sel!(closeDay:), &[0, 15, 30], &controller, &mut relative);

    // Ohne Mac weiterarbeiten: Meeting, Telefonat, Networking, Sonstiges
    let away_sub = NSMenu::new(mtm);
    away_sub.setAutoenablesItems(false);
    for activity in Activity::ALL {
        let title = activity.label();
        let it = menu_item(mtm, title, Some(sel!(goAway:)), "", &controller);
        it.setTag(activity.id());
        set_item_symbol(&it, activity_symbol(activity), title);
        away_sub.addItem(&it);
    }
    let away_note = menu_item(mtm, "Zählt als Arbeitszeit, auch bei gesperrtem Mac – bis zum Entsperren.", None, "", &controller);
    away_sub.addItem(&NSMenuItem::separatorItem(mtm));
    away_sub.addItem(&away_note);
    let away = menu_item(mtm, "Ohne Mac weiterarbeiten", None, "", &controller);
    away.setEnabled(true);
    away.setSubmenu(Some(&away_sub));

    // Pause als Arbeitszeit zählen (die Pausen von heute; wird in `refresh` gefüllt)
    let count_pause_menu = NSMenu::new(mtm);
    count_pause_menu.setAutoenablesItems(false);
    let count_pause = menu_item(mtm, "Pause als Arbeitszeit zählen", None, "", &controller);
    count_pause.setEnabled(true);
    count_pause.setSubmenu(Some(&count_pause_menu));
    count_pause.setHidden(true);

    let items = MenuItems {
        // Status, Kennzahlen und Hinweise sind reine Anzeige (siehe `InfoLine`).
        status: InfoLine::new(mtm),
        worked: InfoLine::new(mtm),
        pauses: InfoLine::new(mtm),
        first: InfoLine::new(mtm),
        balance: InfoLine::new(mtm),
        account: InfoLine::new(mtm),
        hints: (0..6).map(|_| InfoLine::wrapping(mtm)).collect(),
        error: InfoLine::wrapping(mtm),
        count_day: menu_item(mtm, "Heute erfassen", Some(sel!(countDay:)), "", &controller),
        pause: menu_item(mtm, "", Some(sel!(togglePause:)), "p", &controller),
        start: start_item.clone(),
        start_reset,
        close,
        reopen: menu_item(mtm, "Feierabend aufheben", Some(sel!(reopenDay:)), "", &controller),
        relative,
        away,
        count_pause,
        count_pause_menu,
        count_pause_list: Vec::new(),
        controller: controller.clone(),
    };
    let records = menu_item(mtm, "Aufzeichnungen …", Some(sel!(showRecords:)), "a", &controller);
    let settings = menu_item(mtm, "Einstellungen …", Some(sel!(showSettings:)), ",", &controller);
    let about = menu_item(mtm, "Über Tagwerk", Some(sel!(showAbout:)), "", &controller);
    let quit = menu_item(mtm, "Tagwerk beenden", Some(sel!(quit:)), "q", &controller);

    for (item, symbol, desc) in [
        (&items.count_day, "calendar.badge.clock", "Heute erfassen"),
        (&items.away, "figure.walk", "Ohne Mac weiterarbeiten"),
        (&items.count_pause, "arrow.uturn.backward.circle", "Pause als Arbeitszeit zählen"),
        (&start_item, "sunrise", "Arbeitsbeginn"),
        (&items.close, "moon", "Feierabend"),
        (&items.reopen, "arrow.uturn.backward", "Feierabend aufheben"),
        (&records, "tablecells", "Aufzeichnungen"),
        (&settings, "gearshape", "Einstellungen"),
        (&about, "info.circle", "Über Tagwerk"),
        (&quit, "power", "Beenden"),
    ] {
        set_item_symbol(item, symbol, desc);
    }
    // Ein Fehler der Datenbank steht direkt unter dem Status, vor den Kennzahlen.
    for line in
        [&items.status, &items.error, &items.worked, &items.pauses, &items.first, &items.balance, &items.account]
            .into_iter()
            .chain(&items.hints)
    {
        menu.addItem(&line.item);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&items.count_day);
    menu.addItem(&items.pause);
    menu.addItem(&items.away);
    menu.addItem(&items.count_pause);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&start_item);
    menu.addItem(&items.close);
    menu.addItem(&items.reopen);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&records);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&settings);
    menu.addItem(&about);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&quit);

    let status_item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    status_item.setMenu(Some(&menu));

    // Daten laden und – falls entsperrt und kein Feierabend – sofort starten.
    let db = match store::Db::open() {
        Ok(db) => db,
        Err(e) => {
            show_fatal(mtm, &format!("Die Datenbank {} konnte nicht geöffnet werden.", store::db_file().display()), &e.to_string());
            return;
        }
    };
    // Lässt sich die Datenbank öffnen, aber nicht lesen, läuft Tagwerk mit leerem Stand weiter –
    // das steht dann im Menü, statt unbemerkt zu bleiben.
    let mut db_error = None;
    let days = db.load().unwrap_or_else(|e| {
        eprintln!("Tagwerk: Datenbank konnte nicht gelesen werden: {e}");
        db_error = Some((DbFault::Read, db_error_text("Die Datenbank konnte nicht gelesen werden", &e.to_string())));
        Vec::new()
    });
    let settings = db.load_settings().unwrap_or_else(|e| {
        eprintln!("Tagwerk: Einstellungen konnten nicht gelesen werden: {e}");
        db_error.get_or_insert((
            DbFault::Read,
            db_error_text("Die Einstellungen konnten nicht gelesen werden", &e.to_string()),
        ));
        Default::default()
    });
    let mut tracker = Tracker::new(days, settings);
    let t = now();
    tracker.roll_over(t);
    let locked = screen_locked();
    // Pause über „Arbeit unterbrechen“ oder Arbeit ohne Mac vor dem Beenden der App? Dann
    // bleibt sie bestehen (Arbeit ohne Mac auch bei gesperrtem Mac).
    let status = store::read_status();
    let open_break = match status {
        Some(store::Status::Break(since)) if !locked && since <= t && since.date() == t.date() => Some(since),
        _ => None,
    };
    let open_away = match status {
        Some(store::Status::Away(since, a)) if since <= t && since.date() == t.date() => Some((since, a)),
        _ => None,
    };
    if let Some((since, activity)) = open_away {
        tracker.restore_away(since, activity);
    } else if let Some(since) = open_break {
        tracker.restore_break(since);
    } else if locked || !tracker.start(t) {
        tracker.pause_reason = if locked { PauseReason::Locked } else { PauseReason::Manual };
        tracker.paused_since = Some(t);
    }

    // Bereits erreichte Schwellen (Höchstarbeitszeit, Pause) nicht noch einmal melden.
    let today = tracker.today(t);
    let limit = LimitWatch::init(t.date(), today.work_secs);
    let breaks = BreakWatch::init(t.date(), today.stretch_secs);
    notify::init(mtm, running_from_bundle());
    notify::on_lock_answer(lock_answered);
    let mut state = AppState {
        limit,
        breaks,
        asked_lock: None,
        tracker,
        status_item,
        items,
        last_save: Instant::now(),
        last_icon: None,
        button_icon: None,
        button_title: None,
        hotkeys: None,
        db,
        db_error,
    };
    state.persist(t);
    state.refresh();
    STATE.with(|s| *s.borrow_mut() = Some(state));

    // Erster Start der installierten App: automatisch bei Anmeldung starten.
    let marker = store::data_dir().join(".login-item-set");
    if running_from_bundle() && !marker.exists() {
        if let Err(e) = login_item::set(true) {
            eprintln!("Tagwerk: {e}");
        }
        let _ = std::fs::write(&marker, "1");
    }

    // Sperren / Entsperren
    unsafe {
        let dnc = NSDistributedNotificationCenter::defaultCenter();
        dnc.addObserver_selector_name_object(
            &controller,
            sel!(screenLocked:),
            Some(&ns("com.apple.screenIsLocked")),
            None,
        );
        dnc.addObserver_selector_name_object(
            &controller,
            sel!(screenUnlocked:),
            Some(&ns("com.apple.screenIsUnlocked")),
            None,
        );

        // Ruhezustand, Benutzerwechsel
        let wnc = NSWorkspace::sharedWorkspace().notificationCenter();
        for (name, action) in [
            ("NSWorkspaceWillSleepNotification", sel!(willSleep:)),
            ("NSWorkspaceDidWakeNotification", sel!(didWake:)),
            ("NSWorkspaceSessionDidResignActiveNotification", sel!(sessionResigned:)),
            ("NSWorkspaceSessionDidBecomeActiveNotification", sel!(sessionActive:)),
            ("NSWorkspaceWillPowerOffNotification", sel!(willPowerOff:)),
        ] {
            wnc.addObserver_selector_name_object(&controller, action, Some(&ns(name)), None);
        }

        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &controller,
            sel!(willTerminate:),
            Some(&ns("NSApplicationWillTerminateNotification")),
            None,
        );

        // Regelmäßig aktualisieren (auch während das Menü offen ist)
        let timer = NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            15.0,
            &controller,
            sel!(tick:),
            None,
            true,
        );
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }

    if let Some(path) = screenshot {
        schedule_menu_screenshot(path);
    }
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSAttributedStringNSStringDrawing, NSBezierPath, NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
    };
    use objc2_foundation::NSSize;

    #[test]
    fn pause_titles_follow_shown_minutes() {
        // 15:25:40 – 15:45:10 zeigt 15:25 – 15:45 und damit 20 Min. (nicht 19).
        let hms = |h: i64, m: i64, s: i64| h * 3600 + m * 60 + s;
        assert_eq!(pause_title(Span::new(hms(15, 25, 40), hms(15, 45, 10))), "15:25 – 15:45 Uhr  (20 Min.)");
        assert_eq!(pause_title(Span::new(hms(12, 0, 0), hms(13, 5, 0))), "12:00 – 13:05 Uhr  (1:05 h)");
    }

    #[test]
    fn idle_queries_answer() {
        // Nur dass die Abfragen an macOS antworten (die Werte hängen vom Rechner ab).
        assert!(idle_secs() >= 0);
        let _ = display_kept_awake();
    }

    /// Prüft die Abfrage an macOS mit einem Programm, das den Bildschirm wirklich wach hält:
    /// `cargo test caffeinate_keeps_the_display_awake -- --ignored`
    #[test]
    #[ignore]
    fn caffeinate_keeps_the_display_awake() {
        let mut child = std::process::Command::new("caffeinate").args(["-d", "-t", "10"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let awake = display_kept_awake();
        let _ = child.kill();
        let _ = child.wait();
        assert!(awake);
    }

    #[test]
    fn idle_status_names_the_last_input() {
        let at = |h: u32, m: u32| chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap().and_hms_opt(h, m, 0).unwrap();
        let mut t = Tracker::new(vec![], Default::default());
        t.start(at(8, 0));
        t.go_idle(at(10, 0));
        let (text, icon) = status_line(&t, at(10, 12));
        assert!(icon == IconKind::Paused);
        assert_eq!(text, "Keine Eingabe seit 10:00 Uhr – pausiert");
    }

    #[test]
    fn bar_time_is_compact() {
        assert_eq!(bar_time(0), "0:00");
        assert_eq!(bar_time(6 * 3600 + 12 * 60 + 59), "6:12");
        assert_eq!(bar_time(10 * 3600 + 5 * 60), "10:05");
    }

    #[test]
    fn count_pause_tags_round_trip() {
        for activity in Activity::ALL {
            for from in [0, 12 * 3600 + 3 * 60 + 17, day::DAY_END - 1] {
                assert!(count_pause_from_tag(count_pause_tag(from, activity)) == (from, activity));
            }
        }
    }

    #[test]
    fn db_error_text_fits_one_menu_line() {
        assert_eq!(db_error_text("Speichern fehlgeschlagen", "database is locked"), "Speichern fehlgeschlagen: database is locked");
        let long = db_error_text("Speichern fehlgeschlagen", &"disk I/O error\n ".repeat(20));
        assert_eq!(long.chars().count(), DB_ERROR_MAX);
        assert!(long.ends_with('…') && !long.contains('\n'), "{long}");
    }

    #[test]
    fn away_outside_frame_shows_outside() {
        let at = |h: u32, m: u32| chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap().and_hms_opt(h, m, 0).unwrap();
        let mut t = Tracker::new(vec![], Default::default());
        assert!(t.go_away(at(6, 9), Activity::Other));
        let (text, icon) = status_line(&t, at(6, 30));
        assert!(icon == IconKind::Outside, "{text}");
        assert_eq!(text, "Ohne Mac seit 06:09 Uhr – außerhalb des Arbeitszeitrahmens");
        // Ab Beginn des Rahmens zählt es: grün wie „läuft“.
        let (text, icon) = status_line(&t, at(7, 5));
        assert!(icon == IconKind::Away, "{text}");
        assert_eq!(text, "Ohne Mac seit 06:09 Uhr");
        assert_eq!(icon.glyph(), glyphs::State::Running);
        assert_eq!(IconKind::Outside.glyph(), glyphs::State::Outside);
    }

    /// Vorschau der Status- und Kennzahl-Zeilen (hell und dunkel, @2x):
    /// `TAGWERK_MENU_PNG=/pfad/menu cargo test render_menu_lines -- --ignored`
    #[test]
    #[ignore]
    fn render_menu_lines() {
        let base = std::env::var("TAGWERK_MENU_PNG").expect("TAGWERK_MENU_PNG");
        for (mode, name, bg) in [
            ("light", unsafe { NSAppearanceNameAqua }, (0.93, 0.93, 0.93)),
            ("dark", unsafe { NSAppearanceNameDarkAqua }, (0.17, 0.18, 0.2)),
        ] {
            #[allow(deprecated)]
            unsafe {
                NSAppearance::setCurrentAppearance(NSAppearance::appearanceNamed(name).as_deref())
            };
            let lines = [
                status_title("Zeiterfassung läuft", IconKind::Running),
                status_title("Pause seit 12:03 Uhr", IconKind::Break),
                status_title("Ohne Mac seit 14:00 Uhr – Meeting", IconKind::Away),
                status_title("Außerhalb des Arbeitszeitrahmens (07:00–22:00 Uhr)", IconKind::Outside),
                status_title("Feierabend um 17:30 Uhr", IconKind::Closed),
                error_title(&db_error_text("Speichern fehlgeschlagen", "database is locked")),
                warning_title(&rules::Warning::LongStretch { work: 7 * 3600 }.text()),
                rich(&[("Heute gearbeitet:  ", Tone::Label), ("3:57 h", Tone::Value), ("  von 7:48 h", Tone::Muted)]),
                rich(&[("Pausen heute:  ", Tone::Label), ("0:35 h", Tone::Value)]),
                rich(&[("Beginn heute:  ", Tone::Label), ("12:42 Uhr", Tone::Value), ("  (manuell)", Tone::Muted)]),
                rich(&[("Saldo diese Woche:  ", Tone::Label), ("−3:51 h", Tone::Bad)]),
                rich(&[("Saldo diese Woche:  ", Tone::Label), ("+1:12 h", Tone::Good)]),
            ];
            let (w, h) = (900isize, (lines.len() as isize) * 52 + 20);
            let rep = unsafe {
                NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                    NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
                )
            }
            .unwrap();
            // @2x: Punkte = halbe Pixel
            rep.setSize(NSSize::new(w as f64 / 2.0, h as f64 / 2.0));
            NSGraphicsContext::saveGraphicsState_class();
            let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).unwrap();
            NSGraphicsContext::setCurrentContext(Some(&ctx));
            NSColor::colorWithSRGBRed_green_blue_alpha(bg.0, bg.1, bg.2, 1.0).set();
            NSBezierPath::fillRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w as f64, h as f64)));
            for (i, line) in lines.iter().enumerate() {
                line.drawAtPoint(NSPoint::new(16.0, (h as f64 / 2.0) - 30.0 - i as f64 * 26.0));
            }
            NSGraphicsContext::restoreGraphicsState_class();
            let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
                .unwrap();
            std::fs::write(format!("{base}_{mode}.png"), data.to_vec()).unwrap();
        }
    }
}
