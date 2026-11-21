//! Fenster „Einstellungen“ nach den macOS Human Interface Guidelines: nicht modal,
//! Änderungen gelten sofort, kein „OK“/„Abbrechen“.
//!
//! Vier Reiter: „Allgemein“ (Anmeldung, Menüleiste, Kurzbefehle), „Arbeitszeit“ (Rahmen,
//! Regelarbeitszeit, Arbeitstage, Zeitkonto), „Erfassung“ (Mindestpause, Arbeit ohne Mac,
//! Inaktivität, Nachfrage nach langer Sperre) und „Mitteilungen“ (Stand der Erlaubnis, woran
//! Tagwerk erinnert). Gespeichert wird in der Datenbank; die jeweils andere
//! Tagwerk-Instanz (Menüleiste bzw. Fenster „Aufzeichnungen“) übernimmt die Werte.
//!
//! „Bei Anmeldung starten“ steht nicht in der Datenbank, sondern ist das Anmeldeobjekt
//! von macOS; „Standardwerte“ lässt es unverändert.
//!
//! „Mitteilungen“ zeigt, ob Tagwerk Mitteilungen zeigen darf (Erinnerung an die
//! Höchstarbeitszeit); wenn nicht, öffnet ein Knopf Systemeinstellungen › Mitteilungen.
//! Der Stand wird jedes Mal abgefragt, wenn das Fenster aktiv wird.
//!
//! Rechts in der Symbolleiste öffnet ein Ordner-Knopf den Datenordner im Finder.

use crate::day::{secs_of, DAY_END};
use crate::glyphs;
use crate::hotkey;
use crate::login_item;
use crate::notify::{self, Permission};
use crate::rules::{self, IconSet, Settings, IDLE_RANGE, MIN_PAUSE_RANGE, MIN_WORK_RANGE, WEEKLY_RANGE};
use crate::store;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSCopying, NSDate, NSLocale, NSNotification, NSPoint, NSRect, NSSize, NSString, NSURL};
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
    frame_from: Retained<NSDatePicker>,
    frame_to: Retained<NSDatePicker>,
    weekly: Retained<NSTextField>,
    weekly_stepper: Retained<NSStepper>,
    pause: Retained<NSTextField>,
    pause_stepper: Retained<NSStepper>,
    work: Retained<NSTextField>,
    work_stepper: Retained<NSStepper>,
    daily_note: Retained<NSTextField>,
    icon_popup: Retained<NSPopUpButton>,
    icon_preview: Vec<Retained<NSImageView>>,
    show_time: Retained<NSButton>,
    hotkeys: Retained<NSButton>,
    /// Arbeitstage Montag bis Freitag (Tag der Checkbox = Bit in `Settings::workdays`).
    workdays: Vec<Retained<NSButton>>,
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
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
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
        #[unsafe(method(frameChanged:))]
        fn frame_changed(&self, _sender: Option<&AnyObject>) {
            let ok = with_state(|s| {
                let (Some(from), Some(to)) = (picker_secs(&s.frame_from), picker_secs(&s.frame_to)) else {
                    return false;
                };
                if to <= from {
                    s.show_error("Das Ende des Arbeitszeitrahmens muss nach seinem Beginn liegen.");
                    return false;
                }
                s.settings.frame_from = from;
                s.settings.frame_to = to;
                s.error.setHidden(true);
                true
            });
            if ok == Some(true) {
                save();
            }
        }

        #[unsafe(method(weeklyChanged:))]
        fn weekly_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = rules::parse_hours(&s.weekly.stringValue().to_string());
                s.set_weekly(value);
            });
            save();
        }

        #[unsafe(method(weeklyStepped:))]
        fn weekly_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = (s.weekly_stepper.doubleValue() * 3600.0).round() as i64;
                s.set_weekly(Some(value));
            });
            save();
        }

        #[unsafe(method(pauseChanged:))]
        fn pause_changed(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = s.pause.stringValue().to_string().trim().parse::<i64>().ok().map(|m| m * 60);
                s.set_pause(value);
            });
            save();
        }

        #[unsafe(method(pauseStepped:))]
        fn pause_stepped(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                let value = s.pause_stepper.integerValue() as i64 * 60;
                s.set_pause(Some(value));
            });
            save();
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

        /// Arbeitstag ein- oder ausgeschaltet (Tag der Checkbox = Bit); einer bleibt immer.
        #[unsafe(method(workdayChanged:))]
        fn workday_changed(&self, sender: Option<&NSButton>) {
            let Some(sender) = sender else { return };
            let bit = 1u8 << sender.tag().clamp(0, 4);
            let on = sender.state() == NSControlStateValueOn;
            with_state(|s| {
                let days = if on { s.settings.workdays | bit } else { s.settings.workdays & !bit };
                if days & rules::ALL_WORKDAYS == 0 {
                    s.show_error("Mindestens ein Arbeitstag muss bleiben.");
                } else {
                    s.settings.workdays = days;
                    s.error.setHidden(true);
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
            with_state(|s| {
                s.settings = Settings::default();
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
        self.frame_from.setDateValue(&to_nsdate(s.frame_from));
        self.frame_to.setDateValue(&to_nsdate(s.frame_to));
        self.weekly.setStringValue(&ns(&rules::fmt_hours(s.weekly_target)));
        self.weekly_stepper.setDoubleValue(s.weekly_target as f64 / 3600.0);
        self.pause.setStringValue(&ns(&(s.min_pause / 60).to_string()));
        self.pause_stepper.setIntegerValue((s.min_pause / 60) as isize);
        self.work.setStringValue(&ns(&(s.min_work / 60).to_string()));
        self.work_stepper.setIntegerValue((s.min_work / 60) as isize);
        self.daily_note.setStringValue(&ns(&format!(
            "{}  ·  {} pro Arbeitstag",
            s.week_text(),
            store::fmt_hm(s.daily_target_workday())
        )));
        for (bit, check) in self.workdays.iter().enumerate() {
            check.setState(state(s.workdays & (1 << bit) != 0));
        }
        self.account_check.setState(state(s.account_from.is_some()));
        self.account_date.setEnabled(s.account_from.is_some());
        // Ohne Beginn als Vorschlag der 1. Januar dieses Jahres.
        let today = Local::now().date_naive();
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

    fn set_weekly(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (WEEKLY_RANGE.0..=WEEKLY_RANGE.1).contains(&v) => {
                self.settings.weekly_target = v;
                self.error.setHidden(true);
            }
            _ => self.show_error(&format!(
                "Die Regelarbeitszeit muss zwischen {} und {} Stunden pro Woche liegen.",
                WEEKLY_RANGE.0 / 3600,
                WEEKLY_RANGE.1 / 3600
            )),
        }
    }

    fn set_pause(&mut self, value: Option<i64>) {
        match value {
            Some(v) if (MIN_PAUSE_RANGE.0..=MIN_PAUSE_RANGE.1).contains(&v) => {
                self.settings.min_pause = v;
                self.error.setHidden(true);
            }
            _ => self.show_error(&format!(
                "Die Mindestpause muss zwischen {} und {} Minuten liegen.",
                MIN_PAUSE_RANGE.0 / 60,
                MIN_PAUSE_RANGE.1 / 60
            )),
        }
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

/// Speichern, Felder angleichen und die App benachrichtigen.
fn save() {
    let notify = with_state(|s| {
        s.fill();
        let result = match s.db.as_mut() {
            Some(db) => db.save_settings(&s.settings).map_err(|e| e.to_string()),
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

fn time_picker(target: &Controller) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm());
    p.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    p.setDatePickerElements(NSDatePickerElementFlags::HourMinute);
    p.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    p.setBezeled(true);
    p.setDrawsBackground(true);
    unsafe {
        p.setTarget(Some(target));
        p.setAction(Some(sel!(frameChanged:)));
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
    let workdays: Vec<Retained<NSButton>> = ["Mo", "Di", "Mi", "Do", "Fr"]
        .iter()
        .enumerate()
        .map(|(bit, name)| {
            let c = checkbox(name, sel!(workdayChanged:));
            c.setTag(bit as isize);
            c
        })
        .collect();
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
    let frame_from = time_picker(target);
    let frame_to = time_picker(target);
    let weekly = number_field(target, sel!(weeklyChanged:));
    let weekly_stepper = stepper(
        target,
        sel!(weeklyStepped:),
        WEEKLY_RANGE.0 as f64 / 3600.0,
        WEEKLY_RANGE.1 as f64 / 3600.0,
        0.5,
    );
    let pause = number_field(target, sel!(pauseChanged:));
    let pause_stepper = stepper(
        target,
        sel!(pauseStepped:),
        (MIN_PAUSE_RANGE.0 / 60) as f64,
        (MIN_PAUSE_RANGE.1 / 60) as f64,
        1.0,
    );
    let work = number_field(target, sel!(workChanged:));
    let work_stepper = stepper(
        target,
        sel!(workStepped:),
        (MIN_WORK_RANGE.0 / 60) as f64,
        (MIN_WORK_RANGE.1 / 60) as f64,
        WORK_STEP as f64,
    );
    let daily_note = caption("");
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
    let frame_row = hstack(&[&frame_from, &label("bis"), &frame_to, &label("Uhr")]);
    let weekly_row = hstack(&[&weekly, &weekly_stepper, &label("Stunden pro Woche")]);
    let pause_row = hstack(&[&pause, &pause_stepper, &label("Minuten")]);
    // „Arbeit ohne Mac: mindestens 15 Minuten“ – ohne das Wort wäre nicht klar, was die Zahl ist.
    let work_row = hstack(&[&label("mindestens"), &work, &work_stepper, &label("Minuten")]);
    let bar_row = hstack(&[&icon_popup, &show_time]);
    bar_row.setSpacing(14.0);
    let day_views: Vec<&NSView> = workdays.iter().map(|c| &***c as &NSView).collect();
    let days_row = hstack(&day_views);
    days_row.setSpacing(12.0);
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
                row(Some("Arbeitszeitrahmen:"), &frame_row),
                row(
                    None,
                    &caption(
                        "Gilt an jedem Tag. Außerhalb dieses Zeitraums wird weder Arbeitszeit noch eine Pause \
                         gezählt; Arbeit am Wochenende zählt als Mehrarbeit.",
                    ),
                ),
                row(Some("Regelarbeitszeit:"), &weekly_row),
                row(Some("Arbeitstage:"), &days_row),
                row(None, &daily_note),
                row(
                    None,
                    &caption(
                        "Die Regelarbeitszeit verteilt sich gleichmäßig auf die Arbeitstage. An den \
                         übrigen Tagen gibt es kein Soll – Arbeit dort zählt als Mehrarbeit.",
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
            vec![2, 3, 6],
            None,
        ),
        (
            "Erfassung",
            vec![
                row(Some("Mindestpause:"), &pause_row),
                row(
                    None,
                    &caption(
                        "Ist der Mac kürzer gesperrt, zählt die Zeit weiter zur Arbeitszeit. \
                         Nach dem Arbeitszeitgesetz zählen Pausen erst ab 15 Minuten.",
                    ),
                ),
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
            vec![2, 4, 6, 8],
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
            (Some(db), s.as_ref().copied().unwrap_or_default(), s.err().map(|e| e.to_string()))
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
        frame_from,
        frame_to,
        weekly,
        weekly_stepper,
        pause,
        pause_stepper,
        work,
        work_stepper,
        daily_note,
        icon_popup,
        icon_preview,
        show_time,
        hotkeys,
        workdays,
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
/// Mitteilungen über `TAGWERK_NOTIFICATIONS=allowed|denied|hidden|undetermined`).
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
    let window = with_state(|s| s.window.clone()).ok_or("kein Fenster")?;
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
