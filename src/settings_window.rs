//! Fenster „Einstellungen“ nach den macOS Human Interface Guidelines: nicht modal,
//! Änderungen gelten sofort, kein „OK“/„Abbrechen“.
//!
//! Einstellbar sind der Arbeitszeitrahmen, die Regelwochenarbeitszeit und die
//! Mindestdauer einer Pause. Gespeichert wird in der Datenbank; die jeweils andere
//! Tagwerk-Instanz (Menüleiste bzw. Fenster „Aufzeichnungen“) übernimmt die Werte.
//!
//! Rechts in der Symbolleiste öffnet ein Ordner-Knopf den Datenordner im Finder.

use crate::day::{secs_of, DAY_END};
use crate::rules::{self, Settings, MIN_PAUSE_RANGE, WEEKLY_RANGE};
use crate::store;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSCopying, NSDate, NSLocale, NSPoint, NSRect, NSSize, NSString, NSURL};
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

fn picker_secs(p: &NSDatePicker) -> Option<i64> {
    let ts = p.dateValue().timeIntervalSince1970();
    DateTime::from_timestamp(ts.floor() as i64, 0).map(|u| secs_of(u.with_timezone(&Local).naive_local().time()))
}

struct State {
    window: Retained<NSWindow>,
    _controller: Retained<Controller>,
    db: Option<store::Db>,
    settings: Settings,
    frame_from: Retained<NSDatePicker>,
    frame_to: Retained<NSDatePicker>,
    weekly: Retained<NSTextField>,
    weekly_stepper: Retained<NSStepper>,
    pause: Retained<NSTextField>,
    pause_stepper: Retained<NSStepper>,
    daily_note: Retained<NSTextField>,
    error: Retained<NSTextField>,
    on_change: fn(),
}

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

        /// Knopf in der Titelleiste: Datenordner im Finder öffnen.
        #[unsafe(method(openDataFolder:))]
        fn open_data_folder(&self, _sender: Option<&AnyObject>) {
            let dir = store::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let url = NSURL::fileURLWithPath(&ns(&dir.to_string_lossy()));
            NSWorkspace::sharedWorkspace().openURL(&url);
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
        self.frame_from.setDateValue(&to_nsdate(s.frame_from));
        self.frame_to.setDateValue(&to_nsdate(s.frame_to));
        self.weekly.setStringValue(&ns(&rules::fmt_hours(s.weekly_target)));
        self.weekly_stepper.setDoubleValue(s.weekly_target as f64 / 3600.0);
        self.pause.setStringValue(&ns(&(s.min_pause / 60).to_string()));
        self.pause_stepper.setIntegerValue((s.min_pause / 60) as isize);
        self.daily_note.setStringValue(&ns(&format!(
            "Das sind {} pro Arbeitstag (Montag bis Freitag).",
            store::fmt_hm(s.daily_target_workday())
        )));
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

    fn show_error(&self, text: &str) {
        self.error.setStringValue(&ns(text));
        self.error.setHidden(false);
    }
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
    let daily_note = caption("");
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

    let frame_row = hstack(&[&frame_from, &label("bis"), &frame_to, &label("Uhr")]);
    let weekly_row = hstack(&[&weekly, &weekly_stepper, &label("Stunden pro Woche")]);
    let pause_row = hstack(&[&pause, &pause_stepper, &label("Minuten")]);
    let empty = || NSGridCell::emptyContentView(mtm);
    let rows: Vec<Retained<NSArray<NSView>>> = vec![
        NSArray::from_retained_slice(&[view(&label("Arbeitszeitrahmen:")), view(&frame_row)]),
        NSArray::from_retained_slice(&[
            empty(),
            view(&caption(
                "Montag bis Freitag. Außerhalb dieses Zeitraums wird weder Arbeitszeit noch eine Pause erfasst.",
            )),
        ]),
        NSArray::from_retained_slice(&[view(&label("Regelarbeitszeit:")), view(&weekly_row)]),
        NSArray::from_retained_slice(&[empty(), view(&daily_note)]),
        NSArray::from_retained_slice(&[view(&label("Mindestpause:")), view(&pause_row)]),
        NSArray::from_retained_slice(&[
            empty(),
            view(&caption(
                "Ist der Mac kürzer gesperrt, zählt die Zeit weiter zur Arbeitszeit. \
                 Nach dem Arbeitszeitgesetz zählen Pausen erst ab 15 Minuten.",
            )),
        ]),
        NSArray::from_retained_slice(&[empty(), view(&error)]),
        NSArray::from_retained_slice(&[empty(), view(&reset)]),
    ];
    let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
    grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
    grid.setColumnSpacing(8.0);
    grid.setRowSpacing(4.0);
    grid.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Trailing);
    for r in [2, 4, 7] {
        grid.rowAtIndex(r).setTopPadding(14.0);
    }
    grid.setTranslatesAutoresizingMaskIntoConstraints(false);

    let size = grid.fittingSize();
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
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("TagwerkSettingsToolbar"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(target)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    toolbar.setAllowsUserCustomization(false);
    window.setToolbar(Some(&toolbar));
    let root = window.contentView().expect("contentView");
    root.addSubview(&grid);
    grid.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0).setActive(true);
    grid.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -20.0).setActive(true);
    grid.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 20.0).setActive(true);
    grid.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -20.0).setActive(true);
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
        frame_from,
        frame_to,
        weekly,
        weekly_stepper,
        pause,
        pause_stepper,
        daily_note,
        error,
        on_change,
    };
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
}
