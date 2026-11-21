//! Fenster „Aufzeichnungen“ – natives AppKit-Fenster nach den macOS Human Interface
//! Guidelines: Symbolleiste, Übersicht, Tabelle mit Wochensummen, Inspektor zum
//! Korrigieren eines Tages (Arbeitsbeginn, Pausen, Arbeit ohne Mac, Feierabend) und Export als Markdown
//! über den Sichern-Dialog.
//!
//! Der Umschalter „Tage | Woche“ in der Symbolleiste (⌘1, ⌘2) wechselt zwischen der Tabelle
//! mit Inspektor und der Wochenansicht als Kalender ([`week_view`]). Das Fenster merkt sich
//! die zuletzt benutzte Ansicht.
//!
//! Die Menüleisten-App öffnet das Fenster in ihrem eigenen Prozess ([`show`]). Solange es
//! offen ist, hat Tagwerk ein Symbol im Dock und ein Hauptmenü. Das Fenster liest und
//! schreibt über eine eigene Datenbankverbindung und meldet Änderungen sofort an die
//! Menüleisten-App; den Zustand der Erfassung bekommt es von ihr ([`set_status`]), ebenso
//! den Stand der Wochenansicht samt laufender Periode ([`Embed`]).
//!
//! `Tagwerk --records` öffnet das Fenster ohne Menüleisten-App ([`run`]).

use crate::about;
use crate::palette;
use crate::day::{
    fmt_clock, secs_of, Absence, AbsenceKind, Activity, Break, Day, DayRecord, FreeDay, Span, WorkBlock, DAY_END,
};
use crate::rules::{self, Settings, Warning};
use crate::settings_window;
use crate::snapshot;
use crate::store::{self, Status};
use crate::week_view;
use block2::RcBlock;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSCopying, NSDate, NSDictionary, NSEdgeInsets, NSRectEdge, NSIndexSet, NSInteger, NSLocale, NSNotification, NSPoint,
    NSRect, NSSize, NSString, NSTimer, NSURL, NSUserDefaults,
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

/// Fehlertext, wenn Arbeit ohne Mac kürzer als die Mindestdauer `min_work` ist.
pub(crate) fn min_work_text(min_work: i64) -> String {
    format!("Arbeit ohne Mac dauert mindestens {} Min. (siehe Einstellungen).", min_work / 60)
}

pub(crate) use crate::day::shown_len;

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
    let p = time_field(date, secs);
    unsafe {
        p.setTarget(Some(target));
        p.setAction(Some(action));
    }
    p
}

/// Uhrzeit-Feld (Stunde:Minute) mit Pfeiltasten für den Tag `date`, ohne Aktion; heute nicht
/// in der Zukunft (auch für die Wochenansicht).
pub(crate) fn time_field(date: NaiveDate, secs: i64) -> Retained<NSDatePicker> {
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
    p
}

/// Sekunden seit Mitternacht aus einem Uhrzeit-Feld, auf die angezeigte Minute abgerundet
/// (ein Vorschlag mit Sekunden, z. B. das Ende einer Pause, behält sonst seine Sekunden).
pub(crate) fn picker_secs(p: &NSDatePicker) -> Option<i64> {
    from_nsdate(&p.dateValue()).map(|t| secs_of(t.time()) / 60 * 60)
}

/// Hinweis als Sheet am Fenster `window` (nur „OK“; auch für die Wochenansicht).
pub(crate) fn show_info_sheet(window: &NSWindow, title: &str, text: &str) {
    if window.attachedSheet().is_some() {
        return;
    }
    let alert = NSAlert::new(mtm());
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(text));
    alert.addButtonWithTitle(&ns("OK"));
    alert.beginSheetModalForWindow_completionHandler(window, None);
}

/// Auswahlbox für den Grund von Arbeit ohne Mac (auch für die Wochenansicht).
pub(crate) fn activity_popup(selected: Activity) -> Retained<NSPopUpButton> {
    let p = NSPopUpButton::new(mtm());
    for a in Activity::ALL {
        p.addItemWithTitle(&ns(a.label()));
        if let Some(item) = p.lastItem() {
            item.setTag(a.id());
        }
    }
    p.selectItemWithTag(selected.id());
    p
}

/// Gewählter Grund in einer [`activity_popup`].
pub(crate) fn popup_activity(p: &NSPopUpButton) -> Activity {
    Activity::from_id(p.selectedTag())
}

fn pin_width(view: &NSView, to: &NSView, inset: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor().constraintEqualToAnchor_constant(&to.widthAnchor(), -inset).setActive(true);
}

// ---------- Daten ----------

/// Eine Tabellenzeile: Wochenüberschrift oder Arbeitstag.
enum Row {
    /// Wochenzeile: Montag, Titel, Summe (endet vor dem Saldo) und Saldo der Woche.
    Week { monday: NaiveDate, title: String, total: String, balance: i64 },
    Day { day: Day, rec: DayRecord, warnings: Vec<Warning> },
    /// Vergangener Arbeitstag ganz ohne Eintrag: zählt mit seinem Soll (siehe
    /// [`rules::missing_workdays`]), bis Zeiten oder eine Abwesenheit nachgetragen sind.
    Missing { date: NaiveDate, target: i64 },
}

/// Steuerelemente des Inspektors, die Aktionen auslesen müssen.
struct Inspector {
    date: NaiveDate,
    stats: StatsView,
    start_picker: Retained<NSDatePicker>,
    end_picker: Retained<NSDatePicker>,
    break_from: Retained<NSDatePicker>,
    break_to: Retained<NSDatePicker>,
    work_from: Retained<NSDatePicker>,
    work_to: Retained<NSDatePicker>,
    work_reason: Retained<NSPopUpButton>,
    /// Liste der Arbeit ohne Mac (wird auch ohne Neuaufbau des Inspektors aufgefrischt).
    work_list: Retained<NSStackView>,
    /// Hinweise, dass Arbeitsbeginn bzw. Feierabend Arbeit ohne Mac gekürzt haben.
    start_cut: Retained<NSTextField>,
    end_cut: Retained<NSTextField>,
    /// Abwesenheit: Art (erster Eintrag = keine) und ganzer bzw. halber Tag.
    absence_kind: Retained<NSPopUpButton>,
    absence_part: Retained<NSPopUpButton>,
    /// Hinweise zu einer abgelehnten Eingabe, jeweils direkt über den Feldern.
    absence_error: Retained<NSTextField>,
    start_error: Retained<NSTextField>,
    break_error: Retained<NSTextField>,
    work_error: Retained<NSTextField>,
    end_error: Retained<NSTextField>,
}

impl Inspector {
    /// Hinweisfeld zur Eingabe `input`.
    fn error_label(&self, input: Input) -> &Retained<NSTextField> {
        match input {
            Input::Absence => &self.absence_error,
            Input::Start => &self.start_error,
            Input::Break => &self.break_error,
            Input::Work => &self.work_error,
            Input::End => &self.end_error,
        }
    }
}

/// Kennzahlen des Tages im Kopf des Inspektors: drei Werte nebeneinander (Arbeitszeit,
/// Pausen, Saldo) – oder, wenn es keine gibt, ein Satz.
#[derive(Clone)]
struct StatsView {
    row: Retained<NSStackView>,
    values: [Retained<NSTextField>; 3],
    note: Retained<NSTextField>,
}

/// Was im Kopf des Inspektors steht.
#[derive(Clone, Debug, PartialEq)]
enum DayStats {
    /// Arbeitszeit, Pausen und Saldo (Sekunden; `None`: Die Arbeitszeit zählt nicht im Saldo).
    Numbers { work: i64, pause: i64, balance: Option<i64> },
    Note(&'static str),
}

impl StatsView {
    fn new() -> Self {
        let row = hstack(22.0);
        row.setAlignment(NSLayoutAttribute::Top);
        let values = ["Arbeitszeit", "Pausen", "Saldo"].map(|title| {
            let value = label("");
            value.setFont(Some(&font_digits(15.0, unsafe { NSFontWeightSemibold })));
            let caption = label(title);
            caption.setFont(Some(&NSFont::systemFontOfSize(small_size())));
            caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
            let column = vstack(1.0);
            column.addArrangedSubview(&value);
            column.addArrangedSubview(&caption);
            row.addArrangedSubview(&column);
            value
        });
        Self { row, values, note: caption("") }
    }

    fn show(&self, stats: &DayStats) {
        match stats {
            DayStats::Numbers { work, pause, balance } => {
                self.values[0].setStringValue(&ns(&store::fmt_hm(*work)));
                self.values[1].setStringValue(&ns(&store::fmt_hm(*pause)));
                match balance {
                    Some(b) => {
                        self.values[2].setStringValue(&ns(&rules::fmt_balance(*b)));
                        self.values[2].setTextColor(Some(&palette::balance(*b)));
                    }
                    None => {
                        self.values[2].setStringValue(&ns("–"));
                        self.values[2].setTextColor(Some(&NSColor::secondaryLabelColor()));
                    }
                }
            }
            DayStats::Note(text) => self.note.setStringValue(&ns(text)),
        }
        let numbers = matches!(stats, DayStats::Numbers { .. });
        self.row.setHidden(!numbers);
        self.note.setHidden(numbers);
    }
}

/// Eingabe im Inspektor, zu der ein Hinweis gehört.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Absence,
    Start,
    Break,
    Work,
    End,
}

/// Steuerelemente der Zusatzansicht im Sichern-Dialog.
struct Export {
    panel: Retained<NSSavePanel>,
    preset: Retained<NSPopUpButton>,
    from: Retained<NSDatePicker>,
    to: Retained<NSDatePicker>,
    summary: Retained<NSTextField>,
    /// Format (Stelle in [`store::ExportFormat::ALL`]).
    format: Retained<NSPopUpButton>,
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
    /// Zuletzt gemachte Korrekturen zum Widerrufen und Wiederholen.
    history: History,
    /// Welche Tage die Tabelle zeigt.
    filter: Filter,
    /// Im Inspektor ohne Abwesenheit vorgewählt: ganzer (`false`) oder halber Tag (`true`) für
    /// die Art, die als Nächstes gewählt wird – je Tag, damit es ein Neuaufbau nicht vergisst.
    absence_half: Option<(NaiveDate, bool)>,
}

/// Zeitraum, auf den die Tabelle eingeschränkt ist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Period {
    #[default]
    All,
    ThisMonth,
    LastMonth,
    ThisYear,
    LastYear,
}

impl Period {
    const ALL: [Period; 5] = [Period::All, Period::ThisMonth, Period::LastMonth, Period::ThisYear, Period::LastYear];

    fn label(self) -> &'static str {
        match self {
            Period::All => "Alle Tage",
            Period::ThisMonth => "Dieser Monat",
            Period::LastMonth => "Letzter Monat",
            Period::ThisYear => "Dieses Jahr",
            Period::LastYear => "Letztes Jahr",
        }
    }

    /// Liegt `date` im Zeitraum (von `today` aus gesehen)?
    fn contains(self, date: NaiveDate, today: NaiveDate) -> bool {
        let month = |d: NaiveDate| (d.year(), d.month());
        match self {
            Period::All => true,
            Period::ThisMonth => month(date) == month(today),
            Period::LastMonth => month(date) == month(month_start(today) - Duration::days(1)),
            Period::ThisYear => date.year() == today.year(),
            Period::LastYear => date.year() == today.year() - 1,
        }
    }
}

/// Filter der Tabelle: Zeitraum und Merkmale der Tage. Er gilt nur für die Zeilen – die
/// Kennzahlen darüber und die Wochensummen zählen immer alles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Filter {
    period: Period,
    /// Nur Tage mit Hinweisen auf Regelverstöße.
    warnings: bool,
    /// Nur Tage mit manuellen Korrekturen.
    corrected: bool,
}

impl Filter {
    fn is_active(&self) -> bool {
        *self != Self::default()
    }

    fn shows(&self, day: &Day, warnings: &[Warning], today: NaiveDate) -> bool {
        self.period.contains(day.date, today)
            && (!self.warnings || !warnings.is_empty())
            && (!self.corrected || day.has_corrections())
    }
}

/// Die manuellen Korrekturen eines Tages – alles, was sich im Fenster ändern lässt. Die
/// automatisch erfassten Perioden gehören nur nach „Alle Einträge löschen“ dazu
/// ([`Corrections::whole`]): Sonst stellt Widerrufen nur Korrekturen wieder her und lässt
/// inzwischen erfasste Zeit stehen.
#[derive(Clone, Debug, Default, PartialEq)]
struct Corrections {
    start: Option<i64>,
    end: Option<i64>,
    breaks: Vec<Break>,
    work: Vec<WorkBlock>,
    absence: Option<Absence>,
    /// Freier Tag: Was gilt für die automatische Erfassung (siehe [`Day::free_day`])?
    free_day: Option<FreeDay>,
    /// Automatisch erfasste Zeiten (`None`: bleiben unberührt).
    periods: Option<Vec<Span>>,
}

impl Corrections {
    fn of(day: &Day) -> Self {
        Self {
            start: day.start,
            end: day.end,
            breaks: day.breaks.clone(),
            work: day.work.clone(),
            absence: day.absence,
            free_day: day.free_day,
            periods: None,
        }
    }

    /// Der ganze Tag samt automatisch erfasster Zeiten.
    fn whole(day: &Day) -> Self {
        Self { periods: Some(day.periods.clone()), ..Self::of(day) }
    }

    /// Den Schritt von `done` (dem Stand, den er hinterlassen hat) nach `target` am Tag `day`
    /// nachvollziehen – zum Widerrufen ist `target` der Stand davor, zum Wiederholen der danach.
    /// Ein Drei-Wege-Abgleich je Angabe: Zurückgesetzt wird nur, was der Schritt geändert hat
    /// und was noch so ist, wie er es hinterlassen hat. Was inzwischen anders geändert wurde
    /// (Menü: Feierabend, „Ohne Mac weiterarbeiten“, „Heute erfassen“ …), bleibt. Automatisch
    /// erfasste Zeit (nur nach „Alle Einträge löschen“ bzw. einer ganztägigen Abwesenheit für
    /// heute Teil des Schritts) kommt zurück bzw. entfällt, ohne dass verloren geht, was
    /// seitdem erfasst wurde. Ergäbe das keinen zulässigen Tag, bleibt er, wie er ist.
    fn rewind(day: &mut Day, done: &Corrections, target: &Corrections) -> Rewind {
        let mut kept = Vec::new();
        let mut result = day.clone();
        fn pick<T: Clone + PartialEq>(
            cur: &T,
            done: &T,
            target: &T,
            what: &'static str,
            kept: &mut Vec<&'static str>,
        ) -> T {
            if done != target && cur == done {
                target.clone()
            } else {
                // Unberührt, schon so wie gewünscht oder inzwischen anders geändert.
                if done != target && cur != target {
                    kept.push(what);
                }
                cur.clone()
            }
        }
        result.start = pick(&day.start, &done.start, &target.start, "Arbeitsbeginn", &mut kept);
        result.breaks = pick(&day.breaks, &done.breaks, &target.breaks, "Pausen", &mut kept);
        result.work = pick(&day.work, &done.work, &target.work, "Arbeit ohne Mac", &mut kept);
        result.end = pick(&day.end, &done.end, &target.end, "Feierabend", &mut kept);
        result.absence = pick(&day.absence, &done.absence, &target.absence, "Abwesenheit", &mut kept);
        result.free_day = pick(&day.free_day, &done.free_day, &target.free_day, "Erfasste Zeit zählen", &mut kept);
        if let (Some(done), Some(target)) = (&done.periods, &target.periods) {
            // Was seit dem Schritt dazukam, bleibt in jedem Fall.
            let since = minus(&day.periods, done);
            result.periods = crate::day::union(target.iter().copied().chain(since).collect());
        }
        if result == *day {
            return if kept.is_empty() { Rewind::Done } else { Rewind::Refused(RewindBlock::Changed(kept)) };
        }
        // Ein Tag, der vorher schon so war (ältere Daten), bleibt widerrufbar.
        if let Some(why) = inconsistency(&result).filter(|_| inconsistency(day).is_none()) {
            return Rewind::Refused(why);
        }
        *day = result;
        if kept.is_empty() { Rewind::Done } else { Rewind::Partial(kept) }
    }
}

/// Zeiten in `spans`, die `cut` nicht abdeckt.
fn minus(spans: &[Span], cut: &[Span]) -> Vec<Span> {
    let mut out = spans.to_vec();
    for c in cut {
        out = out
            .into_iter()
            .flat_map(|s| {
                [Span::new(s.from, s.to.min(c.from)), Span::new(s.from.max(c.to), s.to)]
                    .into_iter()
                    .filter(|p| p.to > p.from)
                    .collect::<Vec<_>>()
            })
            .collect();
    }
    out
}

/// Warum der Tag `d` so nicht sein darf (`None`: Er ist zulässig): Arbeitsbeginn nicht vor dem
/// Feierabend oder eine ganztägige Abwesenheit neben anderen Einträgen.
fn inconsistency(d: &Day) -> Option<RewindBlock> {
    if !crate::day::bounds_in_order(d.start, d.end) {
        Some(RewindBlock::Order)
    } else if d.locks_entries() && d.has_entries() {
        Some(RewindBlock::AbsenceBesideEntries)
    } else {
        None
    }
}

/// Ergebnis von [`Corrections::rewind`].
#[derive(Clone, Debug, PartialEq)]
enum Rewind {
    /// Ganz zurückgenommen bzw. wiederholt.
    Done,
    /// Nur zum Teil: Diese Angaben wurden inzwischen anders geändert und bleiben, wie sie sind.
    Partial(Vec<&'static str>),
    /// Gar nicht – der Tag bleibt, wie er ist.
    Refused(RewindBlock),
}

/// Warum sich ein Schritt nicht widerrufen bzw. wiederholen lässt.
#[derive(Clone, Debug, PartialEq)]
enum RewindBlock {
    /// Alles, was er geändert hat, wurde inzwischen anders geändert.
    Changed(Vec<&'static str>),
    /// Der Arbeitsbeginn läge nicht vor dem Feierabend.
    Order,
    /// Eine ganztägige Abwesenheit stünde neben anderen Einträgen.
    AbsenceBesideEntries,
}

/// `a, b und c`
fn join_and(parts: &[&str]) -> String {
    match parts.split_last() {
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{} und {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Hinweis nach Widerrufen bzw. Wiederholen (`redo`) des Schritts `name` am Tag `date`, wenn
/// es nicht ganz gelungen ist: Titel und Text (`None`: Es ist ganz gelungen).
fn rewind_note(redo: bool, name: &str, date: NaiveDate, rewind: &Rewind) -> Option<(String, String)> {
    let (verb, done) = if redo { ("wiederholen", "wiederholt") } else { ("widerrufen", "widerrufen") };
    let step = format!("„{name}“ vom {}", store::fmt_date_short(date));
    match rewind {
        Rewind::Done => None,
        Rewind::Partial(kept) => Some((
            format!("Nur zum Teil {done}"),
            format!(
                "{step} ist nur zum Teil {done}. Inzwischen anders geändert und deshalb so geblieben: {}.",
                join_and(kept)
            ),
        )),
        Rewind::Refused(why) => {
            let why = match why {
                RewindBlock::Changed(kept) => format!("inzwischen anders geändert: {}", join_and(kept)),
                RewindBlock::Order => "der Arbeitsbeginn läge dann nicht vor dem Feierabend".to_string(),
                RewindBlock::AbsenceBesideEntries => {
                    "die ganztägige Abwesenheit stünde dann neben inzwischen erfassten Einträgen".to_string()
                }
            };
            Some((
                format!("Nicht {done}"),
                format!("{step} lässt sich nicht mehr {verb} – {why}. Der Tag bleibt, wie er ist."),
            ))
        }
    }
}

/// Ein Schritt zum Widerrufen bzw. Wiederholen: die Korrekturen des Tages davor und danach.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    /// Was geändert wurde, für das Menü („Widerrufen: Pause entfernen“).
    name: &'static str,
    date: NaiveDate,
    before: Corrections,
    after: Corrections,
}

/// Verlauf der Korrekturen (jeweils der jüngste Schritt zuletzt).
#[derive(Default)]
struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// So viele Schritte lassen sich höchstens widerrufen.
    const MAX: usize = 50;

    /// Eine neue Änderung merken. `follow_up`: Die Eingabe läuft weiter (Pfeiltasten im
    /// Uhrzeit-Feld) – gehört sie zur selben Angabe wie der letzte Schritt, bleibt es bei
    /// diesem, damit einmal Widerrufen die ganze Eingabe zurücknimmt.
    fn record(&mut self, step: Step, follow_up: bool) {
        self.redo.clear();
        if let Some(last) = self.undo.last_mut().filter(|last| last.date == step.date && last.name == step.name) {
            if follow_up {
                // Der Stand vor der ganzen Eingabe bleibt, der danach ist der neue.
                last.after = step.after;
                return;
            }
        }
        self.undo.push(step);
        if self.undo.len() > Self::MAX {
            self.undo.remove(0);
        }
    }
}

impl Model {
    fn day(&self, date: NaiveDate) -> Option<&Day> {
        self.days.iter().find(|d| d.date == date)
    }

    fn records(&self) -> Vec<DayRecord> {
        self.days.iter().filter_map(|d| d.record(&self.settings)).collect()
    }

    fn row_of(&self, date: NaiveDate) -> Option<usize> {
        self.rows.iter().position(|r| match r {
            Row::Day { day, .. } => day.date == date,
            Row::Missing { date: d, .. } => *d == date,
            Row::Week { .. } => false,
        })
    }

    /// Tag der Zeile `row` für die Wochenansicht: bei einer Wochenzeile ihr Montag.
    fn week_date_at(&self, row: isize) -> Option<NaiveDate> {
        match self.rows.get(usize::try_from(row).ok()?)? {
            Row::Week { monday, .. } => Some(*monday),
            _ => self.date_at(row),
        }
    }

    fn date_at(&self, row: isize) -> Option<NaiveDate> {
        match self.rows.get(usize::try_from(row).ok()?)? {
            Row::Day { day, .. } => Some(day.date),
            Row::Missing { date, .. } => Some(*date),
            Row::Week { .. } => None,
        }
    }

    /// Liest die Datenbank neu und baut die Tabellenzeilen auf.
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
        self.build_rows();
    }

    fn build_rows(&mut self) {
        let s = &self.settings;
        let today = now().date();
        // Erfasste Tage und Arbeitstage ohne Eintrag, der jüngste zuerst.
        let mut recs: Vec<(NaiveDate, Option<(Day, DayRecord)>)> = self
            .days
            .iter()
            .filter_map(|d| d.record(s).map(|r| (d.date, Some((d.clone(), r)))))
            .chain(rules::missing_workdays(&self.days, NaiveDate::MIN, today, today, s).into_iter().map(|d| (d, None)))
            .collect();
        recs.sort_by_key(|r| std::cmp::Reverse(r.0));
        let mut rows = Vec::new();
        let mut i = 0;
        while i < recs.len() {
            let week = recs[i].0.iso_week();
            let j = recs[i..].iter().position(|(d, _)| d.iso_week() != week).map_or(recs.len(), |n| i + n);
            let monday = recs[i].0 - Duration::days(recs[i].0.weekday().num_days_from_monday() as i64);
            let sunday = monday + Duration::days(6);
            let total: i64 = recs[i..j].iter().filter_map(|(_, x)| x.as_ref()).map(|(_, r)| r.shown_work_secs).sum();
            // Der Saldo ohne Arbeit an freien Tagen, die nicht zählt.
            let (counted, target) = rules::balance(&self.days, monday, sunday, today, s);
            // Arbeitstage wie überall: Tage mit Arbeitszeit (nicht reine Abwesenheiten, nicht die
            // ohne Eintrag).
            let count = recs[i..j].iter().filter(|(_, x)| x.as_ref().is_some_and(|(_, r)| r.work_secs > 0)).count();
            // Die Wochenzeile zählt die ganze Woche, auch wenn der Filter Tage ausblendet;
            // Wochen ohne sichtbaren Tag entfallen.
            let week_row = Row::Week {
                monday,
                title: format!(
                    "KW {}  ·  {} – {}",
                    week.week(),
                    monday.format("%d.%m."),
                    sunday.format("%d.%m.%Y")
                ),
                total: format!("{}  ·  {}  ·  Saldo ", plural(count, "Arbeitstag", "Arbeitstage"), store::fmt_hm(total)),
                balance: counted - target,
            };
            let days: Vec<Row> = recs[i..j]
                .iter()
                .cloned()
                .filter_map(|(date, x)| match x {
                    Some((day, rec)) => {
                        let warnings = rules::day_warnings(&self.days, day.date, s);
                        self.filter.shows(&day, &warnings, today).then_some(Row::Day { day, rec, warnings })
                    }
                    // Ohne Eintrag gibt es weder Hinweise noch Korrekturen.
                    None => (self.filter.period.contains(date, today) && !self.filter.warnings && !self.filter.corrected)
                        .then(|| Row::Missing { date, target: s.daily_target(date) }),
                })
                .collect();
            if !days.is_empty() {
                rows.push(week_row);
                rows.extend(days);
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
    /// Übersichtskarten mit Trennlinie (nur in der Ansicht „Tage“).
    cards_row: Retained<NSStackView>,
    top_sep: Retained<NSBox>,
    /// Ansicht „Woche“.
    week: Retained<NSView>,
    switcher: Retained<NSSegmentedControl>,
}

/// Ansicht im Fenster: Tabelle mit Inspektor oder Woche als Kalender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Days,
    Week,
}

/// Verbindung zur Menüleisten-App.
#[derive(Clone, Copy)]
pub struct Embed {
    /// Nach jeder Änderung an der Datenbank.
    pub on_change: fn(),
    /// Stand für die Wochenansicht (inklusive laufender Periode).
    pub week: fn() -> Option<week_view::Snapshot>,
    /// Arbeit ohne Mac aus der Wochenansicht eintragen bzw. löschen.
    pub add_work: week_view::AddWork,
    pub remove_work: week_view::RemoveWork,
}

/// Schlüssel in den Benutzereinstellungen für die zuletzt benutzte Ansicht.
const MODE_KEY: &str = "TagwerkRecordsView";

thread_local! {
    static MODEL: RefCell<Model> = RefCell::new(Model::default());
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static TICKS: Cell<u64> = const { Cell::new(0) };
    static SCREENSHOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    /// Verbindung zur Menüleisten-App (`None`: eigenständig über `--records` gestartet).
    static EMBED: Cell<Option<Embed>> = const { Cell::new(None) };
    static MODE: Cell<Mode> = const { Cell::new(Mode::Days) };
    /// Fenster ist offen (auch im Dock abgelegt).
    static OPEN: Cell<bool> = const { Cell::new(false) };
    static TIMER: RefCell<Option<Retained<NSTimer>>> = const { RefCell::new(None) };
    /// Umschalter „Tage | Woche“, bevor die Symbolleiste ihn abholt.
    static SWITCHER: RefCell<Option<Retained<NSSegmentedControl>>> = const { RefCell::new(None) };
    /// Zuletzt gezeigte Erklärung aus dem Inspektor.
    static HELP: RefCell<Option<Retained<NSPopover>>> = const { RefCell::new(None) };
    /// Filter in der Symbolleiste (sein Symbol zeigt, ob gefiltert wird).
    static FILTER_ITEM: RefCell<Option<Retained<NSMenuToolbarItem>>> = const { RefCell::new(None) };
}

fn embedded() -> bool {
    EMBED.with(|c| c.get().is_some())
}

/// Meldet der Menüleisten-App, dass sich die Datenbank geändert hat.
fn notify_change() {
    if let Some(e) = EMBED.with(|c| c.get()) {
        (e.on_change)();
    }
}

fn mode() -> Mode {
    MODE.with(|m| m.get())
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
    // Platz für zwei Symbole nebeneinander: Hinweis und Stift.
    ("mark", "", MARK_WIDTH, MARK_WIDTH, MARK_WIDTH),
    ("date", "Datum", 140.0, 110.0, 10_000.0),
    ("start", "Beginn", 64.0, 56.0, 120.0),
    ("end", "Ende", 64.0, 56.0, 120.0),
    ("work", "Arbeitszeit", 90.0, 76.0, 140.0),
    ("pause", "Pausen", 70.0, 60.0, 130.0),
    ("balance", "Saldo", 76.0, 66.0, 130.0),
];

/// Breite der Spalte mit den Symbolen und Platz je Symbol.
const MARK_WIDTH: f64 = 42.0;
const MARK_SLOT: f64 = 18.0;

const TB_RELOAD: &str = "reload";
const TB_ADD: &str = "add";
const TB_EXPORT: &str = "export";
const TB_INSPECTOR: &str = "inspector";
const TB_VIEW: &str = "view";
const TB_FILTER: &str = "filter";

/// Tag der Auswahl „Ganzer Tag | Halber Tag“ im Inspektor (die Art hat 0).
const ABSENCE_PART_TAG: isize = 1;

/// Menü-Tags der Merkmale im Filter.
const FILTER_WARNINGS: isize = 0;
const FILTER_CORRECTED: isize = 1;

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
            MODEL.with(|m| matches!(m.borrow().rows.get(row as usize), Some(Row::Day { .. } | Row::Missing { .. })))
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

    unsafe impl NSMenuItemValidation for Controller {
        /// „Tage“/„Woche“ mit Häkchen; Inspektor nur in „Tage“, „Heute“ nur in „Woche“.
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            validate_menu_item(item)
        }
    }

    unsafe impl NSToolbarItemValidation for Controller {
        #[unsafe(method(validateToolbarItem:))]
        fn validate_toolbar_item(&self, item: &NSToolbarItem) -> bool {
            item.action() != Some(sel!(toggleInspector:)) || mode() == Mode::Days
        }
    }

    unsafe impl NSWindowDelegate for Controller {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _n: &NSNotification) {
            closed();
        }
    }

    impl Controller {
        /// ⌘Q schließt nur das Fenster, die Erfassung läuft weiter.
        #[unsafe(method(closeRecords:))]
        fn close_records(&self, _sender: Option<&AnyObject>) {
            ui().window.performClose(None);
        }

        /// „Widerrufen“: im Fenster „Aufzeichnungen“ die letzte Korrektur, sonst (Dialog,
        /// Einstellungen) wie gewohnt die Eingabe im Textfeld.
        #[unsafe(method(undoEdit:))]
        fn undo_edit(&self, sender: Option<&AnyObject>) {
            if records_is_key() {
                step_history(false);
            } else {
                unsafe { NSApplication::sharedApplication(mtm()).sendAction_to_from(sel!(undo:), None, sender) };
            }
        }

        #[unsafe(method(redoEdit:))]
        fn redo_edit(&self, sender: Option<&AnyObject>) {
            if records_is_key() {
                step_history(true);
            } else {
                unsafe { NSApplication::sharedApplication(mtm()).sendAction_to_from(sel!(redo:), None, sender) };
            }
        }

        /// Filter: Zeitraum (Tag = Stelle in [`Period::ALL`]).
        #[unsafe(method(filterPeriod:))]
        fn filter_period(&self, sender: Option<&NSMenuItem>) {
            let period = sender.and_then(|s| usize::try_from(s.tag()).ok()).and_then(|i| Period::ALL.get(i).copied());
            if let Some(period) = period {
                set_filter(|f| f.period = period);
            }
        }

        /// Filter: Merkmal ein- oder ausschalten.
        #[unsafe(method(filterToggle:))]
        fn filter_toggle(&self, sender: Option<&NSMenuItem>) {
            match sender.map(|s| s.tag()) {
                Some(FILTER_WARNINGS) => set_filter(|f| f.warnings = !f.warnings),
                Some(FILTER_CORRECTED) => set_filter(|f| f.corrected = !f.corrected),
                _ => {}
            }
        }

        #[unsafe(method(filterReset:))]
        fn filter_reset(&self, _sender: Option<&AnyObject>) {
            set_filter(|f| *f = Filter::default());
        }

        #[unsafe(method(reload:))]
        fn reload_action(&self, _sender: Option<&AnyObject>) {
            reload();
            if mode() == Mode::Week {
                refresh_week(None);
            }
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

        #[unsafe(method(showDays:))]
        fn show_days(&self, _sender: Option<&AnyObject>) {
            set_mode(Mode::Days, true);
        }

        /// Rechtsklick „In der Wochenansicht zeigen“ oder Doppelklick auf eine Zeile: die Woche
        /// des Tags zeigen (bei einer Wochenzeile diese Woche).
        #[unsafe(method(showRowInWeek:))]
        fn show_row_in_week(&self, _sender: Option<&AnyObject>) {
            show_row_in_week(ui().table.clickedRow());
        }

        #[unsafe(method(showWeek:))]
        fn show_week(&self, _sender: Option<&AnyObject>) {
            set_mode(Mode::Week, true);
        }

        /// Umschalter „Tage | Woche“ in der Symbolleiste.
        #[unsafe(method(switchView:))]
        fn switch_view(&self, sender: Option<&NSSegmentedControl>) {
            let week = sender.is_some_and(|s| s.selectedSegment() == 1);
            set_mode(if week { Mode::Week } else { Mode::Days }, true);
        }

        #[unsafe(method(goToday:))]
        fn go_today(&self, _sender: Option<&AnyObject>) {
            week_view::today();
        }

        #[unsafe(method(showAbout:))]
        fn show_about(&self, _sender: Option<&AnyObject>) {
            about::show();
        }

        #[unsafe(method(showSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            settings_window::show(settings_changed);
        }

        #[unsafe(method(revealDatabase:))]
        fn reveal_database(&self, _sender: Option<&AnyObject>) {
            reveal(&store::db_file());
        }

        /// Freier Tag: „Erfasste Zeit zählen“ ein- oder ausgeschaltet.
        #[unsafe(method(countingToggled:))]
        fn counting_toggled(&self, sender: Option<&NSButton>) {
            let on = sender.is_some_and(|b| b.state() == NSControlStateValueOn);
            apply(Edit::SetCounting(on), true);
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

        /// Info-Knopf an einer Überschrift des Inspektors: Erklärung als Popover.
        #[unsafe(method(showHelp:))]
        fn show_help(&self, sender: Option<&NSButton>) {
            if let Some(button) = sender {
                show_help(button);
            }
        }

        /// Art der Abwesenheit oder ganzer/halber Tag gewählt. Ganz/halb ohne Abwesenheit gilt
        /// für die Art, die als Nächstes gewählt wird (gespeichert wird erst dann).
        #[unsafe(method(absenceChanged:))]
        fn absence_changed(&self, sender: Option<&NSPopUpButton>) {
            let from_part = sender.is_some_and(|s| s.tag() == ABSENCE_PART_TAG);
            let change = model(|m| {
                let i = m.inspector.as_ref()?;
                let date = i.date;
                let half = i.absence_part.indexOfSelectedItem() == 1;
                // Erster Eintrag: keine Abwesenheit.
                let kind = usize::try_from(i.absence_kind.indexOfSelectedItem() - 1)
                    .ok()
                    .and_then(|k| AbsenceKind::ALL.get(k).copied());
                // Neben anderen Einträgen geht nur ein halber Tag („Ganzer Tag“ ist gesperrt).
                let busy = m.day(date).is_some_and(|d| d.blocks_full_absence(date == now().date()));
                if from_part {
                    m.absence_half = Some((date, half));
                }
                if from_part && kind.is_none() {
                    return None;
                }
                Some((date, kind.map(|k| Absence::new(k, busy || half))))
            });
            if let Some((date, absence)) = change {
                set_absence(date, absence);
            }
        }

        #[unsafe(method(addBreak:))]
        fn add_break(&self, _sender: Option<&AnyObject>) {
            let (times, min) = model(|m| {
                (
                    m.inspector.as_ref().map(|i| (picker_secs(&i.break_from), picker_secs(&i.break_to))),
                    m.inspector.as_ref().map_or(0, |i| m.settings.rules(i.date).min_pause),
                )
            });
            match times {
                Some((Some(from), Some(to))) if to - from >= min => apply(Edit::AddBreak(from, to), true),
                Some((Some(from), Some(to))) if to > from => show_inspector_error(
                    Input::Break,
                    &format!("Eine Pause dauert mindestens {} Min. (siehe Einstellungen).", min / 60),
                ),
                Some((Some(_), Some(_))) => {
                    show_inspector_error(Input::Break, "Das Ende der Pause muss nach ihrem Beginn liegen.")
                }
                _ => {}
            }
        }

        #[unsafe(method(removeBreak:))]
        fn remove_break(&self, sender: Option<&NSButton>) {
            if let Some(b) = sender {
                apply(Edit::RemoveBreak(b.tag() as usize), true);
            }
        }

        #[unsafe(method(addWork:))]
        fn add_work(&self, _sender: Option<&AnyObject>) {
            let input = model(|m| {
                m.inspector.as_ref().map(|i| {
                    (picker_secs(&i.work_from), picker_secs(&i.work_to), popup_activity(&i.work_reason).note().to_string())
                })
            });
            let min_work = model(|m| m.settings.min_work);
            match input {
                Some((Some(from), Some(to), _)) if to > from && shown_len(from, to) < min_work => {
                    show_inspector_error(Input::Work, &min_work_text(min_work))
                }
                Some((Some(from), Some(to), note)) if to > from => {
                    // Kurze Lücken zu benachbarter Arbeit ohne Mac gehören zum neuen Abschnitt.
                    let (span, hint) = model(|m| {
                        let date = m.inspector.as_ref()?.date;
                        let min = m.settings.rules(date).min_pause;
                        let j = m.day(date)?.join_work_gaps(from, to, min);
                        Some((j.span, j.note(min)))
                    })
                    .unwrap_or((Span::new(from, to), None));
                    apply(Edit::AddWork(span.from, span.to, note), true);
                    if let Some(text) = hint {
                        show_info_sheet(&ui().window, "Lücke dazugerechnet", &text);
                    }
                }
                Some((Some(_), Some(_), _)) => {
                    show_inspector_error(Input::Work, "Das Ende der Arbeit ohne Mac muss nach ihrem Beginn liegen.")
                }
                _ => {}
            }
        }

        #[unsafe(method(removeWork:))]
        fn remove_work(&self, sender: Option<&NSButton>) {
            if let Some(b) = sender {
                apply(Edit::RemoveWork(b.tag() as usize), true);
            }
        }

        /// „Alle Einträge löschen …“ im Inspektor: erst nachfragen.
        #[unsafe(method(clearDay:))]
        fn clear_day(&self, _sender: Option<&AnyObject>) {
            if let Some(date) = model(|m| m.inspector.as_ref().map(|i| i.date)) {
                confirm_clear_day(date);
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

        /// Format im Sichern-Dialog gewählt: Der Dateiname bekommt die passende Endung.
        #[unsafe(method(exportFormat:))]
        fn export_format(&self, _sender: Option<&AnyObject>) {
            export_range_changed(false);
        }

        #[unsafe(method(importRecords:))]
        fn import_records(&self, _sender: Option<&AnyObject>) {
            show_import();
        }

        #[unsafe(method(tick:))]
        fn tick(&self, _timer: Option<&AnyObject>) {
            tick();
        }
    }
);

define_class!(
    /// Tabellenzelle, deren Text eine eigene Farbe hat (der Saldo). In der markierten Zeile
    /// nimmt sie die Textfarbe der Markierung an, damit der Text lesbar bleibt – Grün oder Rot
    /// auf dem Blau der Markierung wäre es nicht.
    #[unsafe(super(NSTableCellView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkTintedCell"]
    #[ivars = Retained<NSColor>]
    struct TintedCell;

    impl TintedCell {
        #[unsafe(method(setBackgroundStyle:))]
        fn set_background_style(&self, style: NSBackgroundStyle) {
            let _: () = unsafe { msg_send![super(self), setBackgroundStyle: style] };
            if let Some(text) = unsafe { self.textField() } {
                let label = NSColor::labelColor();
                let marked = style == NSBackgroundStyle::Emphasized;
                text.setTextColor(Some(if marked { &label } else { self.ivars() }));
            }
        }
    }
);

impl TintedCell {
    fn new(mtm: MainThreadMarker, color: Retained<NSColor>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(color);
        unsafe { msg_send![super(this), init] }
    }
}

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
    // Die Saldo-Zelle eines Tages ist farbig – außer in der markierten Zeile.
    let tint = match (row, column) {
        (Row::Day { day, rec, .. }, Some("balance")) => {
            m.settings.day_balance(day, rec, now().date()).map(palette::balance)
        }
        (Row::Missing { target, .. }, Some("balance")) => Some(palette::balance(-target)),
        _ => None,
    };
    let cell: Retained<NSTableCellView> = match tint {
        Some(color) => Retained::into_super(TintedCell::new(mtm, color)),
        None => NSTableCellView::new(mtm),
    };
    match (row, column) {
        (Row::Week { title, total, balance, .. }, _) => {
            let t = label(title);
            t.setFont(Some(&font_semibold(NSFont::systemFontSize())));
            // Der Saldo farbig wie im Menü: grün im Plus, rot im Minus.
            let s = label("");
            s.setAttributedStringValue(&palette::colored(
                &[
                    (total.as_str(), &NSColor::secondaryLabelColor()),
                    (rules::fmt_balance(*balance).as_str(), &palette::balance(*balance)),
                ],
                &font_digits(NSFont::systemFontSize(), font_regular()),
            ));
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
            // Zwei feste Plätze, damit die Symbole über die Zeilen hinweg untereinanderstehen:
            // links das Warnschild für Hinweise, rechts der Stift für manuell korrigierte Tage.
            let warning = (!warnings.is_empty()).then(|| {
                let tip: Vec<String> = warnings.iter().map(Warning::text).collect();
                ("exclamationmark.triangle.fill", NSColor::systemOrangeColor(), tip.join("\n"))
            });
            let pencil = day
                .has_corrections()
                .then(|| ("pencil", NSColor::secondaryLabelColor(), "Manuell korrigiert".to_string()));
            let mut tips = Vec::new();
            for (slot, mark) in [warning, pencil].into_iter().enumerate() {
                let Some((name, color, tip)) = mark else { continue };
                let iv = NSImageView::new(mtm);
                iv.setImage(symbol(name, &tip).as_deref());
                iv.setContentTintColor(Some(&color));
                iv.setToolTip(Some(&ns(&tip)));
                iv.setTranslatesAutoresizingMaskIntoConstraints(false);
                cell.addSubview(&iv);
                let center = 4.0 + MARK_SLOT * (slot as f64 + 0.5);
                iv.centerXAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), center).setActive(true);
                iv.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
                if tips.is_empty() {
                    unsafe { cell.setImageView(Some(&iv)) };
                }
                tips.push(tip);
            }
            if !tips.is_empty() {
                cell.setToolTip(Some(&ns(&tips.join("\n"))));
            }
        }
        (Row::Day { day, rec, .. }, Some(col)) => {
            let today = now().date();
            let is_today = rec.date == today;
            // Ohne Arbeitszeit (z. B. ein Urlaubstag oder ein freier Tag, dessen erfasste Zeit
            // nicht zählt) gibt es weder Beginn noch Ende.
            let worked = rec.work_secs > 0 || (!day.periods.is_empty() && !day.uncounted());
            let (text, align, secondary) = match col {
                "date" => (store::fmt_date(rec.date), NSTextAlignment::Left, false),
                "start" if !worked => ("–".to_string(), NSTextAlignment::Right, true),
                "end" if !worked => ("–".to_string(), NSTextAlignment::Right, true),
                "start" => (fmt_clock(rec.first_start), NSTextAlignment::Right, false),
                "end" => {
                    if is_today
                        && day.end.is_none()
                        && matches!(m.status, Some(Status::Running | Status::Away(..)))
                    {
                        ("läuft".to_string(), NSTextAlignment::Right, true)
                    } else {
                        (fmt_clock(rec.last_end), NSTextAlignment::Right, false)
                    }
                }
                "work" => (store::fmt_hm(rec.shown_work_secs), NSTextAlignment::Right, false),
                "balance" => {
                    // Mit Abwesenheit gibt es auch ohne Arbeitszeit einen Saldo (Zeitausgleich);
                    // Arbeit an einem freien Tag, die nicht zählt, hat keinen.
                    match m.settings.day_balance(day, rec, today) {
                        Some(b) => (rules::fmt_balance(b), NSTextAlignment::Right, true),
                        None => ("–".to_string(), NSTextAlignment::Right, true),
                    }
                }
                "pause" => (store::fmt_hm(rec.shown_pause_secs), NSTextAlignment::Right, false),
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
            if col == "balance" && cell.downcast_ref::<TintedCell>().is_some() {
                // Die Farbe setzt die Zelle selbst (siehe `TintedCell`).
            } else if secondary {
                t.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            t.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&t);
            t.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true);
            t.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            match day.absence.filter(|_| col == "date") {
                // Hinter dem Datum die Abwesenheit als Kürzel („½ U“) – für mehr ist neben dem
                // Datum kein Platz; ausgeschrieben steht sie im Tooltip und im Inspektor.
                Some(absence) => {
                    let a = label(&absence.code());
                    a.setFont(Some(&NSFont::systemFontOfSize(small_size())));
                    a.setTextColor(Some(&NSColor::secondaryLabelColor()));
                    a.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                    a.setToolTip(Some(&ns(&absence.label())));
                    a.setTranslatesAutoresizingMaskIntoConstraints(false);
                    cell.addSubview(&a);
                    t.setContentHuggingPriority_forOrientation(751.0, NSLayoutConstraintOrientation::Horizontal);
                    a.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
                    a.leadingAnchor().constraintEqualToAnchor_constant(&t.trailingAnchor(), 6.0).setActive(true);
                    a.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
                    a.firstBaselineAnchor().constraintEqualToAnchor(&t.firstBaselineAnchor()).setActive(true);
                }
                None => {
                    t.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
                }
            }
            unsafe { cell.setTextField(Some(&t)) };
        }
        (Row::Missing { .. }, Some("mark")) => {
            let tip = "Nichts erfasst – der Arbeitstag zählt mit seinem Soll. Zeiten oder eine \
                       Abwesenheit lassen sich im Inspektor nachtragen.";
            let iv = NSImageView::new(mtm);
            iv.setImage(symbol("questionmark.circle", tip).as_deref());
            iv.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
            iv.setToolTip(Some(&ns(tip)));
            iv.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&iv);
            // Im Platz des Warnschilds, das ein Tag ohne Eintrag nie hat.
            iv.centerXAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 4.0 + MARK_SLOT * 0.5).setActive(true);
            iv.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            unsafe { cell.setImageView(Some(&iv)) };
            cell.setToolTip(Some(&ns(tip)));
        }
        (Row::Missing { date, target }, Some(col)) => {
            let (text, align) = match col {
                "date" => (store::fmt_date(*date), NSTextAlignment::Left),
                "start" | "end" | "pause" => ("–".to_string(), NSTextAlignment::Right),
                "work" => (store::fmt_hm(0), NSTextAlignment::Right),
                "balance" => (rules::fmt_balance(-target), NSTextAlignment::Right),
                _ => return None,
            };
            let t = label(&text);
            t.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
            t.setAlignment(align);
            if col != "balance" {
                t.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            t.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&t);
            t.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true);
            t.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            t.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
            unsafe { cell.setTextField(Some(&t)) };
        }
        (Row::Day { .. } | Row::Missing { .. }, None) => return None,
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
    let (texts, status) = model(|m| {
        let st = m.settings.clone();
        let recs: Vec<DayRecord> = m.records().into_iter().filter(|r| r.work_secs > 0).collect();
        // (Arbeit, Pausen, Tage, Soll)
        let sum = |f: &dyn Fn(NaiveDate) -> bool| {
            recs.iter().filter(|r| f(r.date)).fold((0i64, 0i64, 0usize, 0i64), |(w, p, n, t), r| {
                (w + r.shown_work_secs, p + r.shown_pause_secs, n + 1, t + st.daily_target(r.date))
            })
        };
        let (tw, _tp, tn, _) = sum(&|d| d == today);
        let week_start = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        let (ww, _wp, wn, _) = sum(&|d| d >= week_start && d <= today);
        let (mw, mp, mn, _) = sum(&|d| d.year() == today.year() && d.month() == today.month());
        // Das Soll samt Abwesenheiten (halber Urlaubstag, Zeitausgleich) wie überall sonst.
        // Der Saldo ohne Arbeit an freien Tagen, die nicht zählt.
        let (wc, wt) = rules::balance(&m.days, week_start, today, today, &st);
        let month_end = month_start(month_start(today) + Duration::days(31)) - Duration::days(1);
        let (mc, mt) = rules::balance(&m.days, month_start(today), month_end, today, &st);
        let begin = m.day(today).zip(recs.iter().find(|r| r.date == today)).map(|(d, r)| {
            let manual = if d.start.is_some() { " (manuell)" } else { "" };
            format!("Beginn {} Uhr{manual}", fmt_clock(r.first_start))
        });
        let today_target = st.day_target(today, m.day(today).and_then(|d| d.absence));
        let today_detail = match (tn, begin) {
            (0, _) | (_, None) if today_target > 0 => format!("Soll {}", store::fmt_hm(today_target)),
            (0, _) | (_, None) => "Kein Arbeitstag".to_string(),
            (_, Some(b)) if today_target > 0 => format!("{b} · Soll {}", store::fmt_hm(today_target)),
            (_, Some(b)) => b,
        };
        let avg = |x: i64| if mn > 0 { x / mn as i64 } else { 0 };
        // Je Karte: Wert, Unterzeile und – wo einer steht – der Saldo (farbig): als Wert
        // beim Zeitkonto, sonst am Ende der Unterzeile.
        let account = rules::account(&m.days, today, &st);
        let texts = [
            Card { value: store::fmt_hm(tw), detail: today_detail, value_balance: None, detail_balance: None },
            Card {
                value: store::fmt_hm(ww),
                detail: format!("{} · Saldo ", plural(wn, "Arbeitstag", "Arbeitstage")),
                value_balance: None,
                detail_balance: Some(wc - wt),
            },
            Card {
                value: store::fmt_hm(mw),
                detail: format!("{} · Saldo ", plural(mn, "Arbeitstag", "Arbeitstage")),
                value_balance: None,
                detail_balance: Some(mc - mt),
            },
            Card {
                value: store::fmt_hm(avg(mw)),
                detail: format!("Pausen Ø {} · {}", store::fmt_hm(avg(mp)), MONTHS[today.month0() as usize]),
                value_balance: None,
                detail_balance: None,
            },
            Card {
                value: rules::fmt_balance(account),
                detail: account_detail(&st, m.days.first().map(|d| d.date)),
                value_balance: Some(account),
                detail_balance: None,
            },
        ];
        (texts, m.status)
    });
    let detail_font = font_digits(small_size(), font_regular());
    for ((value, detail), card) in ui.cards.iter().zip(texts) {
        value.setStringValue(&ns(&card.value));
        value.setTextColor(Some(&card.value_balance.map_or_else(NSColor::labelColor, palette::balance)));
        let mut parts = vec![(card.detail.clone(), NSColor::secondaryLabelColor())];
        if let Some(balance) = card.detail_balance {
            parts.push((rules::fmt_balance(balance), palette::balance(balance)));
        }
        let parts: Vec<(&str, &NSColor)> = parts.iter().map(|(text, color)| (text.as_str(), &**color)).collect();
        detail.setAttributedStringValue(&palette::colored(&parts, &detail_font));
    }
    let subtitle = match status {
        Some(Status::Running) => "Zeiterfassung läuft".to_string(),
        Some(Status::Paused) => "Zeiterfassung pausiert".to_string(),
        Some(Status::Break(since)) => format!("Pause seit {} Uhr", since.format("%H:%M")),
        Some(Status::Away(since, _)) => format!("Ohne Mac seit {} Uhr", since.format("%H:%M")),
        Some(Status::Outside) => "Außerhalb des Arbeitszeitrahmens".to_string(),
        Some(Status::FreeDay) => "Freier Tag – Erfassung zählt nicht".to_string(),
        Some(Status::Closed) => "Feierabend".to_string(),
        Some(Status::Absent) => "Heute abwesend".to_string(),
        None => "Menüleisten-App nicht aktiv".to_string(),
    };
    ui.window.setSubtitle(&ns(&subtitle));
    refresh_footer();
}

/// Fußzeile neu setzen – nach jeder Änderung und wenn ein anderer Tag gewählt wird.
fn refresh_footer() {
    let text = model(|m| footer_text(m, now().date()));
    ui().footer.setStringValue(&ns(&text));
}

/// Fußzeile: wie viele Arbeitstage (Tage mit Arbeitszeit) erfasst sind – mit Filter: wie viele
/// Tage die Tabelle von allen zeigt, samt Abwesenheiten und Arbeitstagen ohne Eintrag – und
/// die Arbeitszeitregeln des gewählten Tages, ohne Auswahl die von heute.
fn footer_text(m: &Model, today: NaiveDate) -> String {
    if let Some(e) = &m.error {
        return e.clone();
    }
    let count = if m.filter.is_active() {
        let rows = m.rows.iter().filter(|r| !matches!(r, Row::Week { .. })).count();
        let all = m.records().len() + rules::missing_workdays(&m.days, NaiveDate::MIN, today, today, &m.settings).len();
        format!("Filter: {rows} von {}", plural(all, "Tag", "Tagen"))
    } else {
        let worked = m.records().iter().filter(|r| r.work_secs > 0).count();
        format!("{} erfasst", plural(worked, "Arbeitstag", "Arbeitstage"))
    };
    format!("{count}  ·  {}", rules_footer(&m.settings, m.selected, today))
}

/// Die Arbeitszeitregeln für die Fußzeile: die des Tages `selected`, mit Datum davor, wenn
/// es nicht heute ist (ohne Auswahl die von heute).
fn rules_footer(s: &Settings, selected: Option<NaiveDate>, today: NaiveDate) -> String {
    match selected.filter(|d| *d != today) {
        Some(d) => format!("Regeln am {}: {}", rules::fmt_day(d), s.rules(d).summary()),
        None => s.rules(today).summary(),
    }
}

/// Inhalt einer Karte der Übersicht.
struct Card {
    value: String,
    detail: String,
    /// Der Wert ist ein Saldo (farbig).
    value_balance: Option<i64>,
    /// Saldo am Ende der Unterzeile (farbig).
    detail_balance: Option<i64>,
}

/// Unterzeile der Karte „Zeitkonto“: seit wann es zählt und der Übertrag, falls es einen gibt.
/// `first` ist der erste erfasste Tag.
fn account_detail(s: &Settings, first: Option<NaiveDate>) -> String {
    let since = match (s.account_from, first) {
        (Some(from), _) => format!("ab {}", store::fmt_date_short(from)),
        (None, Some(first)) => format!("seit {}", store::fmt_date_short(first)),
        (None, None) => "noch nichts erfasst".to_string(),
    };
    if s.account_carry == 0 {
        since
    } else {
        format!("{since} · Übertrag {}", rules::fmt_balance(s.account_carry))
    }
}

/// Alles neu einlesen und anzeigen.
fn reload() {
    model(|m| m.load());
    reload_table();
    refresh_summary();
    refresh_inspector(false);
    week_from_model();
}

/// Einstellungen wurden im Fenster „Einstellungen“ geändert.
fn settings_changed() {
    reload();
    notify_change();
}

// ---------- Inspektor ----------

/// Was im Inspektor geändert wurde.
#[derive(Clone, Debug)]
enum Edit {
    SetStart(Option<i64>),
    SetEnd(Option<i64>),
    AddBreak(i64, i64),
    RemoveBreak(usize),
    AddWork(i64, i64, String),
    RemoveWork(usize),
    /// Arbeit ohne Mac in diesem Bereich löschen (aus der Wochenansicht).
    CutWork(i64, i64),
    /// Abwesenheit eintragen, ändern oder (`None`) entfernen.
    SetAbsence(Option<Absence>),
    /// Freier Tag: Die erfasste Zeit zählt bzw. zählt nicht (siehe [`Day::free_day`]).
    SetCounting(bool),
    /// Alle Einträge des Tages löschen – manuelle wie automatisch erfasste.
    ClearDay,
}

impl Edit {
    /// Bezeichnung im Menü „Bearbeiten“ („Widerrufen: …“).
    fn name(&self) -> &'static str {
        match self {
            Edit::SetStart(_) => "Arbeitsbeginn ändern",
            Edit::SetEnd(_) => "Feierabend ändern",
            Edit::AddBreak(..) => "Pause eintragen",
            Edit::RemoveBreak(_) => "Pause entfernen",
            Edit::AddWork(..) => "Arbeit ohne Mac eintragen",
            Edit::RemoveWork(_) | Edit::CutWork(..) => "Arbeit ohne Mac entfernen",
            Edit::SetAbsence(_) => "Abwesenheit ändern",
            Edit::SetCounting(true) => "Erfasste Zeit zählen",
            Edit::SetCounting(false) => "Erfasste Zeit nicht zählen",
            Edit::ClearDay => "Alle Einträge löschen",
        }
    }

    /// Eingabe im Inspektor, über der ein Hinweis zu dieser Änderung steht.
    fn input(&self) -> Option<Input> {
        match self {
            Edit::SetStart(_) => Some(Input::Start),
            Edit::SetEnd(_) => Some(Input::End),
            Edit::AddBreak(..) | Edit::RemoveBreak(_) => Some(Input::Break),
            Edit::AddWork(..) | Edit::RemoveWork(_) | Edit::CutWork(..) => Some(Input::Work),
            Edit::SetAbsence(_) => Some(Input::Absence),
            Edit::SetCounting(_) | Edit::ClearDay => None,
        }
    }

    /// Entsteht damit ein Eintrag (siehe [`rules::missing_workdays`])? Entfernen, Leeren und die
    /// bloße Entscheidung zur Erfassung an einem freien Tag nicht.
    fn adds_entry(&self) -> bool {
        matches!(
            self,
            Edit::SetStart(Some(_))
                | Edit::SetEnd(Some(_))
                | Edit::AddBreak(..)
                | Edit::AddWork(..)
                | Edit::SetAbsence(Some(_))
        )
    }
}

/// Hinweis, wenn an einem künftigen Tag etwas anderes als eine Abwesenheit eingetragen werden soll.
const FUTURE_TEXT: &str =
    "An künftigen Tagen lässt sich nur eine Abwesenheit eintragen – Zeiten erst, wenn der Tag da ist.";
/// Hinweis zu einer Abwesenheit an einem Tag ohne Soll.
const FREE_DAY_ABSENCE: &str = "An freien Tagen (Wochenende, abgewählte Arbeitstage) gibt es kein Soll – eine \
     Abwesenheit lässt sich dort nicht eintragen, eine vorhandene nur entfernen.";

/// Warum der Tag `d` die Änderung `edit` nicht annimmt (`None`: Sie geht): Zeiten in der
/// Zukunft (an künftigen Tagen geht nur eine Abwesenheit, heute nichts nach `now`), ein
/// Arbeitsbeginn nicht vor dem Feierabend (siehe [`crate::day::bounds_in_order`]) oder eine
/// neue Abwesenheit an einem Tag ohne Soll (wie in der Wochenansicht). Entfernen geht immer.
fn refusal(d: &Day, edit: &Edit, settings: &Settings, now: NaiveDateTime) -> Option<String> {
    let today = now.date();
    let latest = match edit {
        Edit::SetStart(Some(t)) | Edit::SetEnd(Some(t)) => Some(*t),
        Edit::AddBreak(_, to) | Edit::AddWork(_, to, _) => Some(*to),
        _ => None,
    };
    if d.date > today && (latest.is_some() || matches!(edit, Edit::SetCounting(_))) {
        return Some(FUTURE_TEXT.to_string());
    }
    let clock = secs_of(now.time());
    if d.date == today && latest.is_some_and(|t| t > clock) {
        return Some(format!("Zeiten nach jetzt ({} Uhr) lassen sich nicht eintragen.", fmt_clock(clock)));
    }
    match *edit {
        Edit::SetStart(Some(s)) if !crate::day::bounds_in_order(Some(s), d.end) => Some(format!(
            "Der Arbeitsbeginn muss vor dem Feierabend um {} Uhr liegen.",
            fmt_clock(d.end.unwrap_or_default())
        )),
        Edit::SetEnd(Some(e)) if !crate::day::bounds_in_order(d.start, Some(e)) => Some(format!(
            "Der Feierabend muss nach dem Arbeitsbeginn um {} Uhr liegen.",
            fmt_clock(d.start.unwrap_or_default())
        )),
        Edit::SetAbsence(Some(a)) if !settings.is_workday(d.date) && d.absence != Some(a) => {
            Some(FREE_DAY_ABSENCE.to_string())
        }
        _ => None,
    }
}

/// Text der Rückfrage, bevor am Tag `date` ein Eintrag vor dem bisher ersten Tag mit Einträgen
/// entsteht (`days`: alle Tage, `today`: heute). Er wird dann zum ersten Tag: Alle Arbeitstage
/// dazwischen zählen ihr Soll (siehe [`rules::missing_workdays`]). `None`, wenn das nichts
/// ändert – es gibt noch gar keinen Eintrag, der Tag liegt nicht davor, oder dazwischen liegt
/// kein Arbeitstag bis gestern.
fn before_first_text(days: &[Day], date: NaiveDate, settings: &Settings, today: NaiveDate) -> Option<String> {
    let first = days.iter().find(|d| d.has_entries() || d.absence.is_some())?.date;
    if date >= first {
        return None;
    }
    let gap: Vec<NaiveDate> = date
        .iter_days()
        .skip(1)
        .take_while(|d| *d < first && *d < today)
        .filter(|d| settings.is_workday(*d))
        .collect();
    if gap.is_empty() {
        return None;
    }
    let target: i64 = gap.iter().map(|d| settings.daily_target(*d)).sum();
    let between = match gap.len() {
        1 => "zählt auch der Arbeitstag dazwischen mit seinem Soll".to_string(),
        n => format!("zählen auch die {n} Arbeitstage dazwischen mit ihrem Soll"),
    };
    Some(format!(
        "Bisher beginnen die Aufzeichnungen am {}. Mit einem Eintrag am {} {between} – \
         zusammen etwa {} Std. Minus in Saldo und Zeitkonto, bis dort Zeiten oder Abwesenheiten \
         eingetragen sind.",
        store::fmt_date_short(first),
        store::fmt_date_short(date),
        (target + 1800) / 3600
    ))
}

/// Lässt der Tag `d` die Änderung nicht zu? Eine ganztägige Abwesenheit lässt nur sich selbst
/// ändern oder den Tag leeren. An einem freien Tag, dessen erfasste Zeit nicht zählt, gibt es
/// Arbeitsbeginn, Pausen und Feierabend erst nach „Erfasste Zeit zählen“ (wie im Menü und in
/// der Wochenansicht) – entfernen geht immer, Arbeit ohne Mac auch.
fn refuses(d: &Day, edit: &Edit) -> bool {
    let locked = d.locks_entries() && !matches!(edit, Edit::SetAbsence(_) | Edit::ClearDay);
    let uncounted =
        d.uncounted() && matches!(edit, Edit::SetStart(Some(_)) | Edit::SetEnd(Some(_)) | Edit::AddBreak(..));
    locked || uncounted
}

fn apply(edit: Edit, rebuild: bool) {
    let Some(date) = model(|m| m.inspector.as_ref().map(|i| i.date)) else { return };
    if let Err(e) = save(date, edit, rebuild) {
        report(date, e, rebuild);
    }
}

/// Warum eine Änderung nicht gespeichert wurde.
#[derive(Debug)]
enum NotSaved {
    /// Abgelehnt (siehe [`refusal`]) – der Tag bleibt, wie er ist. Mit der Eingabe im
    /// Inspektor, über der der Hinweis steht.
    Refused(Option<Input>, String),
    /// Die Datenbank meldet einen Fehler.
    Failed(String),
}

/// Eine nicht gespeicherte Änderung am Tag `date` melden: im Inspektor über der Eingabe, wenn
/// er den Tag zeigt (die Felder wieder auf dem gespeicherten Stand; `rebuild`: neu aufgebaut
/// statt nur die Uhrzeit-Felder zurückgesetzt), sonst als Meldung am Fenster.
fn report(date: NaiveDate, e: NotSaved, rebuild: bool) {
    let shown = mode() == Mode::Days && model(|m| m.inspector.as_ref().is_some_and(|i| i.date == date));
    let (title, text) = match e {
        NotSaved::Refused(Some(input), text) if shown => {
            if rebuild {
                refresh_inspector(true);
            } else {
                reset_bound_pickers();
            }
            show_inspector_error(input, &text);
            return;
        }
        NotSaved::Refused(_, text) => ("Nicht gespeichert", text),
        NotSaved::Failed(text) => ("Speichern fehlgeschlagen", text),
    };
    // Erst wenn ein Sheet, aus dem die Änderung kam, geschlossen ist.
    show_sheet_later(title.to_string(), text);
}

/// Hinweis als Sheet am Fenster, sobald dort keins mehr offen ist (etwa aus dem Handler eines
/// anderen Sheets heraus).
fn show_sheet_later(title: String, text: String) {
    let block = RcBlock::new(move |_timer| show_info_sheet(&ui().window, &title, &text));
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
}

/// Arbeitsbeginn und Feierabend im Inspektor wieder auf den gespeicherten Stand (nach einer
/// abgelehnten Eingabe mit den Pfeiltasten, ohne das Feld zu verlassen).
fn reset_bound_pickers() {
    model(|m| {
        let Some(i) = m.inspector.as_ref() else { return };
        let Some(d) = m.day(i.date) else { return };
        for (picker, value) in [(&i.start_picker, d.start), (&i.end_picker, d.end)] {
            if let Some(secs) = value {
                picker.setDateValue(&to_nsdate(at(i.date, secs)));
            }
        }
    });
}

/// Änderung an einem Tag speichern und anzeigen. `rebuild` baut den Inspektor neu auf. Gibt
/// zurück, ob ein neuer Arbeitsbeginn bzw. Feierabend Arbeit ohne Mac gekürzt hat. Entstünde
/// damit der erste Eintrag weit vor dem bisher ersten Tag, fragt erst ein Dialog nach (siehe
/// [`before_first_text`]); gespeichert wird dann erst nach „Eintragen“.
fn save(date: NaiveDate, edit: Edit, rebuild: bool) -> Result<Option<Bound>, NotSaved> {
    let text = model(|m| {
        // Was ohnehin abgelehnt wird, braucht keine Rückfrage.
        let day = m.day(date).cloned().unwrap_or_else(|| Day::new(date));
        let goes = edit.adds_entry() && !refuses(&day, &edit) && refusal(&day, &edit, &m.settings, now()).is_none();
        goes.then(|| before_first_text(&m.days, date, &m.settings, now().date())).flatten()
    });
    match text {
        Some(text) => {
            let proceed = move || match store_edit(date, edit.clone(), rebuild) {
                Err(e) => report(date, e, true),
                Ok(Some(bound)) if mode() == Mode::Week => {
                    let text = if bound == Bound::Start { START_CUT } else { END_CUT };
                    show_sheet_later("Arbeit ohne Mac entfallen".to_string(), text.to_string());
                }
                Ok(_) => {}
            };
            confirm_before_first(date, text, std::rc::Rc::new(proceed));
            Ok(None)
        }
        None => store_edit(date, edit, rebuild),
    }
}

/// Rückfrage, bevor am Tag `date` der erste Eintrag vor dem bisher ersten Tag entsteht
/// (`text`: was daraus folgt); „Eintragen“ führt `proceed` aus, „Abbrechen“ ändert nichts.
fn confirm_before_first(date: NaiveDate, text: String, proceed: std::rc::Rc<dyn Fn()>) {
    let window = ui().window;
    if window.attachedSheet().is_some() {
        // Erst wenn das andere Sheet geschlossen ist.
        let block = RcBlock::new(move |_timer| confirm_before_first(date, text.clone(), proceed.clone()));
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
        return;
    }
    let alert = NSAlert::new(mtm());
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&ns(&format!("Eintrag vor dem ersten erfassten Tag am {}?", fmt_date_long(date))));
    alert.setInformativeText(&ns(&text));
    let ok = alert.addButtonWithTitle(&ns("Eintragen"));
    // Return bestätigt nicht versehentlich: Standard ist „Abbrechen“.
    ok.setKeyEquivalent(&ns(""));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\r"));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSAlertFirstButtonReturn {
            // Inspektor und Wochenansicht wieder auf den gespeicherten Stand.
            refresh_inspector(true);
            refresh_week(None);
            return;
        }
        proceed();
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

/// Änderung an einem Tag ohne Rückfrage speichern (siehe [`save`]).
fn store_edit(date: NaiveDate, edit: Edit, rebuild: bool) -> Result<Option<Bound>, NotSaved> {
    let name = edit.name();
    let input = edit.input();
    let bound = match edit {
        Edit::SetStart(_) => Some(Bound::Start),
        Edit::SetEnd(_) => Some(Bound::End),
        _ => None,
    };
    let now = now();
    let today = date == now.date();
    // Die Korrekturen davor und danach, wie sie in der Datenbank stehen.
    let mut change = None;
    let mut rejected = None;
    let result = model(|m| {
        let settings = &m.settings;
        let db = m.db.as_mut()?;
        Some(db.edit_day(date, now, |d| {
            // Löschen (auch heute erfasster Zeit für eine ganztägige Abwesenheit) trifft die
            // erfassten Zeiten – Widerrufen muss sie zurückbringen.
            let clears = matches!(edit, Edit::SetAbsence(a) if d.clears_for_absence(a, today));
            let snapshot = if clears || matches!(edit, Edit::ClearDay) { Corrections::whole } else { Corrections::of };
            let before = snapshot(d);
            rejected = refusal(d, &edit, settings, now);
            let refused = refuses(d, &edit) || rejected.is_some();
            match edit {
                _ if refused => {}
                // Arbeit ohne Mac davor bzw. danach entfällt: die letzte Angabe gilt.
                Edit::SetStart(s) => {
                    d.set_start(s);
                }
                Edit::SetEnd(e) => {
                    d.set_end(e);
                }
                Edit::AddBreak(from, to) => d.add_break(from, to),
                Edit::RemoveBreak(i) => {
                    if i < d.breaks.len() {
                        d.breaks.remove(i);
                    }
                }
                Edit::AddWork(from, to, note) => d.add_work(from, to, &note),
                Edit::RemoveWork(i) => {
                    if i < d.work.len() {
                        d.work.remove(i);
                    }
                }
                Edit::CutWork(from, to) => d.cut_work(from, to),
                // Ganztags nur am leeren Tag (die Oberfläche bietet es sonst nicht an); heute
                // entfällt dabei die automatisch erfasste Zeit.
                Edit::SetAbsence(a) if d.accepts_absence(a, today) => {
                    if clears {
                        d.periods.clear();
                    }
                    d.absence = a;
                }
                Edit::SetAbsence(_) => {}
                Edit::SetCounting(on) => d.free_day = Some(if on { FreeDay::Counted } else { FreeDay::Uncounted }),
                Edit::ClearDay => {
                    // Heute an einem freien Tag: Die Erfassung bleibt danach aus, auch wenn
                    // „Heute erfassen“ gewählt war – bis es wieder gewählt ist (sonst ginge sie
                    // sofort weiter, und der Tag wäre gleich wieder da).
                    let free = d.free_day.filter(|_| today);
                    *d = Day::new(date);
                    d.free_day = free.map(|_| FreeDay::Stopped);
                }
            }
            change = Some((before, snapshot(d)));
        }))
    });
    match result {
        None => Ok(None),
        Some(Err(e)) => Err(NotSaved::Failed(e.to_string())),
        Some(Ok(())) if rejected.is_some() => Err(NotSaved::Refused(input, rejected.unwrap_or_default())),
        Some(Ok(())) => {
            let mut cut = None;
            if let Some((before, after)) = change {
                // Hat der neue Beginn bzw. Feierabend Arbeit ohne Mac gekürzt?
                cut = bound.filter(|_| before.work != after.work);
                if before != after {
                    model(|m| m.history.record(Step { name, date, before, after }, !rebuild));
                }
            }
            saved(rebuild);
            if let Some(bound) = cut {
                show_cut_note(date, bound);
            }
            Ok(cut)
        }
    }
}

/// Hinweis, wenn ein neuer Arbeitsbeginn bzw. Feierabend Arbeit ohne Mac gekürzt hat.
const START_CUT: &str = "Arbeit ohne Mac vor dem Arbeitsbeginn ist entfallen – die letzte Angabe gilt. ⌘Z nimmt das zurück.";
const END_CUT: &str = "Arbeit ohne Mac nach dem Feierabend ist entfallen – die letzte Angabe gilt. ⌘Z nimmt das zurück.";

/// Arbeitsbeginn oder Feierabend im Inspektor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound {
    Start,
    End,
}

/// Hinweis unter Arbeitsbeginn bzw. Feierabend: Die Angabe hat Arbeit ohne Mac gekürzt (nur,
/// wenn der Inspektor diesen Tag zeigt).
fn show_cut_note(date: NaiveDate, bound: Bound) {
    let label = model(|m| {
        m.inspector.as_ref().filter(|i| i.date == date).map(|i| match bound {
            Bound::Start => i.start_cut.clone(),
            Bound::End => i.end_cut.clone(),
        })
    });
    if let Some(label) = label {
        label.setHidden(false);
    }
}

/// Liste der Arbeit ohne Mac im Inspektor neu füllen – ohne den Inspektor neu aufzubauen,
/// damit ein Uhrzeit-Feld, in dem gerade geblättert wird, seine Eingabe behält.
fn refresh_work_list() {
    let state = model(|m| {
        let i = m.inspector.as_ref()?;
        let day = m.day(i.date);
        Some((i.work_list.clone(), day.map(|d| d.work.clone()).unwrap_or_default(), day.is_some_and(Day::locks_entries)))
    });
    if let Some((list, work, locked)) = state {
        fill_work_list(&list, &work, &ui().controller, locked);
    }
}

/// Zeilen der Arbeit ohne Mac: Uhrzeiten, Dauer, Notiz und Knopf zum Entfernen (Tag = Stelle
/// in `Day::work`; `locked`: ganztägige Abwesenheit, der Knopf ist gesperrt).
fn fill_work_list(list: &NSStackView, work: &[WorkBlock], target: &Controller, locked: bool) {
    clear(list);
    if work.is_empty() {
        let none = label("Nichts eingetragen");
        none.setTextColor(Some(&NSColor::secondaryLabelColor()));
        list.addArrangedSubview(&none);
    }
    for (i, w) in work.iter().enumerate() {
        let row = hstack(8.0);
        row.setAlignment(NSLayoutAttribute::CenterY);
        let times = label(&format!("{} – {}", fmt_clock(w.from), fmt_clock(w.to)));
        times.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        let dur = label(&fmt_dur(shown_len(w.from, w.to)));
        dur.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        dur.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let note = label(&w.note);
        note.setTextColor(Some(&NSColor::secondaryLabelColor()));
        note.setToolTip(Some(&ns(&w.note)));
        note.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
        let remove = match symbol("minus.circle.fill", "Arbeit ohne Mac entfernen") {
            Some(img) => unsafe {
                NSButton::buttonWithImage_target_action(&img, Some(target), Some(sel!(removeWork:)), mtm())
            },
            None => button("Entfernen", sel!(removeWork:), target),
        };
        remove.setBordered(false);
        remove.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        remove.setToolTip(Some(&ns("Arbeit ohne Mac entfernen")));
        remove.setTag(i as isize);
        remove.setEnabled(!locked);
        let spacer = NSView::new(mtm());
        spacer.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
        row.addArrangedSubview(&times);
        row.addArrangedSubview(&dur);
        row.addArrangedSubview(&note);
        row.addArrangedSubview(&spacer);
        row.addArrangedSubview(&remove);
        list.addArrangedSubview(&row);
        pin_width(&row, list, 0.0);
    }
}

/// Hinweis im Inspektor, dass eine Angabe Arbeit ohne Mac gekürzt hat (zunächst ausgeblendet).
fn cut_note_label(text: &str) -> Retained<NSTextField> {
    let l = caption(text);
    l.setTextColor(Some(&NSColor::systemOrangeColor()));
    l.setHidden(true);
    l
}

/// Nach dem Speichern: neu einlesen, alles anzeigen und der Menüleisten-App Bescheid geben.
fn saved(rebuild: bool) {
    model(|m| {
        m.load();
        if !rebuild {
            // Eingabe läuft weiter: Felder stehen lassen, nur die Zahlen aktualisieren – ein
            // Hinweis zu einer zuvor abgelehnten Uhrzeit ist damit erledigt.
            if let Some(date) = m.selected {
                m.built = Some((date, m.day(date).cloned(), m.settings.clone()));
            }
            if let Some(i) = m.inspector.as_ref() {
                i.start_error.setHidden(true);
                i.end_error.setHidden(true);
            }
        }
    });
    reload_table();
    refresh_summary();
    refresh_inspector(rebuild);
    if !rebuild {
        // Ein neuer Beginn oder Feierabend kann Arbeit ohne Mac gekürzt haben.
        refresh_work_list();
    }
    week_from_model();
    notify_change();
}

/// „Widerrufen“ bzw. „Wiederholen“ (`redo`): zurücknehmen bzw. wiederholen, was der letzte
/// Schritt am Tag geändert hat – nur das, was seitdem nicht anders geändert wurde (siehe
/// [`Corrections::rewind`]) – und den Tag zeigen. Der Schritt wandert in die jeweils andere
/// Richtung, sodass sich beides beliebig abwechseln lässt. Gelingt es nur zum Teil oder gar
/// nicht, sagt ein Hinweis, warum; ein Schritt, der gar nicht geht, entfällt.
fn step_history(redo: bool) {
    let Some(step) = model(|m| if redo { m.history.redo.pop() } else { m.history.undo.pop() }) else { return };
    // Zum Widerrufen vom Stand danach zu dem davor, zum Wiederholen umgekehrt.
    let (done, target) = if redo { (&step.before, &step.after) } else { (&step.after, &step.before) };
    let mut outcome = None;
    let result = model(|m| {
        let db = m.db.as_mut()?;
        Some(db.edit_day(step.date, now(), |d| outcome = Some(Corrections::rewind(d, done, target))))
    });
    let back = |m: &mut Model, s: Step| if redo { m.history.undo.push(s) } else { m.history.redo.push(s) };
    match (result, outcome) {
        (Some(Ok(())), Some(rewind)) => {
            let (name, date) = (step.name, step.date);
            let note = rewind_note(redo, name, date, &rewind);
            if !matches!(rewind, Rewind::Refused(_)) {
                model(|m| back(m, step));
            }
            saved(true);
            if let Some((title, text)) = note {
                show_sheet_later(title, text);
            }
            // Zeigen, was sich geändert hat.
            match mode() {
                Mode::Days => go_to(date),
                Mode::Week => {
                    model(|m| m.selected = Some(date));
                    refresh_footer();
                    refresh_week(Some(date));
                }
            }
        }
        (Some(Err(e)), _) => {
            // Nichts geändert: Der Schritt bleibt, wo er war.
            model(|m| if redo { m.history.redo.push(step) } else { m.history.undo.push(step) });
            show_info_sheet(&ui().window, "Speichern fehlgeschlagen", &e.to_string());
        }
        _ => {}
    }
}

/// Hinweis zu einer abgelehnten Eingabe direkt über deren Feldern zeigen – und in den
/// sichtbaren Bereich holen, falls der Inspektor gerade anders gerollt ist.
fn show_inspector_error(input: Input, text: &str) {
    const ALL: [Input; 5] = [Input::Absence, Input::Start, Input::Break, Input::Work, Input::End];
    let labels = model(|m| m.inspector.as_ref().map(|i| ALL.map(|x| (x, i.error_label(x).clone()))));
    let Some(labels) = labels else { return };
    let mut shown = None;
    for (x, label) in labels {
        if x == input {
            shown = Some(label);
        } else {
            label.setHidden(true);
        }
    }
    let Some(shown) = shown else { return };
    shown.setStringValue(&ns(text));
    shown.setHidden(false);
    // Erst nach dem Layout hat der Hinweis seinen Platz.
    ui().inspector.layoutSubtreeIfNeeded();
    shown.scrollRectToVisible(shown.bounds());
}

/// Erklärung zu einem Abschnitt des Inspektors als Popover am Info-Knopf `button` (der Text
/// ist dessen Tooltip).
fn show_help(button: &NSButton) {
    let Some(text) = button.toolTip() else { return };
    let mtm = mtm();
    let label = NSTextField::wrappingLabelWithString(&text, mtm);
    label.setPreferredMaxLayoutWidth(280.0);
    label.setSelectable(false);
    let stack = vstack(0.0);
    stack.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 14.0, bottom: 12.0, right: 14.0 });
    stack.addArrangedSubview(&label);
    stack.setFrameSize(stack.fittingSize());
    let controller = NSViewController::new(mtm);
    controller.setView(&stack);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setContentViewController(Some(&controller));
    popover.showRelativeToRect_ofView_preferredEdge(button.bounds(), button, NSRectEdge::MinY);
    // Das Popover lebt, bis das nächste es ablöst (AppKit hält es nicht fest).
    HELP.with(|h| *h.borrow_mut() = Some(popover));
}

/// Hinweisfeld des Inspektors: rot, zunächst ausgeblendet (belegt dann keinen Platz).
fn inspector_error_label() -> Retained<NSTextField> {
    let l = caption("");
    l.setTextColor(Some(&NSColor::systemRedColor()));
    l.setHidden(true);
    l
}

/// Kennzahlen des Tages `date` (`day`: seine Einträge, `days`: alle Tage).
fn day_stats(date: NaiveDate, day: Option<&Day>, days: &[Day], settings: &Settings, today: NaiveDate) -> DayStats {
    let rec = day.and_then(|d| d.record(settings).map(|r| (d, r)));
    // Soll, das zählt – auch für einen vergangenen Arbeitstag ganz ohne Eintrag.
    let target = match &rec {
        Some((d, r)) => settings.counted_target(d, r, today),
        None if !rules::missing_workdays(days, date, date, today, settings).is_empty() => settings.daily_target(date),
        None => 0,
    };
    // Künftige Tage: noch keine Zeiten, höchstens eine geplante Abwesenheit.
    if date > today && rec.as_ref().is_none_or(|(_, r)| r.work_secs == 0) {
        return DayStats::Note(if settings.is_workday(date) {
            "Künftiger Tag – eintragen lässt sich nur eine Abwesenheit."
        } else {
            "Künftiger freier Tag – hier gibt es nichts einzutragen."
        });
    }
    match rec {
        Some((d, r)) if r.work_secs > 0 => DayStats::Numbers {
            work: r.shown_work_secs,
            pause: r.shown_pause_secs,
            // Arbeit an einem freien Tag, die nicht zählt: kein Saldo.
            balance: settings.day_balance(d, &r, today),
        },
        // Ohne Arbeitszeit, aber mit Soll (Arbeitstag ohne Eintrag, Zeitausgleich, halber Tag).
        _ if target > 0 => DayStats::Numbers { work: 0, pause: 0, balance: Some(-target) },
        Some((d, _)) if d.absence.is_some() && d.periods.is_empty() => DayStats::Note("Keine Arbeitszeit, kein Soll."),
        Some((d, _)) if d.uncounted() => DayStats::Note("Freier Tag – die erfasste Zeit zählt nicht."),
        Some(_) => DayStats::Note("Keine Arbeitszeit im Arbeitszeitrahmen."),
        None => DayStats::Note("Für diesen Tag ist noch nichts erfasst."),
    }
}

/// Inspektor aktualisieren. Ohne `force` nur, wenn sich der Tag geändert hat und
/// gerade kein Feld im Inspektor bearbeitet wird.
fn refresh_inspector(force: bool) {
    let ui = ui();
    // Die Fußzeile nennt die Regeln des gewählten Tages (ohne `force` hat sich die Auswahl
    // nicht geändert, und `refresh_summary` setzt sie ohnehin).
    if force {
        refresh_footer();
    }
    let (selected, day, unchanged) = model(|m| {
        let day = m.selected.and_then(|d| m.day(d).cloned());
        let unchanged = m.selected.is_some_and(|d| m.built.as_ref() == Some(&(d, day.clone(), m.settings.clone())));
        (m.selected, day, unchanged)
    });
    if !force {
        // Laufende Zahlen (heute) aktualisieren
        model(|m| {
            if let Some(i) = m.inspector.as_ref() {
                i.stats.show(&day_stats(i.date, day.as_ref(), &m.days, &m.settings, now().date()));
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
        let hint = caption(
            "Wähle in der Liste einen Tag aus, um Arbeitsbeginn, Pausen, Arbeit ohne Mac oder Feierabend zu korrigieren.",
        );
        stack.addArrangedSubview(&empty);
        stack.addArrangedSubview(&hint);
        pin_width(&hint, stack, 40.0);
        return;
    };
    let t = now();
    let (settings, warnings) = model(|m| (m.settings.clone(), rules::day_warnings(&m.days, date, &m.settings)));
    let rec = day.as_ref().and_then(|d| d.record(&settings));
    let is_today = date == t.date();
    // Künftiger Tag: nur eine Abwesenheit, keine Zeiten (siehe `refusal`).
    let future = date > t.date();

    // Kopf
    let title = label(&fmt_date_long(date));
    title.setFont(Some(&font_semibold(15.0)));
    // Darunter die Kennzahlen des Tages nebeneinander – in einer Zeile brächen sie um.
    let stats = StatsView::new();
    stats.show(&model(|m| day_stats(date, day.as_ref(), &m.days, &settings, t.date())));
    stack.addArrangedSubview(&title);
    stack.addArrangedSubview(&stats.row);
    stack.addArrangedSubview(&stats.note);
    pin_width(&stats.note, stack, 40.0);
    stack.setCustomSpacing_afterView(10.0, &stats.row);
    stack.setCustomSpacing_afterView(10.0, &stats.note);
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
    stack.setCustomSpacing_afterView(8.0, &title);
    stack.setCustomSpacing_afterView(14.0, &sep);

    // Überschrift eines Abschnitts; die Erklärung dazu steht hinter dem Info-Knopf (und als
    // Tooltip) statt als Absatz darunter.
    let section = |text: &str, prev: Option<&NSView>, help: &str| {
        if let Some(p) = prev {
            stack.setCustomSpacing_afterView(18.0, p);
        }
        let row = hstack(4.0);
        row.setAlignment(NSLayoutAttribute::CenterY);
        let h = headline(text);
        h.setToolTip(Some(&ns(help)));
        row.addArrangedSubview(&h);
        if let Some(img) = symbol("info.circle", "Erklärung") {
            let info =
                unsafe { NSButton::buttonWithImage_target_action(&img, Some(target), Some(sel!(showHelp:)), mtm()) };
            info.setBordered(false);
            info.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
            // Der Knopf zeigt seinen Tooltip auf Klick als Popover.
            info.setToolTip(Some(&ns(help)));
            row.addArrangedSubview(&info);
        }
        stack.addArrangedSubview(&row);
    };

    // Abwesenheit
    section(
        "Abwesenheit",
        None,
        "Urlaub, Krankheit und Feiertag senken das Soll des Tages, ein halber Tag auf die Hälfte; \
         Sonderurlaub gilt immer ganztags. Zeitausgleich lässt das Soll stehen, auch ohne \
         Arbeitszeit – das Zeitkonto sinkt entsprechend. \
         Ganztags geht eine Abwesenheit nur an einem Tag ohne andere Einträge: Die löscht man \
         vorher mit „Alle Einträge löschen …“ unten; ein halber Tag geht auch neben Arbeitszeit – \
         dafür „Halber Tag“ wählen, auch vor der Art. An freien Tagen (Wochenende, abgewählte \
         Arbeitstage) gibt es kein Soll und deshalb keine Abwesenheit.",
    );
    let absence = day.as_ref().and_then(|d| d.absence);
    // Ganztags nur an einem Tag ohne andere Einträge (siehe `Day::accepts_absence`; heute
    // zählen nur die von Hand).
    let busy = day.as_ref().is_some_and(|d| d.blocks_full_absence(is_today));
    // Ganztägige Abwesenheit: Die übrigen Einträge sind gesperrt (siehe `Day::locks_entries`).
    let locked = day.as_ref().is_some_and(Day::locks_entries);
    // Ohne Soll keine neue Abwesenheit (wie in der Wochenansicht); eine vorhandene lässt sich
    // nur entfernen.
    let workday = settings.is_workday(date);
    let blocked = "Ganztags erst, wenn der Tag keine anderen Einträge hat – siehe „Alle Einträge löschen …“ unten.";
    let absence_error = inspector_error_label();
    stack.addArrangedSubview(&absence_error);
    pin_width(&absence_error, stack, 40.0);
    let absence_kind = NSPopUpButton::new(mtm());
    absence_kind.setAutoenablesItems(false);
    absence_kind.addItemWithTitle(&ns("Keine"));
    for kind in AbsenceKind::ALL {
        absence_kind.addItemWithTitle(&ns(kind.label()));
        let current = absence.is_some_and(|a| a.kind == kind);
        let Some(item) = absence_kind.lastItem() else { continue };
        if !workday && !current {
            item.setEnabled(false);
            item.setToolTip(Some(&ns(FREE_DAY_ABSENCE)));
        } else if busy && !kind.allows_half() && !current {
            // Was es nur ganztags gibt (Sonderurlaub, Zeitausgleich), ist neben Einträgen gesperrt.
            item.setEnabled(false);
            item.setToolTip(Some(&ns(blocked)));
        }
    }
    let selected = absence.and_then(|a| AbsenceKind::ALL.iter().position(|k| *k == a.kind)).map_or(0, |i| i + 1);
    absence_kind.selectItemAtIndex(selected as isize);
    if !workday {
        absence_kind.setToolTip(Some(&ns(FREE_DAY_ABSENCE)));
        // Ohne Abwesenheit gibt es nichts zu wählen.
        absence_kind.setEnabled(absence.is_some());
    }
    let absence_part = NSPopUpButton::new(mtm());
    absence_part.setTag(ABSENCE_PART_TAG);
    absence_part.setAutoenablesItems(false);
    absence_part.addItemWithTitle(&ns("Ganzer Tag"));
    absence_part.addItemWithTitle(&ns("Halber Tag"));
    if busy && absence.is_none_or(|a| a.half) {
        if let Some(item) = absence_part.itemAtIndex(0) {
            item.setEnabled(false);
            item.setToolTip(Some(&ns(blocked)));
        }
    }
    // Ohne Abwesenheit gilt die Wahl für die Art, die als Nächstes gewählt wird – so geht ein
    // halber Tag direkt, ohne Umweg über „ganztags“ (der heute die erfasste Zeit löschte).
    let half = match absence {
        Some(a) => a.half,
        None => busy || model(|m| m.absence_half.filter(|(d, _)| *d == date).is_some_and(|(_, h)| h)),
    };
    absence_part.selectItemAtIndex(half as isize);
    // Halbe Tage gibt es bei Urlaub, Krankheit und Feiertag.
    absence_part.setEnabled(match absence {
        Some(a) => a.kind.allows_half() && workday,
        None => workday,
    });
    absence_part.setToolTip(Some(&ns(if absence.is_none() {
        "Ganzer oder halber Tag – gilt für die Art, die danach gewählt wird. Ein halber Tag geht auch neben \
         Arbeitszeit."
    } else {
        "Halbe Tage gibt es bei Urlaub, Krankheit und Feiertag; ein halber Tag geht auch neben Arbeitszeit."
    })));
    for popup in [&absence_kind, &absence_part] {
        unsafe {
            popup.setTarget(Some(target));
            popup.setAction(Some(sel!(absenceChanged:)));
        }
    }
    let row = hstack(8.0);
    row.addArrangedSubview(&absence_kind);
    row.addArrangedSubview(&absence_part);
    stack.addArrangedSubview(&row);
    // Darunter nur, was diese Abwesenheit an diesem Tag bedeutet (ohne Abwesenheit: nichts).
    let full = settings.daily_target(date);
    let effect = match absence {
        Some(_) if !workday || full == 0 => Some(
            "An diesem Tag gibt es kein Soll – die Abwesenheit ändert daran nichts. Entfernen lässt sie sich \
             mit „Keine“."
                .to_string(),
        ),
        None if !workday => Some("Freier Tag ohne Soll – hier gibt es keine Abwesenheit einzutragen.".to_string()),
        Some(a) if !a.kind.reduces_target() => Some(format!(
            "Das Soll von {} bleibt, auch ohne Arbeitszeit – das Zeitkonto sinkt entsprechend.",
            store::fmt_hm(full)
        )),
        Some(a) => Some(format!(
            "Das Soll sinkt von {} auf {}.",
            store::fmt_hm(full),
            store::fmt_hm(settings.day_target(date, Some(a)))
        )),
        None => None,
    };
    // Halber Tag neben Einträgen: warum „Ganzer Tag“ gesperrt ist. Ganzer Tag: warum der Rest
    // gesperrt ist.
    let effect = match effect {
        Some(text) if busy && absence.is_some_and(|a| a.half) => Some(format!("{text} {blocked}")),
        Some(text) if locked => Some(format!(
            "{text} Arbeitsbeginn, Pausen, Arbeit ohne Mac und Feierabend lassen sich erst ändern, \
             wenn die Abwesenheit auf „Keine“ steht."
        )),
        effect => effect,
    };
    let mut absence_last: Retained<NSView> = Retained::into_super(row);
    if let Some(text) = effect {
        let note = caption(&text);
        stack.addArrangedSubview(&note);
        pin_width(&note, stack, 40.0);
        absence_last = Retained::into_super(Retained::into_super(note));
    }

    // Erfassung – nur an einem freien Tag mit Entscheidung zur automatischen Erfassung (wie
    // „Heute erfassen“ im Menü und „Erfasste Zeit zählen“ in der Wochenansicht).
    let uncounted = day.as_ref().is_some_and(Day::uncounted);
    if let Some(free) = day.as_ref().and_then(|d| d.free_day).filter(|_| !locked) {
        // „Zählen“ heißt hier: als Arbeitszeit. Ob sie in Saldo und Zeitkonto eingeht,
        // bestimmen die Arbeitszeitregeln.
        let balance = if settings.work_counts_on(date) {
            "In Saldo und Zeitkonto geht sie an diesem Tag ein – der Stand der Arbeitszeitregeln sieht \
             „Arbeit an freien Tagen in Saldo und Zeitkonto einrechnen“ vor."
        } else {
            "In Saldo und Zeitkonto geht sie an diesem Tag trotzdem nicht ein: Das geschieht nur, wenn im \
             Stand der Arbeitszeitregeln „Arbeit an freien Tagen in Saldo und Zeitkonto einrechnen“ \
             eingeschaltet ist."
        };
        let help = format!(
            "An freien Tagen (Wochenende, abgewählte Arbeitstage) zählt die automatisch erfasste Zeit \
             erst als Arbeitszeit, wenn sie gezählt werden soll – im Menü „Heute erfassen“. Bis dahin \
             lassen sich Arbeitsbeginn, Pausen und Feierabend nicht festlegen; Arbeit ohne Mac zählt \
             immer. {balance}"
        );
        section("Erfassung", Some(&absence_last), &help);
        let check = checkbox("Erfasste Zeit zählen", free.counts(), sel!(countingToggled:), target);
        let tip = format!("Mit Haken zählt die automatisch erfasste Zeit als Arbeitszeit. {balance}");
        check.setToolTip(Some(&ns(&tip)));
        stack.addArrangedSubview(&check);
        let text = match free {
            FreeDay::Counted if settings.work_counts_on(date) => {
                "Freier Tag: Die automatisch erfasste Zeit zählt als Arbeitszeit, auch in Saldo und Zeitkonto."
            }
            FreeDay::Counted => {
                "Freier Tag: Die automatisch erfasste Zeit zählt als Arbeitszeit – in Saldo und Zeitkonto \
                 geht sie nach den Arbeitszeitregeln nicht ein."
            }
            FreeDay::Stopped => {
                "Freier Tag: Alle Einträge wurden gelöscht, die automatische Erfassung ist aus. Mit \
                 dem Haken zählt sie wieder."
            }
            FreeDay::Uncounted => {
                "Freier Tag: Die automatisch erfasste Zeit zählt nicht. Arbeitsbeginn, Pausen und \
                 Feierabend lassen sich erst mit dem Haken festlegen; Arbeit ohne Mac zählt immer."
            }
        };
        let note = caption(text);
        stack.addArrangedSubview(&note);
        pin_width(&note, stack, 40.0);
        absence_last = Retained::into_super(Retained::into_super(note));
    }
    // Was sich nur festlegen lässt, wenn die erfasste Zeit zählt und der Tag nicht in der
    // Zukunft liegt (Entfernen geht immer).
    let settable = !locked && !uncounted && !future;
    const UNCOUNTED_NOTE: &str = "Erst wenn die erfasste Zeit zählt (siehe „Erfassung“ oben).";

    // Arbeitsbeginn
    section(
        "Arbeitsbeginn",
        Some(&absence_last),
        "Ohne Angabe beginnt der Tag mit dem ersten Entsperren. War der Mac zum Beginn gesperrt, zählt \
         die Zeit bis zum Entsperren danach mit; was davor erfasst wurde, entfällt. \
         Arbeit ohne Mac vor einem neu festgelegten Beginn entfällt – die letzte Angabe gilt.",
    );
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
    start_picker.setEnabled(manual_start.is_some() && settable);
    // Einen festgelegten Beginn kann man auch dann noch entfernen.
    start_check.setEnabled(settable || (!locked && manual_start.is_some()));
    if future {
        start_check.setToolTip(Some(&ns(FUTURE_TEXT)));
    }
    // Direkt über dem Feld, wenn ein Beginn abgelehnt wurde (z. B. nicht vor dem Feierabend).
    let start_error = inspector_error_label();
    stack.addArrangedSubview(&start_error);
    pin_width(&start_error, stack, 40.0);
    let row = hstack(8.0);
    row.addArrangedSubview(&start_check);
    row.addArrangedSubview(&start_picker);
    stack.addArrangedSubview(&row);
    // Direkt unter dem Feld, wenn der neue Beginn Arbeit ohne Mac gekürzt hat.
    let start_cut = cut_note_label(
        START_CUT,
    );
    let start_note = caption(&match auto {
        _ if future => FUTURE_TEXT.to_string(),
        _ if uncounted => UNCOUNTED_NOTE.to_string(),
        Some((a, _)) => format!(
            "Ohne Angabe gilt das erste Entsperren{first_in_frame} um {} Uhr. War der Mac zum Beginn \
             gesperrt, zählt die Zeit bis zum Entsperren danach mit.",
            fmt_clock(a)
        ),
        None => "Ohne Angabe gilt das erste Entsperren des Macs im Arbeitszeitrahmen.".to_string(),
    });
    stack.addArrangedSubview(&start_cut);
    pin_width(&start_cut, stack, 40.0);
    stack.addArrangedSubview(&start_note);
    pin_width(&start_note, stack, 40.0);

    // Pausen
    section(
        "Pausen",
        Some(&start_note),
        &format!(
            "Sperren ab {} Min. gelten automatisch als Pause, kürzere zählen zur Arbeitszeit. \
             Hier lassen sich zusätzliche Pausen eintragen.",
            settings.rules(date).min_pause / 60
        ),
    );
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
        let dur = label(&fmt_dur(shown_len(b.from, b.to)));
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
        remove.setEnabled(!locked);
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
    let break_error = inspector_error_label();
    stack.addArrangedSubview(&break_error);
    pin_width(&break_error, stack, 40.0);
    let (bf, bt) = (12 * 3600, 12 * 3600 + settings.rules(date).min_pause.max(30 * 60));
    let break_from = time_picker(date, bf, sel!(exportRange:), target);
    let break_to = time_picker(date, bt, sel!(exportRange:), target);
    // Keine Aktion beim Ändern – übernommen wird erst mit „Hinzufügen“.
    unsafe {
        break_from.setAction(None);
        break_to.setAction(None);
    }
    let add = button("Hinzufügen", sel!(addBreak:), target);
    for c in [&*break_from as &NSControl, &break_to, &add] {
        c.setEnabled(settable);
    }
    if future {
        add.setToolTip(Some(&ns(FUTURE_TEXT)));
    } else if uncounted {
        add.setToolTip(Some(&ns(UNCOUNTED_NOTE)));
    }
    let row = hstack(6.0);
    row.addArrangedSubview(&break_from);
    row.addArrangedSubview(&label("bis"));
    row.addArrangedSubview(&break_to);
    row.addArrangedSubview(&add);
    stack.addArrangedSubview(&row);

    // Arbeit ohne Mac
    let row_view: Retained<NSView> = Retained::into_super(row);
    section(
        "Arbeit ohne Mac",
        Some(&row_view),
        "Telefonat, Meeting oder Termin bei gesperrtem Mac: Diese Zeiten zählen als Arbeitszeit. \
         Eine Pause in dieser Zeit entfällt.",
    );
    let work = day.as_ref().map(|d| d.work.clone()).unwrap_or_default();
    let work_list = vstack(6.0);
    fill_work_list(&work_list, &work, target, locked);
    stack.addArrangedSubview(&work_list);
    pin_width(&work_list, stack, 40.0);
    stack.setCustomSpacing_afterView(12.0, &work_list);
    let work_error = inspector_error_label();
    stack.addArrangedSubview(&work_error);
    pin_width(&work_error, stack, 40.0);
    // Vorschlag: die erste Pause des Tages, sonst die Mittagszeit.
    let (wf, wt) = day
        .as_ref()
        .and_then(|d| d.pauses(&settings).first().map(|p| (p.from, p.to)))
        .unwrap_or((12 * 3600, 12 * 3600 + 30 * 60));
    let work_from = time_picker(date, wf, sel!(exportRange:), target);
    let work_to = time_picker(date, wt, sel!(exportRange:), target);
    unsafe {
        work_from.setAction(None);
        work_to.setAction(None);
    }
    let row = hstack(6.0);
    row.addArrangedSubview(&work_from);
    row.addArrangedSubview(&label("bis"));
    row.addArrangedSubview(&work_to);
    stack.addArrangedSubview(&row);
    stack.setCustomSpacing_afterView(6.0, &row);
    let work_reason = activity_popup(Activity::Meeting);
    work_reason.setToolTip(Some(&ns("Grund")));
    let add = button("Hinzufügen", sel!(addWork:), target);
    for c in [&*work_from as &NSControl, &work_to, &work_reason, &add] {
        c.setEnabled(!locked && !future);
    }
    if future {
        add.setToolTip(Some(&ns(FUTURE_TEXT)));
    }
    let row = hstack(6.0);
    row.addArrangedSubview(&work_reason);
    row.addArrangedSubview(&add);
    stack.addArrangedSubview(&row);

    // Feierabend
    let row_view: Retained<NSView> = Retained::into_super(row);
    section(
        "Feierabend",
        Some(&row_view),
        "Ohne Angabe endet der Tag mit dem letzten Sperren. War der Mac zum Feierabend gesperrt, zählt \
         die Zeit seit dem Sperren davor mit; was danach erfasst wurde, entfällt. \
         Arbeit ohne Mac nach einem neu festgelegten Feierabend entfällt – die letzte Angabe gilt.",
    );
    let manual_end = day.as_ref().and_then(|d| d.end);
    let end_value = manual_end
        .or(rec.as_ref().map(|r| r.last_end))
        .unwrap_or(if is_today { secs_of(t.time()) } else { 17 * 3600 });
    let end_check = checkbox("Manuell festlegen", manual_end.is_some(), sel!(endToggled:), target);
    let end_picker = time_picker(date, end_value, sel!(endChanged:), target);
    end_picker.setEnabled(manual_end.is_some() && settable);
    end_check.setEnabled(settable || (!locked && manual_end.is_some()));
    if future {
        end_check.setToolTip(Some(&ns(FUTURE_TEXT)));
    }
    let end_error = inspector_error_label();
    stack.addArrangedSubview(&end_error);
    pin_width(&end_error, stack, 40.0);
    let row = hstack(8.0);
    row.addArrangedSubview(&end_check);
    row.addArrangedSubview(&end_picker);
    stack.addArrangedSubview(&row);
    let end_cut = cut_note_label(
        END_CUT,
    );
    let end_note = caption(&match auto {
        _ if future => FUTURE_TEXT.to_string(),
        _ if uncounted => UNCOUNTED_NOTE.to_string(),
        Some(_) if is_today && manual_end.is_none() => {
            "Ohne Angabe gilt das letzte Sperren. Mit Feierabend wird heute nichts mehr erfasst.".to_string()
        }
        Some((_, a)) => format!(
            "Ohne Angabe gilt das letzte Sperren{last_in_frame} um {} Uhr. War der Mac zum Feierabend \
             gesperrt, zählt die Zeit seit dem Sperren davor mit.",
            fmt_clock(a)
        ),
        None => "Ohne Angabe gilt das letzte Sperren des Macs im Arbeitszeitrahmen.".to_string(),
    });
    stack.addArrangedSubview(&end_cut);
    pin_width(&end_cut, stack, 40.0);
    stack.addArrangedSubview(&end_note);
    pin_width(&end_note, stack, 40.0);

    // Automatisch erfasst
    section(
        "Automatisch erfasst",
        Some(&end_note),
        &format!(
            "Zeiten, in denen der Mac entsperrt war. Gezählt wird nur im Arbeitszeitrahmen ({}).",
            settings.rules(date).frame_text()
        ),
    );
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
        let dur = label(&fmt_dur(shown_len(p.from, p.to)));
        dur.setFont(Some(&font_digits(NSFont::systemFontSize(), font_regular())));
        dur.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        row.addArrangedSubview(&times);
        row.addArrangedSubview(&dur);
        stack.addArrangedSubview(&row);
    }

    // Ganz unten, abgesetzt: den Tag leeren – nur mit Einträgen außer der Abwesenheit (die
    // allein entfernt „Keine“ oben).
    if day.as_ref().is_some_and(Day::has_entries) {
        if let Some(last) = stack.arrangedSubviews().lastObject() {
            stack.setCustomSpacing_afterView(18.0, &last);
        }
        let sep = separator();
        stack.addArrangedSubview(&sep);
        pin_width(&sep, stack, 40.0);
        stack.setCustomSpacing_afterView(14.0, &sep);
        let clear = button("Alle Einträge löschen …", sel!(clearDay:), target);
        clear.setHasDestructiveAction(true);
        clear.setToolTip(Some(&ns(
            "Löscht die automatisch erfassten Zeiten und alle manuellen Einträge dieses Tages.",
        )));
        stack.addArrangedSubview(&clear);
    }

    model(|m| {
        m.built = Some((date, day, settings));
        m.inspector = Some(Inspector {
            date,
            stats,
            start_picker,
            end_picker,
            break_from,
            break_to,
            work_from,
            work_to,
            work_reason,
            work_list,
            start_cut,
            end_cut,
            absence_kind,
            absence_part,
            absence_error,
            start_error,
            break_error,
            work_error,
            end_error,
        });
    });
}

/// „Alle Einträge löschen …“ (Inspektor oder Wochenansicht): Rückfrage als Sheet, dann den
/// Tag `date` leeren.
fn confirm_clear_day(date: NaiveDate) {
    let window = ui().window;
    if window.attachedSheet().is_some() {
        return;
    }
    let Some(day) = model(|m| m.day(date).cloned()) else { return };
    let alert = NSAlert::new(mtm());
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&ns(&format!("Alle Einträge vom {} löschen?", fmt_date_long(date))));
    alert.setInformativeText(&ns(&clear_day_text(&day, date == now().date())));
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
        match save(date, Edit::ClearDay, true) {
            // Erst wenn dieses Sheet geschlossen ist.
            Err(NotSaved::Failed(e)) => show_sheet_later("Löschen fehlgeschlagen".to_string(), e),
            Err(e) => report(date, e, true),
            Ok(_) => {}
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

/// Erklärung im Sheet „Alle Einträge löschen“: was an diesem Tag wegfällt.
fn clear_day_text(day: &Day, is_today: bool) -> String {
    let count = |n: usize, one: &str, many: &str| if n == 1 { format!("1 {one}") } else { format!("{n} {many}") };
    let mut parts = Vec::new();
    if !day.periods.is_empty() {
        parts.push(count(day.periods.len(), "automatisch erfasste Zeit", "automatisch erfasste Zeiten"));
    }
    if day.start.is_some() {
        parts.push("den festgelegten Arbeitsbeginn".to_string());
    }
    if !day.breaks.is_empty() {
        parts.push(count(day.breaks.len(), "eingetragene Pause", "eingetragene Pausen"));
    }
    if !day.work.is_empty() {
        parts.push(count(day.work.len(), "Eintrag Arbeit ohne Mac", "Einträge Arbeit ohne Mac"));
    }
    if day.end.is_some() {
        parts.push("den festgelegten Feierabend".to_string());
    }
    if let Some(a) = day.absence {
        parts.push(format!("die Abwesenheit „{}“", a.label()));
    }
    let list = match parts.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} und {last}", rest.join(", ")),
        None => "nichts".to_string(),
    };
    let mut text = format!("Das betrifft {list}.");
    if is_today && day.free_day.is_some() {
        text.push_str(" Die automatische Erfassung bleibt heute aus, bis im Menü „Heute erfassen“ gewählt ist.");
    } else if is_today {
        text.push_str(" Die automatische Erfassung läuft danach ab jetzt weiter.");
    }
    text.push_str(" ⌘Z nimmt das zurück.");
    text
}

/// Springt zu einem Tag (auch einem ohne Aufzeichnung).
fn go_to(date: NaiveDate) {
    // Blendet der Filter den Tag aus, wird er aufgehoben – sonst bliebe die Zeile unsichtbar.
    if model(|m| m.filter.is_active() && m.row_of(date).is_none() && m.day(date).is_some()) {
        set_filter(|f| *f = Filter::default());
    }
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

fn validate_menu_item(item: &NSMenuItem) -> bool {
    let Some(action) = item.action() else { return true };
    let mode = mode();
    let check = |on: bool| item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    if action == sel!(showDays:) {
        check(mode == Mode::Days);
    } else if action == sel!(showWeek:) {
        check(mode == Mode::Week);
    } else if action == sel!(toggleInspector:) {
        return mode == Mode::Days;
    } else if action == sel!(showRowInWeek:) {
        // Nur über einer Zeile (Rechtsklick ins Leere: ausgegraut).
        let row = ui().table.clickedRow();
        return model(|m| m.week_date_at(row)).is_some();
    } else if action == sel!(goToday:) {
        return mode == Mode::Week;
    } else if action == sel!(filterPeriod:) {
        let filter = model(|m| m.filter);
        check(usize::try_from(item.tag()).ok().and_then(|i| Period::ALL.get(i)) == Some(&filter.period));
        return mode == Mode::Days;
    } else if action == sel!(filterToggle:) {
        let filter = model(|m| m.filter);
        check(if item.tag() == FILTER_WARNINGS { filter.warnings } else { filter.corrected });
        return mode == Mode::Days;
    } else if action == sel!(filterReset:) {
        return mode == Mode::Days && model(|m| m.filter.is_active());
    } else if action == sel!(undoEdit:) || action == sel!(redoEdit:) {
        let redo = action == sel!(redoEdit:);
        // In einem Dialog oder im Fenster „Einstellungen“ gilt der Befehl dem Textfeld.
        if !records_is_key() {
            item.setTitle(&ns(&history_title(redo, None)));
            return true;
        }
        let name = model(|m| if redo { m.history.redo.last() } else { m.history.undo.last() }.map(|s| s.name));
        item.setTitle(&ns(&history_title(redo, name)));
        return name.is_some();
    }
    true
}

/// Hat das Fenster „Aufzeichnungen“ selbst die Tastatur (kein Dialog davor, kein anderes Fenster)?
fn records_is_key() -> bool {
    UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.window.isKeyWindow()))
}

/// `Widerrufen: Pause entfernen` bzw. ohne Schritt nur `Widerrufen`.
fn history_title(redo: bool, name: Option<&str>) -> String {
    let verb = if redo { "Wiederholen" } else { "Widerrufen" };
    match name {
        Some(n) => format!("{verb}: {n}"),
        None => verb.to_string(),
    }
}

/// Filter der Tabelle ändern und die Zeilen neu aufbauen.
fn set_filter(change: impl FnOnce(&mut Filter)) {
    model(|m| {
        change(&mut m.filter);
        m.build_rows();
    });
    reload_table();
    refresh_summary();
    update_filter_item();
}

/// Das Filter-Symbol ist gefüllt, solange die Tabelle nicht alle Tage zeigt.
fn update_filter_item() {
    let Some(item) = FILTER_ITEM.with(|f| f.borrow().clone()) else { return };
    let active = model(|m| m.filter.is_active());
    let (image, tip) = if active {
        ("line.3.horizontal.decrease.circle.fill", "Filter aktiv – die Tabelle zeigt nicht alle Tage")
    } else {
        ("line.3.horizontal.decrease.circle", "Tabelle auf einen Zeitraum oder bestimmte Tage einschränken")
    };
    item.setImage(symbol(image, "Filter").as_deref());
    item.setToolTip(Some(&ns(tip)));
}

/// Menü des Filters: Zeitraum (einer davon) und Merkmale (beliebig kombinierbar).
fn filter_menu(target: &Controller) -> Retained<NSMenu> {
    let mtm = mtm();
    let none = NSEventModifierFlags::empty();
    let menu = NSMenu::new(mtm);
    for (i, period) in Period::ALL.into_iter().enumerate() {
        let item = menu_item(period.label(), Some(sel!(filterPeriod:)), "", none, Some(target));
        item.setTag(i as isize);
        menu.addItem(&item);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    for (title, tag) in [("Nur Tage mit Hinweisen", FILTER_WARNINGS), ("Nur korrigierte Tage", FILTER_CORRECTED)] {
        let item = menu_item(title, Some(sel!(filterToggle:)), "", none, Some(target));
        item.setTag(tag);
        menu.addItem(&item);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item("Filter zurücksetzen", Some(sel!(filterReset:)), "", none, Some(target)));
    menu
}

/// Einen Tag in der Ansicht „Tage“ zeigen.
/// Der Tag in Bearbeitung (für die Wochenansicht).
fn selected_day() -> Option<NaiveDate> {
    MODEL.with(|m| m.try_borrow().ok().and_then(|m| m.selected))
}

/// Klick in eine Tagesspalte der Wochenansicht: Der Tag wird ausgewählt – auch in der
/// Tabelle, sodass „Tage“ ihn beim Zurückwechseln zeigt.
fn select_day(date: NaiveDate) {
    let row = model(|m| {
        m.selected = Some(date);
        m.row_of(date)
    });
    let ui = ui();
    match row {
        Some(r) => ui.table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(r), false),
        None => unsafe { ui.table.deselectAll(None) },
    }
    refresh_inspector(true);
}

fn open_day(date: NaiveDate) {
    set_mode(Mode::Days, true);
    go_to(date);
}

// ---------- Ansicht „Tage“ oder „Woche“ ----------

/// Zuletzt benutzte Ansicht aus den Benutzereinstellungen.
fn stored_mode() -> Mode {
    let n = NSUserDefaults::standardUserDefaults().integerForKey(&ns(MODE_KEY));
    if n == 1 { Mode::Week } else { Mode::Days }
}

/// Zwischen „Tage“ und „Woche“ wechseln. Die Woche zeigt die Woche des ausgewählten Tags
/// (ohne Auswahl die aktuelle). `focus` gibt der Ansicht die Tastatur.
fn set_mode(new: Mode, focus: bool) {
    let Some(ui) = UI.with(|u| u.borrow().clone()) else { return };
    let old = MODE.with(|m| m.replace(new));
    let days = new == Mode::Days;
    for v in [&*ui.cards_row as &NSView, &ui.top_sep, &ui.split] {
        v.setHidden(!days);
    }
    ui.week.setHidden(days);
    ui.switcher.setSelectedSegment(if days { 0 } else { 1 });
    if SCREENSHOT.with(|s| s.borrow().is_none()) {
        NSUserDefaults::standardUserDefaults().setInteger_forKey(if days { 0 } else { 1 }, &ns(MODE_KEY));
    }
    if !days && old != new {
        let date = model(|m| m.selected).unwrap_or_else(|| now().date());
        refresh_week(Some(date));
    }
    if focus {
        if days {
            ui.window.makeFirstResponder(Some(&ui.table));
        } else {
            week_view::focus();
        }
    }
}

/// Die Woche der Tabellenzeile `row` in der Wochenansicht zeigen. Ein Tag wird dabei auch in
/// der Tabelle ausgewählt, damit „Tage“ ihn beim Zurückwechseln zeigt.
fn show_row_in_week(row: isize) {
    let Some(date) = model(|m| m.week_date_at(row)) else { return };
    let ui = ui();
    if model(|m| m.date_at(row)).is_some() && ui.table.selectedRow() != row {
        ui.table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row as usize), false);
    }
    set_mode(Mode::Week, true);
    // Bei einer Wochenzeile ist kein Tag ausgewählt: die Woche selbst zeigen.
    refresh_week(Some(date));
}

/// Stand der Wochenansicht neu holen: von der Menüleisten-App (samt laufender Periode) oder
/// eigenständig aus der Datenbank. Mit `date` die Woche dieses Tags zeigen.
fn refresh_week(date: Option<NaiveDate>) {
    let snap = match EMBED.with(|c| c.get()) {
        Some(e) => (e.week)(),
        None => Some(model_snapshot()),
    };
    match (snap, date) {
        (Some(snap), Some(date)) => week_view::show_week_of(snap, date),
        (Some(snap), None) => week_view::update(snap),
        (None, _) => {}
    }
}

/// Eigenständig gestartet: Die Wochenansicht zeigt den Stand der Datenbank. (In der
/// Menüleisten-App aktualisiert diese die Wochenansicht selbst.)
fn week_from_model() {
    if !embedded() && UI.with(|u| u.borrow().is_some()) {
        week_view::update(model_snapshot());
    }
}

fn model_snapshot() -> week_view::Snapshot {
    model(|m| week_view::Snapshot {
        days: m.days.clone(),
        settings: m.settings.clone(),
        now: now(),
        running: matches!(m.status, Some(Status::Running | Status::Away(..))),
    })
}

/// Arbeit ohne Mac aus der Wochenansicht eintragen: eigenständig gestartet direkt in die
/// Datenbank, sonst über die Menüleisten-App (sie kennt die laufende Erfassung).
/// Sperrt eine ganztägige Abwesenheit den Tag `date` (siehe [`Day::locks_entries`])?
fn locked(date: NaiveDate) -> bool {
    model(|m| m.day(date).is_some_and(Day::locks_entries))
}

fn week_add_work(date: NaiveDate, from: i64, to: i64, activity: Activity) {
    if locked(date) {
        return;
    }
    let edit = Edit::AddWork(from, to, activity.note().to_string());
    match EMBED.with(|c| c.get()) {
        Some(e) => {
            // Dieselben Prüfungen wie beim Speichern im Fenster (siehe `save`).
            let (refused, first) = model(|m| {
                let day = m.day(date).cloned().unwrap_or_else(|| Day::new(date));
                (refusal(&day, &edit, &m.settings, now()), before_first_text(&m.days, date, &m.settings, now().date()))
            });
            let name = edit.name();
            let proceed = move || via_menubar(name, date, || (e.add_work)(date, from, to, activity));
            match (refused, first) {
                (Some(text), _) => report(date, NotSaved::Refused(None, text), true),
                (None, Some(text)) => confirm_before_first(date, text, std::rc::Rc::new(proceed)),
                (None, None) => proceed(),
            }
        }
        None => {
            if let Err(e) = save(date, edit, true) {
                report(date, e, true);
            }
        }
    }
}

/// Freier Tag aus der Wochenansicht: Die erfasste Zeit zählt (oder nicht mehr) – direkt in
/// die Datenbank wie die Abwesenheit, mit ⌘Z zu widerrufen.
fn week_set_counting(date: NaiveDate, on: bool) {
    if let Err(e) = save(date, Edit::SetCounting(on), true) {
        report(date, e, true);
    }
}

/// Abwesenheit aus der Wochenansicht: wie im Inspektor (siehe [`set_absence`]).
fn week_set_absence(date: NaiveDate, absence: Option<Absence>) {
    set_absence(date, absence);
}

/// Abwesenheit eintragen, ändern oder entfernen (Inspektor und Wochenansicht): direkt in die
/// Datenbank (die Menüleisten-App liest sie danach ein), zum Widerrufen gemerkt. Eine
/// ganztägige Abwesenheit für heute neben automatisch erfasster Zeit fragt vorher nach: Die
/// Zeit entfällt, und die Erfassung endet für heute.
fn set_absence(date: NaiveDate, absence: Option<Absence>) {
    let today = date == now().date();
    let recorded = model(|m| {
        m.day(date)
            .filter(|d| d.clears_for_absence(absence, today))
            .map(|d| d.periods.iter().map(Span::len).sum::<i64>())
    });
    let Some(recorded) = recorded else {
        if let Err(e) = save(date, Edit::SetAbsence(absence), true) {
            report(date, e, true);
        }
        return;
    };
    let window = ui().window;
    if window.attachedSheet().is_some() {
        refresh_inspector(true);
        return;
    }
    let label = absence.map(Absence::label).unwrap_or_default();
    // Bei Urlaub, Krankheit und Feiertag geht auch ein halber Tag – neben der erfassten Zeit.
    let half = absence.filter(|a| a.kind.allows_half()).map(|a| Absence::new(a.kind, true));
    let alert = NSAlert::new(mtm());
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&ns(&format!("Heute ganztägig „{label}“ eintragen?")));
    alert.setInformativeText(&ns(&absence_today_text(recorded, half.is_some())));
    let ok = alert.addButtonWithTitle(&ns("Ganztags eintragen"));
    ok.setHasDestructiveAction(true);
    // Return bestätigt nicht versehentlich: Standard ist „Abbrechen“.
    ok.setKeyEquivalent(&ns(""));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\r"));
    if half.is_some() {
        alert.addButtonWithTitle(&ns("Halben Tag eintragen"));
    }
    let handler = RcBlock::new(move |response: NSModalResponse| {
        let chosen = if response == NSAlertFirstButtonReturn {
            absence
        } else if response == NSAlertThirdButtonReturn && half.is_some() {
            half
        } else {
            // Die Auswahl im Inspektor wieder auf den gespeicherten Stand.
            refresh_inspector(true);
            return;
        };
        if let Err(e) = save(date, Edit::SetAbsence(chosen), true) {
            report(date, e, true);
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

/// Erklärung in der Rückfrage zu einer ganztägigen Abwesenheit für heute (`recorded`: heute
/// erfasste Zeit in Sekunden; `half`: Es gibt sie auch als halben Tag).
fn absence_today_text(recorded: i64, half: bool) -> String {
    let mut text = format!(
        "Die heute automatisch erfasste Zeit ({}) wird gelöscht, und die Erfassung endet für heute – \
         bis die Abwesenheit wieder auf „Keine“ steht. ⌘Z nimmt das zurück.",
        fmt_dur(recorded)
    );
    if half {
        text.push_str(" Ein halber Tag geht auch neben der erfassten Zeit – sie bleibt dann.");
    }
    text
}

/// Arbeitsbeginn bzw. Feierabend aus der Wochenansicht (verschoben, im Dialog geändert oder
/// zurückgesetzt). Wie im Inspektor gespeichert, also auch mit ⌘Z zu widerrufen.
fn week_set_bound(date: NaiveDate, bound: week_view::Bound, at: Option<i64>) {
    let (edit, text) = match bound {
        week_view::Bound::Start => (Edit::SetStart(at), START_CUT),
        week_view::Bound::End => (Edit::SetEnd(at), END_CUT),
    };
    match save(date, edit, true) {
        Err(e) => report(date, e, true),
        // Im Inspektor steht der Hinweis unter dem Feld, in der Woche als Meldung.
        Ok(Some(_)) if mode() == Mode::Week => show_info_sheet(&ui().window, "Arbeit ohne Mac entfallen", text),
        Ok(_) => {}
    }
}

fn week_remove_work(date: NaiveDate, from: i64, to: i64) {
    if locked(date) {
        return;
    }
    let edit = Edit::CutWork(from, to);
    match EMBED.with(|c| c.get()) {
        Some(e) => via_menubar(edit.name(), date, || (e.remove_work)(date, from, to)),
        None => {
            if let Err(e) = save(date, edit, true) {
                report(date, e, true);
            }
        }
    }
}

/// Eine Änderung, die die Menüleisten-App speichert (`change`), zum Widerrufen merken. Sie
/// liest das Fenster danach neu ein – daran zeigt sich, was sich geändert hat.
fn via_menubar(name: &'static str, date: NaiveDate, change: impl FnOnce()) {
    let corrections = || model(|m| m.day(date).map(Corrections::of).unwrap_or_default());
    let before = corrections();
    change();
    let after = corrections();
    if after != before {
        model(|m| m.history.record(Step { name, date, before, after }, false));
    }
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
    alert.setInformativeText(&ns("Wähle einen Tag, um Arbeitsbeginn, Pausen, Arbeit ohne Mac oder Feierabend nachzutragen."));
    alert.addButtonWithTitle(&ns("Öffnen"));
    alert.addButtonWithTitle(&ns("Abbrechen"));
    alert.setAccessoryView(Some(&picker));
    let p = picker.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response == NSAlertFirstButtonReturn {
            if let Some(d) = from_nsdate(&p.dateValue()) {
                open_day(d.date().min(now().date()));
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

/// Gewähltes Format im Sichern-Dialog.
fn export_format(ex: &Export) -> store::ExportFormat {
    usize::try_from(ex.format.indexOfSelectedItem())
        .ok()
        .and_then(|i| store::ExportFormat::ALL.get(i).copied())
        .unwrap_or(store::ExportFormat::Markdown)
}

/// Zeitraum oder Format geändert: Zusammenfassung und Dateiname anpassen.
fn export_range_changed(custom: bool) {
    model(|m| {
        let Some(ex) = m.export.as_ref() else { return };
        if custom {
            ex.preset.selectItemWithTag(PRESET_CUSTOM);
        }
        let Some((from, to)) = export_range(ex) else { return };
        // Dieselben Zahlen wie in der Datei.
        let report = store::Report::new(&m.days, from, to, now().date(), &m.settings);
        ex.summary.setStringValue(&ns(&export_summary(&report)));
        ex.panel.setNameFieldStringValue(&ns(&store::export_file_name(from, to, export_format(ex))));
    });
}

/// Zusammenfassung im Sichern-Dialog: Tage mit Arbeitszeit, Arbeitszeit und Saldo des Zeitraums
/// – wie die Summenzeile der Datei.
fn export_summary(report: &store::Report) -> String {
    let (work, _, _, n) = report.totals();
    format!(
        "{} · {} Arbeitszeit · Saldo {}",
        plural(n, "Arbeitstag", "Arbeitstage"),
        store::fmt_hm(work),
        rules::fmt_balance(report.balance())
    )
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
    panel.setMessage(Some(&ns("Aufzeichnungen eines Zeitraums als Markdown, CSV oder PDF sichern.")));
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
    let format = NSPopUpButton::new(mtm);
    for f in store::ExportFormat::ALL {
        format.addItemWithTitle(&ns(f.label()));
    }
    unsafe {
        format.setTarget(Some(target));
        format.setAction(Some(sel!(exportFormat:)));
    }
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
    let row4 = hstack(8.0);
    row4.addArrangedSubview(&label_col("Format:"));
    row4.addArrangedSubview(&format);
    let acc = vstack(8.0);
    acc.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 20.0, bottom: 12.0, right: 20.0 });
    acc.addArrangedSubview(&row1);
    acc.addArrangedSubview(&row2);
    acc.addArrangedSubview(&row3);
    acc.addArrangedSubview(&row4);
    acc.setTranslatesAutoresizingMaskIntoConstraints(true);
    let size = acc.fittingSize();
    acc.setFrameSize(NSSize::new(size.width.max(420.0), size.height));
    panel.setAccessoryView(Some(&acc));

    model(|m| {
        m.export = Some(Export { panel: panel.clone(), preset, from: from_p, to: to_p, summary, format });
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
    let format = export_format(&ex);
    let loaded = model(|m| -> Result<(Vec<Day>, Settings), String> {
        let db = m.db.as_ref().ok_or("Keine Datenbank geöffnet.")?;
        // Alle Tage: der Vortag für die Ruhezeit am ersten Tag, der erste erfasste Tag für das
        // Soll der Arbeitstage ohne Eintrag.
        let days = db.load().map_err(|e| format!("Lesen fehlgeschlagen: {e}"))?;
        Ok((days, m.settings.clone()))
    });
    // Außerhalb des Modells: Das PDF entsteht über das Drucksystem.
    let result = loaded.and_then(|(days, settings)| {
        store::write_export(&days, from, to, now(), &settings, format, &path)
            .map_err(|e| format!("Schreiben fehlgeschlagen: {e}"))
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

// ---------- Import ----------

/// „Importieren …“: einen Markdown-Export wählen, nachfragen und die Tage übernehmen.
fn show_import() {
    let ui = ui();
    if ui.window.attachedSheet().is_some() {
        return;
    }
    let panel = NSOpenPanel::openPanel(mtm());
    panel.setTitle(Some(&ns("Importieren")));
    panel.setMessage(Some(&ns("Einen Markdown-Export von Tagwerk wählen (CSV und PDF enthalten nicht alle Daten).")));
    panel.setPrompt(Some(&ns("Öffnen")));
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    let p = panel.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSModalResponseOK {
            return;
        }
        p.orderOut(None);
        if let Some(path) = p.URL().and_then(|u| u.path()).map(|s| PathBuf::from(s.to_string())) {
            confirm_import(path);
        }
    });
    activate();
    panel.beginSheetModalForWindow_completionHandler(&ui.window, &handler);
}

/// Gelesene Datei zeigen und vor dem Ersetzen nachfragen.
fn confirm_import(path: PathBuf) {
    let window = ui().window;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Unzulässige Tage (Zeiten in der Zukunft, Beginn nicht vor Feierabend, ganztägige
    // Abwesenheit neben Einträgen) werden übersprungen.
    let import = match std::fs::read_to_string(&path) {
        Ok(text) => store::check_import(store::parse_file(&text), now()),
        Err(e) => {
            show_info_sheet(&window, "Import fehlgeschlagen", &format!("„{name}“ konnte nicht gelesen werden: {e}"));
            return;
        }
    };
    let skipped = import.skipped_text();
    let days = import.days;
    let (Some(first), Some(last)) = (days.first().map(|d| d.date), days.last().map(|d| d.date)) else {
        if skipped.is_empty() {
            show_info_sheet(
                &window,
                "Keine Aufzeichnungen gefunden",
                &format!("„{name}“ enthält keine Aufzeichnungen. Einlesen lässt sich ein Markdown-Export von Tagwerk."),
            );
        } else {
            show_info_sheet(
                &window,
                "Nichts zu importieren",
                &format!("Kein Tag aus „{name}“ lässt sich übernehmen. {skipped}"),
            );
        }
        return;
    };
    let existing = model(|m| days.iter().filter(|d| m.day(d.date).is_some()).count());
    let alert = NSAlert::new(mtm());
    alert.setMessageText(&ns(&format!(
        "{} vom {} bis {} importieren?",
        plural(days.len(), "Tag", "Tage"),
        store::fmt_date_short(first),
        store::fmt_date_short(last)
    )));
    let replaced = match existing {
        0 => "Für diese Tage gibt es noch keine Einträge.".to_string(),
        n => format!(
            "Für {} davon gibt es schon Einträge – sie werden durch die aus der Datei ersetzt.",
            plural(n, "Tag", "Tage")
        ),
    };
    let skipped = if skipped.is_empty() { String::new() } else { format!(" {skipped}") };
    alert.setInformativeText(&ns(&format!(
        "{replaced}{skipped} Vorher sichert Tagwerk den jetzigen Stand eigens im Ordner „backups“ – \
         damit lässt er sich zurückholen."
    )));
    alert.addButtonWithTitle(&ns("Importieren"));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSAlertFirstButtonReturn {
            return;
        }
        let result = model(|m| m.db.as_mut().map(|db| db.import_days(&days, now())));
        match result {
            Some(Ok(backup)) => {
                // Widerrufen bezöge sich auf den Stand vor dem Import.
                model(|m| m.history = History::default());
                saved(true);
                let imported = plural(days.len(), "Tag", "Tage");
                let text = match backup.as_deref().and_then(|p| p.file_name()) {
                    Some(file) => format!(
                        "{imported} übernommen. Der Stand davor ist im Ordner „backups“ als „{}“ gesichert.",
                        file.to_string_lossy()
                    ),
                    None => format!("{imported} übernommen."),
                };
                show_sheet_later("Import abgeschlossen".to_string(), text);
            }
            Some(Err(e)) => {
                let text = e.to_string();
                let block = RcBlock::new(move |_timer| show_info_sheet(&ui().window, "Import fehlgeschlagen", &text));
                unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
            }
            None => {}
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

fn reveal(path: &std::path::Path) {
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
}

// ---------- Symbolleiste ----------

fn toolbar_ids() -> Retained<NSArray<NSString>> {
    let flex: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[ns(TB_VIEW), flex, ns(TB_FILTER), ns(TB_RELOAD), ns(TB_ADD), ns(TB_EXPORT), ns(TB_INSPECTOR)])
}

/// Umschalter „Tage | Woche“ (in der Symbolleiste).
fn view_switcher(target: &Controller) -> Retained<NSSegmentedControl> {
    let labels = NSArray::from_retained_slice(&[ns("Tage"), ns("Woche")]);
    let seg = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &labels,
            NSSegmentSwitchTracking::SelectOne,
            Some(target),
            Some(sel!(switchView:)),
            mtm(),
        )
    };
    seg.setToolTip_forSegment(Some(&ns("Tabelle mit Inspektor (⌘1)")), 0);
    seg.setToolTip_forSegment(Some(&ns("Woche als Kalender (⌘2)")), 1);
    seg.setSelectedSegment(0);
    seg
}

fn toolbar_item(target: &Controller, id: &NSString) -> Option<Retained<NSToolbarItem>> {
    let mtm = mtm();
    if id.to_string() == TB_VIEW {
        let seg = SWITCHER.with(|s| s.borrow().clone())?;
        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), id);
        item.setLabel(&ns("Ansicht"));
        item.setPaletteLabel(&ns("Ansicht"));
        item.setToolTip(Some(&ns("Zwischen Tagen und Woche wechseln")));
        item.setView(Some(&seg));
        return Some(item);
    }
    if id.to_string() == TB_FILTER {
        let item = NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm), id);
        item.setLabel(&ns("Filter"));
        item.setPaletteLabel(&ns("Filter"));
        item.setMenu(&filter_menu(target));
        item.setShowsIndicator(true);
        FILTER_ITEM.with(|f| *f.borrow_mut() = Some(item.clone()));
        update_filter_item();
        return Some(Retained::into_super(item));
    }
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
            Some(menu_item("Über Tagwerk", Some(sel!(showAbout:)), "", cmd, Some(target))),
            None,
            Some(menu_item("Einstellungen …", Some(sel!(showSettings:)), ",", cmd, Some(target))),
            None,
            Some(menu_item("Tagwerk ausblenden", Some(sel!(hide:)), "h", cmd, None)),
            Some(menu_item("Andere ausblenden", Some(sel!(hideOtherApplications:)), "h", cmd | NSEventModifierFlags::Option, None)),
            Some(menu_item("Alle einblenden", Some(sel!(unhideAllApplications:)), "", cmd, None)),
            None,
            Some(menu_item("Aufzeichnungen schließen", Some(sel!(closeRecords:)), "q", cmd, Some(target))),
        ],
    );
    submenu(
        "Ablage",
        vec![
            Some(menu_item("Tag bearbeiten …", Some(sel!(addDay:)), "n", cmd, Some(target))),
            Some(menu_item("Exportieren …", Some(sel!(exportRecords:)), "e", cmd, Some(target))),
            Some(menu_item("Importieren …", Some(sel!(importRecords:)), "i", cmd | NSEventModifierFlags::Shift, Some(target))),
            None,
            Some(menu_item("Datenbank im Finder zeigen", Some(sel!(revealDatabase:)), "", cmd, Some(target))),
            None,
            Some(menu_item("Schließen", Some(sel!(performClose:)), "w", cmd, None)),
        ],
    );
    submenu(
        "Bearbeiten",
        vec![
            // Korrekturen des Fensters (siehe `step_history`); Titel und Zustand setzt
            // `validate_menu_item`.
            Some(menu_item("Widerrufen", Some(sel!(undoEdit:)), "z", cmd, Some(target))),
            Some(menu_item("Wiederholen", Some(sel!(redoEdit:)), "z", cmd | NSEventModifierFlags::Shift, Some(target))),
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
            Some(menu_item("Tage", Some(sel!(showDays:)), "1", cmd, Some(target))),
            Some(menu_item("Woche", Some(sel!(showWeek:)), "2", cmd, Some(target))),
            None,
            Some({
                let filter = menu_item("Filter", None, "", cmd, None);
                filter.setSubmenu(Some(&filter_menu(target)));
                filter
            }),
            None,
            Some(menu_item("Heute", Some(sel!(goToday:)), "t", cmd, Some(target))),
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
    // Wird es eng, kürzt die Unterzeile, statt die Karte zu sprengen.
    detail.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    detail.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
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
    // Schmaler passen die fünf Karten der Übersicht nicht mehr nebeneinander.
    window.setMinSize(NSSize::new(940.0, 480.0));
    window.setDelegate(Some(ProtocolObject::from_ref(&**controller)));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);

    let switcher = view_switcher(controller);
    SWITCHER.with(|s| *s.borrow_mut() = Some(switcher.clone()));
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
    for title in ["Heute", "Diese Woche", "Dieser Monat", "Ø pro Arbeitstag", "Zeitkonto"] {
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
            col.setHeaderToolTip(Some(&ns("Hinweise auf Regelverstöße und manuelle Korrekturen")));
        } else if id != "date" {
            col.headerCell().setAlignment(NSTextAlignment::Right);
        }
        table.addTableColumn(&col);
    }
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(&**controller)));
        table.setDelegate(Some(ProtocolObject::from_ref(&**controller)));
    }
    // Neuer Name, seit die Symbolspalte zwei Symbole fasst (davor: seit der Spalte „Saldo“):
    // Mit den alten gespeicherten Spaltenbreiten ragte die Tabelle rechts aus dem Fenster.
    // Doppelklick und Rechtsklick auf eine Zeile: in die Woche springen.
    unsafe {
        table.setTarget(Some(&**controller));
        table.setDoubleAction(Some(sel!(showRowInWeek:)));
    }
    let row_menu = NSMenu::new(mtm);
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns("In der Wochenansicht zeigen"),
            Some(sel!(showRowInWeek:)),
            &ns(""),
        )
    };
    unsafe { item.setTarget(Some(&**controller)) };
    row_menu.addItem(&item);
    unsafe { table.setMenu(Some(&row_menu)) };
    table.setAutosaveName(Some(&ns("TagwerkRecordsTable3")));
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

    // Woche (statt Übersicht und Tabelle; die Fußzeile bleibt).
    let week = week_view::container();
    week.setHidden(true);

    for v in [&*cards_row as &NSView, &top_sep, &split, &week, &footer] {
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
    c(week.topAnchor().constraintEqualToAnchor(&root.topAnchor()));
    c(week.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()));
    c(week.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()));
    c(week.bottomAnchor().constraintEqualToAnchor_constant(&root.bottomAnchor(), -footer_height));
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
        cards_row,
        top_sep,
        week,
        switcher,
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

    // Änderungen von außen (Menüleisten-App, `--import`) übernehmen, sonst regelmäßig die
    // laufenden Zahlen von heute auffrischen. Eigenständig gestartet, gibt es den
    // Zustand der Erfassung nur über die Statusdatei.
    if n.is_multiple_of(3) {
        let embedded = embedded();
        let changed = model(|m| {
            let db_changed = m.db.as_mut().is_some_and(|db| db.changed_externally());
            let status_changed = !embedded && {
                let status = store::read_status();
                std::mem::replace(&mut m.status, status) != status
            };
            db_changed || status_changed
        });
        if changed || n.is_multiple_of(30) {
            reload();
        }
    }
}

fn start_timer(controller: &Controller, interval: f64) {
    if TIMER.with(|t| t.borrow().is_some()) {
        return;
    }
    let timer = unsafe {
        NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
            interval,
            controller,
            sel!(tick:),
            None,
            true,
        )
    };
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
}

fn stop_timer() {
    if let Some(timer) = TIMER.with(|t| t.borrow_mut().take()) {
        timer.invalidate();
    }
}

fn save_screenshot(path: &std::path::Path) -> Result<(), String> {
    let ui = ui();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Das Fenster, wie es auf dem Bildschirm steht; nur wenn macOS das nicht liefert, die
    // Ansicht selbst gezeichnet (dann fehlt z. B. das gewählte Segment des Umschalters).
    if let Some(png) = snapshot::window_png(ui.window.windowNumber() as u32, false) {
        return std::fs::write(path, png).map_err(|e| e.to_string());
    }
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

/// Baut Hauptmenü, Modell und Fenster beim ersten Öffnen auf. Danach bleibt das Fenster
/// erhalten und wird beim nächsten Öffnen wieder gezeigt.
fn build(shot: bool) -> Ui {
    if let Some(ui) = UI.with(|u| u.borrow().clone()) {
        return ui;
    }
    let mtm = mtm();
    let controller = Controller::new(mtm);
    build_main_menu(&NSApplication::sharedApplication(mtm), &controller);

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
    refresh_inspector(true);
    ui
}

/// Öffnet das Fenster „Aufzeichnungen“ in der Menüleisten-App (in der zuletzt benutzten
/// Ansicht) oder holt es nach vorn. `status` ist der Zustand der Erfassung, `embed` die
/// Verbindung zur Menüleisten-App.
pub fn show(status: Status, embed: Embed) {
    EMBED.with(|c| c.set(Some(embed)));
    // Mit Fenster gehört Tagwerk ins Dock und in den App-Umschalter (⌘⇥), und die
    // Menüleiste zeigt das Hauptmenü des Fensters.
    NSApplication::sharedApplication(mtm()).setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let fresh = !OPEN.with(|o| o.replace(true));
    let ui = build(false);
    week_view::set_actions(
        Some(week_add_work),
        Some(week_remove_work),
        Some(week_set_bound),
        Some(week_set_absence),
        Some(confirm_clear_day),
        Some(week_set_counting),
    );
    week_view::set_open_day(Some(open_day));
    week_view::set_selection(Some(week_view::Selection { get: selected_day, set: select_day }));
    model(|m| m.status = Some(status));
    reload();
    let today = now().date();
    let shown = if fresh { stored_mode() } else { mode() };
    set_mode(shown, false);
    if shown == Mode::Week {
        // Neu geöffnet: aktuelle Woche; sonst bleibt die gezeigte Woche.
        if let Some(snap) = (embed.week)() {
            if fresh {
                week_view::show_week_of(snap, today);
            } else {
                week_view::update(snap);
            }
        }
    }
    ui.window.makeKeyAndOrderFront(None);
    if fresh {
        match mode() {
            Mode::Days => {
                ui.window.makeFirstResponder(Some(&ui.table));
            }
            Mode::Week => week_view::focus(),
        }
    }
    activate();
    start_timer(&ui.controller, 1.0);
}

/// Ob das Fenster gerade offen ist.
pub fn is_open() -> bool {
    OPEN.with(|o| o.get())
}

/// Neuer Zustand der Erfassung (von der Menüleisten-App).
pub fn set_status(status: Status) {
    if !is_open() {
        return;
    }
    if model(|m| m.status.replace(status) != Some(status)) {
        reload();
    }
}

/// Die Datenbank wurde anderswo geändert (z. B. im Fenster „Einstellungen“).
pub fn reload_if_open() {
    if is_open() {
        reload();
    }
}

/// Das Fenster wird geschlossen: eigenständig gestartet beenden, sonst das Dock-Symbol
/// und das Hauptmenü wieder abgeben – die Erfassung läuft weiter.
fn closed() {
    let app = NSApplication::sharedApplication(mtm());
    if !embedded() {
        app.terminate(None);
        return;
    }
    OPEN.with(|o| o.set(false));
    stop_timer();
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

/// Öffnet das Fenster „Aufzeichnungen“ ohne Menüleisten-App (`Tagwerk --records`) in der
/// zuletzt benutzten Ansicht. Mit `screenshot` wird ein Bild des Fensters gespeichert und
/// das Programm danach beendet, mit `week` in der Ansicht „Woche“ (sonst „Tage“).
pub fn run(screenshot: Option<PathBuf>, week: bool) -> Result<(), String> {
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

    // Ohne Menüleisten-App gibt es den Zustand der Erfassung nur über die Statusdatei.
    model(|m| m.status = store::read_status());
    OPEN.with(|o| o.set(true));
    let ui = build(shot);
    if shot {
        // Bildschirmfotos nur zum Ansehen.
        week_view::set_actions(None, None, None, None, None, None);
    } else {
        week_view::set_actions(
            Some(week_add_work),
            Some(week_remove_work),
            Some(week_set_bound),
            Some(week_set_absence),
            Some(confirm_clear_day),
            Some(week_set_counting),
        );
        week_view::set_open_day(Some(open_day));
    }
    // Den Tag in Bearbeitung zeigt die Woche auch auf Bildschirmfotos.
    week_view::set_selection(Some(week_view::Selection { get: selected_day, set: select_day }));
    let mode = match (shot, week) {
        (_, true) => Mode::Week,
        (true, false) => Mode::Days,
        (false, false) => stored_mode(),
    };
    set_mode(mode, false);

    // Zum Testen/für Screenshots: TAGWERK_SELECT=latest oder =TT.MM.JJJJ
    let select = std::env::var("TAGWERK_SELECT").ok().and_then(|sel| {
        if sel == "latest" {
            model(|m| m.days.last().map(|d| d.date))
        } else {
            store::parse_date(&sel)
        }
    });
    if let Some(d) = select {
        go_to(d);
        if mode == Mode::Week {
            refresh_week(Some(d));
        }
    }
    // Zum Testen: Rückmeldung „kopiert“ in der Wochenansicht (TAGWERK_FLASH=1).
    if mode == Mode::Week && std::env::var("TAGWERK_FLASH").is_ok() {
        week_view::flash_first_block();
    }
    // Zum Testen: Arbeitsbeginn des ersten Tages beim Ziehen (TAGWERK_DRAG=1).
    if mode == Mode::Week && std::env::var("TAGWERK_DRAG").is_ok() {
        week_view::drag_first_start();
    }

    if shot {
        ui.window.orderFront(None);
    } else {
        ui.window.makeKeyAndOrderFront(None);
        match mode {
            Mode::Days => {
                ui.window.makeFirstResponder(Some(&ui.table));
            }
            Mode::Week => week_view::focus(),
        }
        activate();
    }

    start_timer(&ui.controller, if shot { 0.8 } else { 1.0 });
    app.run();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    fn step(name: &'static str, d: u32, start: Option<i64>) -> Step {
        let before = Corrections { start, ..Corrections::default() };
        Step { name, date: date(d), before: before.clone(), after: before }
    }

    fn hm(h: i64, m: i64) -> i64 {
        h * 3600 + m * 60
    }

    fn at_time(d: u32, h: u32, m: u32) -> NaiveDateTime {
        date(d).and_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn footer_names_the_rules_of_the_selected_day() {
        let date = |d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        let mut s = Settings::default();
        s.set_rules(Some(date(16)), rules::WorkRules { weekly_target: 32 * 3600, ..Default::default() }, None);
        let today = date(21);
        let now = s.rules(today).summary();
        assert!(now.contains("32 Std./Woche"), "{now}");
        assert_eq!(rules_footer(&s, None, today), now);
        assert_eq!(rules_footer(&s, Some(today), today), now);
        let before = rules_footer(&s, Some(date(13)), today);
        assert_eq!(before, format!("Regeln am 13.11.2026: {}", rules::WorkRules::DEFAULT.summary()));
        assert!(before.contains("39 Std./Woche"), "{before}");
    }

    #[test]
    fn undo_takes_back_only_what_the_step_changed() {
        let mut day = Day::new(date(17));
        day.add_period(hm(8, 0), hm(12, 0));
        day.add_work(hm(7, 0), hm(8, 0), "Meeting");
        let before = Corrections::of(&day);
        // Der Schritt: Arbeitsbeginn 8:00 – die Arbeit ohne Mac davor entfällt.
        day.set_start(Some(hm(8, 0)));
        let after = Corrections::of(&day);
        // Inzwischen erfasst der Mac weiter, und im Menü wird Feierabend gemacht.
        day.add_period(hm(13, 0), hm(17, 0));
        day.end = Some(hm(17, 0));
        assert_eq!(Corrections::rewind(&mut day, &after, &before), Rewind::Done);
        assert_eq!((day.start, day.work.len()), (None, 1), "Beginn und Arbeit ohne Mac zurück");
        assert_eq!(day.end, Some(hm(17, 0)), "der Feierabend aus dem Menü bleibt");
        assert_eq!(day.periods.len(), 2, "erfasste Zeit bleibt");
        // Wiederholen: wieder der Beginn, die Arbeit ohne Mac entfällt wieder.
        assert_eq!(Corrections::rewind(&mut day, &before, &after), Rewind::Done);
        assert_eq!((day.start, day.work.len(), day.end), (Some(hm(8, 0)), 0, Some(hm(17, 0))));
    }

    #[test]
    fn undo_keeps_what_changed_since() {
        let mut day = Day::new(date(17));
        day.add_period(hm(8, 0), hm(12, 0));
        day.add_work(hm(7, 0), hm(8, 0), "Meeting");
        let before = Corrections::of(&day);
        day.set_start(Some(hm(8, 0)));
        let after = Corrections::of(&day);
        // Inzwischen neue Arbeit ohne Mac („Ohne Mac weiterarbeiten“ im Menü).
        day.add_work(hm(12, 0), hm(13, 0), "Telefonat");
        let changed = day.clone();
        let rewind = Corrections::rewind(&mut day, &after, &before);
        assert_eq!(rewind, Rewind::Partial(vec!["Arbeit ohne Mac"]));
        assert_eq!(day.start, None);
        assert_eq!(day.work, changed.work, "die neue Arbeit ohne Mac bleibt");
        let (title, text) = rewind_note(false, "Arbeitsbeginn ändern", date(17), &rewind).unwrap();
        assert_eq!(title, "Nur zum Teil widerrufen");
        assert_eq!(
            text,
            "„Arbeitsbeginn ändern“ vom 17.11.2026 ist nur zum Teil widerrufen. Inzwischen anders geändert und \
             deshalb so geblieben: Arbeit ohne Mac."
        );

        // Alles, was der Schritt geändert hat, ist inzwischen anders: gar nicht.
        let mut day = Day::new(date(17));
        day.add_period(hm(8, 0), hm(12, 0));
        let before = Corrections::of(&day);
        day.set_start(Some(hm(8, 0)));
        let after = Corrections::of(&day);
        day.start = Some(hm(7, 30));
        let changed = day.clone();
        let rewind = Corrections::rewind(&mut day, &after, &before);
        assert_eq!(rewind, Rewind::Refused(RewindBlock::Changed(vec!["Arbeitsbeginn"])));
        assert_eq!(day, changed);
        assert_eq!(
            rewind_note(true, "Arbeitsbeginn ändern", date(17), &rewind).unwrap().1,
            "„Arbeitsbeginn ändern“ vom 17.11.2026 lässt sich nicht mehr wiederholen – inzwischen anders \
             geändert: Arbeitsbeginn. Der Tag bleibt, wie er ist."
        );
        assert_eq!(rewind_note(false, "x", date(17), &Rewind::Done), None);
    }

    #[test]
    fn undo_never_leaves_an_invalid_day() {
        // Ganztägige Abwesenheit entfernt, dann wieder erfasst: Widerrufen ergäbe Urlaub neben
        // erfasster Zeit.
        let mut day = Day::new(date(17));
        day.absence = Some(Absence::new(AbsenceKind::Vacation, false));
        let before = Corrections::of(&day);
        day.absence = None;
        let after = Corrections::of(&day);
        day.add_period(hm(9, 0), hm(10, 0));
        let changed = day.clone();
        let rewind = Corrections::rewind(&mut day, &after, &before);
        assert_eq!(rewind, Rewind::Refused(RewindBlock::AbsenceBesideEntries));
        assert_eq!(day, changed);
        assert!(rewind_note(false, "Abwesenheit ändern", date(17), &rewind).unwrap().1.contains("neben inzwischen"));

        // Feierabend entfernt, inzwischen ein späterer Beginn: Der alte Feierabend läge davor.
        let mut day = Day::new(date(17));
        day.add_period(hm(8, 0), hm(15, 0));
        day.end = Some(hm(12, 0));
        let before = Corrections::of(&day);
        day.end = None;
        let after = Corrections::of(&day);
        day.start = Some(hm(13, 0));
        assert_eq!(Corrections::rewind(&mut day, &after, &before), Rewind::Refused(RewindBlock::Order));
        assert_eq!(day.end, None);
    }

    #[test]
    fn whole_day_restores_recorded_periods() {
        let mut day = Day::new(date(17));
        day.add_period(hm(8, 0), hm(12, 0));
        day.add_break(hm(10, 0), hm(10, 30));
        day.absence = Some(Absence::new(AbsenceKind::Vacation, true));
        let before = Corrections::whole(&day);
        let original = day.clone();
        let mut cleared = Day::new(date(17));
        let after = Corrections::whole(&cleared);
        assert_eq!(Corrections::rewind(&mut cleared, &after, &before), Rewind::Done);
        assert_eq!(cleared, original, "Widerrufen bringt den ganzen Tag zurück");
        // Wiederholen löscht wieder alles – auch die erfassten Zeiten.
        assert_eq!(Corrections::rewind(&mut cleared, &before, &after), Rewind::Done);
        assert!(cleared.is_empty());

        // Heute: Nach dem Löschen erfasst der Mac weiter – das bleibt beim Widerrufen.
        let mut today = Day::new(date(17));
        today.add_period(hm(13, 0), hm(14, 0));
        assert_eq!(Corrections::rewind(&mut today, &after, &before), Rewind::Done);
        assert_eq!(today.periods, vec![Span::new(hm(8, 0), hm(12, 0)), Span::new(hm(13, 0), hm(14, 0))]);
        assert_eq!(today.breaks, original.breaks);
        // Wiederholen löscht nur, was das Löschen gelöscht hatte.
        assert_eq!(Corrections::rewind(&mut today, &before, &after), Rewind::Done);
        assert_eq!(today.periods, vec![Span::new(hm(13, 0), hm(14, 0))]);
        assert!(today.breaks.is_empty() && today.absence.is_none());
    }

    #[test]
    fn refuses_future_times_and_bounds_out_of_order() {
        let s = Settings::default();
        let now = at_time(18, 10, 0);
        let mut day = Day::new(date(18));
        day.add_period(hm(8, 0), hm(10, 0));
        // Heute nichts nach jetzt.
        assert!(refusal(&day, &Edit::SetEnd(Some(hm(10, 1))), &s, now).unwrap().contains("nach jetzt (10:00 Uhr)"));
        assert!(refusal(&day, &Edit::AddWork(hm(9, 0), hm(11, 0), String::new()), &s, now).is_some());
        assert_eq!(refusal(&day, &Edit::SetEnd(Some(hm(10, 0))), &s, now), None);
        // Künftige Tage: nur die Abwesenheit (und Entfernen).
        let tomorrow = Day::new(date(19));
        for edit in [
            Edit::SetStart(Some(hm(8, 0))),
            Edit::SetEnd(Some(hm(17, 0))),
            Edit::AddBreak(hm(12, 0), hm(12, 30)),
            Edit::AddWork(hm(9, 0), hm(10, 0), String::new()),
        ] {
            assert_eq!(refusal(&tomorrow, &edit, &s, now).as_deref(), Some(FUTURE_TEXT), "{}", edit.name());
        }
        for edit in [Edit::SetAbsence(Some(Absence::new(AbsenceKind::Vacation, false))), Edit::SetStart(None), Edit::ClearDay] {
            assert_eq!(refusal(&tomorrow, &edit, &s, now), None, "{}", edit.name());
        }
        // Arbeitsbeginn vor dem Feierabend.
        let mut past = Day::new(date(17));
        past.add_period(hm(8, 0), hm(16, 0));
        past.end = Some(hm(12, 0));
        assert_eq!(
            refusal(&past, &Edit::SetStart(Some(hm(12, 0))), &s, now).as_deref(),
            Some("Der Arbeitsbeginn muss vor dem Feierabend um 12:00 Uhr liegen.")
        );
        assert_eq!(refusal(&past, &Edit::SetStart(Some(hm(11, 59))), &s, now), None);
        past.start = Some(hm(9, 0));
        assert_eq!(
            refusal(&past, &Edit::SetEnd(Some(hm(8, 30))), &s, now).as_deref(),
            Some("Der Feierabend muss nach dem Arbeitsbeginn um 09:00 Uhr liegen.")
        );
        assert_eq!(refusal(&past, &Edit::SetEnd(None), &s, now), None);
        assert_eq!(refusal(&past, &Edit::SetStart(None), &s, now), None);
    }

    #[test]
    fn no_new_absence_on_days_without_target() {
        let s = Settings::default();
        let now = at_time(23, 12, 0);
        let vacation = Some(Absence::new(AbsenceKind::Vacation, false));
        let mut sat = Day::new(date(21));
        assert_eq!(refusal(&sat, &Edit::SetAbsence(vacation), &s, now).as_deref(), Some(FREE_DAY_ABSENCE));
        // Eine vorhandene bleibt und lässt sich entfernen, aber nicht ändern.
        sat.absence = vacation;
        assert_eq!(refusal(&sat, &Edit::SetAbsence(vacation), &s, now), None);
        assert_eq!(refusal(&sat, &Edit::SetAbsence(None), &s, now), None);
        let sick = Some(Absence::new(AbsenceKind::Sick, false));
        assert!(refusal(&sat, &Edit::SetAbsence(sick), &s, now).is_some());
        // An einem Arbeitstag geht sie.
        assert_eq!(refusal(&Day::new(date(20)), &Edit::SetAbsence(vacation), &s, now), None);
    }

    #[test]
    fn asks_before_an_entry_long_before_the_first_day() {
        let s = Settings::default();
        let today = date(23);
        let mut first = Day::new(date(16));
        first.add_period(hm(8, 0), hm(16, 0));
        let days = vec![first];
        assert_eq!(before_first_text(&[], date(2), &s, today), None, "noch gar nichts erfasst");
        assert_eq!(before_first_text(&days, date(16), &s, today), None);
        assert_eq!(before_first_text(&days, date(20), &s, today), None, "nach dem ersten Tag");
        // Freitag davor: Dazwischen liegt nur das Wochenende.
        assert_eq!(before_first_text(&days, date(13), &s, today), None);
        // Montag, 2.11.: Arbeitstage vom 3. bis 13. (9 Tage à 7:48 h).
        assert_eq!(
            before_first_text(&days, date(2), &s, today).unwrap(),
            "Bisher beginnen die Aufzeichnungen am 16.11.2026. Mit einem Eintrag am 02.11.2026 zählen auch die \
             9 Arbeitstage dazwischen mit ihrem Soll – zusammen etwa 70 Std. Minus in Saldo und Zeitkonto, bis \
             dort Zeiten oder Abwesenheiten eingetragen sind."
        );
        assert!(before_first_text(&days, date(12), &s, today).unwrap().contains("zählt auch der Arbeitstag dazwischen"));
        assert!(Edit::AddBreak(0, 1).adds_entry() && !Edit::SetStart(None).adds_entry() && !Edit::SetCounting(true).adds_entry());
    }

    #[test]
    fn export_summary_counts_days_with_work_like_the_file() {
        let s = Settings::default();
        let today = date(23);
        let mut worked = Day::new(date(16));
        worked.add_period(hm(8, 0) + 20, hm(16, 0) + 40);
        let mut vacation = Day::new(date(17));
        vacation.absence = Some(Absence::new(AbsenceKind::Vacation, false));
        let days = vec![worked, vacation];
        let report = store::Report::new(&days, date(16), date(17), today, &s);
        let (work, _, _, n) = report.totals();
        assert_eq!((work, n), (hm(8, 0), 1), "ohne Sekunden, ohne den Urlaubstag");
        let balance = rules::fmt_balance(report.balance());
        assert_eq!(export_summary(&report), format!("1 Arbeitstag · 8:00 h Arbeitszeit · Saldo {balance}"));
    }

    #[test]
    fn absence_today_offers_half_day() {
        assert!(!absence_today_text(3600, false).contains("halber Tag"));
        assert!(absence_today_text(3600, true).ends_with("Ein halber Tag geht auch neben der erfassten Zeit – sie bleibt dann."));
    }

    #[test]
    fn clear_day_text_lists_entries() {
        let mut day = Day::new(date(17));
        day.add_period(8 * 3600, 12 * 3600);
        day.add_period(13 * 3600, 17 * 3600);
        assert_eq!(clear_day_text(&day, false), "Das betrifft 2 automatisch erfasste Zeiten. ⌘Z nimmt das zurück.");
        day.start = Some(7 * 3600);
        day.add_break(10 * 3600, 10 * 3600 + 1800);
        day.add_work(12 * 3600, 13 * 3600, "Meeting");
        day.absence = Some(Absence::new(AbsenceKind::Vacation, true));
        assert_eq!(
            clear_day_text(&day, true),
            "Das betrifft 2 automatisch erfasste Zeiten, den festgelegten Arbeitsbeginn, 1 eingetragene Pause, \
             1 Eintrag Arbeit ohne Mac und die Abwesenheit „Urlaub (halber Tag)“. \
             Die automatische Erfassung läuft danach ab jetzt weiter. ⌘Z nimmt das zurück."
        );
        // Heute an einem freien Tag, der nicht zählt: Die Erfassung bleibt aus.
        let mut sat = Day::new(date(21));
        sat.add_period(10 * 3600, 12 * 3600);
        sat.free_day = Some(FreeDay::Uncounted);
        assert_eq!(
            clear_day_text(&sat, true),
            "Das betrifft 1 automatisch erfasste Zeit. Die automatische Erfassung bleibt heute aus, bis im \
             Menü „Heute erfassen“ gewählt ist. ⌘Z nimmt das zurück."
        );
        // Auch mit „Heute erfassen“: Der Haken fällt mit weg.
        sat.free_day = Some(FreeDay::Counted);
        assert!(clear_day_text(&sat, true).contains("bleibt heute aus"));
        assert!(clear_day_text(&sat, false).ends_with("Das betrifft 1 automatisch erfasste Zeit. ⌘Z nimmt das zurück."));
    }

    #[test]
    fn uncounted_free_day_allows_only_what_menu_and_week_allow() {
        let s = Settings::default();
        let mut sat = Day::new(date(21));
        sat.add_period(9 * 3600 + 42 * 60, 15 * 3600);
        sat.free_day = Some(FreeDay::Uncounted);
        // Arbeitsbeginn, Feierabend und Pausen erst nach „Erfasste Zeit zählen“ …
        for edit in [Edit::SetStart(Some(9 * 3600)), Edit::SetEnd(Some(14 * 3600)), Edit::AddBreak(12 * 3600, 12 * 3600 + 1800)] {
            assert!(refuses(&sat, &edit), "{}", edit.name());
        }
        // … entfernen, Arbeit ohne Mac, zählen lassen und leeren gehen.
        for edit in [
            Edit::SetStart(None),
            Edit::SetEnd(None),
            Edit::RemoveBreak(0),
            Edit::AddWork(16 * 3600, 17 * 3600, String::new()),
            Edit::SetCounting(true),
            Edit::ClearDay,
        ] {
            assert!(!refuses(&sat, &edit), "{}", edit.name());
        }
        // Kennzahlen: kein „keine Arbeitszeit im Rahmen“, sondern warum nichts zählt.
        assert_eq!(
            day_stats(date(21), Some(&sat), &[sat.clone()], &s, date(23)),
            DayStats::Note("Freier Tag – die erfasste Zeit zählt nicht.")
        );
        // Gezählt: alles wie an jedem Tag.
        sat.free_day = Some(FreeDay::Counted);
        assert!(!refuses(&sat, &Edit::SetStart(Some(9 * 3600))));
        assert!(matches!(day_stats(date(21), Some(&sat), &[sat.clone()], &s, date(23)), DayStats::Numbers { .. }));
    }

    #[test]
    fn history_merges_follow_ups_and_forgets_redo() {
        let mut h = History::default();
        h.record(step("Arbeitsbeginn ändern", 17, None), false);
        // Pfeiltasten im Uhrzeit-Feld: derselbe Schritt.
        h.record(step("Arbeitsbeginn ändern", 17, Some(7 * 3600)), true);
        h.record(step("Arbeitsbeginn ändern", 17, Some(7 * 3600 + 600)), true);
        assert_eq!(h.undo.len(), 1);
        assert_eq!(h.undo[0].before.start, None, "der Stand vor der ganzen Eingabe");
        assert_eq!(h.undo[0].after.start, Some(7 * 3600 + 600), "der Stand nach der letzten Eingabe");
        // Ein anderer Tag oder eine andere Angabe ist ein neuer Schritt – auch als Folgeeingabe.
        h.record(step("Arbeitsbeginn ändern", 18, None), true);
        h.record(step("Feierabend ändern", 18, None), true);
        assert_eq!(h.undo.len(), 3);
        // Eine neue Änderung verwirft, was sich wiederholen ließ.
        h.redo.push(step("Pause entfernen", 17, None));
        h.record(step("Pause eintragen", 17, None), false);
        assert!(h.redo.is_empty());
        for _ in 0..History::MAX + 10 {
            h.record(step("Pause eintragen", 17, None), false);
        }
        assert_eq!(h.undo.len(), History::MAX);
        assert_eq!(history_title(false, Some("Pause entfernen")), "Widerrufen: Pause entfernen");
        assert_eq!(history_title(true, None), "Wiederholen");
    }

    #[test]
    fn account_card_names_start_and_carry() {
        let s = Settings::default();
        assert_eq!(account_detail(&s, Some(date(2))), "seit 02.11.2026");
        assert_eq!(account_detail(&s, None), "noch nichts erfasst");
        let from = Settings { account_from: Some(date(9)), account_carry: -90 * 60, ..s };
        assert_eq!(account_detail(&from, Some(date(2))), "ab 09.11.2026 · Übertrag −1:30 h");
    }

    #[test]
    fn filter_by_period_and_marks() {
        let today = date(19);
        let on = |y: i32, m: u32, d: u32| NaiveDate::from_ymd_opt(y, m, d).unwrap();
        assert!(Period::ThisMonth.contains(date(1), today) && !Period::ThisMonth.contains(on(2026, 10, 31), today));
        assert!(Period::LastMonth.contains(on(2026, 10, 31), today) && !Period::LastMonth.contains(date(1), today));
        assert!(Period::ThisYear.contains(on(2026, 1, 1), today) && !Period::ThisYear.contains(on(2025, 12, 31), today));
        assert!(Period::LastYear.contains(on(2025, 12, 31), today));
        // Januar: „Letzter Monat“ ist der Dezember des Vorjahrs.
        assert!(Period::LastMonth.contains(on(2026, 12, 15), on(2027, 1, 10)));

        let plain = Day::new(date(17));
        let mut corrected = Day::new(date(18));
        corrected.start = Some(7 * 3600);
        let warning = [Warning::Weekend { counted: false }];
        assert!(!Filter::default().is_active());
        assert!(Filter::default().shows(&plain, &[], today));
        let warnings = Filter { warnings: true, ..Filter::default() };
        assert!(warnings.is_active() && !warnings.shows(&plain, &[], today) && warnings.shows(&plain, &warning, today));
        let both = Filter { warnings: true, corrected: true, period: Period::ThisMonth };
        assert!(both.shows(&corrected, &warning, today) && !both.shows(&plain, &warning, today));
        assert!(!both.shows(&corrected, &warning, on(2026, 12, 1)), "anderer Monat");
    }
}
