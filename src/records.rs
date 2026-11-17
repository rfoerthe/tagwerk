//! Fenster „Aufzeichnungen“ – natives AppKit-Fenster nach den macOS Human Interface
//! Guidelines: Symbolleiste, Übersicht, Tabelle mit Wochensummen, Inspektor zum
//! Korrigieren eines Tages (Arbeitsbeginn, Pausen, Feierabend) und Export als Markdown
//! über den Sichern-Dialog.
//!
//! Das Fenster läuft als eigener Prozess (`Tagwerk --records`), den die Menüleisten-App
//! startet. Änderungen gehen direkt in die Datenbank; die Menüleisten-App übernimmt sie.

use crate::day::{fmt_clock, secs_of, Break, Day, DayRecord, Span, DAY_END};
use crate::rules::{self, Settings, Warning};
use crate::settings_window;
use crate::store::{self, Status};
use block2::RcBlock;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSCopying, NSDate, NSDictionary, NSEdgeInsets, NSRectEdge, NSIndexSet, NSInteger, NSLocale, NSNotification, NSPoint,
    NSRect, NSSize, NSString, NSTimer, NSURL,
};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

// ---------- Kleine Helfer ----------

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn now() -> NaiveDateTime {
    Local::now().naive_local()
}

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("Hauptthread")
}

const WEEKDAYS: [&str; 7] = ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"];
const MONTHS: [&str; 12] = [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober",
    "November", "Dezember",
];

/// `Dienstag, 17. November 2026`
fn fmt_date_long(d: NaiveDate) -> String {
    format!(
        "{}, {}. {} {}",
        WEEKDAYS[d.weekday().num_days_from_monday() as usize],
        d.day(),
        MONTHS[d.month0() as usize],
        d.year()
    )
}

/// `40 Min.` bzw. `1:05 h`
fn fmt_dur(secs: i64) -> String {
    if secs < 3600 {
        format!("{} Min.", secs.max(0) / 60)
    } else {
        store::fmt_hm(secs)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
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

/// Uhrzeit `secs` am Tag `date` (24:00 wird zu 23:59).
fn at(date: NaiveDate, secs: i64) -> NaiveDateTime {
    date.and_hms_opt(0, 0, 0).unwrap() + Duration::seconds(secs.clamp(0, DAY_END - 60))
}

fn symbol(name: &str, desc: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(desc)))
}

fn font_semibold(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, unsafe { NSFontWeightSemibold })
}

fn font_digits(size: f64, weight: NSFontWeight) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
}

fn font_regular() -> NSFontWeight {
    unsafe { NSFontWeightRegular }
}

fn small_size() -> f64 {
    NSFont::smallSystemFontSize()
}

fn label(text: &str) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm());
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l
}

fn caption(text: &str) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm());
    l.setFont(Some(&NSFont::systemFontOfSize(small_size())));
    l.setTextColor(Some(&NSColor::secondaryLabelColor()));
    l.setSelectable(false);
    l
}

fn headline(text: &str) -> Retained<NSTextField> {
    let l = label(text);
    l.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
    l
}

fn separator() -> Retained<NSBox> {
    let b = NSBox::new(mtm());
    b.setBoxType(NSBoxType::Separator);
    b
}

fn hstack(spacing: f64) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm());
    s.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    s.setAlignment(NSLayoutAttribute::FirstBaseline);
    s.setSpacing(spacing);
    s
}

fn vstack(spacing: f64) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm());
    s.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(spacing);
    s
}

fn button(title: &str, action: Sel, target: &Controller) -> Retained<NSButton> {
    unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(target), Some(action), mtm()) }
}

fn checkbox(title: &str, on: bool, action: Sel, target: &Controller) -> Retained<NSButton> {
    let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), Some(target), Some(action), mtm()) };
    b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    b
}

/// Uhrzeit-Feld (Stunde:Minute) mit Pfeiltasten für den Tag `date`; heute nicht in der Zukunft.
fn time_picker(date: NaiveDate, secs: i64, action: Sel, target: &Controller) -> Retained<NSDatePicker> {
    let p = NSDatePicker::new(mtm());
    p.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
    p.setDatePickerElements(NSDatePickerElementFlags::HourMinute);
    p.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    p.setBezeled(true);
    p.setDrawsBackground(true);
    let t = now();
    let min = at(date, 0);
    let max = if date == t.date() { t } else { at(date, DAY_END) };
    p.setMinDate(Some(&to_nsdate(min)));
    p.setMaxDate(Some(&to_nsdate(max)));
    p.setDateValue(&to_nsdate(at(date, secs).clamp(min, max)));
    unsafe {
        p.setTarget(Some(target));
        p.setAction(Some(action));
    }
    p
}

/// Sekunden seit Mitternacht aus einem Uhrzeit-Feld.
fn picker_secs(p: &NSDatePicker) -> Option<i64> {
    from_nsdate(&p.dateValue()).map(|t| secs_of(t.time()))
}

fn pin_width(view: &NSView, to: &NSView, inset: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor().constraintEqualToAnchor_constant(&to.widthAnchor(), -inset).setActive(true);
}

// ---------- Daten ----------

/// Eine Tabellenzeile: Wochenüberschrift oder Arbeitstag.
enum Row {
    Week { title: String, total: String },
    Day { day: Day, rec: DayRecord, warnings: Vec<Warning> },
}

/// Steuerelemente des Inspektors, die Aktionen auslesen müssen.
struct Inspector {
    date: NaiveDate,
    subtitle: Retained<NSTextField>,
    start_picker: Retained<NSDatePicker>,
    end_picker: Retained<NSDatePicker>,
    break_from: Retained<NSDatePicker>,
    break_to: Retained<NSDatePicker>,
    error: Retained<NSTextField>,
}

/// Steuerelemente der Zusatzansicht im Sichern-Dialog.
struct Export {
    panel: Retained<NSSavePanel>,
    preset: Retained<NSPopUpButton>,
    from: Retained<NSDatePicker>,
    to: Retained<NSDatePicker>,
    summary: Retained<NSTextField>,
}

#[derive(Default)]
struct Model {
    db: Option<store::Db>,
    days: Vec<Day>,
    settings: Settings,
    rows: Vec<Row>,
    status: Option<Status>,
    selected: Option<NaiveDate>,
    /// Stand, aus dem der Inspektor zuletzt aufgebaut wurde.
    built: Option<(NaiveDate, Option<Day>, Settings)>,
    inspector: Option<Inspector>,
    export: Option<Export>,
    error: Option<String>,
}

impl Model {
    fn day(&self, date: NaiveDate) -> Option<&Day> {
        self.days.iter().find(|d| d.date == date)
    }

    fn records(&self) -> Vec<DayRecord> {
        self.days.iter().filter_map(|d| d.record(&self.settings)).collect()
    }

    fn row_of(&self, date: NaiveDate) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r, Row::Day { day, .. } if day.date == date))
    }

    fn date_at(&self, row: isize) -> Option<NaiveDate> {
        match self.rows.get(usize::try_from(row).ok()?)? {
            Row::Day { day, .. } => Some(day.date),
            Row::Week { .. } => None,
        }
    }

    /// Liest Datenbank und Status neu und baut die Tabellenzeilen auf.
    fn load(&mut self) {
        if let Some(db) = self.db.as_ref() {
            match db.load().and_then(|d| Ok((d, db.load_settings()?))) {
                Ok((d, settings)) => {
                    self.days = d;
                    self.settings = settings;
                    self.error = None;
                }
                Err(e) => self.error = Some(format!("Die Datenbank konnte nicht gelesen werden: {e}")),
            }
        }
        self.status = store::read_status();
        self.build_rows();
    }

    fn build_rows(&mut self) {
        let s = &self.settings;
        let mut recs: Vec<(Day, DayRecord)> =
            self.days.iter().filter_map(|d| d.record(s).map(|r| (d.clone(), r))).collect();
        recs.reverse();
        let mut rows = Vec::new();
        let mut i = 0;
        while i < recs.len() {
            let week = recs[i].1.date.iso_week();
            let j = recs[i..].iter().position(|(_, r)| r.date.iso_week() != week).map_or(recs.len(), |n| i + n);
            let monday = recs[i].1.date - Duration::days(recs[i].1.date.weekday().num_days_from_monday() as i64);
            let sunday = monday + Duration::days(6);
            let total: i64 = recs[i..j].iter().map(|(_, r)| r.work_secs).sum();
            let (_, target) = rules::balance(&self.days, monday, sunday, s);
            rows.push(Row::Week {
                title: format!(
                    "KW {}  ·  {} – {}",
                    week.week(),
                    monday.format("%d.%m."),
                    sunday.format("%d.%m.%Y")
                ),
                total: format!(
                    "{}  ·  {}  ·  Saldo {}",
                    plural(j - i, "Tag", "Tage"),
                    store::fmt_hm(total),
                    rules::fmt_balance(total - target)
                ),
            });
            for (day, rec) in recs[i..j].iter().cloned() {
                let warnings = rules::day_warnings(&self.days, day.date, s);
                rows.push(Row::Day { day, rec, warnings });
            }
            i = j;
        }
        self.rows = rows;
    }
}

/// Fenster und Ansichten (werden nur im Hauptthread benutzt).
#[derive(Clone)]
struct Ui {
    window: Retained<NSWindow>,
    table: Retained<NSTableView>,
    split: Retained<NSSplitView>,
    inspector_pane: Retained<NSView>,
    inspector: Retained<NSStackView>,
    cards: Vec<(Retained<NSTextField>, Retained<NSTextField>)>,
    footer: Retained<NSTextField>,
    controller: Retained<Controller>,
}

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static TICKS: Cell<u64> = const { Cell::new(0) };
    static SCREENSHOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn ui() -> Ui {
    UI.with(|u| u.borrow().clone()).expect("Fenster aufgebaut")
}

/// Kurzer Zugriff auf das Modell. Innerhalb von `f` keine AppKit-Aufrufe, die
/// zurückrufen könnten (Tabelle neu laden, Auswahl ändern).
fn model<R>(f: impl FnOnce(&mut Model) -> R) -> R {
    MODEL.with(|m| f(&mut m.borrow_mut()))
}

// ---------- Controller (Ziel aller Aktionen, Datenquelle, Delegate) ----------

const COLUMNS: [(&str, &str, f64, f64, f64); 7] = [
    // Kennung, Titel, Breite, Mindest- und Höchstbreite (zusätzlicher Platz geht an „Datum“)
    ("mark", "", 22.0, 22.0, 22.0),
    ("date", "Datum", 140.0, 110.0, 10_000.0),
    ("start", "Beginn", 64.0, 56.0, 120.0),
    ("end", "Ende", 64.0, 56.0, 120.0),
    ("work", "Arbeitszeit", 90.0, 76.0, 140.0),
    ("pause", "Pausen", 70.0, 60.0, 130.0),
    ("balance", "Saldo", 76.0, 66.0, 130.0),
];

const TB_RELOAD: &str = "reload";
const TB_ADD: &str = "add";
const TB_EXPORT: &str = "export";
const TB_INSPECTOR: &str = "inspector";

/// Vorgaben im Export-Dialog (Tag des Menüpunkts).
const PRESET_4W: isize = 0;
const PRESET_MONTH: isize = 1;
const PRESET_LAST_MONTH: isize = 2;
const PRESET_YEAR: isize = 3;
const PRESET_CUSTOM: isize = 9;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkRecordsController"]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSTableViewDataSource for Controller {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            MODEL.with(|m| m.borrow().rows.len() as NSInteger)
        }
    }

    unsafe impl NSControlTextEditingDelegate for Controller {}

    unsafe impl NSTableViewDelegate for Controller {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(&self, _table: &NSTableView, column: Option<&NSTableColumn>, row: NSInteger) -> Option<Retained<NSView>> {
            let id = column.map(|c| c.identifier().to_string());
            MODEL.with(|m| cell_view(&m.borrow(), id.as_deref(), row))
        }

        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group_row(&self, _table: &NSTableView, row: NSInteger) -> bool {
            MODEL.with(|m| matches!(m.borrow().rows.get(row as usize), Some(Row::Week { .. })))
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select(&self, _table: &NSTableView, row: NSInteger) -> bool {
            MODEL.with(|m| matches!(m.borrow().rows.get(row as usize), Some(Row::Day { .. })))
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn height_of_row(&self, _table: &NSTableView, row: NSInteger) -> f64 {
            let group = MODEL.with(|m| matches!(m.borrow().rows.get(row as usize), Some(Row::Week { .. })));
            if group { 28.0 } else { 26.0 }
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            let row = ui().table.selectedRow();
            let changed = model(|m| match m.date_at(row) {
                Some(d) if m.selected != Some(d) => {
                    m.selected = Some(d);
                    true
                }
                Some(_) => false,
                // Abwählen in der Tabelle; ein nachgetragener Tag ohne Zeile bleibt ausgewählt.
                None => match m.selected {
                    Some(d) if m.row_of(d).is_some() => {
                        m.selected = None;
                        true
                    }
                    _ => false,
                },
            });
            if changed {
                refresh_inspector(true);
            }
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

    unsafe impl NSWindowDelegate for Controller {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _n: &NSNotification) {
            NSApplication::sharedApplication(mtm()).terminate(None);
        }
    }

    impl Controller {
        #[unsafe(method(reload:))]
        fn reload_action(&self, _sender: Option<&AnyObject>) {
            reload();
        }

        #[unsafe(method(addDay:))]
        fn add_day(&self, _sender: Option<&AnyObject>) {
            ask_day();
        }

        #[unsafe(method(exportRecords:))]
        fn export_records(&self, _sender: Option<&AnyObject>) {
            show_export();
        }

        #[unsafe(method(toggleInspector:))]
        fn toggle_inspector(&self, _sender: Option<&AnyObject>) {
            let ui = ui();
            ui.inspector_pane.setHidden(!ui.inspector_pane.isHidden());
            ui.split.adjustSubviews();
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            settings_window::show(reload);
        }

        #[unsafe(method(revealDatabase:))]
        fn reveal_database(&self, _sender: Option<&AnyObject>) {
            reveal(&store::db_file());
        }

        #[unsafe(method(startToggled:))]
        fn start_toggled(&self, sender: Option<&NSButton>) {
            let on = sender.is_some_and(|b| b.state() == NSControlStateValueOn);
            let secs = model(|m| m.inspector.as_ref().and_then(|i| picker_secs(&i.start_picker)));
            apply(Edit::SetStart(if on { secs } else { None }), true);
        }

        #[unsafe(method(startChanged:))]
        fn start_changed(&self, sender: Option<&NSDatePicker>) {
            if let Some(secs) = sender.and_then(picker_secs) {
                apply(Edit::SetStart(Some(secs)), false);
            }
        }

        #[unsafe(method(endToggled:))]
        fn end_toggled(&self, sender: Option<&NSButton>) {
            let on = sender.is_some_and(|b| b.state() == NSControlStateValueOn);
            let secs = model(|m| m.inspector.as_ref().and_then(|i| picker_secs(&i.end_picker)));
            apply(Edit::SetEnd(if on { secs } else { None }), true);
        }

        #[unsafe(method(endChanged:))]
        fn end_changed(&self, sender: Option<&NSDatePicker>) {
            if let Some(secs) = sender.and_then(picker_secs) {
                apply(Edit::SetEnd(Some(secs)), false);
            }
        }

        #[unsafe(method(addBreak:))]
        fn add_break(&self, _sender: Option<&AnyObject>) {
            let (times, min) = model(|m| {
                (
                    m.inspector.as_ref().map(|i| (picker_secs(&i.break_from), picker_secs(&i.break_to))),
                    m.settings.min_pause,
                )
            });
            match times {
                Some((Some(from), Some(to))) if to - from >= min => apply(Edit::AddBreak(from, to), true),
                Some((Some(from), Some(to))) if to > from => show_inspector_error(&format!(
                    "Eine Pause dauert mindestens {} Min. (siehe Einstellungen).",
                    min / 60
                )),
                Some((Some(_), Some(_))) => show_inspector_error("Das Ende der Pause muss nach ihrem Beginn liegen."),
                _ => {}
            }
        }

        #[unsafe(method(removeBreak:))]
        fn remove_break(&self, sender: Option<&NSButton>) {
            if let Some(b) = sender {
                apply(Edit::RemoveBreak(b.tag() as usize), true);
            }
        }

        #[unsafe(method(exportPreset:))]
        fn export_preset(&self, sender: Option<&NSPopUpButton>) {
            let tag = sender.map(|p| p.selectedTag()).unwrap_or(PRESET_4W);
            if let Some((from, to)) = preset_range(tag) {
                model(|m| {
                    if let Some(ex) = m.export.as_ref() {
                        ex.from.setDateValue(&to_nsdate(at(from, 0)));
                        ex.to.setDateValue(&to_nsdate(at(to, 0)));
                    }
                });
            }
            export_range_changed(false);
        }

        #[unsafe(method(exportRange:))]
        fn export_range(&self, _sender: Option<&AnyObject>) {
            export_range_changed(true);
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            tick();
        }
    }
);

define_class!(
    /// Dokumentansicht des Inspektors: oben beginnend (wie Text) statt unten.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkFlippedView"]
    struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

// ---------- Tabelle ----------

fn cell_view(m: &Model, column: Option<&str>, row: NSInteger) -> Option<Retained<NSView>> {
    let mtm = mtm();
    let row = m.rows.get(usize::try_from(row).ok()?)?;
    let cell = NSTableCellView::new(mtm);
    match (row, column) {
        (Row::Week { title, total }, _) => {
            let t = label(title);
            t.setFont(Some(&font_semibold(NSFont::systemFontSize())));
            let s = label(total);
            s.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
            s.setTextColor(Some(&NSColor::secondaryLabelColor()));
            s.setAlignment(NSTextAlignment::Right);
            for v in [&t, &s] {
                v.setTranslatesAutoresizingMaskIntoConstraints(false);
                cell.addSubview(v);
                v.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            }
            t.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 6.0).setActive(true);
            s.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -12.0).setActive(true);
            s.leadingAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&t.trailingAnchor(), 12.0)
                .setActive(true);
            unsafe { cell.setTextField(Some(&t)) };
        }
        (Row::Day { day, warnings, .. }, Some("mark")) => {
            // Hinweise gehen vor; sonst Stift für manuell korrigierte Tage.
            let mark = if !warnings.is_empty() {
                let tip: Vec<String> = warnings.iter().map(Warning::text).collect();
                Some(("exclamationmark.triangle.fill", NSColor::systemOrangeColor(), tip.join("\n")))
            } else if day.has_corrections() {
                Some(("pencil", NSColor::secondaryLabelColor(), "Manuell korrigiert".to_string()))
            } else {
                None
            };
            if let Some((name, color, tip)) = mark {
                let iv = NSImageView::new(mtm);
                iv.setImage(symbol(name, &tip).as_deref());
                iv.setContentTintColor(Some(&color));
                iv.setTranslatesAutoresizingMaskIntoConstraints(false);
                cell.addSubview(&iv);
                iv.centerXAnchor().constraintEqualToAnchor(&cell.centerXAnchor()).setActive(true);
                iv.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
                unsafe { cell.setImageView(Some(&iv)) };
                cell.setToolTip(Some(&ns(&tip)));
            }
        }
        (Row::Day { day, rec, .. }, Some(col)) => {
            let today = now().date();
            let is_today = rec.date == today;
            let (text, align, secondary) = match col {
                "date" => (store::fmt_date(rec.date), NSTextAlignment::Left, false),
                "start" => (fmt_clock(rec.first_start), NSTextAlignment::Right, false),
                "end" => {
                    if is_today && day.end.is_none() && m.status == Some(Status::Running) {
                        ("läuft".to_string(), NSTextAlignment::Right, true)
                    } else {
                        (fmt_clock(rec.last_end), NSTextAlignment::Right, false)
                    }
                }
                "work" => (store::fmt_hm(rec.work_secs), NSTextAlignment::Right, false),
                "balance" => {
                    let target = m.settings.daily_target(rec.date);
                    if rec.work_secs > 0 {
                        (rules::fmt_balance(rec.work_secs - target), NSTextAlignment::Right, true)
                    } else {
                        ("–".to_string(), NSTextAlignment::Right, true)
                    }
                }
                "pause" => (store::fmt_hm(rec.pause_secs()), NSTextAlignment::Right, false),
                _ => return None,
            };
            let t = label(&text);
            let weight = if is_today && col == "date" {
                unsafe { NSFontWeightSemibold }
            } else {
                font_regular()
            };
            t.setFont(Some(&font_digits(NSFont::systemFontSize(), weight)));
            t.setAlignment(align);
            if secondary {
                t.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            t.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&t);
            t.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true);
            t.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
            t.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            unsafe { cell.setTextField(Some(&t)) };
        }
        (Row::Day { .. }, None) => return None,
    }
    Some(Retained::into_super(cell))
}

/// Tabelle neu laden und die Auswahl wiederherstellen.
fn reload_table() {
    let ui = ui();
    ui.table.reloadData();
    let (selected, row) = model(|m| (m.selected, m.selected.and_then(|d| m.row_of(d))));
    match row {
        Some(r) => {
            if ui.table.selectedRow() != r as isize {
                ui.table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(r), false);
            }
        }
        None if selected.is_some() => unsafe { ui.table.deselectAll(None) },
        None => {}
    }
}

// ---------- Übersicht, Untertitel, Fußzeile ----------

fn refresh_summary() {
    let ui = ui();
    let t = now();
    let today = t.date();
    let (texts, status, count, error, rules_text) = model(|m| {
        let st = m.settings;
        let recs: Vec<DayRecord> = m.records().into_iter().filter(|r| r.work_secs > 0).collect();
        // (Arbeit, Pausen, Tage, Soll)
        let sum = |f: &dyn Fn(NaiveDate) -> bool| {
            recs.iter().filter(|r| f(r.date)).fold((0i64, 0i64, 0usize, 0i64), |(w, p, n, t), r| {
                (w + r.work_secs, p + r.pause_secs(), n + 1, t + st.daily_target(r.date))
            })
        };
        let (tw, _tp, tn, _) = sum(&|d| d == today);
        let week_start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        let (ww, _wp, wn, wt) = sum(&|d| d >= week_start && d <= today);
        let (mw, mp, mn, mt) = sum(&|d| d.year() == today.year() && d.month() == today.month());
        let begin = m.day(today).zip(recs.iter().find(|r| r.date == today)).map(|(d, r)| {
            let manual = if d.start.is_some() { " (manuell)" } else { "" };
            format!("Beginn {} Uhr{manual}", fmt_clock(r.first_start))
        });
        let today_target = st.daily_target(today);
        let today_detail = match (tn, begin) {
            (0, _) | (_, None) if today_target > 0 => format!("Soll {}", store::fmt_hm(today_target)),
            (0, _) | (_, None) => "Kein Arbeitstag".to_string(),
            (_, Some(b)) if today_target > 0 => format!("{b} · Soll {}", store::fmt_hm(today_target)),
            (_, Some(b)) => b,
        };
        let avg = |x: i64| if mn > 0 { x / mn as i64 } else { 0 };
        let texts = [
            (store::fmt_hm(tw), today_detail),
            (store::fmt_hm(ww), format!("{} · Saldo {}", plural(wn, "Arbeitstag", "Arbeitstage"), rules::fmt_balance(ww - wt))),
            (store::fmt_hm(mw), format!("{} · Saldo {}", plural(mn, "Arbeitstag", "Arbeitstage"), rules::fmt_balance(mw - mt))),
            (store::fmt_hm(avg(mw)), format!("Pausen Ø {} · im {}", store::fmt_hm(avg(mp)), MONTHS[today.month0() as usize])),
        ];
        (texts, m.status, recs.len(), m.error.clone(), st.summary())
    });
    for ((value, detail), (v, d)) in ui.cards.iter().zip(texts) {
        value.setStringValue(&ns(&v));
        detail.setStringValue(&ns(&d));
    }
    let subtitle = match status {
        Some(Status::Running) => "Zeiterfassung läuft".to_string(),
        Some(Status::Paused) => "Zeiterfassung pausiert".to_string(),
        Some(Status::Break(since)) => format!("Pause seit {} Uhr", since.format("%H:%M")),
        Some(Status::Outside) => "Außerhalb des Arbeitszeitrahmens".to_string(),
        Some(Status::Closed) => "Feierabend".to_string(),
        None => "Menüleisten-App nicht aktiv".to_string(),
    };
    ui.window.setSubtitle(&ns(&subtitle));
    let footer = match error {
        Some(e) => e,
        None => format!("{} erfasst  ·  {rules_text}", plural(count, "Arbeitstag", "Arbeitstage")),
    };
    ui.footer.setStringValue(&ns(&footer));
}

/// Alles neu einlesen und anzeigen.
fn reload() {
    model(|m| m.load());
    reload_table();
    refresh_summary();
    refresh_inspector(false);
}

// ---------- Inspektor ----------

/// Was im Inspektor geändert wurde.
enum Edit {
    SetStart(Option<i64>),
    SetEnd(Option<i64>),
    AddBreak(i64, i64),
    RemoveBreak(usize),
}

fn apply(edit: Edit, rebuild: bool) {
    let result = model(|m| {
        let date = m.inspector.as_ref()?.date;
        let db = m.db.as_mut()?;
        Some(db.edit_day(date, now(), |d| match edit {
            Edit::SetStart(s) => d.start = s,
            Edit::SetEnd(e) => d.end = e,
            Edit::AddBreak(from, to) => {
                d.breaks.push(Break { from, to });
                d.sort_breaks();
            }
            Edit::RemoveBreak(i) => {
                if i < d.breaks.len() {
                    d.breaks.remove(i);
                }
            }
        }))
    });
    match result {
        None => {}
        Some(Err(e)) => show_inspector_error(&format!("Speichern fehlgeschlagen: {e}")),
        Some(Ok(())) => {
            model(|m| {
                m.load();
                if !rebuild {
                    // Eingabe läuft weiter: Felder stehen lassen, nur die Zahlen aktualisieren.
                    if let Some(date) = m.selected {
                        m.built = Some((date, m.day(date).cloned(), m.settings));
                    }
                }
            });
            reload_table();
            refresh_summary();
            refresh_inspector(rebuild);
        }
    }
}

fn show_inspector_error(text: &str) {
    model(|m| {
        if let Some(i) = m.inspector.as_ref() {
            i.error.setStringValue(&ns(text));
            i.error.setHidden(false);
        }
    });
}

fn day_subtitle(day: Option<&Day>, settings: &Settings) -> String {
    match day.and_then(|d| d.record(settings)) {
        Some(r) if r.work_secs > 0 => format!(
            "Arbeitszeit {}  ·  Pausen {}  ·  Saldo {}",
            store::fmt_hm(r.work_secs),
            store::fmt_hm(r.pause_secs()),
            rules::fmt_balance(r.work_secs - settings.daily_target(r.date))
        ),
        Some(_) => "Keine Arbeitszeit im Arbeitszeitrahmen.".to_string(),
        None => "Für diesen Tag ist noch nichts erfasst.".to_string(),
    }
}

/// Inspektor aktualisieren. Ohne `force` nur, wenn sich der Tag geändert hat und
/// gerade kein Feld im Inspektor bearbeitet wird.
fn refresh_inspector(force: bool) {
    let ui = ui();
    let (selected, day, unchanged) = model(|m| {
        let day = m.selected.and_then(|d| m.day(d).cloned());
        let unchanged = m.selected.is_some_and(|d| m.built.as_ref() == Some(&(d, day.clone(), m.settings)));
        (m.selected, day, unchanged)
    });
    if !force {
        // Laufende Zahlen (heute) aktualisieren
        model(|m| {
            if let Some(i) = m.inspector.as_ref() {
                i.subtitle.setStringValue(&ns(&day_subtitle(day.as_ref(), &m.settings)));
            }
        });
        if unchanged || (selected.is_none() && model(|m| m.inspector.is_none() && m.built.is_none())) {
            return;
        }
        if editing_in_inspector(&ui) {
            return;
        }
    }
    build_inspector(&ui, selected, day);
}

fn editing_in_inspector(ui: &Ui) -> bool {
    let Some(responder) = ui.window.firstResponder() else { return false };
    let Ok(view) = responder.downcast::<NSView>() else { return false };
    // Feldeditor eines Textfelds: dessen Delegate ist das Feld selbst.
    view.isDescendantOf(&ui.inspector)
}

fn clear(stack: &NSStackView) {
    for v in stack.arrangedSubviews().iter() {
        v.removeFromSuperview();
    }
}

fn build_inspector(ui: &Ui, selected: Option<NaiveDate>, day: Option<Day>) {
    let target = &*ui.controller;
    let stack = &ui.inspector;
    clear(stack);
    let Some(date) = selected else {
        model(|m| {
            m.inspector = None;
            m.built = None;
        });
        let empty = label("Kein Tag ausgewählt");
        empty.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        empty.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        let hint = caption("Wähle in der Liste einen Tag aus, um Arbeitsbeginn, Pausen oder Feierabend zu korrigieren.");
        stack.addArrangedSubview(&empty);
        stack.addArrangedSubview(&hint);
        pin_width(&hint, stack, 40.0);
        return;
    };
    let t = now();
    let (settings, warnings) = model(|m| (m.settings, rules::day_warnings(&m.days, date, &m.settings)));
    let rec = day.as_ref().and_then(|d| d.record(&settings));
    let is_today = date == t.date();

    // Kopf
    let title = label(&fmt_date_long(date));
    title.setFont(Some(&font_semibold(15.0)));
    let subtitle = caption(&day_subtitle(day.as_ref(), &settings));
    subtitle.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
    stack.addArrangedSubview(&title);
    stack.addArrangedSubview(&subtitle);
    pin_width(&subtitle, stack, 40.0);
    for w in &warnings {
        let row = hstack(6.0);
        row.setAlignment(NSLayoutAttribute::CenterY);
        if let Some(img) = symbol("exclamationmark.triangle.fill", "Hinweis") {
            let iv = NSImageView::imageViewWithImage(&img, mtm());
            iv.setContentTintColor(Some(&NSColor::systemOrangeColor()));
            row.addArrangedSubview(&iv);
        }
        let l = caption(&format!("{}.", w.text()));
        row.addArrangedSubview(&l);
        stack.addArrangedSubview(&row);
        pin_width(&row, stack, 40.0);
    }
    let sep = separator();
    stack.addArrangedSubview(&sep);
    pin_width(&sep, stack, 40.0);
    stack.setCustomSpacing_afterView(4.0, &title);
    stack.setCustomSpacing_afterView(14.0, &sep);

    let section = |text: &str, prev: Option<&NSView>| {
        if let Some(p) = prev {
            stack.setCustomSpacing_afterView(18.0, p);
        }
        let h = headline(text);
        stack.addArrangedSubview(&h);
    };

    // Arbeitsbeginn
    section("Arbeitsbeginn", None);
    let auto = day.as_ref().and_then(|d| d.auto_bounds_in_frame(&settings));
    // War der Mac auch außerhalb des Rahmens entsperrt, gilt der Wert im Rahmen – dann so benennen.
    let raw_first = day.as_ref().and_then(|d| d.periods.first()).map(|p| p.from);
    let raw_last = day.as_ref().and_then(|d| d.periods.last()).map(|p| p.to);
    let note = |raw: Option<i64>, used: Option<i64>| if raw != used { " im Arbeitszeitrahmen" } else { "" };
    let (first_in_frame, last_in_frame) = (note(raw_first, auto.map(|a| a.0)), note(raw_last, auto.map(|a| a.1)));
    let manual_start = day.as_ref().and_then(|d| d.start);
    let start_value = manual_start.or(rec.as_ref().map(|r| r.first_start)).unwrap_or(8 * 3600);
    let start_check = checkbox("Manuell festlegen", manual_start.is_some(), sel!(startToggled:), target);
    let start_picker = time_picker(date, start_value, sel!(startChanged:), target);
    start_picker.setEnabled(manual_start.is_some());
    let row = hstack(8.0);
    row.addArrangedSubview(&start_check);
    row.addArrangedSubview(&start_picker);
    stack.addArrangedSubview(&row);
    let start_note = caption(&match auto {
        Some((a, _)) => format!("Ohne Angabe gilt das erste Entsperren{first_in_frame} um {} Uhr.", fmt_clock(a)),
        None => "Ohne Angabe gilt das erste Entsperren des Macs im Arbeitszeitrahmen.".to_string(),
    });
    stack.addArrangedSubview(&start_note);
    pin_width(&start_note, stack, 40.0);

    // Pausen
    section("Pausen", Some(&start_note));
    let pause_note = caption(&format!(
        "Sperren ab {} Min. gelten automatisch als Pause, kürzere zählen zur Arbeitszeit. \
         Hier lassen sich zusätzliche Pausen eintragen.",
        settings.min_pause / 60
    ));
    stack.addArrangedSubview(&pause_note);
    pin_width(&pause_note, stack, 40.0);
    let breaks = day.as_ref().map(|d| d.breaks.clone()).unwrap_or_default();
    let mut last: Retained<NSView> = Retained::into_super(Retained::into_super(headline("")));
    if breaks.is_empty() {
        let none = label("Keine Pausen eingetragen");
        none.setTextColor(Some(&NSColor::secondaryLabelColor()));
        stack.addArrangedSubview(&none);
        last = Retained::into_super(Retained::into_super(none));
    }
    for (i, b) in breaks.iter().enumerate() {
        let row = hstack(8.0);
        row.setAlignment(NSLayoutAttribute::CenterY);
        let times = label(&format!("{} – {}", fmt_clock(b.from), fmt_clock(b.to)));
        times.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        let dur = label(&fmt_dur(b.to - b.from));
        dur.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        dur.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let remove = match symbol("minus.circle.fill", "Pause entfernen") {
            Some(img) => unsafe {
                NSButton::buttonWithImage_target_action(&img, Some(target), Some(sel!(removeBreak:)), mtm())
            },
            None => button("Entfernen", sel!(removeBreak:), target),
        };
        remove.setBordered(false);
        remove.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        remove.setToolTip(Some(&ns("Pause entfernen")));
        remove.setTag(i as isize);
        let spacer = NSView::new(mtm());
        spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
        row.addArrangedSubview(&times);
        row.addArrangedSubview(&dur);
        row.addArrangedSubview(&spacer);
        row.addArrangedSubview(&remove);
        stack.addArrangedSubview(&row);
        pin_width(&row, stack, 40.0);
        last = Retained::into_super(row);
    }
    stack.setCustomSpacing_afterView(12.0, &last);
    let (bf, bt) = (12 * 3600, 12 * 3600 + settings.min_pause.max(30 * 60));
    let break_from = time_picker(date, bf, sel!(exportRange:), target);
    let break_to = time_picker(date, bt, sel!(exportRange:), target);
    // Keine Aktion beim Ändern – übernommen wird erst mit „Hinzufügen“.
    unsafe {
        break_from.setAction(None);
        break_to.setAction(None);
    }
    let add = button("Hinzufügen", sel!(addBreak:), target);
    let row = hstack(6.0);
    row.addArrangedSubview(&break_from);
    row.addArrangedSubview(&label("bis"));
    row.addArrangedSubview(&break_to);
    row.addArrangedSubview(&add);
    stack.addArrangedSubview(&row);

    // Feierabend
    let row_view: Retained<NSView> = Retained::into_super(row);
    section("Feierabend", Some(&row_view));
    let manual_end = day.as_ref().and_then(|d| d.end);
    let end_value = manual_end
        .or(rec.as_ref().map(|r| r.last_end))
        .unwrap_or(if is_today { secs_of(t.time()) } else { 17 * 3600 });
    let end_check = checkbox("Manuell festlegen", manual_end.is_some(), sel!(endToggled:), target);
    let end_picker = time_picker(date, end_value, sel!(endChanged:), target);
    end_picker.setEnabled(manual_end.is_some());
    let row = hstack(8.0);
    row.addArrangedSubview(&end_check);
    row.addArrangedSubview(&end_picker);
    stack.addArrangedSubview(&row);
    let end_note = caption(&match auto {
        Some(_) if is_today && manual_end.is_none() => {
            "Ohne Angabe gilt das letzte Sperren. Mit Feierabend wird heute nichts mehr erfasst.".to_string()
        }
        Some((_, a)) => format!(
            "Ohne Angabe gilt das letzte Sperren{last_in_frame} um {} Uhr. Ein späterer Feierabend wird mitgezählt.",
            fmt_clock(a)
        ),
        None => "Ohne Angabe gilt das letzte Sperren des Macs im Arbeitszeitrahmen.".to_string(),
    });
    stack.addArrangedSubview(&end_note);
    pin_width(&end_note, stack, 40.0);

    // Automatisch erfasst
    section("Automatisch erfasst", Some(&end_note));
    let raw_note = caption(&format!(
        "Zeiten, in denen der Mac entsperrt war. Gezählt wird nur im Arbeitszeitrahmen ({}).",
        settings.frame_text()
    ));
    stack.addArrangedSubview(&raw_note);
    pin_width(&raw_note, stack, 40.0);
    let periods: Vec<Span> = day.as_ref().map(|d| d.periods.clone()).unwrap_or_default();
    if periods.is_empty() {
        let none = label("Keine – der Mac war nicht entsperrt.");
        none.setTextColor(Some(&NSColor::secondaryLabelColor()));
        stack.addArrangedSubview(&none);
    }
    for p in &periods {
        let row = hstack(8.0);
        let times = label(&format!("{} – {}", fmt_clock(p.from), fmt_clock(p.to)));
        times.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        times.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let dur = label(&fmt_dur(p.len()));
        dur.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        dur.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        row.addArrangedSubview(&times);
        row.addArrangedSubview(&dur);
        stack.addArrangedSubview(&row);
    }

    let error = caption("");
    error.setTextColor(Some(&NSColor::systemRedColor()));
    error.setHidden(true);
    stack.addArrangedSubview(&error);
    pin_width(&error, stack, 40.0);

    model(|m| {
        m.built = Some((date, day, settings));
        m.inspector = Some(Inspector {
            date,
            subtitle,
            start_picker,
            end_picker,
            break_from,
            break_to,
            error,
        });
    });
}

/// Springt zu einem Tag (auch einem ohne Aufzeichnung).
fn go_to(date: NaiveDate) {
    let row = model(|m| {
        m.selected = Some(date);
        m.row_of(date)
    });
    let ui = ui();
    match row {
        Some(r) => {
            ui.table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(r), false);
            ui.table.scrollRowToVisible(r as isize);
        }
        None => unsafe { ui.table.deselectAll(None) },
    }
    if ui.inspector_pane.isHidden() {
        ui.inspector_pane.setHidden(false);
        ui.split.adjustSubviews();
    }
    refresh_inspector(true);
}

/// „Tag nachtragen …“: Datum im Kalender wählen.
fn ask_day() {
    let ui = ui();
    let mtm = mtm();
    let today = now().date();
    let picker = NSDatePicker::new(mtm);
    picker.setDatePickerStyle(NSDatePickerStyle::ClockAndCalendar);
    picker.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
    picker.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
    picker.setMaxDate(Some(&to_nsdate(at(today, DAY_END))));
    let initial = model(|m| m.selected).unwrap_or(today);
    picker.setDateValue(&to_nsdate(at(initial, 12 * 3600)));
    picker.setBezeled(false);
    picker.setDrawsBackground(false);
    picker.sizeToFit();

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns("Tag bearbeiten"));
    alert.setInformativeText(&ns("Wähle einen Tag, um Arbeitsbeginn, Pausen oder Feierabend nachzutragen."));
    alert.addButtonWithTitle(&ns("Öffnen"));
    alert.addButtonWithTitle(&ns("Abbrechen"));
    alert.setAccessoryView(Some(&picker));
    let p = picker.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response == NSAlertFirstButtonReturn {
            if let Some(d) = from_nsdate(&p.dateValue()) {
                go_to(d.date().min(now().date()));
            }
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&ui.window, Some(&handler));
}

// ---------- Export ----------

fn month_start(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

fn preset_range(tag: isize) -> Option<(NaiveDate, NaiveDate)> {
    let today = now().date();
    match tag {
        PRESET_4W => Some(store::default_export_range(today)),
        PRESET_MONTH => Some((month_start(today), today)),
        PRESET_LAST_MONTH => {
            let last = month_start(today) - Duration::days(1);
            Some((month_start(last), last))
        }
        PRESET_YEAR => Some((today.with_ordinal(1).unwrap_or(today), today)),
        _ => None,
    }
}

fn export_range(ex: &Export) -> Option<(NaiveDate, NaiveDate)> {
    let from = from_nsdate(&ex.from.dateValue())?.date();
    let to = from_nsdate(&ex.to.dateValue())?.date();
    Some((from.min(to), from.max(to)))
}

/// Zeitraum geändert: Zusammenfassung und Dateiname anpassen.
fn export_range_changed(custom: bool) {
    let records = model(|m| m.records());
    model(|m| {
        let Some(ex) = m.export.as_ref() else { return };
        if custom {
            ex.preset.selectItemWithTag(PRESET_CUSTOM);
        }
        let Some((from, to)) = export_range(ex) else { return };
        let (n, w) = records
            .iter()
            .filter(|r| r.date >= from && r.date <= to)
            .fold((0usize, 0i64), |(n, w), r| (n + 1, w + r.work_secs));
        ex.summary.setStringValue(&ns(&format!(
            "{} mit zusammen {} Arbeitszeit",
            plural(n, "Arbeitstag", "Arbeitstage"),
            store::fmt_hm(w)
        )));
        ex.panel.setNameFieldStringValue(&ns(&store::export_file_name(from, to)));
    });
}

fn show_export() {
    if model(|m| m.export.is_some()) {
        return;
    }
    let ui = ui();
    let mtm = mtm();
    let target = &*ui.controller;
    let (from, to) = store::default_export_range(now().date());

    let panel = NSSavePanel::savePanel(mtm);
    panel.setTitle(Some(&ns("Exportieren")));
    panel.setMessage(Some(&ns("Aufzeichnungen eines Zeitraums als Markdown-Tabelle sichern.")));
    panel.setPrompt(Some(&ns("Exportieren")));
    panel.setCanCreateDirectories(true);
    panel.setExtensionHidden(false);
    if let Some(dir) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads")) {
        if dir.is_dir() {
            let url = NSURL::fileURLWithPath(&ns(&dir.to_string_lossy()));
            panel.setDirectoryURL(Some(&url));
        }
    }

    let preset = NSPopUpButton::new(mtm);
    for (title, tag) in [
        ("Letzte 4 Wochen", PRESET_4W),
        ("Dieser Monat", PRESET_MONTH),
        ("Letzter Monat", PRESET_LAST_MONTH),
        ("Dieses Jahr", PRESET_YEAR),
    ] {
        preset.addItemWithTitle(&ns(title));
        if let Some(item) = preset.lastItem() {
            item.setTag(tag);
        }
    }
    if let Some(menu) = preset.menu() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    preset.addItemWithTitle(&ns("Eigener Zeitraum"));
    if let Some(item) = preset.lastItem() {
        item.setTag(PRESET_CUSTOM);
    }
    preset.selectItemWithTag(PRESET_4W);
    unsafe {
        preset.setTarget(Some(target));
        preset.setAction(Some(sel!(exportPreset:)));
    }

    let date_picker = |d: NaiveDate| {
        let p = NSDatePicker::new(mtm);
        p.setDatePickerStyle(NSDatePickerStyle::TextFieldAndStepper);
        p.setDatePickerElements(NSDatePickerElementFlags::YearMonthDay);
        p.setLocale(Some(&NSLocale::localeWithLocaleIdentifier(&ns("de_DE"))));
        p.setBezeled(true);
        p.setDrawsBackground(true);
        p.setDateValue(&to_nsdate(at(d, 0)));
        unsafe {
            p.setTarget(Some(target));
            p.setAction(Some(sel!(exportRange:)));
        }
        p
    };
    let from_p = date_picker(from);
    let to_p = date_picker(to);
    let summary = label("");
    summary.setFont(Some(&font_digits(small_size(), font_regular())));
    summary.setTextColor(Some(&NSColor::secondaryLabelColor()));

    let label_col = |text: &str| {
        let l = label(text);
        l.setAlignment(NSTextAlignment::Right);
        l.setTranslatesAutoresizingMaskIntoConstraints(false);
        l.widthAnchor().constraintEqualToConstant(70.0).setActive(true);
        l
    };
    let row1 = hstack(8.0);
    row1.addArrangedSubview(&label_col("Zeitraum:"));
    row1.addArrangedSubview(&preset);
    let row2 = hstack(8.0);
    row2.addArrangedSubview(&label_col("Von:"));
    row2.addArrangedSubview(&from_p);
    row2.addArrangedSubview(&label("bis"));
    row2.addArrangedSubview(&to_p);
    let row3 = hstack(8.0);
    row3.addArrangedSubview(&label_col(""));
    row3.addArrangedSubview(&summary);
    let acc = vstack(8.0);
    acc.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 20.0, bottom: 12.0, right: 20.0 });
    acc.addArrangedSubview(&row1);
    acc.addArrangedSubview(&row2);
    acc.addArrangedSubview(&row3);
    acc.setTranslatesAutoresizingMaskIntoConstraints(true);
    let size = acc.fittingSize();
    acc.setFrameSize(NSSize::new(size.width.max(420.0), size.height));
    panel.setAccessoryView(Some(&acc));

    model(|m| {
        m.export = Some(Export { panel: panel.clone(), preset, from: from_p, to: to_p, summary });
    });
    export_range_changed(false);

    let handler = RcBlock::new(move |response: NSModalResponse| {
        export_finished(response);
    });
    activate();
    panel.beginSheetModalForWindow_completionHandler(&ui.window, &handler);
}

fn export_finished(response: NSModalResponse) {
    let Some(ex) = model(|m| m.export.take()) else { return };
    if response != NSModalResponseOK {
        return;
    }
    ex.panel.orderOut(None);
    let Some(path) = ex.panel.URL().and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string())) else {
        return;
    };
    let Some((from, to)) = export_range(&ex) else { return };
    let result = model(|m| -> Result<usize, String> {
        let db = m.db.as_ref().ok_or("Keine Datenbank geöffnet.")?;
        let days = db
            .load_range(from - Duration::days(1), to)
            .map_err(|e| format!("Lesen fehlgeschlagen: {e}"))?;
        std::fs::write(&path, store::export_markdown(&days, from, to, now(), &m.settings))
            .map_err(|e| format!("Schreiben fehlgeschlagen: {e}"))?;
        Ok(days.len())
    });
    let mtm = mtm();
    let alert = NSAlert::new(mtm);
    match result {
        Ok(_) => {
            alert.setMessageText(&ns("Export gesichert"));
            alert.setInformativeText(&ns(&format!(
                "Die Aufzeichnungen vom {} bis {} wurden in „{}“ gesichert.",
                store::fmt_date_short(from),
                store::fmt_date_short(to),
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            )));
            alert.addButtonWithTitle(&ns("OK"));
            alert.addButtonWithTitle(&ns("Im Finder zeigen"));
        }
        Err(e) => {
            alert.setAlertStyle(NSAlertStyle::Warning);
            alert.setMessageText(&ns("Export fehlgeschlagen"));
            alert.setInformativeText(&ns(&e));
            alert.addButtonWithTitle(&ns("OK"));
        }
    }
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response == NSAlertSecondButtonReturn {
            reveal(&path);
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&ui().window, Some(&handler));
}

fn reveal(path: &std::path::Path) {
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}

// ---------- Symbolleiste ----------

fn toolbar_ids() -> Retained<NSArray<NSString>> {
    let flex: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[flex, ns(TB_RELOAD), ns(TB_ADD), ns(TB_EXPORT), ns(TB_INSPECTOR)])
}

fn toolbar_item(target: &Controller, id: &NSString) -> Option<Retained<NSToolbarItem>> {
    let mtm = mtm();
    let (label, tip, image, action) = match id.to_string().as_str() {
        TB_RELOAD => ("Aktualisieren", "Aufzeichnungen neu laden", "arrow.clockwise", sel!(reload:)),
        TB_ADD => ("Tag bearbeiten", "Einen beliebigen Tag öffnen und Zeiten nachtragen", "calendar.badge.plus", sel!(addDay:)),
        TB_EXPORT => ("Exportieren", "Zeitraum als Markdown exportieren", "square.and.arrow.up", sel!(exportRecords:)),
        TB_INSPECTOR => ("Informationen", "Informationen ein- oder ausblenden", "sidebar.right", sel!(toggleInspector:)),
        _ => return None,
    };
    let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), id);
    item.setLabel(&ns(label));
    item.setPaletteLabel(&ns(label));
    item.setToolTip(Some(&ns(tip)));
    item.setImage(symbol(image, label).as_deref());
    item.setBordered(true);
    unsafe {
        item.setTarget(Some(target));
        item.setAction(Some(action));
    }
    Some(item)
}

// ---------- Menüleiste des Programms ----------

fn menu_item(title: &str, action: Option<Sel>, key: &str, mods: NSEventModifierFlags, target: Option<&Controller>) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(title), action, &ns(key))
    };
    item.setKeyEquivalentModifierMask(mods);
    if let Some(t) = target {
        unsafe { item.setTarget(Some(t)) };
    }
    item
}

fn build_main_menu(app: &NSApplication, target: &Controller) {
    let mtm = mtm();
    let cmd = NSEventModifierFlags::Command;
    let bar = NSMenu::new(mtm);
    let submenu = |title: &str, items: Vec<Option<Retained<NSMenuItem>>>| {
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(title));
        for it in items {
            match it {
                Some(i) => menu.addItem(&i),
                None => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            }
        }
        let top = menu_item(title, None, "", cmd, None);
        top.setSubmenu(Some(&menu));
        bar.addItem(&top);
        menu
    };
    submenu(
        "Tagwerk",
        vec![
            Some(menu_item("Über Tagwerk", Some(sel!(orderFrontStandardAboutPanel:)), "", cmd, None)),
            None,
            Some(menu_item("Einstellungen …", Some(sel!(showSettings:)), ",", cmd, Some(target))),
            None,
            Some(menu_item("Tagwerk ausblenden", Some(sel!(hide:)), "h", cmd, None)),
            Some(menu_item("Andere ausblenden", Some(sel!(hideOtherApplications:)), "h", cmd | NSEventModifierFlags::Option, None)),
            Some(menu_item("Alle einblenden", Some(sel!(unhideAllApplications:)), "", cmd, None)),
            None,
            Some(menu_item("Aufzeichnungen schließen", Some(sel!(terminate:)), "q", cmd, None)),
        ],
    );
    submenu(
        "Ablage",
        vec![
            Some(menu_item("Tag bearbeiten …", Some(sel!(addDay:)), "n", cmd, Some(target))),
            Some(menu_item("Exportieren …", Some(sel!(exportRecords:)), "e", cmd, Some(target))),
            None,
            Some(menu_item("Datenbank im Finder zeigen", Some(sel!(revealDatabase:)), "", cmd, Some(target))),
            None,
            Some(menu_item("Schließen", Some(sel!(performClose:)), "w", cmd, None)),
        ],
    );
    submenu(
        "Bearbeiten",
        vec![
            Some(menu_item("Widerrufen", Some(sel!(undo:)), "z", cmd, None)),
            Some(menu_item("Wiederholen", Some(sel!(redo:)), "z", cmd | NSEventModifierFlags::Shift, None)),
            None,
            Some(menu_item("Ausschneiden", Some(sel!(cut:)), "x", cmd, None)),
            Some(menu_item("Kopieren", Some(sel!(copy:)), "c", cmd, None)),
            Some(menu_item("Einsetzen", Some(sel!(paste:)), "v", cmd, None)),
            Some(menu_item("Alles auswählen", Some(sel!(selectAll:)), "a", cmd, None)),
        ],
    );
    submenu(
        "Darstellung",
        vec![
            Some(menu_item("Aktualisieren", Some(sel!(reload:)), "r", cmd, Some(target))),
            Some(menu_item("Informationen ein-/ausblenden", Some(sel!(toggleInspector:)), "i", cmd | NSEventModifierFlags::Option, Some(target))),
            None,
            Some(menu_item("Vollbildmodus", Some(sel!(toggleFullScreen:)), "f", cmd | NSEventModifierFlags::Control, None)),
        ],
    );
    let window_menu = submenu(
        "Fenster",
        vec![
            Some(menu_item("Im Dock ablegen", Some(sel!(performMiniaturize:)), "m", cmd, None)),
            Some(menu_item("Zoomen", Some(sel!(performZoom:)), "", cmd, None)),
        ],
    );
    app.setMainMenu(Some(&bar));
    app.setWindowsMenu(Some(&window_menu));
}

// ---------- Fenster aufbauen ----------

fn summary_card(title: &str) -> (Retained<NSBox>, Retained<NSTextField>, Retained<NSTextField>) {
    let mtm = mtm();
    let card = NSBox::new(mtm);
    card.setBoxType(NSBoxType::Custom);
    card.setTitlePosition(NSTitlePosition::NoTitle);
    card.setCornerRadius(10.0);
    card.setBorderWidth(0.0);
    card.setFillColor(&card_fill());
    card.setContentViewMargins(NSSize::new(0.0, 0.0));

    let t = label(title);
    t.setFont(Some(&NSFont::systemFontOfSize(small_size())));
    t.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let value = label("–");
    value.setFont(Some(&font_digits(24.0, unsafe { NSFontWeightSemibold })));
    let detail = label("");
    detail.setFont(Some(&font_digits(small_size(), font_regular())));
    detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let stack = vstack(2.0);
    stack.addArrangedSubview(&t);
    stack.addArrangedSubview(&value);
    stack.addArrangedSubview(&detail);
    stack.setEdgeInsets(NSEdgeInsets { top: 10.0, left: 14.0, bottom: 12.0, right: 14.0 });
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    if let Some(content) = card.contentView() {
        content.addSubview(&stack);
        stack.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()).setActive(true);
        stack.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()).setActive(true);
        stack.topAnchor().constraintEqualToAnchor(&content.topAnchor()).setActive(true);
        stack.bottomAnchor().constraintEqualToAnchor(&content.bottomAnchor()).setActive(true);
    }
    (card, value, detail)
}

/// Füllfarbe der Übersichtskarten: `quaternarySystemFillColor` (erst ab macOS 14).
fn card_fill() -> Retained<NSColor> {
    let responds: bool =
        unsafe { msg_send![objc2::class!(NSColor), respondsToSelector: sel!(quaternarySystemFillColor)] };
    if responds {
        unsafe { msg_send![objc2::class!(NSColor), quaternarySystemFillColor] }
    } else {
        NSColor::quaternaryLabelColor()
    }
}

fn build_window(controller: &Retained<Controller>, shot: bool) -> Ui {
    let mtm = mtm();
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 640.0)),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&ns("Aufzeichnungen"));
    window.setMinSize(NSSize::new(780.0, 480.0));
    window.setDelegate(Some(ProtocolObject::from_ref(&**controller)));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);

    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &ns("TagwerkRecordsToolbar"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&**controller)));
    toolbar.setDisplayMode(NSToolbarDisplayMode::IconOnly);
    toolbar.setAllowsUserCustomization(false);
    window.setToolbar(Some(&toolbar));

    let root = window.contentView().expect("contentView");

    // Übersicht
    let cards_row = NSStackView::new(mtm);
    cards_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    cards_row.setDistribution(NSStackViewDistribution::FillEqually);
    cards_row.setSpacing(12.0);
    let mut cards = Vec::new();
    for title in ["Heute", "Diese Woche", "Dieser Monat", "Ø pro Arbeitstag"] {
        let (card, value, detail) = summary_card(title);
        cards_row.addArrangedSubview(&card);
        cards.push((value, detail));
    }

    // Tabelle
    let table = NSTableView::new(mtm);
    table.setStyle(NSTableViewStyle::FullWidth);
    table.setUsesAlternatingRowBackgroundColors(true);
    table.setAllowsMultipleSelection(false);
    table.setAllowsEmptySelection(true);
    table.setAllowsColumnReordering(false);
    table.setFloatsGroupRows(true);
    table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::UniformColumnAutoresizingStyle);
    for (id, title, width, min, max) in COLUMNS {
        let col = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(id));
        col.setTitle(&ns(title));
        col.setWidth(width);
        col.setMinWidth(min);
        col.setMaxWidth(max);
        if id == "mark" {
            col.setResizingMask(NSTableColumnResizingOptions::empty());
            col.setHeaderToolTip(Some(&ns("Manuell korrigiert")));
        } else if id != "date" {
            col.headerCell().setAlignment(NSTextAlignment::Right);
        }
        table.addTableColumn(&col);
    }
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&**controller)));
        table.setDelegate(Some(ProtocolObject::from_ref(&**controller)));
    }
    // Neuer Name seit der Spalte „Saldo“: alte gespeicherte Spaltenbreiten passen nicht mehr.
    table.setAutosaveName(Some(&ns("TagwerkRecordsTable2")));
    table.setAutosaveTableColumns(true);
    let table_scroll = NSScrollView::new(mtm);
    table_scroll.setDocumentView(Some(&table));
    table_scroll.setHasVerticalScroller(true);
    table_scroll.setAutohidesScrollers(true);
    table_scroll.setBorderType(NSBorderType::NoBorder);

    // Inspektor
    let inspector = vstack(6.0);
    inspector.setEdgeInsets(NSEdgeInsets { top: 18.0, left: 20.0, bottom: 20.0, right: 20.0 });
    inspector.setTranslatesAutoresizingMaskIntoConstraints(false);
    let inspector_scroll = NSScrollView::new(mtm);
    inspector_scroll.setHasVerticalScroller(true);
    inspector_scroll.setAutohidesScrollers(true);
    inspector_scroll.setDrawsBackground(false);
    inspector_scroll.setBorderType(NSBorderType::NoBorder);
    let doc = FlippedView::new(mtm);
    doc.setTranslatesAutoresizingMaskIntoConstraints(false);
    doc.addSubview(&inspector);
    inspector_scroll.setDocumentView(Some(&doc));
    let clip = inspector_scroll.contentView();
    let c = |a: Retained<NSLayoutConstraint>| a.setActive(true);
    c(doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()));
    c(doc.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()));
    c(doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()));
    c(inspector.leadingAnchor().constraintEqualToAnchor(&doc.leadingAnchor()));
    c(inspector.trailingAnchor().constraintEqualToAnchor(&doc.trailingAnchor()));
    c(inspector.topAnchor().constraintEqualToAnchor(&doc.topAnchor()));
    c(inspector.bottomAnchor().constraintEqualToAnchor(&doc.bottomAnchor()));
    let inspector_pane: Retained<NSView> = Retained::into_super(inspector_scroll);
    inspector_pane.setTranslatesAutoresizingMaskIntoConstraints(false);
    inspector_pane.widthAnchor().constraintGreaterThanOrEqualToConstant(300.0).setActive(true);
    let w = inspector_pane.widthAnchor().constraintLessThanOrEqualToConstant(440.0);
    w.setActive(true);

    let split = NSSplitView::new(mtm);
    split.setVertical(true);
    split.setDividerStyle(NSSplitViewDividerStyle::Thin);
    split.addArrangedSubview(&table_scroll);
    split.addArrangedSubview(&inspector_pane);
    split.setHoldingPriority_forSubviewAtIndex(250.0, 0);
    split.setHoldingPriority_forSubviewAtIndex(270.0, 1);
    split.setAutosaveName(Some(&ns("TagwerkRecordsSplit")));

    let top_sep = separator();
    let footer = label("");
    footer.setFont(Some(&NSFont::systemFontOfSize(small_size())));
    footer.setTextColor(Some(&NSColor::secondaryLabelColor()));

    for v in [&*cards_row as &NSView, &top_sep, &split, &footer] {
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(v);
    }
    let footer_height = 26.0;
    let c = |a: Retained<NSLayoutConstraint>| a.setActive(true);
    c(cards_row.topAnchor().constraintEqualToAnchor_constant(&root.topAnchor(), 16.0));
    c(cards_row.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0));
    c(cards_row.trailingAnchor().constraintEqualToAnchor_constant(&root.trailingAnchor(), -20.0));
    c(top_sep.topAnchor().constraintEqualToAnchor_constant(&cards_row.bottomAnchor(), 16.0));
    c(top_sep.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()));
    c(top_sep.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()));
    c(split.topAnchor().constraintEqualToAnchor(&top_sep.bottomAnchor()));
    c(split.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()));
    c(split.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()));
    c(split.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -footer_height));
    c(footer.leadingAnchor().constraintEqualToAnchor_constant(&root.leadingAnchor(), 20.0));
    c(footer.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&root.trailingAnchor(), -20.0));
    c(footer.centerYAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -footer_height / 2.0));
    window.setAutorecalculatesContentBorderThickness_forEdge(false, NSRectEdge::MinY);
    window.setContentBorderThickness_forEdge(footer_height, NSRectEdge::MinY);

    if shot || !window.setFrameUsingName(&ns("TagwerkRecords")) {
        window.center();
    }
    window.setFrameAutosaveName(&ns("TagwerkRecords"));
    root.layoutSubtreeIfNeeded();
    if split.autosaveName().is_none_or(|_| shot) {
        let width = split.frame().size.width;
        split.setPosition_ofDividerAtIndex(width - 340.0, 0);
    }

    Ui {
        window,
        table,
        split,
        inspector_pane,
        inspector,
        cards,
        footer,
        controller: controller.clone(),
    }
}

fn activate() {
    if SCREENSHOT.with(|s| s.borrow().is_some()) {
        return;
    }
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
}

// ---------- Zeitgeber ----------

fn tick() {
    let n = TICKS.with(|t| {
        t.set(t.get() + 1);
        t.get()
    });

    if let Some(path) = SCREENSHOT.with(|s| s.borrow().clone()) {
        if n == 2 {
            if let Err(e) = save_screenshot(&path) {
                eprintln!("Screenshot: {e}");
            }
            NSApplication::sharedApplication(mtm()).terminate(None);
        }
        return;
    }

    // Änderungen von außen (Menüleisten-App) übernehmen, sonst regelmäßig die
    // laufenden Zahlen von heute auffrischen.
    if n.is_multiple_of(3) {
        let changed = model(|m| {
            let db_changed = m.db.as_mut().is_some_and(|db| db.changed_externally());
            db_changed || store::read_status() != m.status
        });
        if changed || n.is_multiple_of(30) {
            reload();
        }
    }
}

fn save_screenshot(path: &std::path::Path) -> Result<(), String> {
    let ui = ui();
    let content = ui.window.contentView().ok_or("kein Inhalt")?;
    let view = unsafe { content.superview() }.unwrap_or(content);
    view.layoutSubtreeIfNeeded();
    view.displayIfNeeded();
    let bounds = view.bounds();
    let rep = view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or("kein Bildpuffer")?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props = NSDictionary::new();
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("PNG konnte nicht erzeugt werden")?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, data.to_vec()).map_err(|e| e.to_string())
}

// ---------- Start ----------

/// Öffnet das Fenster „Aufzeichnungen“. Mit `screenshot` wird ein Bild des Fensters
/// gespeichert und das Programm danach beendet.
pub fn run(screenshot: Option<PathBuf>) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Das Fenster muss im Hauptthread laufen.")?;
    let shot = screenshot.is_some();
    SCREENSHOT.with(|s| *s.borrow_mut() = screenshot);

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // Zum Testen/für Screenshots: TAGWERK_APPEARANCE=dark|light
    if let Ok(a) = std::env::var("TAGWERK_APPEARANCE") {
        let name = if a == "dark" { unsafe { NSAppearanceNameDarkAqua } } else { unsafe { NSAppearanceNameAqua } };
        app.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }

    let controller = Controller::new(mtm);
    build_main_menu(&app, &controller);

    let (db, error) = match store::Db::open() {
        Ok(db) => (Some(db), None),
        Err(e) => (None, Some(format!("Die Datenbank konnte nicht geöffnet werden: {e}"))),
    };
    model(|m| {
        m.db = db;
        m.error = error;
        m.load();
    });

    let ui = build_window(&controller, shot);
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    reload_table();
    refresh_summary();

    // Zum Testen/für Screenshots: TAGWERK_SELECT=latest oder =TT.MM.JJJJ
    let select = std::env::var("TAGWERK_SELECT").ok().and_then(|sel| {
        if sel == "latest" {
            model(|m| m.days.last().map(|d| d.date))
        } else {
            store::parse_date(&sel)
        }
    });
    match select {
        Some(d) => go_to(d),
        None => refresh_inspector(true),
    }

    if shot {
        ui.window.orderFront(None);
    } else {
        ui.window.makeKeyAndOrderFront(None);
        ui.window.makeFirstResponder(Some(&ui.table));
        activate();
    }

    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            if shot { 0.8 } else { 1.0 },
            &controller,
            sel!(tick:),
            None,
            true,
        )
    };
    let _ = timer;
    app.run();
    Ok(())
}

