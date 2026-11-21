//! Fenster „Einstellungen“ nach den macOS Human Interface Guidelines: nicht modal,
//! Änderungen gelten sofort, kein „OK“/„Abbrechen“.
//!
//! Vier Reiter: „Allgemein“ (Anmeldung, Menüleiste, Kurzbefehle), „Arbeitszeit“
//! (Arbeitszeitregeln je Stand, Zeitkonto), „Erfassung“ (Arbeit ohne Mac, Inaktivität,
//! Nachfrage nach langer Sperre, freie Tage) und „Mitteilungen“ (Stand der Erlaubnis, woran
//! Tagwerk erinnert). Gespeichert wird in der Datenbank; die jeweils andere
//! Tagwerk-Instanz (Menüleiste bzw. Fenster „Aufzeichnungen“) übernimmt die Werte.
//!
//! Ausnahme vom sofortigen Gelten sind die Arbeitszeitregeln (Rahmen, Regelarbeitszeit,
//! Arbeitstage, Mindestpause): Sie gelten ab einem Datum, damit eine Änderung frühere Tage
//! nicht neu rechnet. Der Reiter zeigt alle Stände als Liste; ein Blatt legt einen Stand an
//! oder ändert ihn, gespeichert wird erst mit „Übernehmen“.
//!
//! „Bei Anmeldung starten“ steht nicht in der Datenbank, sondern ist das Anmeldeobjekt
//! von macOS; „Standardwerte“ lässt es unverändert.
//!
//! „Mitteilungen“ zeigt, ob Tagwerk Mitteilungen zeigen darf (Erinnerung an die
//! Höchstarbeitszeit); wenn nicht, öffnet ein Knopf Systemeinstellungen › Mitteilungen.
//! Der Stand wird jedes Mal abgefragt, wenn das Fenster aktiv wird.
//!
//! Rechts in der Symbolleiste öffnet ein Ordner-Knopf den Datenordner im Finder.

use crate::day::{fmt_clock, secs_of, DAY_END};
use crate::glyphs;
use crate::hotkey;
use crate::login_item;
use crate::notify::{self, Permission};
use crate::rules::{self, fmt_day, IconSet, Settings, WorkRules, IDLE_RANGE, MIN_PAUSE_RANGE, MIN_WORK_RANGE, WEEKLY_RANGE};
use crate::store;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use block2::RcBlock;
use objc2_foundation::{
    NSArray, NSCopying, NSDate, NSInteger, NSLocale, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::cell::RefCell;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("Hauptthread")
}

/// Fester Bezugstag für die Uhrzeit-Felder (keine Zeitumstellung).
fn ref_date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2001, 1, 1).unwrap()
}

fn to_nsdate(secs: i64) -> Retained<NSDate> {
    let t: NaiveDateTime = ref_date().and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(secs.clamp(0, DAY_END - 60));
    let ts = Local.from_local_datetime(&t).earliest().map(|d| d.timestamp()).unwrap_or(0);
    NSDate::dateWithTimeIntervalSince1970(ts as f64)
}

/// Mittag des Tages `day` (für Datumsfelder; Mittag, damit die Zeitzone den Tag nicht verschiebt).
fn day_to_nsdate(day: NaiveDate) -> Retained<NSDate> {
    let ts = day
        .and_hms_opt(12, 0, 0)
        .and_then(|t| Local.from_local_datetime(&t).earliest())
        .map(|d| d.timestamp())
        .unwrap_or_else(|| Local::now().timestamp());
    NSDate::dateWithTimeIntervalSince1970(ts as f64)
}

fn nsdate_to_day(d: &NSDate) -> Option<NaiveDate> {
    DateTime::from_timestamp(d.timeIntervalSince1970().floor() as i64, 0).map(|u| u.with_timezone(&Local).date_naive())
}

fn picker_secs(p: &NSDatePicker) -> Option<i64> {
    let ts = p.dateValue().timeIntervalSince1970();
    DateTime::from_timestamp(ts.floor() as i64, 0).map(|u| secs_of(u.with_timezone(&Local).naive_local().time()))
}

struct State {
    window: Retained<NSWindow>,
    _controller: Retained<Controller>,
    db: Option<store::Db>,
    settings: Settings,
    login: Retained<NSButton>,
    notify_icon: Retained<NSImageView>,
    notify_status: Retained<NSTextField>,
    notify_button: Retained<NSButton>,
    notify_note: Retained<NSTextField>,
    /// Arbeitszeitregeln, die heute gelten (und geplante Stände).
    rules_now: Retained<NSTextField>,
    /// Liste aller Stände (Zeilen in [`ROWS`]).
    rules_table: Retained<NSTableView>,
    rules_edit: Retained<NSButton>,
    rules_delete: Retained<NSButton>,
    sheet: Sheet,
    work: Retained<NSTextField>,
    work_stepper: Retained<NSStepper>,
    icon_popup: Retained<NSPopUpButton>,
    icon_preview: Vec<Retained<NSImageView>>,
    show_time: Retained<NSButton>,
    hotkeys: Retained<NSButton>,
    account_check: Retained<NSButton>,
    account_date: Retained<NSDatePicker>,
    carry: Retained<NSTextField>,
    idle_check: Retained<NSButton>,
    idle: Retained<NSTextField>,
    idle_stepper: Retained<NSStepper>,
    ask: Retained<NSButton>,
    /// Automatische Erfassung nur an Arbeitstagen zählen.
    free_days: Retained<NSButton>,
    notify_limit: Retained<NSButton>,
    notify_break: Retained<NSButton>,
    tabs: Retained<NSTabView>,
    error: Retained<NSTextField>,
    on_change: fn(),
}

/// Blatt „Arbeitszeit ändern“: ein Stand der Arbeitszeitregeln. Die Felder ändern nur den
/// Entwurf `draft`; gespeichert wird erst mit „Übernehmen“.
struct Sheet {
    window: Retained<NSWindow>,
    title: Retained<NSTextField>,
    /// Welcher Stand bearbeitet wird: `None` ein neuer, `Some(from)` der vorhandene ab `from`
    /// (`Some(None)`: der erste, der seit jeher gilt – sein Datum steht fest).
    editing: Option<Option<NaiveDate>>,
    draft: WorkRules,
    valid_from: Retained<NSDatePicker>,
    /// Statt des Datums beim ersten Stand: „seit Beginn“.
    begin: Retained<NSTextField>,
    /// Was der Stand bewirkt (bis wann er gilt, ob er rückwirkend ändert …).
    hint: Retained<NSTextField>,
    frame_from: Retained<NSDatePicker>,
    frame_to: Retained<NSDatePicker>,
    weekly: Retained<NSTextField>,
    weekly_stepper: Retained<NSStepper>,
    /// Arbeitstage Montag bis Freitag (Tag der Checkbox = Bit in `WorkRules::workdays`).
    workdays: Vec<Retained<NSButton>>,
    daily_note: Retained<NSTextField>,
    pause: Retained<NSTextField>,
    pause_stepper: Retained<NSStepper>,
    error: Retained<NSTextField>,
}

/// Eine Zeile der Liste der Stände.
struct RuleRow {
    from: Option<NaiveDate>,
    cells: [String; 5],
    /// Gilt heute.
    current: bool,
    /// Gilt erst ab einem künftigen Tag.
    planned: bool,
}

/// Spalten der Liste: Kennung, Titel, Breite.
const RULE_COLUMNS: [(&str, &str, f64); 5] = [
    ("from", "Gültig ab", 86.0),
    ("weekly", "Woche", 72.0),
    ("days", "Arbeitstage", 108.0),
    ("frame", "Rahmen", 98.0),
    ("pause", "Pause ab", 60.0),
];

/// Vorgabe im Feld „Minuten ohne Eingabe“, solange die Erkennung aus ist.
const IDLE_DEFAULT: i64 = 10 * 60;

/// Höhe der Vorschau (wie in der Menüleiste) und der Symbole im Auswahlmenü.
const PREVIEW_H: f64 = 18.0;
const POPUP_ICON_H: f64 = 14.0;

/// Zustände in der Reihenfolge der Vorschau.
const PREVIEW_STATES: [(glyphs::State, &str); 4] = [
    (glyphs::State::Running, "Zeiterfassung läuft"),
    (glyphs::State::Break, "Pause"),
    (glyphs::State::Outside, "Außerhalb des Arbeitszeitrahmens"),
    (glyphs::State::Closed, "Feierabend"),
];

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Zeilen der Liste der Stände – getrennt von `STATE`, weil die Tabelle sie auch abfragt,
    /// während `STATE` gerade geliehen ist (beim Neuladen in `State::fill`).
    static ROWS: RefCell<Vec<RuleRow>> = const { RefCell::new(Vec::new()) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Wie [`with_state`], aber ohne Wirkung, solange `STATE` schon geliehen ist (Meldungen der
/// Tabelle mitten in `State::fill`).
fn try_with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkSettingsController"]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSWindowDelegate for Controller {
        /// Zurück aus den Systemeinstellungen: Stand der Mitteilungen neu abfragen.
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _n: &NSNotification) {
            refresh_permission();
        }
    }

    unsafe impl NSTableViewDataSource for Controller {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            ROWS.with(|r| r.borrow().len() as NSInteger)
        }
    }

    unsafe impl NSControlTextEditingDelegate for Controller {}

    unsafe impl NSTableViewDelegate for Controller {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _table: &NSTableView, column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<NSView>> {
            let id = column.map(|c| c.identifier().to_string());
            rule_cell(id.as_deref(), row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            try_with_state(|s| s.update_rule_buttons());
        }
    }

    unsafe impl NSToolbarDelegate for Controller {
        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn toolbar_default(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn toolbar_allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids()
        }

        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn toolbar_item(&self, _toolbar: &NSToolbar, id: &NSString, _insert: bool) -> Option<Retained<NSToolbarItem>> {
            toolbar_item(self, id)
        }
    }

    impl Controller {
        /// Liste der Stände: „Neu …“.
        #[unsafe(method(newRules:))]
        fn new_rules(&self, _sender: Option<&AnyObject>) {
            open_sheet(None);
        }

        /// Liste der Stände: „Bearbeiten …“ für die gewählte Zeile.
        #[unsafe(method(editRules:))]
        fn edit_rules(&self, _sender: Option<&AnyObject>) {
            if let Some(from) = with_state(|s| row_from(s.rules_table.selectedRow())).flatten() {
                open_sheet(Some(from));
            }
        }

        /// Doppelklick auf eine Zeile: sie bearbeiten.
        #[unsafe(method(rulesDoubleClicked:))]
        fn rules_double_clicked(&self, _sender: Option<&AnyObject>) {
            if let Some(from) = with_state(|s| row_from(s.rules_table.clickedRow())).flatten() {
                open_sheet(Some(from));
            }
        }

        /// Liste der Stände: „Löschen“ für die gewählte Zeile (nicht der erste Stand).
        #[unsafe(method(deleteRules:))]
        fn delete_rules(&self, _sender: Option<&AnyObject>) {
            if let Some(Some(from)) = with_state(|s| row_from(s.rules_table.selectedRow())).flatten() {
                confirm_delete_rules(from);
            }
        }

        #[unsafe(method(sheetDateChanged:))]
        fn sheet_date_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| s.fill_sheet());
        }

        #[unsafe(method(sheetFrameChanged:))]
        fn sheet_frame_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let sh = &mut s.sheet;
                let (Some(from), Some(to)) = (picker_secs(&sh.frame_from), picker_secs(&sh.frame_to)) else {
                    return;
                };
                if to <= from {
                    show_sheet_error(sh, "Das Ende des Arbeitszeitrahmens muss nach seinem Beginn liegen.");
                    return;
                }
                sh.draft.frame_from = from;
                sh.draft.frame_to = to;
                sh.error.setHidden(true);
                s.fill_sheet();
            });
        }

        #[unsafe(method(sheetWeeklyChanged:))]
        fn sheet_weekly_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = rules::parse_hours(&s.sheet.weekly.stringValue().to_string());
                s.set_weekly(value);
            });
        }

        #[unsafe(method(sheetWeeklyStepped:))]
        fn sheet_weekly_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = (s.sheet.weekly_stepper.doubleValue() * 3600.0).round() as i64;
                s.set_weekly(Some(value));
            });
        }

        #[unsafe(method(sheetPauseChanged:))]
        fn sheet_pause_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = s.sheet.pause.stringValue().to_string().trim().parse::<i64>().ok().map(|m| m * 60);
                s.set_pause(value);
            });
        }

        #[unsafe(method(sheetPauseStepped:))]
        fn sheet_pause_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = s.sheet.pause_stepper.integerValue() as i64 * 60;
                s.set_pause(Some(value));
            });
        }

        /// Arbeitstag ein- oder ausgeschaltet (Tag der Checkbox = Bit); einer bleibt immer.
        #[unsafe(method(sheetWorkdayChanged:))]
        fn sheet_workday_changed(&self, sender: Option<&NSButton>) {
            let Some(sender) = sender else { return };
            let bit = 1u8 << sender.tag().clamp(0, 4);
            let on = sender.state() == NSControlStateValueOn;
            with_state(|s| {
                let sh = &mut s.sheet;
                let days = if on { sh.draft.workdays | bit } else { sh.draft.workdays & !bit };
                if days & rules::ALL_WORKDAYS == 0 {
                    show_sheet_error(sh, "Mindestens ein Arbeitstag muss bleiben.");
                } else {
                    sh.draft.workdays = days;
                    sh.error.setHidden(true);
                }
                s.fill_sheet();
            });
        }

        #[unsafe(method(sheetCancel:))]
        fn sheet_cancel(&self, _sender: Option<&AnyObject>) {
            close_sheet();
        }

        /// „Übernehmen“: Stand setzen, speichern, Blatt schließen.
        #[unsafe(method(sheetApply:))]
        fn sheet_apply(&self, _sender: Option<&AnyObject>) {
            // Eine laufende Eingabe abschließen – das löst die Aktion des Felds aus.
            if let Some(sheet) = with_state(|s| s.sheet.window.clone()) {
                sheet.makeFirstResponder(None);
            }
            let ok = with_state(|s| {
                // Ungültige Eingabe: Das Blatt bleibt offen, die Meldung steht darin.
                if !s.sheet.error.isHidden() {
                    return false;
                }
                let replaces = s.sheet.editing.flatten();
                let from = s.sheet_from();
                s.settings.set_rules(from, s.sheet.draft, replaces);
                true
            });
            if ok == Some(true) {
                close_sheet();
                save();
            }
        }

        #[unsafe(method(workChanged:))]
        fn work_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = s.work.stringValue().to_string().trim().parse::<i64>().ok().map(|m| m * 60);
                s.set_work(value);
            });
            save();
        }

        #[unsafe(method(workStepped:))]
        fn work_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = step_minutes(s.settings.min_work / 60, s.work_stepper.integerValue() as i64);
                s.set_work(Some(value * 60));
            });
            save();
        }

        /// Knopf in der Titelleiste: Datenordner im Finder öffnen.
        #[unsafe(method(openDataFolder:))]
        fn open_data_folder(&self, _sender: Option<&AnyObject>) {
            let dir = store::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let url = NSURL::fileURLWithPath(&ns(&dir.to_string_lossy()));
            NSWorkspace::sharedWorkspace().openURL(&url);
        }

        #[unsafe(method(iconSetChanged:))]
        fn icon_set_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let i = s.icon_popup.indexOfSelectedItem();
                if let Some(&set) = usize::try_from(i).ok().and_then(|i| IconSet::ALL.get(i)) {
                    s.settings.icon_set = set;
                }
            });
            save();
        }

        /// „Zählt erst ab“ ein- oder ausgeschaltet bzw. das Datum geändert.
        #[unsafe(method(accountFromChanged:))]
        fn account_from_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let on = s.account_check.state() == NSControlStateValueOn;
                s.settings.account_from = if on { nsdate_to_day(&s.account_date.dateValue()) } else { None };
            });
            save();
        }

        #[unsafe(method(carryChanged:))]
        fn carry_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| match rules::parse_balance(&s.carry.stringValue().to_string()) {
                Some(v) => {
                    s.settings.account_carry = v;
                    s.error.setHidden(true);
                }
                None => s.show_error("Der Übertrag ist ein Saldo wie +12:30 oder −3:15."),
            });
            save();
        }

        #[unsafe(method(idleToggled:))]
        fn idle_toggled(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                if s.idle_check.state() == NSControlStateValueOn {
                    let v = s.idle.stringValue().to_string().trim().parse::<i64>().ok().map(|m| m * 60);
                    s.settings.idle_secs = v.filter(|v| (IDLE_RANGE.0..=IDLE_RANGE.1).contains(v)).unwrap_or(IDLE_DEFAULT);
                } else {
                    s.settings.idle_secs = 0;
                }
                s.error.setHidden(true);
            });
            save();
        }

        #[unsafe(method(idleChanged:))]
        fn idle_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let v = s.idle.stringValue().to_string().trim().parse::<i64>().ok().map(|m| m * 60);
                s.set_idle(v);
            });
            save();
        }

        #[unsafe(method(idleStepped:))]
        fn idle_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let v = s.idle_stepper.integerValue() as i64 * 60;
                s.set_idle(Some(v));
            });
            save();
        }

        /// Die übrigen Schalter: Nachfrage nach Sperre und woran Tagwerk erinnert.
        #[unsafe(method(switchChanged:))]
        fn switch_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let on = |b: &NSButton| b.state() == NSControlStateValueOn;
                s.settings.ask_after_lock = on(&s.ask);
                s.settings.auto_workdays_only = on(&s.free_days);
                s.settings.notify_limit = on(&s.notify_limit);
                s.settings.notify_break = on(&s.notify_break);
            });
            save();
        }

        #[unsafe(method(showTimeChanged:))]
        fn show_time_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| s.settings.show_time = s.show_time.state() == NSControlStateValueOn);
            save();
        }

        #[unsafe(method(hotkeysChanged:))]
        fn hotkeys_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| s.settings.hotkeys = s.hotkeys.state() == NSControlStateValueOn);
            save();
        }

        /// Anmeldeobjekt sofort ein- oder austragen (nicht in der Datenbank).
        #[unsafe(method(loginChanged:))]
        fn login_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let enable = s.login.state() == NSControlStateValueOn;
                match login_item::set(enable) {
                    Ok(()) => s.error.setHidden(true),
                    Err(e) => s.show_error(&e),
                }
                s.fill();
            });
        }

        #[unsafe(method(openNotificationSettings:))]
        fn open_notification_settings(&self, _sender: Option<&AnyObject>) {
            notify::open_settings();
        }

        #[unsafe(method(resetDefaults:))]
        fn reset_defaults(&self, _sender: Option<&AnyObject>) {
            // Die Arbeitszeitregeln gelten ab heute wieder wie zu Beginn; frühere Tage behalten
            // ihre Stände, geplante entfallen.
            let today = today();
            with_state(|s| {
                let mut fresh = Settings { work: s.settings.work.clone(), ..Settings::default() };
                fresh.work.retain(|p| p.from.is_none_or(|f| f <= today));
                fresh.set_rules(Some(today), WorkRules::DEFAULT, None);
                s.settings = fresh;
                s.error.setHidden(true);
            });
            save();
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

impl State {
    /// Alle Felder aus `self.settings` neu befüllen.
    fn fill(&self) {
        let s = &self.settings;
        let state = |on: bool| if on { NSControlStateValueOn } else { NSControlStateValueOff };
        self.login.setState(if login_item::enabled() { NSControlStateValueOn } else { NSControlStateValueOff });
        let today = today();
        self.rules_now.setStringValue(&ns(&rules_now_text(s, today)));
        ROWS.with(|r| *r.borrow_mut() = rule_rows(s, today));
        self.rules_table.reloadData();
        self.update_rule_buttons();
        self.work.setStringValue(&ns(&(s.min_work / 60).to_string()));
        self.work_stepper.setIntegerValue((s.min_work / 60) as isize);
        self.account_check.setState(state(s.account_from.is_some()));
        self.account_date.setEnabled(s.account_from.is_some());
        // Ohne Beginn als Vorschlag der 1. Januar dieses Jahres.
        let from = s.account_from.unwrap_or_else(|| today.with_month(1).and_then(|d| d.with_day(1)).unwrap_or(today));
        self.account_date.setDateValue(&day_to_nsdate(from));
        self.carry.setStringValue(&ns(&rules::fmt_signed_hm(s.account_carry)));
        let idle_on = s.idle_secs > 0;
        let idle_min = if idle_on { s.idle_secs } else { IDLE_DEFAULT } / 60;
        self.idle_check.setState(state(idle_on));
        self.idle.setEnabled(idle_on);
        self.idle_stepper.setEnabled(idle_on);
        self.idle.setStringValue(&ns(&idle_min.to_string()));
        self.idle_stepper.setIntegerValue(idle_min as isize);
        self.ask.setState(state(s.ask_after_lock));
        self.free_days.setState(state(s.auto_workdays_only));
        self.notify_limit.setState(state(s.notify_limit));
        self.notify_break.setState(state(s.notify_break));
        let index = IconSet::ALL.iter().position(|&x| x == s.icon_set).unwrap_or(0);
        self.icon_popup.selectItemAtIndex(index as isize);
        for (view, (state, desc)) in self.icon_preview.iter().zip(PREVIEW_STATES) {
            view.setImage(Some(&glyphs::image(s.icon_set, state, PREVIEW_H, 0.0, desc)));
        }
        self.show_time.setState(state(s.show_time));
        self.hotkeys.setState(state(s.hotkeys));
    }

    /// „Bearbeiten …“ und „Löschen“ nach der Auswahl in der Liste (der erste Stand bleibt).
    fn update_rule_buttons(&self) {
        let row = self.rules_table.selectedRow();
        self.rules_edit.setEnabled(row >= 0);
        self.rules_delete.setEnabled(row > 0);
    }

    /// Ab wann der Stand im Blatt gilt: `None` beim ersten Stand (seit jeher), sonst das Datum.
    fn sheet_from(&self) -> Option<NaiveDate> {
        match self.sheet.editing {
            Some(None) => None,
            _ => Some(nsdate_to_day(&self.sheet.valid_from.dateValue()).unwrap_or_else(today)),
        }
    }

    /// Felder des Blatts aus dem Entwurf neu befüllen, dazu Hinweis und Tagessoll.
    fn fill_sheet(&self) {
        let sh = &self.sheet;
        let r = &sh.draft;
        let first = sh.editing == Some(None);
        sh.valid_from.setHidden(first);
        sh.begin.setHidden(!first);
        sh.frame_from.setDateValue(&to_nsdate(r.frame_from));
        sh.frame_to.setDateValue(&to_nsdate(r.frame_to));
        sh.weekly.setStringValue(&ns(&rules::fmt_hours(r.weekly_target)));
        sh.weekly_stepper.setDoubleValue(r.weekly_target as f64 / 3600.0);
        sh.pause.setStringValue(&ns(&(r.min_pause / 60).to_string()));
        sh.pause_stepper.setIntegerValue((r.min_pause / 60) as isize);
        sh.daily_note.setStringValue(&ns(&format!(
            "{}  ·  {} pro Arbeitstag",
            r.week_text(),
            store::fmt_hm(r.daily_target_workday())
        )));
        for (bit, check) in sh.workdays.iter().enumerate() {
            check.setState(if r.workdays & (1 << bit) != 0 { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        let (text, warn) = sheet_hint(&self.settings, sh.editing, self.sheet_from(), r, today());
        sh.hint.setStringValue(&ns(&text));
        sh.hint.setTextColor(Some(&*if warn { NSColor::systemOrangeColor() } else { NSColor::secondaryLabelColor() }));
    }

    fn set_weekly(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (WEEKLY_RANGE.0..=WEEKLY_RANGE.1).contains(&v) => {
                self.sheet.draft.weekly_target = v;
                self.sheet.error.setHidden(true);
            }
            _ => show_sheet_error(
                &self.sheet,
                &format!(
                    "Die Regelarbeitszeit muss zwischen {} und {} Stunden pro Woche liegen.",
                    WEEKLY_RANGE.0 / 3600,
                    WEEKLY_RANGE.1 / 3600
                ),
            ),
        }
        self.fill_sheet();
    }

    fn set_pause(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (MIN_PAUSE_RANGE.0..=MIN_PAUSE_RANGE.1).contains(&v) => {
                self.sheet.draft.min_pause = v;
                self.sheet.error.setHidden(true);
            }
            _ => show_sheet_error(
                &self.sheet,
                &format!(
                    "Die Mindestpause muss zwischen {} und {} Minuten liegen.",
                    MIN_PAUSE_RANGE.0 / 60,
                    MIN_PAUSE_RANGE.1 / 60
                ),
            ),
        }
        self.fill_sheet();
    }

    fn set_work(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (MIN_WORK_RANGE.0..=MIN_WORK_RANGE.1).contains(&v) => {
                self.settings.min_work = v;
                self.error.setHidden(true);
            }
            _ => self.show_error(&format!(
                "Die Mindestdauer für Arbeit ohne Mac muss zwischen {} und {} Minuten liegen.",
                MIN_WORK_RANGE.0 / 60,
                MIN_WORK_RANGE.1 / 60
            )),
        }
    }

    /// Inaktivität in Sekunden übernehmen (nur wenn die Erkennung eingeschaltet ist).
    fn set_idle(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (IDLE_RANGE.0..=IDLE_RANGE.1).contains(&v) => {
                self.settings.idle_secs = v;
                self.error.setHidden(true);
            }
            _ => self.show_error(&format!(
                "Die Inaktivität muss zwischen {} und {} Minuten liegen.",
                IDLE_RANGE.0 / 60,
                IDLE_RANGE.1 / 60
            )),
        }
    }

    /// Stand der Mitteilungen anzeigen; Knopf nur, wenn in den Systemeinstellungen etwas zu tun ist.
    fn set_permission(&self, p: Permission) {
        let (symbol, color) = match p {
            Permission::Allowed => ("checkmark.circle.fill", NSColor::systemGreenColor()),
            Permission::Unavailable => ("bell.slash", NSColor::secondaryLabelColor()),
            _ => ("exclamationmark.triangle.fill", NSColor::systemOrangeColor()),
        };
        self.notify_icon
            .setImage(NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), Some(&ns(p.text()))).as_deref());
        self.notify_icon.setContentTintColor(Some(&color));
        self.notify_status.setStringValue(&ns(p.text()));
        self.notify_note.setStringValue(&ns(p.note()));
        self.notify_button.setHidden(!p.needs_settings());
    }

    fn show_error(&self, text: &str) {
        self.error.setStringValue(&ns(text));
        self.error.setHidden(false);
    }
}

/// Stand der Mitteilungen abfragen und anzeigen (Antwort kommt später im Hauptthread).
fn refresh_permission() {
    notify::permission(|p| {
        with_state(|s| s.set_permission(p));
    });
}

// ---------- Arbeitszeitregeln ----------

/// `39 Std./Woche (Mo–Fr, 7:48 h pro Arbeitstag)  ·  Rahmen 07:00–22:00 Uhr  ·  Pausen ab 15 Min.`
fn rules_line(r: &WorkRules) -> String {
    format!(
        "{} Std./Woche ({}, {} pro Arbeitstag)  ·  Rahmen {}  ·  Pausen ab {} Min.",
        rules::fmt_hours(r.weekly_target),
        r.workdays_text(),
        store::fmt_hm(r.daily_target_workday()),
        r.frame_text(),
        r.min_pause / 60
    )
}

/// Über der Liste: welcher Stand heute gilt und welcher als nächster geplant ist – mit dem
/// Tagessoll, das die Liste nicht zeigt (höchstens zwei Zeilen, siehe `build`).
fn rules_now_text(s: &Settings, today: NaiveDate) -> String {
    let per_day = |r: &WorkRules| format!("{} pro Arbeitstag ({})", store::fmt_hm(r.daily_target_workday()), r.week_text());
    let current = s.work.iter().rev().find(|p| p.from.is_none_or(|f| f <= today)).copied().unwrap_or(s.work[0]);
    let since = current.from.map_or_else(|| "seit Beginn".to_string(), |f| format!("ab {}", fmt_day(f)));
    let mut text = format!("Heute gilt der Stand {since}: {}.", per_day(&current.rules));
    if let Some(next) = s.work.iter().find(|p| p.from.is_some_and(|f| f > today)) {
        text.push_str(&format!("\nGeplant ab {}: {}.", fmt_day(next.from.unwrap_or(today)), per_day(&next.rules)));
    }
    text
}

fn rule_rows(s: &Settings, today: NaiveDate) -> Vec<RuleRow> {
    let current = s.work.iter().rposition(|p| p.from.is_none_or(|f| f <= today));
    s.work
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let r = &p.rules;
            RuleRow {
                from: p.from,
                cells: [
                    p.from.map_or_else(|| "Beginn".to_string(), fmt_day),
                    format!("{} Std.", rules::fmt_hours(r.weekly_target)),
                    r.workdays_text(),
                    format!("{}–{}", fmt_clock(r.frame_from), fmt_clock(r.frame_to)),
                    format!("{} Min.", r.min_pause / 60),
                ],
                current: current == Some(i),
                planned: p.from.is_some_and(|f| f > today),
            }
        })
        .collect()
}

/// Stand der Zeile `row` (`None`: keine Zeile; `Some(None)`: der erste Stand).
fn row_from(row: NSInteger) -> Option<Option<NaiveDate>> {
    let row = usize::try_from(row).ok()?;
    ROWS.with(|r| r.borrow().get(row).map(|x| x.from))
}

fn rule_cell(column: Option<&str>, row: NSInteger) -> Option<Retained<NSView>> {
    let mtm = mtm();
    let i = RULE_COLUMNS.iter().position(|c| Some(c.0) == column)?;
    ROWS.with(|rows| {
        let rows = rows.borrow();
        let r = rows.get(usize::try_from(row).ok()?)?;
        let text = label(&r.cells[i]);
        let weight = if r.current { unsafe { NSFontWeightSemibold } } else { unsafe { NSFontWeightRegular } };
        text.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::systemFontSize(), weight)));
        if r.planned {
            text.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }
        text.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let tip = match (r.current, r.planned) {
            (true, _) => "Gilt heute",
            (_, true) => "Geplant – gilt ab diesem Tag",
            _ => "Galt früher",
        };
        let cell = NSTableCellView::new(mtm);
        cell.setToolTip(Some(&ns(tip)));
        text.setTranslatesAutoresizingMaskIntoConstraints(false);
        cell.addSubview(&text);
        text.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
        text.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 4.0).setActive(true);
        text.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
        unsafe { cell.setTextField(Some(&text)) };
        Some(Retained::into_super(cell))
    })
}

/// Hinweis im Blatt: bis wann der Stand gilt und ob er rückwirkend ändert (dann `true`:
/// orange). `editing`/`from` wie in [`Sheet`], `draft` die Regeln im Blatt.
fn sheet_hint(
    s: &Settings,
    editing: Option<Option<NaiveDate>>,
    from: Option<NaiveDate>,
    draft: &WorkRules,
    today: NaiveDate,
) -> (String, bool) {
    // Die anderen Stände – ohne den, der gerade bearbeitet wird.
    let others: Vec<_> = s.work.iter().filter(|p| editing != Some(p.from)).collect();
    let next = others.iter().filter_map(|p| p.from).find(|f| from.is_none_or(|from| *f > from));
    let Some(from) = from else {
        return match next {
            Some(n) => (
                format!(
                    "Gilt für alle Tage vor dem {}. Saldo und Aufzeichnungen dieser Tage ändern sich.",
                    fmt_day(n)
                ),
                true,
            ),
            None => ("Gilt für alle Tage. Saldo und Aufzeichnungen ändern sich auch rückwirkend.".to_string(), true),
        };
    };
    let mut parts = Vec::new();
    parts.push(match next {
        Some(n) => format!("Gilt vom {} bis {}.", fmt_day(from), fmt_day(n.pred_opt().unwrap_or(n))),
        None => format!("Gilt ab {}.", fmt_day(from)),
    });
    let replaced = others.iter().any(|p| p.from == Some(from));
    if replaced {
        parts.push(format!("Ersetzt den vorhandenen Stand ab {}.", fmt_day(from)));
    }
    let before = others.iter().rev().find(|p| p.from.is_none_or(|f| f < from)).map(|p| p.rules);
    if !replaced && before == Some(*draft) {
        parts.push("Gleicht dem Stand davor – es ändert sich nichts.".to_string());
    }
    let past = from < today;
    if past {
        parts.push(format!("Rückwirkend: Saldo und Aufzeichnungen ab dem {} ändern sich.", fmt_day(from)));
    }
    (parts.join(" "), past || replaced)
}

fn show_sheet_error(sheet: &Sheet, text: &str) {
    sheet.error.setStringValue(&ns(text));
    sheet.error.setHidden(false);
}

/// Blatt öffnen: für einen neuen Stand (`None`, ab heute mit den Regeln von heute) oder den
/// vorhandenen ab `Some(from)`.
fn open_sheet(editing: Option<Option<NaiveDate>>) {
    let today = today();
    let windows = with_state(|s| {
        if s.window.attachedSheet().is_some() {
            return None;
        }
        let existing = editing.and_then(|from| s.settings.work.iter().find(|p| p.from == from).copied());
        let (date, rules, title) = match (editing, existing) {
            (Some(None), Some(p)) => (today, p.rules, "Arbeitszeit seit Beginn".to_string()),
            (Some(Some(from)), Some(p)) => (from, p.rules, format!("Arbeitszeit ab {} bearbeiten", fmt_day(from))),
            _ => (today, *s.settings.rules(today), "Arbeitszeit ändern".to_string()),
        };
        let sh = &mut s.sheet;
        sh.editing = if existing.is_some() { editing } else { None };
        sh.draft = rules;
        sh.title.setStringValue(&ns(&title));
        sh.valid_from.setDateValue(&day_to_nsdate(date));
        sh.error.setHidden(true);
        s.fill_sheet();
        Some((s.window.clone(), s.sheet.window.clone()))
    })
    .flatten();
    if let Some((window, sheet)) = windows {
        window.beginSheet_completionHandler(&sheet, None);
        sheet.makeFirstResponder(None);
    }
}

fn close_sheet() {
    if let Some((window, sheet)) = with_state(|s| (s.window.clone(), s.sheet.window.clone())) {
        window.endSheet(&sheet);
        sheet.orderOut(None);
    }
}

/// Nachfrage vor dem Löschen des Stands ab `from`.
fn confirm_delete_rules(from: NaiveDate) {
    let Some((window, before)) = with_state(|s| {
        let before = *s.settings.rules(from.pred_opt().unwrap_or(from));
        (s.window.clone(), before)
    }) else {
        return;
    };
    if window.attachedSheet().is_some() {
        return;
    }
    let alert = NSAlert::new(mtm());
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&ns(&format!("Stand ab {} löschen?", fmt_day(from))));
    let mut info = format!("Ab dem {} gilt dann wieder der Stand davor: {}.", fmt_day(from), rules_line(&before));
    if from <= today() {
        info.push_str(&format!(" Saldo und Aufzeichnungen ab dem {} ändern sich.", fmt_day(from)));
    }
    alert.setInformativeText(&ns(&info));
    let delete = alert.addButtonWithTitle(&ns("Löschen"));
    delete.setHasDestructiveAction(true);
    // Return bestätigt nicht versehentlich: Standard ist „Abbrechen“.
    delete.setKeyEquivalent(&ns(""));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\r"));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSAlertFirstButtonReturn {
            return;
        }
        with_state(|s| s.settings.remove_rules(from));
        save();
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

/// Speichern, Felder angleichen und die App benachrichtigen.
fn save() {
    let notify = with_state(|s| {
        s.fill();
        let result = match s.db.as_mut() {
            Some(db) => db.save_settings(&s.settings, today()).map_err(|e| e.to_string()),
            None => Err("Die Datenbank ist nicht geöffnet.".to_string()),
        };
        match result {
            Ok(()) => Some(s.on_change),
            Err(e) => {
                s.show_error(&format!("Speichern fehlgeschlagen: {e}"));
                None
            }
        }
    })
    .flatten();
    if let Some(f) = notify {
        f();
    }
}

// ---------- Aufbau ----------

fn label(text: &str) -> Retained<NSTextField> {
    NSTextField::labelWithString(&ns(text), mtm())
}

fn caption(text: &str) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm());
    l.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l.setSelectable(false);
    l.setPreferredMaxLayoutWidth(330.0);
    l
}

fn time_picker(target: &Controller, action: Sel) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm());
    p.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    p.setDatePickerElements(NSDatePickerElementFlags::HourMinute);
    p.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    p.setBezeled(true);
    p.setDrawsBackground(true);
    unsafe {
        p.setTarget(Some(target));
        p.setAction(Some(action));
    }
    p
}

fn number_field(target: &Controller, action: Sel) -> Retained<NSTextField> {
    let f = NSTextField::textFieldWithString(&ns(""), mtm());
    f.setAlignment(NSTextAlignment::Right);
    f.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(NSFont::systemFontSize(), unsafe {
        NSFontWeightRegular
    })));
    f.setTranslatesAutoresizingMaskIntoConstraints(false);
    f.widthAnchor().constraintEqualToConstant(52.0).setActive(true);
    if let Some(cell) = f.cell() {
        cell.setSendsActionOnEndEditing(true);
    }
    unsafe {
        f.setTarget(Some(target));
        f.setAction(Some(action));
    }
    f
}

/// Schrittweite der Pfeile für die Mindestdauer von Arbeit ohne Mac (Minuten).
const WORK_STEP: i64 = 5;

/// Neuer Wert nach einem Klick auf die Pfeile, wenn vorher `old` Minuten galten und der
/// Schalter `stepped` meldet: Es geht in Fünferschritten über glatte Werte (1 → 5 → 10 …),
/// auch von einem krummen eingetippten Wert aus (12 → 15 bzw. 10), und unter 5 direkt auf
/// die Untergrenze.
fn step_minutes(old: i64, stepped: i64) -> i64 {
    let (min, max) = (MIN_WORK_RANGE.0 / 60, MIN_WORK_RANGE.1 / 60);
    let value = if stepped > old {
        (stepped / WORK_STEP * WORK_STEP).max(old + 1)
    } else if stepped < old {
        let up = (stepped + WORK_STEP - 1) / WORK_STEP * WORK_STEP;
        if up < old { up } else { min }
    } else {
        old
    };
    value.clamp(min, max)
}

fn stepper(target: &Controller, action: Sel, min: f64, max: f64, step: f64) -> Retained<NSStepper> {
    let s = NSStepper::new(mtm());
    s.setMinValue(min);
    s.setMaxValue(max);
    s.setIncrement(step);
    s.setValueWraps(false);
    s.setAutorepeat(true);
    unsafe {
        s.setTarget(Some(target));
        s.setAction(Some(action));
    }
    s
}

fn hstack(views: &[&NSView]) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm());
    s.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    s.setAlignment(NSLayoutAttribute::FirstBaseline);
    s.setSpacing(6.0);
    for v in views {
        s.addArrangedSubview(v);
    }
    s
}

/// Auswahl des Symbolsatzes; jeder Eintrag zeigt sein Symbol für „läuft“.
fn icon_popup(target: &Controller) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm()), NSRect::ZERO, false);
    // gleich breite Symbole, damit die Namen untereinander stehen
    let w = IconSet::ALL.iter().map(|&set| glyphs::width(set, POPUP_ICON_H)).fold(0.0, f64::max);
    for set in IconSet::ALL {
        p.addItemWithTitle(&ns(set.name()));
        if let Some(item) = p.lastItem() {
            item.setImage(Some(&glyphs::image(set, glyphs::State::Running, POPUP_ICON_H, w, set.name())));
        }
    }
    unsafe {
        p.setTarget(Some(target));
        p.setAction(Some(sel!(iconSetChanged:)));
    }
    p
}

/// Vorschau aller Zustände des gewählten Satzes (Bilder setzt `State::fill`).
fn icon_preview() -> (Retained<NSStackView>, Vec<Retained<NSImageView>>) {
    let stack = NSStackView::new(mtm());
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    stack.setAlignment(NSLayoutAttribute::CenterY);
    stack.setSpacing(16.0);
    let views: Vec<_> = PREVIEW_STATES
        .iter()
        .map(|(_, desc)| {
            let v = NSImageView::new(mtm());
            v.setImageScaling(NSImageScaling::ScaleNone);
            // Template-Bilder wie in der Menüleiste in Textfarbe (sonst grau)
            v.setContentTintColor(Some(&NSColor::textColor()));
            v.setToolTip(Some(&ns(desc)));
            stack.addArrangedSubview(&v);
            v
        })
        .collect();
    (stack, views)
}

fn view<T: AsRef<NSView>>(v: &T) -> Retained<NSView> {
    v.as_ref().retain()
}

/// Kennung des Ordner-Knopfs in der Symbolleiste.
const TB_FOLDER: &str = "folder";

fn toolbar_ids() -> Retained<NSArray<NSString>> {
    let flex: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[flex, ns(TB_FOLDER)])
}

/// Ordner-Knopf rechts in der Symbolleiste (wie die Knöpfe im Fenster „Aufzeichnungen“).
fn toolbar_item(target: &Controller, id: &NSString) -> Option<Retained<NSToolbarItem>> {
    if id.to_string() != TB_FOLDER {
        return None;
    }
    let label = "Datenordner";
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm()), id);
    item.setLabel(&ns(label));
    item.setPaletteLabel(&ns(label));
    item.setToolTip(Some(&ns("Datenordner im Finder zeigen")));
    item.setImage(NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("folder"), Some(&ns(label))).as_deref());
    item.setBordered(true);
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(sel!(openDataFolder:)));
    }
    Some(item)
}

/// Breite der Liste der Stände (und der Texte darum).
const RULES_W: f64 = 470.0;

/// Wie [`caption`], aber so breit wie die Liste der Stände.
fn wide_caption(text: &str) -> Retained<NSTextField> {
    let l = caption(text);
    l.setPreferredMaxLayoutWidth(RULES_W);
    l
}

/// Liste der Stände in einer Rolle mit fester Größe (gut vier Zeilen).
fn rules_table(target: &Controller) -> (Retained<NSScrollView>, Retained<NSTableView>) {
    let mtm = mtm();
    let table = NSTableView::new(mtm);
    table.setStyle(NSTableViewStyle::Plain);
    table.setUsesAlternatingRowBackgroundColors(true);
    table.setAllowsMultipleSelection(false);
    table.setAllowsEmptySelection(true);
    table.setAllowsColumnReordering(false);
    table.setAllowsColumnResizing(false);
    table.setRowHeight(20.0);
    table.setIntercellSpacing(NSSize::new(8.0, 2.0));
    table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
    for (id, title, width) in RULE_COLUMNS {
        let col = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(id));
        col.setTitle(&ns(title));
        col.setWidth(width);
        col.setResizingMask(NSTableColumnResizingOptions::empty());
        table.addTableColumn(&col);
    }
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(target)));
        table.setDelegate(Some(ProtocolObject::from_ref(target)));
        table.setTarget(Some(target));
        table.setDoubleAction(Some(sel!(rulesDoubleClicked:)));
    }
    let scroll = NSScrollView::new(mtm);
    scroll.setDocumentView(Some(&table));
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setBorderType(NSBorderType::BezelBorder);
    scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    scroll.widthAnchor().constraintEqualToConstant(RULES_W).setActive(true);
    scroll.heightAnchor().constraintEqualToConstant(116.0).setActive(true);
    (scroll, table)
}

/// Das Blatt „Arbeitszeit ändern“ (einmal gebaut, bei jedem Öffnen neu befüllt).
fn build_sheet(target: &Controller) -> Sheet {
    let mtm = mtm();
    let title = label("");
    title.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
    let valid_from = NSDatePicker::new(mtm);
    valid_from.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    valid_from.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
    valid_from.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    valid_from.setBezeled(true);
    valid_from.setDrawsBackground(true);
    unsafe {
        valid_from.setTarget(Some(target));
        valid_from.setAction(Some(sel!(sheetDateChanged:)));
    }
    let begin = label("Beginn");
    let hint = caption("");
    let frame_from = time_picker(target, sel!(sheetFrameChanged:));
    let frame_to = time_picker(target, sel!(sheetFrameChanged:));
    let weekly = number_field(target, sel!(sheetWeeklyChanged:));
    let weekly_stepper = stepper(
        target,
        sel!(sheetWeeklyStepped:),
        WEEKLY_RANGE.0 as f64 / 3600.0,
        WEEKLY_RANGE.1 as f64 / 3600.0,
        0.5,
    );
    let workdays: Vec<Retained<NSButton>> = ["Mo", "Di", "Mi", "Do", "Fr"]
        .iter()
        .enumerate()
        .map(|(bit, name)| {
            let c = unsafe {
                NSButton::checkboxWithTitle_target_action(&ns(name), Some(target), Some(sel!(sheetWorkdayChanged:)), mtm)
            };
            c.setTag(bit as isize);
            c
        })
        .collect();
    let daily_note = caption("");
    let pause = number_field(target, sel!(sheetPauseChanged:));
    let pause_stepper = stepper(
        target,
        sel!(sheetPauseStepped:),
        (MIN_PAUSE_RANGE.0 / 60) as f64,
        (MIN_PAUSE_RANGE.1 / 60) as f64,
        1.0,
    );
    let error = caption("");
    error.setTextColor(Some(&NSColor::systemRedColor()));
    error.setHidden(true);
    let cancel = unsafe {
        NSButton::buttonWithTitle_target_action(&ns("Abbrechen"), Some(target), Some(sel!(sheetCancel:)), mtm)
    };
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    let apply = unsafe {
        NSButton::buttonWithTitle_target_action(&ns("Übernehmen"), Some(target), Some(sel!(sheetApply:)), mtm)
    };
    apply.setKeyEquivalent(&ns("\r"));

    let date_row = hstack(&[&valid_from, &begin]);
    let weekly_row = hstack(&[&weekly, &weekly_stepper, &label("Stunden pro Woche")]);
    let day_views: Vec<&NSView> = workdays.iter().map(|c| &***c as &NSView).collect();
    let days_row = hstack(&day_views);
    days_row.setSpacing(12.0);
    let frame_row = hstack(&[&frame_from, &label("bis"), &frame_to, &label("Uhr")]);
    let pause_row = hstack(&[&pause, &pause_stepper, &label("Minuten")]);
    let row = |left: Option<&str>, right: &NSView| -> Retained<NSArray<NSView>> {
        let left = match left {
            Some(text) => view(&label(text)),
            None => NSGridCell::emptyContentView(mtm),
        };
        NSArray::from_retained_slice(&[left, right.retain()])
    };
    let rows = [
        row(Some("Gültig ab:"), &date_row),
        row(None, &hint),
        row(Some("Regelarbeitszeit:"), &weekly_row),
        row(Some("Arbeitstage:"), &days_row),
        row(None, &daily_note),
        row(Some("Arbeitszeitrahmen:"), &frame_row),
        row(
            None,
            &caption("Außerhalb dieses Zeitraums wird weder Arbeitszeit noch eine Pause gezählt."),
        ),
        row(Some("Mindestpause:"), &pause_row),
        row(
            None,
            &caption(
                "Ist der Mac kürzer gesperrt, zählt die Zeit weiter zur Arbeitszeit. \
                 Nach dem Arbeitszeitgesetz zählen Pausen erst ab 15 Minuten.",
            ),
        ),
    ];
    let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
    grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
    grid.setColumnSpacing(8.0);
    grid.setRowSpacing(4.0);
    grid.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Trailing);
    for r in [2, 5, 7] {
        grid.rowAtIndex(r).setTopPadding(14.0);
    }
    // Knöpfe rechts, wie in Blättern üblich
    let buttons = NSStackView::new(mtm);
    buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    buttons.setSpacing(10.0);
    buttons.addView_inGravity(&cancel, NSStackViewGravity::Trailing);
    buttons.addView_inGravity(&apply, NSStackViewGravity::Trailing);
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(14.0);
    for v in [&*title as &NSView, &grid, &error, &buttons] {
        column.addArrangedSubview(v);
    }
    column.setCustomSpacing_afterView(10.0, &grid);
    buttons.widthAnchor().constraintEqualToAnchor(&column.widthAnchor()).setActive(true);
    column.setTranslatesAutoresizingMaskIntoConstraints(false);
    let size = column.fittingSize();
    let (w, h) = (size.width + 40.0, size.height + 40.0);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let root = window.contentView().expect("contentView");
    root.addSubview(&column);
    column.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    column.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -20.0).setActive(true);
    column.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    column.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -20.0).setActive(true);
    Sheet {
        window,
        title,
        editing: None,
        draft: WorkRules::DEFAULT,
        valid_from,
        begin,
        hint,
        frame_from,
        frame_to,
        weekly,
        weekly_stepper,
        workdays,
        daily_note,
        pause,
        pause_stepper,
        error,
    }
}

fn build(on_change: fn()) -> State {
    let mtm = mtm();
    let controller = Controller::new(mtm);
    let target = &*controller;

    let login = unsafe {
        NSButton::checkboxWithTitle_target_action(&ns("Bei Anmeldung starten"), Some(target), Some(sel!(loginChanged:)), mtm)
    };
    let show_time = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &ns("Arbeitszeit von heute zeigen"),
            Some(target),
            Some(sel!(showTimeChanged:)),
            mtm,
        )
    };
    show_time.setToolTip(Some(&ns("Zeigt die Arbeitszeit von heute neben dem Symbol in der Menüleiste, z. B. 6:12")));
    let hotkeys = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &ns("In jeder App, auch bei geschlossenem Menü"),
            Some(target),
            Some(sel!(hotkeysChanged:)),
            mtm,
        )
    };
    let checkbox = |title: &str, action: Sel| unsafe {
        NSButton::checkboxWithTitle_target_action(&ns(title), Some(target), Some(action), mtm)
    };
    let account_check = checkbox("Zählt erst ab", sel!(accountFromChanged:));
    let account_date = NSDatePicker::new(mtm);
    account_date.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    account_date.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
    account_date.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    account_date.setBezeled(true);
    account_date.setDrawsBackground(true);
    unsafe {
        account_date.setTarget(Some(target));
        account_date.setAction(Some(sel!(accountFromChanged:)));
    }
    let carry = number_field(target, sel!(carryChanged:));
    carry.widthAnchor().constraintEqualToConstant(76.0).setPriority(999.0);
    let idle_check = checkbox("Erfassung anhalten nach", sel!(idleToggled:));
    let idle = number_field(target, sel!(idleChanged:));
    let idle_stepper =
        stepper(target, sel!(idleStepped:), (IDLE_RANGE.0 / 60) as f64, (IDLE_RANGE.1 / 60) as f64, 1.0);
    let ask = checkbox("Beim Entsperren nach langer Sperre nachfragen", sel!(switchChanged:));
    let free_days = checkbox("Automatische Erfassung nur an Arbeitstagen zählen", sel!(switchChanged:));
    let notify_limit = checkbox("Höchstarbeitszeit (bei 9:45 und 10 Std. Arbeit)", sel!(switchChanged:));
    let notify_break = checkbox("Pause (bei 5:45 und 6 Std. Arbeit am Stück)", sel!(switchChanged:));
    let notify_icon = NSImageView::new(mtm);
    notify_icon.setImageScaling(NSImageScaling::ScaleNone);
    let notify_status = label("");
    let notify_button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &ns("Systemeinstellungen öffnen"),
            Some(target),
            Some(sel!(openNotificationSettings:)),
            mtm,
        )
    };
    notify_button.setControlSize(NSControlSize::Small);
    notify_button.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    notify_button.setHidden(true);
    let notify_note = caption("");
    // Zwei Zeilen Platz (heute und geplant), auch wenn beim Aufbau noch nichts darin steht.
    let rules_now = wide_caption("Heute\nGeplant");
    rules_now.setTextColor(Some(&NSColor::labelColor()));
    let (rules_scroll, rules_table) = rules_table(target);
    let button = |title: &str, action: Sel| unsafe {
        NSButton::buttonWithTitle_target_action(&ns(title), Some(target), Some(action), mtm)
    };
    let rules_new = button("Neu …", sel!(newRules:));
    rules_new.setToolTip(Some(&ns("Neuen Stand der Arbeitszeitregeln anlegen, gültig ab einem Datum")));
    let rules_edit = button("Bearbeiten …", sel!(editRules:));
    let rules_delete = button("Löschen", sel!(deleteRules:));
    rules_delete.setToolTip(Some(&ns("Ab dem Datum gilt wieder der Stand davor")));
    let sheet = build_sheet(target);
    let work = number_field(target, sel!(workChanged:));
    let work_stepper = stepper(
        target,
        sel!(workStepped:),
        (MIN_WORK_RANGE.0 / 60) as f64,
        (MIN_WORK_RANGE.1 / 60) as f64,
        WORK_STEP as f64,
    );
    let icon_popup = icon_popup(target);
    let (preview_row, icon_preview) = icon_preview();
    let error = caption("");
    error.setTextColor(Some(&NSColor::systemRedColor()));
    error.setHidden(true);
    let reset = unsafe {
        NSButton::buttonWithTitle_target_action(
            &ns("Standardwerte"),
            Some(target),
            Some(sel!(resetDefaults:)),
            mtm,
        )
    };

    let notify_row = hstack(&[&notify_icon, &notify_status, &notify_button]);
    notify_row.setCustomSpacing_afterView(10.0, &notify_status);
    let rules_buttons = hstack(&[&rules_new, &rules_edit, &rules_delete]);
    // „Arbeit ohne Mac: mindestens 15 Minuten“ – ohne das Wort wäre nicht klar, was die Zahl ist.
    let work_row = hstack(&[&label("mindestens"), &work, &work_stepper, &label("Minuten")]);
    let bar_row = hstack(&[&icon_popup, &show_time]);
    bar_row.setSpacing(14.0);
    let account_row = hstack(&[&account_check, &account_date]);
    let carry_row = hstack(&[&carry, &label("Std.")]);
    let idle_row = hstack(&[&idle_check, &idle, &idle_stepper, &label("Minuten ohne Eingabe")]);

    let empty = || NSGridCell::emptyContentView(mtm);
    // Eine Zeile: Beschriftung links (oder leer), Inhalt rechts.
    let row = |left: Option<&str>, right: &NSView| -> Retained<NSArray<NSView>> {
        let left = match left {
            Some(text) => view(&label(text)),
            None => empty(),
        };
        NSArray::from_retained_slice(&[left, right.retain()])
    };
    // Reiter: Titel, Zeilen, Zeilen mit Abstand davor (neuer Abschnitt), Zeile mit Bildern.
    type Page = (&'static str, Vec<Retained<NSArray<NSView>>>, Vec<isize>, Option<isize>);
    let pages: Vec<Page> = vec![
        (
            "Allgemein",
            vec![
                row(Some("Anmeldung:"), &login),
                row(Some("Menüleiste:"), &bar_row),
                row(None, &preview_row),
                row(
                    None,
                    &caption("Von links: läuft, Pause, außerhalb des Arbeitszeitrahmens oder freier Tag, Feierabend."),
                ),
                row(Some("Kurzbefehle:"), &hotkeys),
                row(None, &caption(&hotkey::summary())),
            ],
            vec![1, 4],
            Some(2),
        ),
        (
            "Arbeitszeit",
            vec![
                row(Some("Regeln:"), &rules_now),
                row(None, &rules_scroll),
                row(None, &rules_buttons),
                row(
                    None,
                    &wide_caption(
                        "Jeder Stand gilt ab seinem Datum bis zum nächsten – eine Änderung rechnet frühere \
                         Tage nicht neu. Die Regelarbeitszeit verteilt sich gleichmäßig auf die \
                         Arbeitstage; an den übrigen Tagen gibt es kein Soll. Außerhalb des Rahmens wird \
                         weder Arbeitszeit noch eine Pause gezählt.",
                    ),
                ),
                row(Some("Zeitkonto:"), &account_row),
                row(Some("Übertrag:"), &carry_row),
                row(
                    None,
                    &caption(
                        "Das Zeitkonto ist der Saldo aller erfassten Tage – oder erst ab dem Datum – plus \
                         Übertrag: der Saldo aus der Zeit davor, z. B. +12:30 oder −3:15.",
                    ),
                ),
            ],
            vec![1, 4],
            None,
        ),
        (
            "Erfassung",
            vec![
                row(Some("Arbeit ohne Mac:"), &work_row),
                row(
                    None,
                    &caption(
                        "Kürzere Abschnitte lassen sich im Fenster „Aufzeichnungen“ nicht eintragen. \
                         Das schützt vor versehentlichen Einträgen.",
                    ),
                ),
                row(Some("Inaktivität:"), &idle_row),
                row(
                    None,
                    &caption(
                        "Der entsperrte Mac gilt dann als verlassen: Die Erfassung endet rückwirkend mit \
                         der letzten Eingabe, wie bei einer Sperre. Solange eine App den Bildschirm wach \
                         hält (Videokonferenz, Präsentation), zählt die Zeit weiter.",
                    ),
                ),
                row(Some("Lange Sperre:"), &ask),
                row(
                    None,
                    &caption(&format!(
                        "Ab {} Min. Sperre fragt eine Mitteilung, ob es Arbeit ohne Mac oder schon \
                         Feierabend war. Ohne Antwort bleibt es eine Pause. Braucht Mitteilungen.",
                        rules::ASK_AFTER_LOCK / 60
                    )),
                ),
                row(Some("Freie Tage:"), &free_days),
                row(
                    None,
                    &caption(
                        "Am Wochenende und an den übrigen freien Tagen wird erfasst, aber erst gezählt, \
                         wenn im Menü „Heute erfassen“ gewählt ist – nachträglich auch in der \
                         Wochenansicht. Einträge von Hand zählen immer.",
                    ),
                ),
            ],
            vec![2, 4, 6],
            None,
        ),
        (
            "Mitteilungen",
            vec![
                row(Some("Mitteilungen:"), &notify_row),
                row(None, &notify_note),
                row(Some("Erinnern an:"), &notify_limit),
                row(None, &notify_break),
                row(
                    None,
                    &caption(
                        "Jede Schwelle wird einmal gemeldet, solange gearbeitet wird. Die Hinweise im Menü \
                         und im Fenster „Aufzeichnungen“ bleiben davon unberührt.",
                    ),
                ),
            ],
            vec![2],
            None,
        ),
    ];

    // Jeder Reiter ist ein Raster; alle Reiter sind so groß wie der größte.
    let (pad_x, pad_y) = (18.0, 16.0);
    let tabs = NSTabView::new(mtm);
    let mut content = NSSize::new(0.0, 0.0);
    for (title, rows, padded, images) in pages {
        let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
        grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
        grid.setColumnSpacing(8.0);
        grid.setRowSpacing(4.0);
        grid.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Trailing);
        for r in padded {
            grid.rowAtIndex(r).setTopPadding(14.0);
        }
        if let Some(r) = images {
            // Bilder haben keine Grundlinie
            grid.rowAtIndex(r).setRowAlignment(NSGridRowAlignment::None);
            grid.rowAtIndex(r).setTopPadding(6.0);
            grid.rowAtIndex(r).setBottomPadding(2.0);
        }
        grid.setTranslatesAutoresizingMaskIntoConstraints(false);
        let size = grid.fittingSize();
        content = NSSize::new(content.width.max(size.width), content.height.max(size.height));
        let page = NSView::new(mtm);
        page.addSubview(&grid);
        grid.leadingAnchor().constraintEqualToAnchor_constant(&page.leadingAnchor(), pad_x).setActive(true);
        grid.topAnchor().constraintEqualToAnchor_constant(&page.topAnchor(), pad_y).setActive(true);
        let item = NSTabViewItem::new();
        item.setLabel(&ns(title));
        item.setView(Some(&page));
        tabs.addTabViewItem(&item);
    }
    // Rahmen und Reiterleiste kommen zur Größe des Inhalts dazu.
    tabs.setFrameSize(NSSize::new(600.0, 600.0));
    let inner = tabs.contentRect().size;
    let tabs_size = NSSize::new(
        content.width + 2.0 * pad_x + (600.0 - inner.width),
        content.height + 2.0 * pad_y + (600.0 - inner.height),
    );
    tabs.setTranslatesAutoresizingMaskIntoConstraints(false);
    tabs.widthAnchor().constraintEqualToConstant(tabs_size.width).setActive(true);
    tabs.heightAnchor().constraintEqualToConstant(tabs_size.height).setActive(true);

    // Unter den Reitern: Fehler (meist ausgeblendet) und „Standardwerte“.
    let column = NSStackView::new(mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(10.0);
    for v in [&*tabs as &NSView, &error, &reset] {
        column.addArrangedSubview(v);
    }
    column.setTranslatesAutoresizingMaskIntoConstraints(false);

    let size = column.fittingSize();
    let (w, h) = (size.width + 40.0, size.height + 40.0);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&ns("Einstellungen"));
    window.setDelegate(Some(ProtocolObject::from_ref(target)));
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("TagwerkSettingsToolbar"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(target)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    toolbar.setAllowsUserCustomization(false);
    window.setToolbar(Some(&toolbar));
    let root = window.contentView().expect("contentView");
    root.addSubview(&column);
    column.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    column.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -20.0).setActive(true);
    column.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    column.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -20.0).setActive(true);
    // Die Symbolleiste verändert die Fenstergröße; Inhalt wieder passend machen.
    window.setContentSize(NSSize::new(w, h));
    window.center();

    let (db, settings, err) = match store::Db::open() {
        Ok(db) => {
            let s = db.load_settings();
            (Some(db), s.as_ref().cloned().unwrap_or_default(), s.err().map(|e| e.to_string()))
        }
        Err(e) => (None, Settings::default(), Some(e.to_string())),
    };
    let state = State {
        window,
        _controller: controller,
        db,
        settings,
        login,
        notify_icon,
        notify_status,
        notify_button,
        notify_note,
        rules_now,
        rules_table,
        rules_edit,
        rules_delete,
        sheet,
        work,
        work_stepper,
        icon_popup,
        icon_preview,
        show_time,
        hotkeys,
        account_check,
        account_date,
        carry,
        idle_check,
        idle,
        idle_stepper,
        ask,
        free_days,
        notify_limit,
        notify_break,
        tabs,
        error,
        on_change,
    };
    // Bis zur Antwort der Mitteilungszentrale der letzte bekannte Stand (sonst leer).
    if let Some(p) = notify::last_permission() {
        state.set_permission(p);
    }
    if let Some(e) = err {
        state.show_error(&format!("Die Datenbank konnte nicht gelesen werden: {e}"));
    }
    state
}

/// Öffnet das Fenster „Einstellungen“ (oder holt es nach vorn). `on_change` wird nach
/// jeder gespeicherten Änderung aufgerufen.
pub fn show(on_change: fn()) {
    let exists = STATE.with(|s| s.borrow().is_some());
    if !exists {
        let state = build(on_change);
        STATE.with(|s| *s.borrow_mut() = Some(state));
    } else {
        // Stand der Datenbank übernehmen (die andere Instanz kann geändert haben).
        with_state(|s| {
            if let Some(Ok(settings)) = s.db.as_ref().map(|db| db.load_settings()) {
                s.settings = settings;
            }
            s.error.setHidden(true);
        });
    }
    with_state(|s| {
        s.fill();
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
        s.window.makeKeyAndOrderFront(None);
    });
    // Falls das Fenster schon aktiv war (dann kommt kein windowDidBecomeKey).
    refresh_permission();
}

/// Zum Testen: Fenster „Einstellungen“ mit dem Stand der Datenbank öffnen, ein Bild
/// speichern und beenden (`Tagwerk --settings-screenshot <png>`; Reiter über
/// `TAGWERK_SETTINGS_TAB=0|1|2|3`, Symbolsatz zur Ansicht
/// über `TAGWERK_ICON_SET=0|1|2`, Erscheinungsbild über `TAGWERK_APPEARANCE=dark`, Stand der
/// Mitteilungen über `TAGWERK_NOTIFICATIONS=allowed|denied|hidden|undetermined`; mit
/// `TAGWERK_SETTINGS_SHEET=new|first|<Zeile>` stattdessen das Blatt „Arbeitszeit ändern“ für
/// einen neuen Stand, den ersten oder den in der Zeile der Liste).
/// Es wird nichts gespeichert.
pub fn screenshot(path: &std::path::Path) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Das Fenster muss im Hauptthread laufen.")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if let Ok(a) = std::env::var("TAGWERK_APPEARANCE") {
        let name = if a == "dark" { unsafe { NSAppearanceNameDarkAqua } } else { unsafe { NSAppearanceNameAqua } };
        app.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }
    let state = build(|| {});
    STATE.with(|s| *s.borrow_mut() = Some(state));
    with_state(|s| {
        if let Some(id) = std::env::var("TAGWERK_ICON_SET").ok().and_then(|v| v.parse().ok()) {
            s.settings.icon_set = IconSet::from_id(id);
        }
        s.fill();
        let permission = match std::env::var("TAGWERK_NOTIFICATIONS").as_deref() {
            Ok("allowed") => Permission::Allowed,
            Ok("denied") => Permission::Denied,
            Ok("hidden") => Permission::Hidden,
            Ok("undetermined") => Permission::NotDetermined,
            _ => Permission::Unavailable,
        };
        s.set_permission(permission);
        if let Some(tab) = std::env::var("TAGWERK_SETTINGS_TAB").ok().and_then(|v| v.parse::<isize>().ok()) {
            if (0..s.tabs.numberOfTabViewItems()).contains(&tab) {
                s.tabs.selectTabViewItemAtIndex(tab);
            }
        }
        s.window.orderFront(None);
    });
    let sheet = std::env::var("TAGWERK_SETTINGS_SHEET").ok();
    if let Some(which) = &sheet {
        let editing = match which.as_str() {
            "new" => None,
            "first" => Some(None),
            row => Some(row_from(row.parse().unwrap_or(-1)).ok_or("keine solche Zeile")?),
        };
        open_sheet(editing);
    }
    let window = with_state(|s| if sheet.is_some() { s.sheet.window.clone() } else { s.window.clone() })
        .ok_or("kein Fenster")?;
    // Das Fenster, wie es auf dem Bildschirm steht (mit Reiterleiste) – dafür muss es erst
    // einmal gezeichnet sein. Nur wenn macOS das nicht liefert, die Ansicht selbst gezeichnet.
    objc2_foundation::NSRunLoop::mainRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.8));
    if let Some(png) = crate::snapshot::window_png(window.windowNumber() as u32, false) {
        return std::fs::write(path, png).map_err(|e| e.to_string());
    }
    let content = window.contentView().ok_or("kein Inhalt")?;
    let frame_view = unsafe { content.superview() }.unwrap_or(content);
    frame_view.layoutSubtreeIfNeeded();
    frame_view.displayIfNeeded();
    let bounds = frame_view.bounds();
    let rep = frame_view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or("kein Bildpuffer")?;
    frame_view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new()) }
        .ok_or("PNG konnte nicht erzeugt werden")?;
    std::fs::write(path, data.to_vec()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn sheet_explains_what_a_change_does() {
        let today = date(21);
        let mut s = Settings::default();
        let d = WorkRules::DEFAULT;
        let part = WorkRules { weekly_target: 32 * 3600, ..d };
        // Neuer Stand ab heute: keine Warnung.
        assert_eq!(sheet_hint(&s, None, Some(today), &part, today), ("Gilt ab 21.11.2026.".to_string(), false));
        // Ohne Änderung
        assert_eq!(
            sheet_hint(&s, None, Some(today), &d, today).0,
            "Gilt ab 21.11.2026. Gleicht dem Stand davor – es ändert sich nichts."
        );
        // Rückwirkend: orange
        let (text, warn) = sheet_hint(&s, None, Some(date(2)), &part, today);
        assert!(warn && text.ends_with("Rückwirkend: Saldo und Aufzeichnungen ab dem 02.11.2026 ändern sich."), "{text}");
        // Mit einem späteren Stand gilt der neue nur bis davor; derselbe Tag ersetzt ihn.
        s.set_rules(Some(date(30)), part, None);
        assert_eq!(sheet_hint(&s, None, Some(date(23)), &part, today).0, "Gilt vom 23.11.2026 bis 29.11.2026.");
        let (text, warn) = sheet_hint(&s, None, Some(date(30)), &d, today);
        assert!(warn && text.contains("Ersetzt den vorhandenen Stand ab 30.11.2026."), "{text}");
        // Den Stand selbst bearbeiten ersetzt nichts.
        assert_eq!(sheet_hint(&s, Some(Some(date(30))), Some(date(30)), &part, today).0, "Gilt ab 30.11.2026.");
        // Der erste Stand gilt bis zum nächsten.
        assert_eq!(
            sheet_hint(&s, Some(None), None, &d, today),
            ("Gilt für alle Tage vor dem 30.11.2026. Saldo und Aufzeichnungen dieser Tage ändern sich.".to_string(), true)
        );
    }

    #[test]
    fn rule_rows_mark_today_and_planned() {
        let mut s = Settings::default();
        s.set_rules(Some(date(2)), WorkRules { weekly_target: 38 * 3600, ..WorkRules::DEFAULT }, None);
        s.set_rules(Some(date(30)), WorkRules { workdays: 0b0_1111, ..WorkRules::DEFAULT }, None);
        let rows = rule_rows(&s, date(21));
        let flags: Vec<_> = rows.iter().map(|r| (r.cells[0].as_str(), r.current, r.planned)).collect();
        assert_eq!(flags, vec![("Beginn", false, false), ("02.11.2026", true, false), ("30.11.2026", false, true)]);
        assert_eq!(rows[1].cells[1..], ["38 Std.", "Mo–Fr", "07:00–22:00", "15 Min."].map(String::from));
        assert_eq!(
            rules_now_text(&s, date(21)),
            "Heute gilt der Stand ab 02.11.2026: 7:36 h pro Arbeitstag (5-Tage-Woche).\n\
             Geplant ab 30.11.2026: 9:45 h pro Arbeitstag (4-Tage-Woche)."
        );
    }

    #[test]
    fn work_stepper_moves_over_round_values() {
        // abwärts: 15 → 10 → 5 → 1 (der Schalter bleibt an der Untergrenze stehen)
        assert_eq!(step_minutes(15, 10), 10);
        assert_eq!(step_minutes(10, 5), 5);
        assert_eq!(step_minutes(5, 1), 1);
        assert_eq!(step_minutes(3, 1), 1);
        assert_eq!(step_minutes(1, 1), 1);
        // aufwärts: 1 → 5 → 10
        assert_eq!(step_minutes(1, 6), 5);
        assert_eq!(step_minutes(5, 10), 10);
        // von einem krummen Wert zum nächsten glatten
        assert_eq!(step_minutes(12, 17), 15);
        assert_eq!(step_minutes(12, 7), 10);
        assert_eq!(step_minutes(240, 240), 240);
    }
}
