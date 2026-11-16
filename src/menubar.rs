//! Menüleisten-App (NSStatusItem) mit Erkennung von Sperren/Entsperren des Macs
//! und manuellen Entscheidungen: Arbeitsbeginn, Mittagspause, Feierabend.

use crate::day::{self, fmt_clock, Mode};
use crate::store;
use crate::tracker::{PauseReason, Tracker};
use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSApplicationActivationOptions,
    NSApplicationActivationPolicy, NSDatePicker, NSDatePickerElementFlags, NSDatePickerStyle, NSImage,
    NSMenu, NSMenuDelegate, NSMenuItem, NSRunningApplication, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength, NSWorkspace,
};
use objc2_foundation::{
    NSDate, NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSRunLoop,
    NSRunLoopCommonModes, NSString, NSTimer,
};
use std::cell::{Cell, RefCell};
use std::process::{Child, Command};
use std::time::{Instant, SystemTime};

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
    status: Retained<NSMenuItem>,
    worked: Retained<NSMenuItem>,
    pauses: Retained<NSMenuItem>,
    first: Retained<NSMenuItem>,
    hint: Retained<NSMenuItem>,
    start_reset: Retained<NSMenuItem>,
    break_start: Retained<NSMenuItem>,
    break_end: Retained<NSMenuItem>,
    close: Retained<NSMenuItem>,
    reopen: Retained<NSMenuItem>,
    lunch_gap: Retained<NSMenuItem>,
    login: Retained<NSMenuItem>,
    /// Untermenü-Einträge „vor N Min.“, deren Titel die Uhrzeit zeigen.
    relative: Vec<(Retained<NSMenuItem>, i64)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum IconKind {
    Running,
    Paused,
    Break,
    Closed,
}

struct AppState {
    tracker: Tracker,
    status_item: Retained<NSStatusItem>,
    items: MenuItems,
    records_window: Option<Child>,
    last_save: Instant,
    last_icon: Option<IconKind>,
    /// Änderungszeit der Datei nach dem letzten eigenen Schreiben.
    file_mtime: Option<SystemTime>,
}

thread_local! {
    static STATE: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static ALERT_OPEN: Cell<bool> = const { Cell::new(false) };
}

fn with_state<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Manuelle Aktion: vorher Änderungen von außen übernehmen, danach speichern und anzeigen.
fn act(f: impl FnOnce(&mut AppState, NaiveDateTime)) {
    with_state(|s| {
        let t = now();
        s.sync_from_disk(t);
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
                s.sync_from_disk(t);
                if s.tracker.roll_over(t) {
                    s.persist(t);
                }
                s.refresh();
            });
        }
    }

    impl Controller {
        #[unsafe(method(setStart:))]
        fn set_start(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|s| s.tag()).unwrap_or(0);
            if tag == TAG_RESET {
                act(|s, t| s.tracker.set_start(t, None));
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
            act(|s, t| s.tracker.set_start(t, Some(secs(at))));
        }

        #[unsafe(method(startBreak:))]
        fn start_break(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map(|s| s.tag()).unwrap_or(0);
            let Some(at) = chosen_time(tag, "Mittagspause starten", "Seit wann läuft die Pause?", None) else {
                return;
            };
            act(|s, t| {
                s.tracker.start_break(t, secs(at));
            });
        }

        #[unsafe(method(endBreak:))]
        fn end_break(&self, _sender: Option<&AnyObject>) {
            end_break_now();
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
            act(|s, t| s.tracker.close_day(t, secs(at)));
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

        #[unsafe(method(acceptLunchGap:))]
        fn accept_lunch_gap(&self, _sender: Option<&AnyObject>) {
            act(|s, t| {
                if let Some(gap) = s.tracker.today(t).lunch_gap {
                    s.tracker.add_break(t, gap.from, gap.to);
                }
            });
        }

        #[unsafe(method(showRecords:))]
        fn show_records(&self, _sender: Option<&AnyObject>) {
            with_state(|s| {
                // Vorher speichern, damit das Fenster den aktuellen Stand zeigt.
                let t = now();
                s.sync_from_disk(t);
                s.persist(t);
                s.open_records_window();
            });
        }

        #[unsafe(method(openFolder:))]
        fn open_folder(&self, _sender: Option<&AnyObject>) {
            let _ = std::fs::create_dir_all(store::data_dir());
            let _ = Command::new("/usr/bin/open").arg(store::data_dir()).spawn();
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _sender: Option<&AnyObject>) {
            let enable = !login_item_enabled();
            set_login_item(enable);
            with_state(|s| s.refresh());
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
                let mut dirty = s.sync_from_disk(t);
                dirty |= s.tracker.roll_over(t);
                let locked = screen_locked();
                if s.tracker.is_running() && locked {
                    // Sicherheitsnetz: falls ein Sperr-Ereignis verpasst wurde.
                    dirty |= s.tracker.stop(t, PauseReason::Locked);
                } else if !s.tracker.is_running()
                    && !locked
                    && s.tracker.paused_since.is_some_and(|p| p.date() == t.date())
                {
                    // Entsperrt, aber nicht erfasst (z. B. Feierabend im Fenster zurückgenommen).
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
            ask_end_break();
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
                ask_end_break();
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
                ask_end_break();
            }
        }

        #[unsafe(method(willTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
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
        s.sync_from_disk(t);
        s.tracker.roll_over(t);
        if s.tracker.start(t) {
            s.persist(t);
        }
        s.refresh();
    });
}

fn end_break_now() {
    act(|s, t| {
        s.tracker.end_break(t);
        if !s.tracker.is_running() && !screen_locked() {
            s.tracker.start(t);
        }
    });
}

fn shutdown() {
    with_state(|s| {
        let t = now();
        s.sync_from_disk(t);
        s.tracker.stop(t, PauseReason::Manual);
        s.persist(t);
        if let Some(mut child) = s.records_window.take() {
            let _ = child.kill();
        }
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

/// Nach dem Entsperren während einer Mittagspause nachfragen, ob sie vorbei ist.
fn ask_end_break() {
    let since = with_state(|s| match s.tracker.mode(now()) {
        Mode::Break(from) => Some(from),
        _ => None,
    })
    .flatten();
    let Some(since) = since else { return };
    if ALERT_OPEN.with(|a| a.replace(true)) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else {
        ALERT_OPEN.with(|a| a.set(false));
        return;
    };
    let mins = ((secs(now()) - since) / 60).max(0);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(&format!("Mittagspause läuft seit {} Uhr", fmt_clock(since))));
    alert.setInformativeText(&ns(&format!(
        "Die Pause dauert jetzt {mins} Min. Pause beenden und die Arbeitszeit wieder zählen?"
    )));
    alert.addButtonWithTitle(&ns("Pause beenden"));
    alert.addButtonWithTitle(&ns("Pause fortsetzen"));
    activate_app(mtm);
    let response = alert.runModal();
    ALERT_OPEN.with(|a| a.set(false));
    if response == NSAlertFirstButtonReturn {
        end_break_now();
    }
}

impl AppState {
    /// Übernimmt Änderungen, die das Fenster „Aufzeichnungen“ (oder ein Editor) in die
    /// Datei geschrieben hat. Gibt `true` zurück, wenn neu eingelesen wurde.
    fn sync_from_disk(&mut self, t: NaiveDateTime) -> bool {
        let m = store::file_mtime();
        if m.is_none() || m == self.file_mtime {
            return false;
        }
        self.file_mtime = m;
        match store::load() {
            Ok(days) => {
                self.tracker.replace_days(days, t);
                true
            }
            Err(e) => {
                eprintln!("Tagwerk: Datei konnte nicht neu gelesen werden: {e}");
                false
            }
        }
    }

    fn persist(&mut self, t: NaiveDateTime) {
        if let Err(e) = self.tracker.save(t) {
            eprintln!("Tagwerk: Speichern fehlgeschlagen: {e}");
        }
        self.file_mtime = store::file_mtime();
        store::write_status(self.tracker.is_running(), self.tracker.mode(t));
        self.last_save = Instant::now();
    }

    fn refresh(&mut self) {
        let t = now();
        let running = self.tracker.is_running();
        let info = self.tracker.today(t);
        let mtm = MainThreadMarker::new().expect("main thread");
        let midnight = t.date().and_hms_opt(0, 0, 0).unwrap();

        let (status, icon) = match info.mode {
            Mode::Closed(at) => (format!("✓  Feierabend um {} Uhr", fmt_clock(at)), IconKind::Closed),
            Mode::Break(from) => (format!("☕  Mittagspause seit {} Uhr", fmt_clock(from)), IconKind::Break),
            Mode::Auto if running => ("▶︎  Zeiterfassung läuft".to_string(), IconKind::Running),
            Mode::Auto => (
                match self.tracker.paused_since {
                    Some(p) if p.date() == t.date() => {
                        format!("⏸  Zeiterfassung pausiert seit {} Uhr", p.format("%H:%M"))
                    }
                    _ => "⏸  Zeiterfassung pausiert".to_string(),
                },
                IconKind::Paused,
            ),
        };
        self.items.status.setTitle(&ns(&status));
        self.items
            .worked
            .setTitle(&ns(&format!("Heute gearbeitet:  {}", store::fmt_hm(info.work_secs))));
        self.items
            .pauses
            .setTitle(&ns(&format!("Pausen heute:  {}", store::fmt_hm(info.pause_secs))));
        let first = match info.first_start {
            Some(f) if info.start_manual => format!("Beginn heute:  {} Uhr (manuell)", fmt_clock(f)),
            Some(f) => format!("Beginn heute:  {} Uhr", fmt_clock(f)),
            None => "Beginn heute:  –".to_string(),
        };
        self.items.first.setTitle(&ns(&first));
        match info.legal_hint {
            Some(h) => {
                self.items.hint.setTitle(&ns(&format!("⚠︎  {h}")));
                self.items.hint.setHidden(false);
            }
            None => self.items.hint.setHidden(true),
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

        let in_break = matches!(info.mode, Mode::Break(_));
        let closed = matches!(info.mode, Mode::Closed(_));
        self.items.break_start.setHidden(in_break);
        self.items.break_start.setEnabled(!closed);
        self.items.break_end.setHidden(!in_break);
        self.items.close.setHidden(closed);
        self.items.reopen.setHidden(!closed);
        match info.lunch_gap {
            Some(g) => {
                self.items.lunch_gap.setTitle(&ns(&format!(
                    "Lücke {}–{} als Mittagspause eintragen",
                    fmt_clock(g.from),
                    fmt_clock(g.to)
                )));
                self.items.lunch_gap.setHidden(false);
            }
            None => self.items.lunch_gap.setHidden(true),
        }
        self.items.login.setState(if login_item_enabled() { 1 } else { 0 });

        if let Some(button) = self.status_item.button(mtm) {
            let state = match icon {
                IconKind::Running => "läuft",
                IconKind::Paused => "pausiert",
                IconKind::Break => "Mittagspause",
                IconKind::Closed => "Feierabend",
            };
            let tip = format!(
                "Tagwerk – {state}\nHeute gearbeitet: {}\nPausen: {}",
                store::fmt_hm(info.work_secs),
                store::fmt_hm(info.pause_secs)
            );
            button.setToolTip(Some(&ns(&tip)));
            if self.last_icon != Some(icon) {
                set_icon(&button, icon);
                self.last_icon = Some(icon);
            }
        }
    }

    fn open_records_window(&mut self) {
        if let Some(child) = self.records_window.as_mut() {
            if matches!(child.try_wait(), Ok(None)) {
                let pid = child.id() as i32;
                if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                    #[allow(deprecated)]
                    app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
                }
                return;
            }
        }
        match std::env::current_exe() {
            Ok(exe) => match Command::new(exe).arg("--records").spawn() {
                Ok(child) => self.records_window = Some(child),
                Err(e) => eprintln!("Tagwerk: Fenster konnte nicht geöffnet werden: {e}"),
            },
            Err(e) => eprintln!("Tagwerk: {e}"),
        }
    }
}

fn set_icon(button: &objc2_app_kit::NSStatusBarButton, kind: IconKind) {
    let (symbol, fallback, desc) = match kind {
        IconKind::Running => ("clock.fill", "◷", "Zeiterfassung läuft"),
        IconKind::Paused => ("pause.circle", "⏸", "Zeiterfassung pausiert"),
        IconKind::Break => ("cup.and.saucer.fill", "☕", "Mittagspause"),
        IconKind::Closed => ("moon.zzz", "☾", "Feierabend"),
    };
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(symbol), Some(&ns(desc)));
    match image {
        Some(img) => {
            img.setTemplate(true);
            button.setImage(Some(&img));
            button.setTitle(&ns(""));
        }
        None => {
            button.setImage(None);
            button.setTitle(&ns(fallback));
        }
    }
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

// ---------- Bei Anmeldung starten (SMAppService, macOS 13+) ----------

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

fn login_service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    unsafe { msg_send![cls, mainAppService] }
}

fn login_item_enabled() -> bool {
    login_service()
        .map(|svc| {
            let status: isize = unsafe { msg_send![&*svc, status] };
            status == 1 // SMAppServiceStatusEnabled
        })
        .unwrap_or(false)
}

fn set_login_item(enable: bool) {
    let Some(svc) = login_service() else { return };
    let err: *mut *mut AnyObject = std::ptr::null_mut();
    let ok: bool = unsafe {
        if enable {
            msg_send![&*svc, registerAndReturnError: err]
        } else {
            msg_send![&*svc, unregisterAndReturnError: err]
        }
    };
    if !ok {
        eprintln!("Tagwerk: Anmeldeobjekt konnte nicht geändert werden (nur in der installierten App möglich).");
    }
}

fn running_from_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

// ---------- Aufbau ----------

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

pub fn run() {
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
    let (break_start, _) =
        time_menu(mtm, "Mittagspause starten", sel!(startBreak:), &[0, 5, 15, 30], &controller, &mut relative);
    let (close, _) = time_menu(mtm, "Feierabend", sel!(closeDay:), &[0, 15, 30], &controller, &mut relative);

    let items = MenuItems {
        status: menu_item(mtm, "Zeiterfassung", None, "", &controller),
        worked: menu_item(mtm, "Heute gearbeitet:", None, "", &controller),
        pauses: menu_item(mtm, "Pausen heute:", None, "", &controller),
        first: menu_item(mtm, "Beginn heute:", None, "", &controller),
        hint: menu_item(mtm, "", None, "", &controller),
        start_reset,
        break_start,
        break_end: menu_item(mtm, "Mittagspause beenden", Some(sel!(endBreak:)), "", &controller),
        close,
        reopen: menu_item(mtm, "Feierabend aufheben", Some(sel!(reopenDay:)), "", &controller),
        lunch_gap: menu_item(mtm, "", Some(sel!(acceptLunchGap:)), "", &controller),
        login: menu_item(mtm, "Bei Anmeldung starten", Some(sel!(toggleLogin:)), "", &controller),
        relative,
    };
    let records = menu_item(mtm, "Aufzeichnungen …", Some(sel!(showRecords:)), "a", &controller);
    let folder = menu_item(mtm, "Datenordner im Finder zeigen", Some(sel!(openFolder:)), "", &controller);
    let quit = menu_item(mtm, "Tagwerk beenden", Some(sel!(quit:)), "q", &controller);

    menu.addItem(&items.status);
    menu.addItem(&items.worked);
    menu.addItem(&items.pauses);
    menu.addItem(&items.first);
    menu.addItem(&items.hint);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&start_item);
    menu.addItem(&items.break_start);
    menu.addItem(&items.break_end);
    menu.addItem(&items.close);
    menu.addItem(&items.reopen);
    menu.addItem(&items.lunch_gap);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&records);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&folder);
    menu.addItem(&items.login);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&quit);

    let status_item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    status_item.setMenu(Some(&menu));

    // Daten laden und – falls entsperrt und kein Feierabend – sofort starten.
    let days = store::load().unwrap_or_else(|e| {
        eprintln!("Tagwerk: Datei konnte nicht gelesen werden: {e}");
        Vec::new()
    });
    let mut tracker = Tracker::new(days);
    let t = now();
    tracker.roll_over(t);
    let locked = screen_locked();
    if locked || !tracker.start(t) {
        tracker.pause_reason = if locked { PauseReason::Locked } else { PauseReason::Manual };
        tracker.paused_since = Some(t);
    }

    let mut state = AppState {
        tracker,
        status_item,
        items,
        records_window: None,
        last_save: Instant::now(),
        last_icon: None,
        file_mtime: store::file_mtime(),
    };
    state.persist(t);
    state.refresh();
    STATE.with(|s| *s.borrow_mut() = Some(state));

    // Erster Start der installierten App: automatisch bei Anmeldung starten.
    let marker = store::data_dir().join(".login-item-set");
    if running_from_bundle() && !marker.exists() {
        set_login_item(true);
        let _ = std::fs::write(&marker, "1");
        with_state(|s| s.refresh());
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
            ("NSWorkspaceWillPowerOffNotification", sel!(willTerminate:)),
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

    app.run();
}
