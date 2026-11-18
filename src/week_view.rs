//! Ansicht „Woche“ im Fenster „Aufzeichnungen“: eine Kalenderwoche als Kalender (wie
//! Termine in Outlook oder Kalender). Jeder Tag ist eine Spalte, jeder Arbeitsabschnitt ein
//! Block von Beginn bis Ende. Gezeigt wird das Ergebnis aus automatischer Erfassung und
//! manuellen Korrekturen (Arbeitsbeginn, Feierabend, Pausen) – also genau die Abschnitte,
//! die als Arbeitszeit zählen. Arbeit außerhalb des Arbeitszeitrahmens erscheint orange als
//! „nicht gezählt“, Tage mit Regelverstößen tragen ein Warnschild im Tageskopf.
//!
//! Das Fenster „Aufzeichnungen“ bettet die Ansicht ein ([`container`]) und übergibt ihr den
//! Stand ([`update`]): in der Menüleisten-App den der Erfassung samt laufender Periode,
//! eigenständig den der Datenbank. Blättern mit ‹ › oder den Pfeiltasten, „Heute“ (⌘T)
//! springt zur aktuellen Woche. Ein Klick auf die Uhrzeit von Beginn oder Ende eines
//! Abschnitts kopiert sie, ein Klick auf einen Tageskopf mit Warnschild zeigt die Hinweise.
//! Arbeit ohne Mac (Telefonat, Meeting …) ist schraffiert; ein Klick darauf bietet an, sie
//! wieder zu löschen. Ein Klick auf einen freien Bereich öffnet einen Dialog, um Arbeit ohne
//! Mac einzutragen (von, bis, Grund): in einer Pause ist die ganze Pause vorgeschlagen, vor
//! dem Arbeitsbeginn und nach dem Feierabend eine halbe Stunde ab dem Klick.

use crate::day::{fmt_clock, secs_of, Activity, Day, Span, DAY_END};
use crate::records;
use crate::rules::{self, Settings, Warning};
use crate::store;
use block2::RcBlock;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSDictionary, NSEdgeInsets, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSTimer};
use std::cell::{Cell, RefCell};

const WEEKDAYS_SHORT: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];
const MONTHS: [&str; 12] = [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober",
    "November", "Dezember",
];

/// Höhe der Tagesköpfe, Breite der Stundenspalte, Abstände im Raster.
const HEADER_H: f64 = 46.0;
const GUTTER_W: f64 = 54.0;
const GRID_PAD_TOP: f64 = 10.0;
const GRID_PAD_BOTTOM: f64 = 10.0;
/// So lange bleibt die Rückmeldung „Kopiert“ stehen (Sekunden).
const FLASH_SECS: f64 = 1.2;

// ---------- Daten ----------

/// Stand, den das Fenster „Aufzeichnungen“ übergibt.
#[derive(Clone)]
pub struct Snapshot {
    /// Alle Tage inklusive der laufenden Periode bis `now`, nach Datum sortiert.
    pub days: Vec<Day>,
    pub settings: Settings,
    pub now: NaiveDateTime,
    /// Wird gerade erfasst?
    pub running: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Zählt als Arbeitszeit.
    Work,
    /// Außerhalb des Arbeitszeitrahmens – erfasst, aber nicht gezählt.
    Outside,
}

/// Ein Abschnitt im Kalender.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    from: i64,
    to: i64,
    kind: Kind,
    /// Läuft gerade (endet „jetzt“).
    running: bool,
    /// Welche Grenzen von manuellen Korrekturen stammen, Arbeit ohne Mac im Abschnitt.
    notes: Vec<String>,
}

/// Eine Tagesspalte.
#[derive(Clone, Debug, PartialEq)]
struct Column {
    date: NaiveDate,
    blocks: Vec<Block>,
    work_secs: i64,
    /// Pausen zwischen Beginn und Ende (wie in „Aufzeichnungen“).
    pause_secs: i64,
    /// Hinweise zu Regelverstößen.
    warnings: Vec<String>,
    /// Arbeit ohne Mac, die zählt (schraffiert, anklickbar).
    manual: Vec<Manual>,
    /// Pausen zwischen den Abschnitten (anklickbar).
    gaps: Vec<Span>,
}

/// Arbeit ohne Mac in einer Tagesspalte.
#[derive(Clone, Debug, PartialEq)]
struct Manual {
    /// Der Teil, der zählt (schraffiert).
    span: Span,
    /// Der eingetragene Block, aus dem er stammt (wird beim Löschen entfernt).
    block: Span,
    note: String,
}

/// Montag der Woche von `d`.
fn monday_of(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

/// `KW 47  ·  16. – 22. November 2026` (über Monats- und Jahresgrenzen hinweg vollständig).
fn week_title(monday: NaiveDate) -> String {
    let sunday = monday + Duration::days(6);
    let month = |d: NaiveDate| MONTHS[d.month0() as usize];
    let range = if monday.year() != sunday.year() {
        format!(
            "{}. {} {} – {}. {} {}",
            monday.day(),
            month(monday),
            monday.year(),
            sunday.day(),
            month(sunday),
            sunday.year()
        )
    } else if monday.month() != sunday.month() {
        format!("{}. {} – {}. {} {}", monday.day(), month(monday), sunday.day(), month(sunday), sunday.year())
    } else {
        format!("{}. – {}. {} {}", monday.day(), sunday.day(), month(sunday), sunday.year())
    };
    format!("KW {}  ·  {range}", monday.iso_week().week())
}

/// Volle Stunden, die das Raster zeigt: der Arbeitszeitrahmen und – falls in dieser
/// Woche außerhalb gearbeitet wurde – auch diese Zeit.
fn hour_range(s: &Settings, cols: &[Column]) -> (i64, i64) {
    let blocks = cols.iter().flat_map(|c| &c.blocks);
    let from = blocks.clone().map(|b| b.from).fold(s.frame_from, i64::min);
    let to = blocks.map(|b| b.to).fold(s.frame_to, i64::max);
    let h0 = (from / 3600).clamp(0, 23);
    let h1 = ((to + 3599) / 3600).clamp(h0 + 1, 24);
    (h0, h1)
}

/// Spalten der Woche ab `monday`.
fn columns(snap: &Snapshot, monday: NaiveDate) -> Vec<Column> {
    let s = &snap.settings;
    (0..7)
        .map(|i| {
            let date = monday + Duration::days(i);
            let Some(day) = snap.days.iter().find(|d| d.date == date) else {
                return Column {
                    date,
                    blocks: Vec::new(),
                    work_secs: 0,
                    pause_secs: 0,
                    warnings: Vec::new(),
                    manual: Vec::new(),
                    gaps: Vec::new(),
                };
            };
            let spans = day.spans(s);
            let now_secs = secs_of(snap.now.time());
            let n = spans.len();
            let mut blocks: Vec<Block> = spans
                .iter()
                .enumerate()
                .map(|(k, sp)| Block {
                    from: sp.from,
                    to: sp.to,
                    kind: Kind::Work,
                    running: snap.running && date == snap.now.date() && k + 1 == n && sp.to >= now_secs - 60,
                    notes: notes(day, s, *sp, k == 0, k + 1 == n),
                })
                .collect();
            // Wie bei den Hinweisen: Abschnitte unter einer Minute lohnen keine Anzeige.
            blocks.extend(day.outside_frame_spans(s).into_iter().filter(|x| x.len() >= 60).map(|sp| Block {
                from: sp.from,
                to: sp.to,
                kind: Kind::Outside,
                running: false,
                notes: Vec::new(),
            }));
            let (work_secs, pause_secs) = day.record(s).map(|r| (r.work_secs, r.pause_secs())).unwrap_or((0, 0));
            let warnings = rules::day_warnings(&snap.days, date, s).iter().map(Warning::text).collect();
            let manual = day
                .counted_work(s)
                .into_iter()
                .map(|(span, w)| Manual { span, block: w.span(), note: w.note.clone() })
                .collect();
            let gaps = day.pauses(s);
            Column { date, blocks, work_secs, pause_secs, warnings, manual, gaps }
        })
        .collect()
}

/// Hinweise, welche Grenzen eines Abschnitts manuell festgelegt wurden und wann darin
/// ohne Mac gearbeitet wurde.
fn notes(day: &Day, s: &Settings, sp: Span, first: bool, last: bool) -> Vec<String> {
    let mut out = Vec::new();
    if first && day.start.is_some_and(|x| x.clamp(s.frame_from, s.frame_to) == sp.from) {
        out.push("Arbeitsbeginn manuell festgelegt".to_string());
    }
    if day.breaks.iter().any(|b| b.to == sp.from) {
        out.push("beginnt nach einer manuellen Pause".to_string());
    }
    if day.breaks.iter().any(|b| b.from == sp.to) {
        out.push("endet mit einer manuellen Pause".to_string());
    }
    for (w, block) in day.counted_work(s).into_iter().filter(|(w, _)| w.from >= sp.from && w.to <= sp.to) {
        let note = if block.note.is_empty() { String::new() } else { format!(" ({})", block.note) };
        out.push(format!("ohne Mac gearbeitet {} – {}{note}", fmt_clock(w.from), fmt_clock(w.to)));
    }
    if last && day.end.is_some_and(|x| x.clamp(s.frame_from, s.frame_to) == sp.to) {
        out.push("Feierabend manuell festgelegt".to_string());
    }
    out
}

/// `Woche: 31:20 h  ·  Pausen: 3:05 h  ·  Saldo +0:08 h` (Saldo bis heute, wie in der
/// Menüleiste).
fn week_summary(snap: &Snapshot, monday: NaiveDate, cols: &[Column]) -> String {
    let total: i64 = cols.iter().map(|c| c.work_secs).sum();
    let pauses: i64 = cols.iter().map(|c| c.pause_secs).sum();
    let today = snap.now.date();
    if monday > today {
        return "Woche: –".to_string();
    }
    let head = format!("Woche: {}  ·  Pausen: {}", store::fmt_hm(total), store::fmt_hm(pauses));
    let to = (monday + Duration::days(6)).min(today);
    let (work, target) = rules::balance(&snap.days, monday, to, &snap.settings);
    if work == 0 {
        return head;
    }
    format!("{head}  ·  Saldo {}", rules::fmt_balance(work - target))
}

/// `Hinweise an 2 Tagen` (leer, wenn es keine gibt).
fn warnings_summary(cols: &[Column]) -> String {
    match cols.iter().filter(|c| !c.warnings.is_empty()).count() {
        0 => String::new(),
        1 => "Hinweise an 1 Tag".to_string(),
        n => format!("Hinweise an {n} Tagen"),
    }
}

/// `Di, 17.11.2026`
fn fmt_day(d: NaiveDate) -> String {
    format!("{}, {}", store::weekday_short(d), d.format("%d.%m.%Y"))
}

struct Data {
    snap: Option<Snapshot>,
    monday: NaiveDate,
    cols: Vec<Column>,
    hours: (i64, i64),
}

impl Data {
    fn rebuild(&mut self) {
        if let Some(snap) = &self.snap {
            self.cols = columns(snap, self.monday);
            self.hours = hour_range(&snap.settings, &self.cols);
        }
    }
}

// ---------- Geometrie ----------

struct Geo {
    w: f64,
    h: f64,
    col_w: f64,
    hour_h: f64,
    h0: i64,
    h1: i64,
}

impl Geo {
    fn new(bounds: NSRect, hours: (i64, i64)) -> Self {
        let (w, h) = (bounds.size.width, bounds.size.height);
        let n = (hours.1 - hours.0).max(1) as f64;
        Geo {
            w,
            h,
            col_w: ((w - GUTTER_W) / 7.0).max(1.0),
            hour_h: ((h - HEADER_H - GRID_PAD_TOP - GRID_PAD_BOTTOM) / n).max(1.0),
            h0: hours.0,
            h1: hours.1,
        }
    }

    fn grid_top(&self) -> f64 {
        HEADER_H + GRID_PAD_TOP
    }

    /// y-Koordinate einer Uhrzeit (Ansicht ist gespiegelt: oben = früh).
    fn y(&self, secs: i64) -> f64 {
        self.grid_top() + (secs - self.h0 * 3600) as f64 / 3600.0 * self.hour_h
    }

    /// Uhrzeit an der y-Koordinate (nur innerhalb des Rasters).
    fn secs_at(&self, y: f64) -> Option<i64> {
        let secs = self.h0 * 3600 + ((y - self.grid_top()) / self.hour_h * 3600.0).floor() as i64;
        (self.h0 * 3600..self.h1 * 3600).contains(&secs).then_some(secs)
    }

    fn x(&self, col: usize) -> f64 {
        GUTTER_W + col as f64 * self.col_w
    }

    fn block_rect(&self, col: usize, b: &Block) -> NSRect {
        let top = self.y(b.from.max(self.h0 * 3600));
        let bottom = self.y(b.to.min(self.h1 * 3600)).max(top + 3.0);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top + 0.5), NSSize::new(self.col_w - 6.0, bottom - top - 1.0))
    }

    /// Bereich einer Pause zwischen zwei Abschnitten.
    fn gap_rect(&self, col: usize, gap: Span) -> NSRect {
        let top = self.y(gap.from.max(self.h0 * 3600));
        let bottom = self.y(gap.to.min(self.h1 * 3600)).max(top);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top), NSSize::new(self.col_w - 6.0, bottom - top))
    }

    /// Bereich von Arbeit ohne Mac (innerhalb ihres Blocks).
    fn span_rect(&self, col: usize, sp: Span) -> NSRect {
        let top = self.y(sp.from.max(self.h0 * 3600));
        let bottom = self.y(sp.to.min(self.h1 * 3600)).max(top + 1.0);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top + 0.5), NSSize::new(self.col_w - 6.0, bottom - top - 1.0))
    }

    fn header_rect(&self, col: usize) -> NSRect {
        NSRect::new(NSPoint::new(self.x(col), 0.0), NSSize::new(self.col_w, HEADER_H))
    }

    fn column_at(&self, x: f64) -> Option<usize> {
        if x < GUTTER_W || x >= self.w {
            return None;
        }
        Some((((x - GUTTER_W) / self.col_w) as usize).min(6))
    }
}

fn contains(r: NSRect, p: NSPoint) -> bool {
    p.x >= r.origin.x && p.x < r.origin.x + r.size.width && p.y >= r.origin.y && p.y < r.origin.y + r.size.height
}

fn inset(r: NSRect, dx: f64, dy: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(r.origin.x + dx, r.origin.y + dy),
        NSSize::new(r.size.width - 2.0 * dx, r.size.height - 2.0 * dy),
    )
}

// ---------- Schrift und Textlayout der Blöcke ----------

type Attrs = Retained<NSDictionary<NSString, AnyObject>>;

fn attrs(font: &NSFont, color: &NSColor, align: NSTextAlignment) -> Attrs {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(align);
    style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName] };
    let values: [Retained<AnyObject>; 3] = [
        Retained::into_super(Retained::into_super(font.retain())),
        Retained::into_super(Retained::into_super(color.retain())),
        Retained::into_super(Retained::into_super(Retained::into_super(style))),
    ];
    NSDictionary::from_retained_objects(&keys, &values)
}

fn font(size: f64, weight: NSFontWeight) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight)
}

fn digits(size: f64, weight: NSFontWeight) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight)
}

fn text_width(s: &str, a: &Attrs) -> f64 {
    // Die Attribute passen zu ihren Schlüsseln.
    unsafe { ns(s).sizeWithAttributes(Some(a)) }.width.ceil()
}

/// Beginn oder Ende eines Abschnitts (anklickbar zum Kopieren).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Start,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Strong,
    Plain,
}

/// Ein Stück Text in einem Block.
struct Piece {
    text: String,
    rect: NSRect,
    style: Style,
    part: Option<Part>,
}

/// Ein Block mit Rechteck und Text, wie er gezeichnet wird.
struct BlockBox {
    col: usize,
    block: Block,
    rect: NSRect,
    pieces: Vec<Piece>,
}

struct Fonts {
    strong: Attrs,
    plain: Attrs,
}

impl Fonts {
    fn new(strong_color: &NSColor, plain_color: &NSColor) -> Self {
        let (regular, semibold) = unsafe { (NSFontWeightRegular, NSFontWeightSemibold) };
        Fonts {
            strong: attrs(&digits(11.0, semibold), strong_color, NSTextAlignment::Left),
            plain: attrs(&digits(11.0, regular), plain_color, NSTextAlignment::Left),
        }
    }

    fn get(&self, s: Style) -> &Attrs {
        match s {
            Style::Strong => &self.strong,
            Style::Plain => &self.plain,
        }
    }
}

const LINE_H: f64 = 14.0;

/// Text eines Blocks in Stücken, damit Beginn und Ende einzeln anklickbar sind.
fn block_pieces(r: NSRect, b: &Block, f: &Fonts) -> Vec<Piece> {
    let mut out = Vec::new();
    if r.size.height < 13.0 {
        return out;
    }
    let tx = r.origin.x + 7.0;
    let right = r.origin.x + r.size.width - 2.0;
    let top = r.origin.y + if r.size.height >= 18.0 { 2.0 } else { 0.0 };
    let start = fmt_clock(b.from);
    let (end, end_part) = if b.running { ("jetzt".to_string(), None) } else { (fmt_clock(b.to), Some(Part::End)) };
    let detail = match b.kind {
        Kind::Work => store::fmt_hm(records::shown_len(b.from, b.to)),
        Kind::Outside => "nicht gezählt".to_string(),
    };
    let mut push = |text: &str, x: f64, line: usize, style: Style, part: Option<Part>| -> f64 {
        let w = text_width(text, f.get(style));
        let y = top + line as f64 * LINE_H;
        let fits = x + w <= right && y + LINE_H <= r.origin.y + r.size.height + 2.0;
        if fits {
            out.push(Piece { text: text.to_string(), rect: NSRect::new(NSPoint::new(x, y), NSSize::new(w, 15.0)), style, part });
        }
        x + w
    };
    let full = format!("{start} – {end}");
    if tx + text_width(&full, &f.strong) <= right {
        // Breit: „08:12 – 11:58“, darunter die Dauer.
        let x = push(&start, tx, 0, Style::Strong, Some(Part::Start));
        let x = push(" – ", x, 0, Style::Strong, None);
        push(&end, x, 0, Style::Strong, end_part);
        if r.size.height >= 32.0 {
            push(&detail, tx, 1, Style::Plain, None);
        }
    } else {
        // Schmal: Beginn, „– Ende“ und Dauer untereinander.
        push(&start, tx, 0, Style::Strong, Some(Part::Start));
        if r.size.height >= 32.0 {
            let x = push("– ", tx, 1, Style::Strong, None);
            push(&end, x, 1, Style::Strong, end_part);
        }
        if r.size.height >= 46.0 {
            push(&detail, tx, 2, Style::Plain, None);
        }
    }
    out
}

/// Alle Blöcke der Woche mit ihrem Text.
fn block_boxes(geo: &Geo, d: &Data, f: &Fonts) -> Vec<BlockBox> {
    let mut out = Vec::new();
    for (i, c) in d.cols.iter().enumerate() {
        for b in &c.blocks {
            let rect = geo.block_rect(i, b);
            out.push(BlockBox { col: i, block: b.clone(), rect, pieces: block_pieces(rect, b, f) });
        }
    }
    out
}

/// Bereich eines anklickbaren Textstücks (etwas größer als der Text).
fn hit_rect(p: &Piece) -> NSRect {
    inset(p.rect, -3.0, -1.0)
}

// ---------- Zustand ----------

struct Ui {
    /// Wochenleiste, Trennlinie und Raster (wird ins Fenster „Aufzeichnungen“ eingebettet).
    root: Retained<NSView>,
    view: Retained<WeekView>,
    title: Retained<NSTextField>,
    warn_row: Retained<NSStackView>,
    warn_label: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    _controller: Retained<Controller>,
}

/// Gerade kopierte Uhrzeit (für die Rückmeldung im Raster).
#[derive(Clone, PartialEq)]
struct Flash {
    date: NaiveDate,
    from: i64,
    kind: Kind,
    part: Part,
    generation: u64,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static DATA: RefCell<Data> = const { RefCell::new(Data {
        snap: None,
        monday: NaiveDate::MIN,
        cols: Vec::new(),
        hours: (7, 22),
    }) };
    /// Texte der Tooltips (AppKit hält die Besitzer der Tooltip-Bereiche nicht fest).
    static TIPS: RefCell<Vec<Retained<NSString>>> = const { RefCell::new(Vec::new()) };
    static FLASH: RefCell<Option<Flash>> = const { RefCell::new(None) };
    /// Mausposition über dem Raster (für den Rahmen um den freien Bereich darunter).
    static HOVER: Cell<Option<NSPoint>> = const { Cell::new(None) };
    /// Zuletzt gezeigter Rahmen (Tag und Bereich) – neu gezeichnet wird nur bei Änderung.
    static HOVER_SPAN: Cell<Option<(NaiveDate, Span)>> = const { Cell::new(None) };
    static FLASH_GEN: Cell<u64> = const { Cell::new(0) };
    static POPOVER: RefCell<Option<Retained<NSPopover>>> = const { RefCell::new(None) };
    /// Arbeit ohne Mac eintragen (Tag, von, bis, Grund) – stellt „Aufzeichnungen“ bereit.
    static ADD_WORK: Cell<Option<AddWork>> = const { Cell::new(None) };
    /// Arbeit ohne Mac löschen (Tag, von, bis) – stellt „Aufzeichnungen“ bereit.
    static REMOVE_WORK: Cell<Option<RemoveWork>> = const { Cell::new(None) };
    /// Arbeit ohne Mac, deren Kontextmenü gerade offen ist.
    static PENDING_WORK: Cell<Option<(NaiveDate, Span)>> = const { Cell::new(None) };
}

/// Arbeit ohne Mac eintragen: Tag, von, bis, Grund.
pub type AddWork = fn(NaiveDate, i64, i64, Activity);

/// Arbeit ohne Mac löschen: Tag, von, bis (der eingetragene Block).
pub type RemoveWork = fn(NaiveDate, i64, i64);

/// Pausen unter dieser Höhe (Punkte) bekommen keine Zeigehand und keinen Tooltip.
const MIN_GAP_H: f64 = 4.0;
/// Vorschlag für Arbeit ohne Mac vor dem Arbeitsbeginn und nach dem Feierabend (Sekunden).
const FREE_PRESET: i64 = 30 * 60;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("Hauptthread")
}

// ---------- Controller und Kalenderansicht ----------

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkWeekController"]
    struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    impl Controller {
        /// ‹ bzw. › im Segment-Steuerelement.
        #[unsafe(method(navigate:))]
        fn navigate(&self, sender: Option<&NSSegmentedControl>) {
            let seg = sender.map(|s| s.selectedSegment()).unwrap_or(1);
            step(if seg == 0 { -1 } else { 1 });
        }

        #[unsafe(method(today:))]
        fn today(&self, _sender: Option<&AnyObject>) {
            go_today();
        }

        /// Eintrag im Kontextmenü von Arbeit ohne Mac: löschen.
        #[unsafe(method(removeWork:))]
        fn remove_work(&self, _sender: Option<&NSMenuItem>) {
            if let (Some((date, block)), Some(f)) = (PENDING_WORK.with(|p| p.take()), REMOVE_WORK.with(|c| c.get())) {
                f(date, block.from, block.to);
            }
        }
    }
);

impl Controller {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Was unter dem Mauszeiger liegt.
enum Hit {
    /// Uhrzeit zum Kopieren.
    Time { flash: Flash, text: String },
    /// Tageskopf mit Hinweisen.
    Warnings { date: NaiveDate, warnings: Vec<String>, rect: NSRect },
    /// Freier Bereich (auch eine Pause) mit dem Vorschlag für Arbeit ohne Mac.
    Free { date: NaiveDate, preset: Span },
    /// Arbeit ohne Mac (schraffiert).
    Work { date: NaiveDate, block: Span, note: String },
}

define_class!(
    /// Das Kalenderraster der Woche.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkWeekView"]
    struct WeekView;

    impl WeekView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// Ein Klick ins inaktive Fenster soll gleich kopieren können.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let flash = FLASH.with(|f| f.borrow().clone());
            // Rahmen aus dem aktuellen Stand neu bestimmen (die Daten können sich geändert haben).
            let hover = hover_at(self, HOVER.with(|h| h.get()));
            HOVER_SPAN.with(|h| h.set(hover));
            DATA.with(|d| {
                if let Ok(d) = d.try_borrow() {
                    draw(self.bounds(), &d, flash.as_ref(), hover);
                }
            });
        }

        /// Rahmen um den freien Bereich unter der Maus: in einer Pause die ganze Pause, sonst
        /// der vorgeschlagene Bereich von 30 Minuten.
        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            set_hover(self, Some(self.convertPoint_fromView(event.locationInWindow(), None)));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            set_hover(self, None);
        }

        #[unsafe(method(layout))]
        fn layout(&self) {
            unsafe { msg_send![super(self), layout] }
            update_tooltips(self);
        }

        /// Zeigehand über anklickbaren Uhrzeiten und Tagesköpfen mit Hinweisen.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let cursor = NSCursor::pointingHandCursor();
            for r in clickable_rects(self) {
                self.addCursorRect_cursor(r, &cursor);
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            match event.keyCode() {
                123 => step(-1), // ←
                124 => step(1),  // →
                _ => unsafe { msg_send![super(self), keyDown: event] },
            }
        }

        /// Klick auf eine Uhrzeit: kopieren. Klick auf einen Tageskopf mit Warnschild:
        /// Hinweise zeigen. Klick auf einen freien Bereich: Arbeit ohne Mac eintragen.
        /// Weitere Klicks eines Mehrfachklicks tun nichts.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if event.clickCount() != 1 {
                return;
            }
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            match hit_test(self, p) {
                Some(Hit::Time { flash, text, .. }) => copy_time(self, flash, &text),
                Some(Hit::Warnings { date, warnings, rect }) => show_warnings(self, date, &warnings, rect),
                Some(Hit::Free { date, preset }) => show_add_work(date, preset, None),
                Some(Hit::Work { date, block, note }) => show_work_menu(self, date, block, &note, p),
                None => {}
            }
        }
    }
);

impl WeekView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        // Mausbewegungen für den Rahmen um freie Bereiche (folgt der sichtbaren Fläche).
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                <NSTrackingArea as objc2::AnyThread>::alloc(),
                NSRect::ZERO,
                options,
                Some(&this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this
    }
}

/// Freier Bereich unter `p` mit dem Vorschlag für Arbeit ohne Mac (für den Rahmen).
fn hover_at(view: &WeekView, p: Option<NSPoint>) -> Option<(NaiveDate, Span)> {
    match hit_test(view, p?) {
        Some(Hit::Free { date, preset }) => Some((date, preset)),
        _ => None,
    }
}

fn set_hover(view: &WeekView, p: Option<NSPoint>) {
    HOVER.with(|h| h.set(p));
    if hover_at(view, p) != HOVER_SPAN.with(|h| h.get()) {
        view.setNeedsDisplay(true);
    }
}

fn hit_test(view: &WeekView, p: NSPoint) -> Option<Hit> {
    DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let geo = Geo::new(view.bounds(), d.hours);
        let label = NSColor::labelColor();
        let fonts = Fonts::new(&label, &label);
        for bx in block_boxes(&geo, &d, &fonts) {
            for piece in &bx.pieces {
                let Some(part) = piece.part else { continue };
                if contains(hit_rect(piece), p) {
                    let flash = Flash {
                        date: d.cols[bx.col].date,
                        from: bx.block.from,
                        kind: bx.block.kind,
                        part,
                        generation: 0,
                    };
                    return Some(Hit::Time { flash, text: piece.text.clone() });
                }
            }
        }
        let col = geo.column_at(p.x)?;
        let c = d.cols.get(col)?;
        if editable() {
            if let Some(m) = c.manual.iter().find(|m| contains(geo.span_rect(col, m.span), p)) {
                return Some(Hit::Work { date: c.date, block: m.block, note: m.note.clone() });
            }
            let snap = d.snap.as_ref()?;
            let (now, min_work) = (snap.now, snap.settings.min_work);
            if let Some(preset) = geo.secs_at(p.y).and_then(|at| free_preset(c, at, now, min_work)) {
                return Some(Hit::Free { date: c.date, preset });
            }
        }
        let rect = geo.header_rect(col);
        (!c.warnings.is_empty() && contains(rect, p))
            .then(|| Hit::Warnings { date: c.date, warnings: c.warnings.clone(), rect })
    })
}

/// Lässt sich Arbeit ohne Mac eintragen und löschen (nicht im Bildschirmfoto-Modus)?
fn editable() -> bool {
    ADD_WORK.with(|c| c.get()).is_some() && REMOVE_WORK.with(|c| c.get()).is_some()
}

/// Vorschlag für Arbeit ohne Mac bei einem Klick um `at` in die Spalte `c`: in einer Pause
/// die ganze Pause, vor dem Arbeitsbeginn und nach dem Feierabend 30 Minuten (bzw. die
/// Mindestdauer `min_work`, wenn sie länger ist) ab dem Klick, auf 5 Minuten abgerundet.
/// `None` auf einem Abschnitt und in der Zukunft.
fn free_preset(c: &Column, at: i64, now: NaiveDateTime, min_work: i64) -> Option<Span> {
    let limit = free_limit(c.date, now)?;
    if !(0..limit).contains(&at) || c.blocks.iter().any(|b| (b.from..b.to).contains(&at)) {
        return None;
    }
    if let Some(gap) = c.gaps.iter().find(|g| (g.from..g.to).contains(&at)) {
        return Some(*gap);
    }
    let from = at / 300 * 300;
    Some(Span::new(from, (from + FREE_PRESET.max(min_work)).min(limit)))
}

/// Bis zu welcher Uhrzeit sich am Tag `date` Arbeit ohne Mac eintragen lässt: heute bis
/// jetzt, an zukünftigen Tagen gar nicht.
fn free_limit(date: NaiveDate, now: NaiveDateTime) -> Option<i64> {
    match date.cmp(&now.date()) {
        std::cmp::Ordering::Less => Some(DAY_END - 60),
        std::cmp::Ordering::Equal => Some(secs_of(now.time())),
        std::cmp::Ordering::Greater => None,
    }
}

/// Freie Bereiche einer Spalte (vor dem Arbeitsbeginn, Pausen, nach dem Feierabend), in
/// denen ein Klick Arbeit ohne Mac einträgt.
fn free_spans(c: &Column, now: NaiveDateTime) -> Vec<Span> {
    let Some(limit) = free_limit(c.date, now) else { return Vec::new() };
    let mut blocks: Vec<Span> = c.blocks.iter().map(|b| Span::new(b.from, b.to)).collect();
    blocks.sort_by_key(|b| b.from);
    let mut out = Vec::new();
    let mut t = 0;
    for b in blocks {
        out.push(Span::new(t, b.from.min(limit)));
        t = t.max(b.to);
    }
    out.push(Span::new(t, limit));
    out.retain(|s| s.to > s.from);
    out
}

/// Pausen einer Spalte mit ihrem Bereich (nur ausreichend hohe).
fn gap_rects(geo: &Geo, d: &Data) -> Vec<(usize, Span, NSRect)> {
    d.cols
        .iter()
        .enumerate()
        .flat_map(|(i, c)| c.gaps.iter().map(move |g| (i, *g, geo.gap_rect(i, *g))))
        .filter(|(_, _, r)| r.size.height >= MIN_GAP_H)
        .collect()
}

/// `12:03 – 12:58 Uhr (55 Min.)`
fn fmt_span(sp: Span) -> String {
    let len = records::shown_len(sp.from, sp.to);
    let dur = if len < 3600 { format!("{} Min.", len / 60) } else { store::fmt_hm(len) };
    format!("{} – {} Uhr ({dur})", fmt_clock(sp.from), fmt_clock(sp.to))
}

/// Dialog „Arbeit ohne Mac eintragen“ als Sheet am Fenster „Aufzeichnungen“: von, bis und Grund.
/// `error` zeigt einen Hinweis zur vorigen Eingabe (dann sind die Felder schon gefüllt).
fn show_add_work(date: NaiveDate, preset: Span, error: Option<(&str, Span, Activity)>) {
    let Some(window) = UI.with(|u| u.borrow().as_ref().and_then(|u| u.view.window())) else { return };
    if window.attachedSheet().is_some() {
        return;
    }
    let mtm = mtm();
    let (span, activity) = error.map(|(_, sp, a)| (sp, a)).unwrap_or((preset, Activity::Meeting));
    let from = records::time_field(date, span.from);
    let to = records::time_field(date, span.to);
    let reason = records::activity_popup(activity);

    let label = |text: &str| NSTextField::labelWithString(&ns(text), mtm);
    let row = |views: &[&NSView]| {
        let r = NSStackView::new(mtm);
        r.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        r.setAlignment(NSLayoutAttribute::FirstBaseline);
        r.setSpacing(6.0);
        for v in views {
            r.addArrangedSubview(v);
        }
        r
    };
    let von = label("Von");
    let grund = label("Grund");
    let times = row(&[&von, &from, &label("bis"), &to]);
    let why = row(&[&grund, &reason]);
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setSpacing(10.0);
    stack.addArrangedSubview(&times);
    stack.addArrangedSubview(&why);
    // Felder bündig unter „Von“ bzw. „Grund“ (erst jetzt: beide brauchen eine gemeinsame
    // Ansicht, sonst wirft AppKit eine Ausnahme).
    grund.widthAnchor().constraintEqualToAnchor(&von.widthAnchor()).setActive(true);
    stack.setFrameSize(stack.fittingSize());

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns("Arbeit ohne Mac eintragen"));
    let info = match error {
        Some((text, ..)) => text.to_string(),
        None => format!(
            "{}: Telefonat, Meeting oder Termin bei gesperrtem Mac zählt als Arbeitszeit. \
             Eine Pause in dieser Zeit entfällt.",
            fmt_day(date)
        ),
    };
    alert.setInformativeText(&ns(&info));
    if error.is_some() {
        alert.setAlertStyle(NSAlertStyle::Warning);
    }
    alert.addButtonWithTitle(&ns("Hinzufügen"));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    alert.setAccessoryView(Some(&stack));
    alert.layout();
    alert.window().setInitialFirstResponder(Some(&from));

    let win = window.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSAlertFirstButtonReturn {
            return;
        }
        let activity = records::popup_activity(&reason);
        let (Some(f), Some(t)) = (records::picker_secs(&from), records::picker_secs(&to)) else { return };
        let min_work = DATA.with(|d| d.try_borrow().ok()?.snap.as_ref().map(|s| s.settings.min_work)).unwrap_or(0);
        let problem = if t <= f {
            Some("Das Ende muss nach dem Beginn liegen.".to_string())
        } else if records::shown_len(f, t) < min_work {
            Some(records::min_work_text(min_work))
        } else {
            None
        };
        if let Some(text) = problem {
            // Erneut fragen; erst wenn das Sheet geschlossen ist.
            let block = RcBlock::new(move |_timer| {
                show_add_work(date, preset, Some((&text, Span::new(f, t), activity)))
            });
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
            return;
        }
        // Kurze Lücken zu benachbarter Arbeit ohne Mac gehören zum neuen Abschnitt.
        let (span, note) = DATA
            .with(|d| {
                let d = d.try_borrow().ok()?;
                let snap = d.snap.as_ref()?;
                let min = snap.settings.min_pause;
                let j = snap.days.iter().find(|x| x.date == date)?.join_work_gaps(f, t, min);
                Some((j.span, j.note(min)))
            })
            .unwrap_or((Span::new(f, t), None));
        if let Some(add) = ADD_WORK.with(|c| c.get()) {
            add(date, span.from, span.to, activity);
        }
        if let Some(text) = note {
            let win = win.clone();
            let block = RcBlock::new(move |_timer| records::show_info_sheet(&win, "Lücke dazugerechnet", &text));
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

/// Kontextmenü von Arbeit ohne Mac: löschen, damit dort wieder eine Pause entsteht.
fn show_work_menu(view: &WeekView, date: NaiveDate, block: Span, note: &str, p: NSPoint) {
    let Some(controller) = UI.with(|u| u.borrow().as_ref().map(|u| u._controller.clone())) else { return };
    let mtm = mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let head = NSMenuItem::new(mtm);
    let what = if note.is_empty() { "Arbeit ohne Mac" } else { note };
    head.setTitle(&ns(&format!("{what} {}", fmt_span(block))));
    head.setEnabled(false);
    menu.addItem(&head);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &ns("Arbeit ohne Mac löschen"),
            Some(sel!(removeWork:)),
            &ns(""),
        )
    };
    unsafe { item.setTarget(Some(&controller)) };
    menu.addItem(&item);
    PENDING_WORK.with(|w| w.set(Some((date, block))));
    menu.popUpMenuPositioningItem_atLocation_inView(None, p, Some(view));
}

/// Anklickbare Bereiche (für die Zeigehand).
fn clickable_rects(view: &WeekView) -> Vec<NSRect> {
    DATA.with(|d| {
        let Ok(d) = d.try_borrow() else { return Vec::new() };
        let geo = Geo::new(view.bounds(), d.hours);
        let label = NSColor::labelColor();
        let fonts = Fonts::new(&label, &label);
        let mut out: Vec<NSRect> = block_boxes(&geo, &d, &fonts)
            .iter()
            .flat_map(|bx| bx.pieces.iter().filter(|p| p.part.is_some()).map(hit_rect))
            .collect();
        out.extend(
            d.cols.iter().enumerate().filter(|(_, c)| !c.warnings.is_empty()).map(|(i, _)| geo.header_rect(i)),
        );
        if editable() {
            // Freie Bereiche samt Pausen: Ein Klick trägt Arbeit ohne Mac ein.
            if let Some(now) = d.snap.as_ref().map(|s| s.now) {
                let geo = &geo;
                out.extend(d.cols.iter().enumerate().flat_map(|(i, c)| {
                    free_spans(c, now).into_iter().map(move |sp| geo.gap_rect(i, sp)).filter(|r| r.size.height > 0.0)
                }));
            }
            let geo = &geo;
            out.extend(
                d.cols.iter().enumerate().flat_map(|(i, c)| c.manual.iter().map(move |m| geo.span_rect(i, m.span))),
            );
        }
        out
    })
}

// ---------- Kopieren mit Rückmeldung ----------

fn copy_time(view: &WeekView, mut flash: Flash, text: &str) {
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.setString_forType(&ns(text), unsafe { NSPasteboardTypeString });
    // Spürbare Bestätigung auf dem Trackpad (wo vorhanden).
    NSHapticFeedbackManager::defaultPerformer()
        .performFeedbackPattern_performanceTime(NSHapticFeedbackPattern::Generic, NSHapticFeedbackPerformanceTime::Now);

    let generation = FLASH_GEN.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    flash.generation = generation;
    FLASH.with(|f| *f.borrow_mut() = Some(flash));
    view.setNeedsDisplay(true);

    // Rückmeldung nach kurzer Zeit wieder ausblenden (nur, wenn nicht neu kopiert wurde).
    let block = RcBlock::new(move |_timer| {
        let current = FLASH.with(|f| f.borrow().as_ref().map(|f| f.generation));
        if current == Some(generation) {
            FLASH.with(|f| *f.borrow_mut() = None);
            if let Some(v) = UI.with(|u| u.borrow().as_ref().map(|u| u.view.clone())) {
                v.setNeedsDisplay(true);
            }
        }
    });
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(FLASH_SECS, false, &block) };
}

// ---------- Hinweise im Popover ----------

/// Gefülltes Warnschild in Warnfarbe (wie in Menüleiste und „Aufzeichnungen“).
fn warning_image(point_size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &ns("exclamationmark.triangle.fill"),
        Some(&ns("Hinweis")),
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, unsafe {
        NSFontWeightRegular
    })
    .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[NSColor::whiteColor(), NSColor::systemOrangeColor()]),
    ));
    let image = image.imageWithSymbolConfiguration(&config)?;
    image.setTemplate(false);
    Some(image)
}

/// Tasse für die Pausen im Tageskopf.
fn pause_image(point_size: f64, color: &NSColor) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("cup.and.saucer"), Some(&ns("Pausen")))?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(point_size, unsafe {
        NSFontWeightRegular
    })
    .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[color.retain()]),
    ));
    let image = image.imageWithSymbolConfiguration(&config)?;
    image.setTemplate(false);
    Some(image)
}

fn show_warnings(view: &WeekView, date: NaiveDate, warnings: &[String], rect: NSRect) {
    let mtm = mtm();
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::Leading);
    stack.setSpacing(8.0);
    stack.setEdgeInsets(NSEdgeInsets { top: 14.0, left: 16.0, bottom: 14.0, right: 16.0 });
    let title = NSTextField::labelWithString(&ns(&format!("Hinweise für {}", fmt_day(date))), mtm);
    title.setFont(Some(&font(NSFont::systemFontSize(), unsafe { NSFontWeightSemibold })));
    stack.addArrangedSubview(&title);
    for w in warnings {
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::FirstBaseline);
        row.setSpacing(6.0);
        if let Some(img) = warning_image(NSFont::systemFontSize()) {
            let iv = NSImageView::imageViewWithImage(&img, mtm);
            row.addArrangedSubview(&iv);
        }
        let label = NSTextField::wrappingLabelWithString(&ns(w), mtm);
        label.setPreferredMaxLayoutWidth(300.0);
        label.setSelectable(false);
        row.addArrangedSubview(&label);
        stack.addArrangedSubview(&row);
    }
    stack.setFrameSize(stack.fittingSize());

    let controller = NSViewController::new(mtm);
    controller.setView(&stack);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setAnimates(true);
    popover.setContentViewController(Some(&controller));
    // Die Ansicht ist gespiegelt: MaxY ist unten, das Popover hängt unter dem Tageskopf.
    popover.showRelativeToRect_ofView_preferredEdge(rect, view, NSRectEdge::MaxY);
    POPOVER.with(|p| *p.borrow_mut() = Some(popover));
}

// ---------- Zeichnen ----------

/// Beschriftung für Arbeit ohne Mac: die Notiz bzw. der Grund („Sonstiges“ ohne Notiz).
fn work_label(note: &str) -> &str {
    if note.trim().is_empty() {
        Activity::Other.label()
    } else {
        note.trim()
    }
}

/// Platz für die Beschriftung mit Breite `w` in der Schraffur `r`: mittig, sonst am
/// unteren bzw. oberen Rand, sonst rechts neben den Uhrzeiten – nie über den Uhrzeiten des
/// Blocks (`taken`).
fn work_label_rect(r: NSRect, taken: &[NSRect], w: f64) -> Option<NSRect> {
    const H: f64 = 12.0;
    if w + 10.0 > r.size.width {
        return None;
    }
    let right = r.origin.x + r.size.width - 5.0;
    let beside = || {
        // Zeile mit Uhrzeit, die in der Schraffur liegt: rechtsbündig daneben.
        let line = taken
            .iter()
            .filter(|t| t.origin.y >= r.origin.y - 2.0 && t.origin.y + H <= r.origin.y + r.size.height + 2.0)
            .min_by(|a, b| a.origin.y.total_cmp(&b.origin.y))?;
        let used = taken
            .iter()
            .filter(|t| (t.origin.y - line.origin.y).abs() < 1.0)
            .map(|t| t.origin.x + t.size.width)
            .fold(f64::MIN, f64::max);
        (used + 8.0 + w <= right).then(|| NSRect::new(NSPoint::new(right - w, line.origin.y + 1.0), NSSize::new(w, H)))
    };
    if r.size.height < H {
        return beside();
    }
    let free = |c: &NSRect| {
        !taken.iter().any(|t| {
            c.origin.x < t.origin.x + t.size.width
                && t.origin.x < c.origin.x + c.size.width
                && c.origin.y < t.origin.y + t.size.height
                && t.origin.y < c.origin.y + c.size.height
        })
    };
    let pad = ((r.size.height - H) / 2.0).min(1.5);
    let (y0, y1) = (r.origin.y + pad, r.origin.y + r.size.height - H - pad);
    [r.origin.y + (r.size.height - H) / 2.0, y1, y0]
        .into_iter()
        .map(|y| NSRect::new(NSPoint::new(r.origin.x, y), NSSize::new(r.size.width, H)))
        .find(free)
        .or_else(beside)
}

fn text_in(s: &str, rect: NSRect, a: &Attrs) {
    // Die Attribute passen zu ihren Schlüsseln.
    unsafe { ns(s).drawInRect_withAttributes(rect, Some(a)) };
}

fn text_at(s: &str, p: NSPoint, a: &Attrs) {
    unsafe { ns(s).drawAtPoint_withAttributes(p, Some(a)) };
}

fn fill(rect: NSRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::fillRect(rect);
}

fn round_fill(rect: NSRect, radius: f64, color: &NSColor) {
    color.setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
}

/// Diagonale Schraffur in einem abgerundeten Rechteck.
fn hatch(rect: NSRect, color: &NSColor) {
    if rect.size.height <= 0.0 || rect.size.width <= 0.0 {
        return;
    }
    NSGraphicsContext::saveGraphicsState_class();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, 4.0, 4.0).addClip();
    color.setStroke();
    let (x0, y0) = (rect.origin.x, rect.origin.y);
    let (w, h) = (rect.size.width, rect.size.height);
    let mut off = -h;
    while off < w {
        NSBezierPath::strokeLineFromPoint_toPoint(NSPoint::new(x0 + off, y0 + h), NSPoint::new(x0 + off + h, y0));
        off += 6.0;
    }
    NSGraphicsContext::restoreGraphicsState_class();
}

fn hline(x0: f64, x1: f64, y: f64, color: &NSColor) {
    fill(NSRect::new(NSPoint::new(x0, y.floor()), NSSize::new(x1 - x0, 1.0)), color);
}

fn vline(x: f64, y0: f64, y1: f64, color: &NSColor) {
    fill(NSRect::new(NSPoint::new(x.floor(), y0), NSSize::new(1.0, y1 - y0)), color);
}

fn draw(bounds: NSRect, d: &Data, flash: Option<&Flash>, hover: Option<(NaiveDate, Span)>) {
    let geo = Geo::new(bounds, d.hours);
    let (regular, medium, semibold) = unsafe { (NSFontWeightRegular, NSFontWeightMedium, NSFontWeightSemibold) };
    let label = NSColor::labelColor();
    let secondary = NSColor::secondaryLabelColor();
    let separator = NSColor::separatorColor();
    let accent = NSColor::controlAccentColor();
    let orange = NSColor::systemOrangeColor();
    let shade = NSColor::labelColor().colorWithAlphaComponent(0.035);
    let grid_top = geo.grid_top();
    let grid_bottom = geo.y(geo.h1 * 3600);
    let today = d.snap.as_ref().map(|s| s.now.date());

    fill(bounds, &NSColor::textBackgroundColor());

    // Wochenende und Zeit außerhalb des Arbeitszeitrahmens schattiert (wird nicht gezählt).
    for (i, c) in d.cols.iter().enumerate() {
        let x = geo.x(i);
        if !Settings::is_workday(c.date) {
            fill(NSRect::new(NSPoint::new(x, HEADER_H), NSSize::new(geo.col_w, geo.h - HEADER_H)), &shade);
        } else if let Some(s) = &d.snap {
            for (a, b) in [(geo.h0 * 3600, s.settings.frame_from), (s.settings.frame_to, geo.h1 * 3600)] {
                if b > a {
                    let (ya, yb) = (geo.y(a), geo.y(b));
                    fill(NSRect::new(NSPoint::new(x, ya), NSSize::new(geo.col_w, yb - ya)), &shade);
                }
            }
        }
        if Some(c.date) == today {
            fill(geo.header_rect(i), &accent.colorWithAlphaComponent(0.05));
            let body = NSRect::new(NSPoint::new(x, HEADER_H), NSSize::new(geo.col_w, geo.h - HEADER_H));
            fill(body, &accent.colorWithAlphaComponent(0.05));
        }
        if !c.warnings.is_empty() {
            fill(geo.header_rect(i), &orange.colorWithAlphaComponent(0.08));
        }
    }

    // Stundenlinien und -beschriftung, halbe Stunden heller.
    let hour_attrs = attrs(&digits(10.0, regular), &secondary, NSTextAlignment::Right);
    // `separatorColor` ist schon halbtransparent; die halben Stunden noch dezenter.
    let half = NSColor::labelColor().colorWithAlphaComponent(0.04);
    for h in geo.h0..=geo.h1 {
        let y = geo.y(h * 3600);
        hline(GUTTER_W - 4.0, geo.w, y, &separator);
        let r = NSRect::new(NSPoint::new(0.0, y - 7.0), NSSize::new(GUTTER_W - 8.0, 14.0));
        text_in(&format!("{h:02}:00"), r, &hour_attrs);
        if h < geo.h1 && geo.hour_h >= 24.0 {
            hline(GUTTER_W, geo.w, geo.y(h * 3600 + 1800), &half);
        }
    }

    // Spaltentrenner und Tagesköpfe (mit Warnschild bei Regelverstößen).
    for i in 0..=7 {
        vline(geo.x(i), 0.0, grid_bottom + GRID_PAD_BOTTOM, &separator);
    }
    hline(0.0, geo.w, HEADER_H - 1.0, &separator);
    let icon = warning_image(11.0);
    let cup = pause_image(9.0, &secondary);
    for (i, c) in d.cols.iter().enumerate() {
        let x = geo.x(i);
        let is_today = Some(c.date) == today;
        let warn = !c.warnings.is_empty();
        let color = if is_today { &accent } else if Settings::is_workday(c.date) { &label } else { &secondary };
        let title = format!("{} {}", WEEKDAYS_SHORT[i], c.date.format("%d.%m."));
        let a = attrs(&font(12.0, if is_today { semibold } else { medium }), color, NSTextAlignment::Left);
        let tw = text_width(&title, &a);
        let icon_w = match (&icon, warn) {
            (Some(img), true) => img.size().width + 4.0,
            _ => 0.0,
        };
        let x0 = (x + (geo.col_w - tw - icon_w) / 2.0).max(x + 2.0).round();
        if let (Some(img), true) = (&icon, warn) {
            let sz = img.size();
            img.drawInRect(NSRect::new(NSPoint::new(x0, 7.0 + (15.0 - sz.height) / 2.0), sz));
        }
        let title_rect = NSRect::new(NSPoint::new(x0 + icon_w, 7.0), NSSize::new(geo.x(i + 1) - 2.0 - x0 - icon_w, 16.0));
        text_in(&title, title_rect, &a);
        let total = if c.work_secs > 0 { store::fmt_hm(c.work_secs) } else { "–".to_string() };
        let total_color = if warn { &orange } else { &secondary };
        let total_rect = NSRect::new(NSPoint::new(x + 2.0, 24.0), NSSize::new(geo.col_w - 4.0, 15.0));
        // Daneben die Pausen mit Tasse (wie „Pausen heute“ im Menü) – nur wenn es Platz hat.
        let a = attrs(&digits(11.0, regular), total_color, NSTextAlignment::Left);
        let pa = attrs(&digits(11.0, regular), &secondary, NSTextAlignment::Left);
        let pause = store::fmt_hm(c.pause_secs);
        let (tw, pw) = (text_width(&total, &a), text_width(&pause, &pa));
        let cup_w = cup.as_ref().map_or(0.0, |img| img.size().width + 3.0);
        let width = tw + 12.0 + cup_w + pw;
        if c.work_secs > 0 && width <= total_rect.size.width {
            let x0 = (x + (geo.col_w - width) / 2.0).round();
            text_in(&total, NSRect::new(NSPoint::new(x0, 24.0), NSSize::new(tw, 15.0)), &a);
            let mut px = x0 + tw + 12.0;
            if let Some(img) = &cup {
                let sz = img.size();
                img.drawInRect(NSRect::new(NSPoint::new(px, 24.0 + (14.0 - sz.height) / 2.0), sz));
                px += cup_w;
            }
            text_in(&pause, NSRect::new(NSPoint::new(px, 24.0), NSSize::new(pw, 15.0)), &pa);
        } else {
            text_in(&total, total_rect, &attrs(&digits(11.0, regular), total_color, NSTextAlignment::Center));
        }
    }

    // Abschnitte als Blöcke (wie Termine).
    let fonts = Fonts::new(&label, &secondary);
    let white = NSColor::whiteColor();
    let flash_fonts = Fonts::new(&white, &white);
    let mut bubble: Option<(NSRect, String)> = None;
    let work_attrs = attrs(&font(10.0, medium), &NSColor::secondaryLabelColor(), NSTextAlignment::Center);
    for bx in block_boxes(&geo, d, &fonts) {
        let b = &bx.block;
        let (tint, bar) = match b.kind {
            Kind::Work if b.running => (accent.colorWithAlphaComponent(0.13), &accent),
            Kind::Work => (accent.colorWithAlphaComponent(0.22), &accent),
            Kind::Outside => (orange.colorWithAlphaComponent(0.16), &orange),
        };
        round_fill(bx.rect, 4.0, &tint);
        // Arbeit ohne Mac schraffiert.
        if b.kind == Kind::Work {
            for m in d.cols[bx.col].manual.iter().filter(|m| m.span.from < b.to && m.span.to > b.from) {
                hatch(geo.span_rect(bx.col, m.span), &accent.colorWithAlphaComponent(0.35));
            }
        }
        // Farbiger Rand links, wie bei Terminen in Kalender.
        round_fill(NSRect::new(bx.rect.origin, NSSize::new(3.0, bx.rect.size.height)), 1.5, bar);
        for p in &bx.pieces {
            let flashed = p.part.is_some()
                && flash.is_some_and(|f| {
                    f.date == d.cols[bx.col].date && f.from == b.from && f.kind == b.kind && Some(f.part) == p.part
                });
            if flashed {
                let pill = hit_rect(p);
                round_fill(pill, 4.0, &accent);
                text_at(&p.text, p.rect.origin, flash_fonts.get(p.style));
                bubble = Some((pill, format!("{} kopiert", p.text)));
            } else {
                text_at(&p.text, p.rect.origin, fonts.get(p.style));
            }
        }
        // Art der Arbeit ohne Mac hell in der Schraffur, wenn Platz ist.
        if b.kind == Kind::Work {
            let taken: Vec<NSRect> = bx.pieces.iter().map(hit_rect).collect();
            for m in d.cols[bx.col].manual.iter().filter(|m| m.span.from < b.to && m.span.to > b.from) {
                let r = geo.span_rect(bx.col, m.span);
                if let Some(at) = work_label_rect(r, &taken, text_width(work_label(&m.note), &work_attrs)) {
                    text_in(work_label(&m.note), at, &work_attrs);
                }
            }
        }
    }

    // Pausen zwischen den Abschnitten: Dauer, wenn Platz ist.
    let gap_attrs = attrs(&digits(10.0, medium), &NSColor::secondaryLabelColor(), NSTextAlignment::Center);
    for (_, gap, r) in gap_rects(&geo, d) {
        if r.size.height >= 16.0 {
            let text = format!("Pause {}", store::fmt_hm(records::shown_len(gap.from, gap.to)));
            let y = r.origin.y + (r.size.height - 13.0) / 2.0;
            text_in(&text, NSRect::new(NSPoint::new(r.origin.x, y), NSSize::new(r.size.width, 13.0)), &gap_attrs);
        }
    }

    // Rahmen um den freien Bereich unter der Maus (ein Klick trägt dort Arbeit ohne Mac ein).
    if let Some((i, span)) = hover.and_then(|(date, sp)| Some((d.cols.iter().position(|c| c.date == date)?, sp))) {
        let r = inset(geo.gap_rect(i, span), 0.75, 0.75);
        if r.size.height > 0.0 {
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 4.0, 4.0);
            accent.colorWithAlphaComponent(0.08).setFill();
            path.fill();
            accent.setStroke();
            path.setLineWidth(1.5);
            path.stroke();
        }
    }

    // Aktuelle Uhrzeit als rote Linie in der Spalte von heute.
    if let (Some(snap), Some(i)) = (&d.snap, d.cols.iter().position(|c| Some(c.date) == today)) {
        let now = secs_of(snap.now.time());
        if (geo.h0 * 3600..=geo.h1 * 3600).contains(&now) {
            let red = NSColor::systemRedColor();
            let y = geo.y(now);
            let x = geo.x(i);
            fill(NSRect::new(NSPoint::new(x, y - 0.75), NSSize::new(geo.col_w, 1.5)), &red);
            red.setFill();
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(x - 4.0, y - 4.0), NSSize::new(8.0, 8.0)))
                .fill();
        }
    }

    // Leere Woche
    if d.cols.iter().all(|c| c.blocks.is_empty()) {
        let a = attrs(&font(13.0, regular), &secondary, NSTextAlignment::Center);
        let y = (grid_top + grid_bottom) / 2.0 - 9.0;
        text_in(
            "Keine Arbeitszeit in dieser Woche",
            NSRect::new(NSPoint::new(GUTTER_W, y), NSSize::new(geo.w - GUTTER_W, 18.0)),
            &a,
        );
    }

    // Rückmeldung „… kopiert“ als Sprechblase über der Uhrzeit (zuletzt, damit obenauf).
    if let Some((pill, text)) = bubble {
        let text = format!("✓ {text}");
        let a = attrs(&font(11.0, medium), &NSColor::textBackgroundColor(), NSTextAlignment::Center);
        let w = text_width(&text, &a) + 16.0;
        let h = 20.0;
        let x = (pill.origin.x + pill.size.width / 2.0 - w / 2.0).clamp(GUTTER_W + 2.0, (geo.w - w - 2.0).max(GUTTER_W + 2.0));
        let above = pill.origin.y - h - 4.0;
        let y = if above >= HEADER_H { above } else { pill.origin.y + pill.size.height + 4.0 };
        let r = NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        round_fill(r, h / 2.0, &NSColor::labelColor().colorWithAlphaComponent(0.85));
        text_in(&text, NSRect::new(NSPoint::new(x, y + 3.0), NSSize::new(w, 15.0)), &a);
    }
}

/// Tooltips für die Blöcke (Uhrzeiten, Dauer, manuelle Korrekturen) und die Tagesköpfe
/// (Arbeitszeit, Pausen, Hinweise).
fn update_tooltips(view: &WeekView) {
    view.removeAllToolTips();
    let tips: Vec<(NSRect, String)> = DATA.with(|d| {
        let Ok(d) = d.try_borrow() else { return Vec::new() };
        let geo = Geo::new(view.bounds(), d.hours);
        let mut out = Vec::new();
        for (i, c) in d.cols.iter().enumerate() {
            let mut text = String::new();
            if c.work_secs > 0 {
                text = format!(
                    "{}\nArbeitszeit {}  ·  Pausen {}",
                    fmt_day(c.date),
                    store::fmt_hm(c.work_secs),
                    store::fmt_hm(c.pause_secs)
                );
            }
            if !c.warnings.is_empty() {
                text.push_str(&if text.is_empty() { format!("Hinweise für {}:", fmt_day(c.date)) } else { "\n\nHinweise:".to_string() });
                for w in &c.warnings {
                    text.push_str(&format!("\n• {w}"));
                }
            }
            if !text.is_empty() {
                out.push((geo.header_rect(i), text));
            }
            for b in &c.blocks {
                let end = if b.running { "jetzt (läuft)".to_string() } else { format!("{} Uhr", fmt_clock(b.to)) };
                let mut text =
                    format!("{}\n{} Uhr – {end}\n{}", fmt_day(c.date), fmt_clock(b.from), store::fmt_hm(records::shown_len(b.from, b.to)));
                if b.kind == Kind::Outside {
                    text.push_str("\naußerhalb des Arbeitszeitrahmens – nicht gezählt");
                }
                for n in &b.notes {
                    text.push_str(&format!("\n{n}"));
                }
                text.push_str("\nKlick auf eine Uhrzeit kopiert sie.");
                if editable() && c.manual.iter().any(|m| m.span.from < b.to && m.span.to > b.from) {
                    text.push_str("\nKlick auf die Schraffur: Arbeit ohne Mac löschen.");
                }
                out.push((geo.block_rect(i, b), text));
            }
        }
        if editable() {
            for (i, gap, r) in gap_rects(&geo, &d) {
                let text = format!(
                    "{}\nPause {}\nKlick: als Arbeit ohne Mac eintragen (z. B. Telefonat oder Meeting)",
                    fmt_day(d.cols[i].date),
                    fmt_span(gap)
                );
                out.push((r, text));
            }
        }
        out
    });
    TIPS.with(|t| {
        let mut t = t.borrow_mut();
        t.clear();
        for (rect, text) in tips {
            let s = ns(&text);
            // Ein NSString als Besitzer liefert seinen Text als Tooltip; `TIPS` hält ihn fest.
            unsafe { view.addToolTipRect_owner_userData(rect, &s, std::ptr::null_mut()) };
            t.push(s);
        }
    });
}

// ---------- Navigation und Aktualisierung ----------

fn step(weeks: i64) {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        d.monday += Duration::weeks(weeks);
        d.rebuild();
    });
    render();
}

fn go_today() {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        if let Some(s) = &d.snap {
            d.monday = monday_of(s.now.date());
        }
        d.rebuild();
    });
    render();
}

/// Beschriftungen setzen und das Raster neu zeichnen.
fn render() {
    let (title, summary, warn) = DATA.with(|d| {
        let d = d.borrow();
        let summary = d.snap.as_ref().map(|s| week_summary(s, d.monday, &d.cols)).unwrap_or_default();
        (week_title(d.monday), summary, warnings_summary(&d.cols))
    });
    let view = UI.with(|u| {
        let u = u.borrow();
        let u = u.as_ref()?;
        u.title.setStringValue(&ns(&title));
        u.summary.setStringValue(&ns(&summary));
        u.warn_label.setStringValue(&ns(&warn));
        u.warn_row.setHidden(warn.is_empty());
        Some(u.view.clone())
    });
    if let Some(v) = view {
        v.setNeedsDisplay(true);
        update_tooltips(&v);
        if let Some(w) = v.window() {
            w.invalidateCursorRectsForView(&v);
        }
    }
}

/// Ist die Ansicht zu sehen (dann soll die Menüleisten-App sie regelmäßig aktualisieren)?
pub fn is_visible() -> bool {
    UI.with(|u| {
        u.borrow()
            .as_ref()
            .is_some_and(|u| !u.root.isHidden() && u.view.window().is_some_and(|w| w.isVisible()))
    })
}

/// Neuen Stand übernehmen (die angezeigte Woche bleibt).
pub fn update(snap: Snapshot) {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        if d.monday == NaiveDate::MIN {
            d.monday = monday_of(snap.now.date());
        }
        d.snap = Some(snap);
        d.rebuild();
    });
    render();
}

// ---------- Aufbau ----------

fn build() -> Ui {
    let mtm = mtm();
    let controller = Controller::new(mtm);
    let target = &*controller;

    let images: Vec<Retained<NSImage>> = [("chevron.left", "Vorherige Woche"), ("chevron.right", "Nächste Woche")]
        .iter()
        .filter_map(|(n, d)| NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(n), Some(&ns(d))))
        .collect();
    let nav = unsafe {
        NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
            &NSArray::from_retained_slice(&images),
            NSSegmentSwitchTracking::Momentary,
            Some(target),
            Some(sel!(navigate:)),
            mtm,
        )
    };
    nav.setToolTip_forSegment(Some(&ns("Vorherige Woche (←)")), 0);
    nav.setToolTip_forSegment(Some(&ns("Nächste Woche (→)")), 1);
    let today = unsafe { NSButton::buttonWithTitle_target_action(&ns("Heute"), Some(target), Some(sel!(today:)), mtm) };
    today.setToolTip(Some(&ns("Zur aktuellen Woche (⌘T)")));

    let title = NSTextField::labelWithString(&ns(""), mtm);
    title.setFont(Some(&font(15.0, unsafe { NSFontWeightSemibold })));
    title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    title.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    title.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);

    // Hinweise der Woche (nur sichtbar, wenn es welche gibt).
    let warn_row = NSStackView::new(mtm);
    warn_row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    warn_row.setAlignment(NSLayoutAttribute::CenterY);
    warn_row.setSpacing(4.0);
    if let Some(img) = warning_image(NSFont::systemFontSize()) {
        warn_row.addArrangedSubview(&NSImageView::imageViewWithImage(&img, mtm));
    }
    let warn_label = NSTextField::labelWithString(&ns(""), mtm);
    warn_label.setTextColor(Some(&NSColor::systemOrangeColor()));
    warn_row.addArrangedSubview(&warn_label);
    warn_row.setToolTip(Some(&ns("Tage mit Warnschild anklicken, um die Hinweise zu sehen")));
    warn_row.setHidden(true);

    let summary = NSTextField::labelWithString(&ns(""), mtm);
    summary.setFont(Some(&digits(NSFont::systemFontSize(), unsafe { NSFontWeightRegular })));
    summary.setTextColor(Some(&NSColor::secondaryLabelColor()));

    let bar = NSStackView::new(mtm);
    bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    bar.setAlignment(NSLayoutAttribute::CenterY);
    bar.setSpacing(10.0);
    bar.setEdgeInsets(NSEdgeInsets { top: 10.0, left: 16.0, bottom: 10.0, right: 16.0 });
    for v in [&*nav as &NSView, &today, &title, &warn_row, &summary] {
        bar.addArrangedSubview(v);
    }
    bar.setCustomSpacing_afterView(14.0, &today);
    bar.setCustomSpacing_afterView(18.0, &warn_row);

    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);

    let view = WeekView::new(mtm);

    let root = NSView::new(mtm);
    for v in [&*bar as &NSView, &line, &view] {
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(v);
        v.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()).setActive(true);
        v.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
    }
    bar.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    line.topAnchor().constraintEqualToAnchor(&bar.bottomAnchor()).setActive(true);
    view.topAnchor().constraintEqualToAnchor(&line.bottomAnchor()).setActive(true);
    view.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()).setActive(true);

    Ui { root, view, title, warn_row, warn_label, summary, _controller: controller }
}

/// Die Ansicht zum Einbetten (beim ersten Aufruf aufgebaut). Zeigt zunächst die aktuelle
/// Woche, sobald ein Stand da ist.
pub fn container() -> Retained<NSView> {
    if UI.with(|u| u.borrow().is_none()) {
        let ui = build();
        UI.with(|u| *u.borrow_mut() = Some(ui));
    }
    UI.with(|u| u.borrow().as_ref().map(|u| u.root.clone())).expect("Wochenansicht aufgebaut")
}

/// Wie sich Arbeit ohne Mac eintragen und löschen lässt (ohne `add_work`/`remove_work` nur
/// ansehen, etwa für Bildschirmfotos).
pub fn set_actions(add_work: Option<AddWork>, remove_work: Option<RemoveWork>) {
    ADD_WORK.with(|c| c.set(add_work));
    REMOVE_WORK.with(|c| c.set(remove_work));
}

/// Neuen Stand übernehmen und die Woche von `date` zeigen.
pub fn show_week_of(snap: Snapshot, date: NaiveDate) {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        d.monday = monday_of(date);
        d.snap = Some(snap);
        d.rebuild();
    });
    render();
}

/// Zur aktuellen Woche springen („Heute“, ⌘T).
pub fn today() {
    go_today();
}

/// Tastatureingaben (Pfeiltasten) an das Raster.
pub fn focus() {
    if let Some(v) = UI.with(|u| u.borrow().as_ref().map(|u| u.view.clone())) {
        if let Some(w) = v.window() {
            w.makeFirstResponder(Some(&v));
        }
    }
}

/// Zum Testen: Rückmeldung „kopiert“ für den Beginn des ersten Abschnitts der Woche zeigen
/// (`TAGWERK_FLASH=1` bei `--week-screenshot`).
pub fn flash_first_block() {
    let flash = DATA.with(|d| {
        let d = d.borrow();
        d.cols.iter().find_map(|c| {
            let b = c.blocks.iter().find(|b| b.kind == Kind::Work)?;
            Some(Flash { date: c.date, from: b.from, kind: b.kind, part: Part::Start, generation: 0 })
        })
    });
    FLASH.with(|f| *f.borrow_mut() = flash);
    if let Some(v) = UI.with(|u| u.borrow().as_ref().map(|u| u.view.clone())) {
        v.setNeedsDisplay(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::day::Break;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn snap(days: Vec<Day>, now: NaiveDateTime, running: bool) -> Snapshot {
        Snapshot { days, settings: Settings::default(), now, running }
    }

    #[test]
    fn titles() {
        assert_eq!(week_title(date(2026, 11, 16)), "KW 47  ·  16. – 22. November 2026");
        assert_eq!(week_title(date(2026, 11, 30)), "KW 49  ·  30. November – 6. Dezember 2026");
        assert_eq!(week_title(date(2025, 12, 29)), "KW 1  ·  29. Dezember 2025 – 4. Januar 2026");
        assert_eq!(monday_of(date(2026, 11, 22)), date(2026, 11, 16));
    }

    #[test]
    fn hours_cover_frame_and_outside_work() {
        assert_eq!(hour_range(&Settings::default(), &[]), (7, 22));
        let s = Settings { frame_from: h(6, 30), frame_to: h(18, 15), ..Settings::default() };
        assert_eq!(hour_range(&s, &[]), (6, 19));
        // Arbeit außerhalb des Rahmens erweitert das Raster.
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(5, 40), h(9, 0));
        d.add_period(h(21, 0), h(22, 30));
        let cols = columns(&snap(vec![d], date(2026, 11, 18).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(hour_range(&Settings::default(), &cols), (5, 23));
        let outside: Vec<(i64, i64)> =
            cols[1].blocks.iter().filter(|b| b.kind == Kind::Outside).map(|b| (b.from, b.to)).collect();
        assert_eq!(outside, vec![(h(5, 40), h(7, 0)), (h(22, 0), h(22, 30))]);
        assert_eq!(cols[1].warnings.len(), 1);
        assert!(cols[1].warnings[0].contains("außerhalb des Arbeitszeitrahmens"));
    }

    #[test]
    fn blocks_include_corrections() {
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d.start = Some(h(7, 30));
        d.end = Some(h(17, 0));
        d.breaks.push(Break { from: h(10, 0), to: h(10, 30) });
        let cols = columns(&snap(vec![d], date(2026, 11, 17).and_hms_opt(20, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(cols.len(), 7);
        assert!(cols[0].blocks.is_empty());
        let spans: Vec<(i64, i64)> = cols[1].blocks.iter().map(|b| (b.from, b.to)).collect();
        assert_eq!(spans, vec![(h(7, 30), h(10, 0)), (h(10, 30), h(11, 58)), (h(12, 41), h(17, 0))]);
        assert_eq!(cols[1].work_secs, h(2, 30) + h(1, 28) + h(4, 19));
        // 7:30 – 17:00 abzüglich Arbeitszeit: manuelle Pause und Mittagslücke.
        assert_eq!(cols[1].pause_secs, h(0, 30) + h(0, 43));
        assert_eq!(cols[1].blocks[0].notes, vec!["Arbeitsbeginn manuell festgelegt", "endet mit einer manuellen Pause"]);
        assert_eq!(cols[1].gaps, vec![Span::new(h(10, 0), h(10, 30)), Span::new(h(11, 58), h(12, 41))]);
        assert!(cols[1].manual.is_empty());
        assert_eq!(cols[1].blocks[2].notes, vec!["Feierabend manuell festgelegt"]);
        assert!(cols[1].warnings.is_empty());
    }

    #[test]
    fn work_without_mac_joins_blocks() {
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(12, 0));
        d.add_period(h(14, 0), h(17, 0));
        d.add_work(h(12, 5), h(13, 30), "Meeting");
        let cols = columns(&snap(vec![d], date(2026, 11, 17).and_hms_opt(20, 0, 0).unwrap(), false), date(2026, 11, 16));
        let spans: Vec<(i64, i64)> = cols[1].blocks.iter().map(|b| (b.from, b.to)).collect();
        // 12:00–12:05 ist kürzer als die Mindestpause und zählt mit.
        assert_eq!(spans, vec![(h(8, 0), h(13, 30)), (h(14, 0), h(17, 0))]);
        let block = Span::new(h(12, 5), h(13, 30));
        assert_eq!(cols[1].manual, vec![Manual { span: block, block, note: "Meeting".to_string() }]);
        assert_eq!(cols[1].gaps, vec![Span::new(h(13, 30), h(14, 0))]);
        assert_eq!(cols[1].blocks[0].notes, vec!["ohne Mac gearbeitet 12:05 – 13:30 (Meeting)"]);
        assert_eq!(fmt_span(cols[1].gaps[0]), "13:30 – 14:00 Uhr (30 Min.)");
    }

    #[test]
    fn free_area_presets() {
        const M: i64 = 15 * 60;
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(12, 0));
        d.add_period(h(12, 45), h(17, 0));
        let at = |hh, mm| date(2026, 11, 17).and_hms_opt(hh, mm, 0).unwrap();
        let cols = columns(&snap(vec![d], at(20, 0), false), date(2026, 11, 16));
        let c = &cols[1];
        // In der Pause: die ganze Pause.
        assert_eq!(free_preset(c, h(12, 20), at(20, 0), M), Some(Span::new(h(12, 0), h(12, 45))));
        // Vor dem Beginn und nach dem Feierabend: 30 Minuten ab dem Klick (5-Minuten-Raster).
        assert_eq!(free_preset(c, h(7, 13), at(20, 0), M), Some(Span::new(h(7, 10), h(7, 40))));
        assert_eq!(free_preset(c, h(18, 2), at(20, 0), M), Some(Span::new(h(18, 0), h(18, 30))));
        // Auf einem Abschnitt nichts.
        assert_eq!(free_preset(c, h(9, 0), at(20, 0), M), None);
        // Heute nicht in der Zukunft; das Ende höchstens jetzt.
        assert_eq!(free_preset(c, h(17, 50), at(18, 0), M), Some(Span::new(h(17, 50), h(18, 0))));
        assert_eq!(free_preset(c, h(18, 5), at(18, 0), M), None);
        // Zukünftige Tage nicht, ein freier Tag in der Vergangenheit schon.
        assert_eq!(free_preset(&cols[2], h(9, 0), at(20, 0), M), None);
        assert_eq!(free_preset(&cols[0], h(9, 0), at(20, 0), M), Some(Span::new(h(9, 0), h(9, 30))));
        // Freie Bereiche für die Zeigehand: vor dem Beginn, Pause, nach dem Feierabend (bis jetzt).
        assert_eq!(
            free_spans(c, at(20, 0)),
            vec![Span::new(0, h(8, 0)), Span::new(h(12, 0), h(12, 45)), Span::new(h(17, 0), h(20, 0))]
        );
        assert_eq!(free_spans(c, at(18, 0)).last(), Some(&Span::new(h(17, 0), h(18, 0))));
        assert!(free_spans(&cols[2], at(20, 0)).is_empty());
        assert_eq!(free_spans(&cols[0], at(20, 0)), vec![Span::new(0, h(23, 59))]);
        // Bis höchstens 23:59.
        assert_eq!(free_preset(&cols[0], h(23, 50), at(20, 0), M), Some(Span::new(h(23, 50), h(23, 59))));
        // Längere Mindestdauer: Vorschlag mindestens so lang.
        assert_eq!(free_preset(c, h(18, 2), at(20, 0), 45 * 60), Some(Span::new(h(18, 0), h(18, 45))));
    }

    #[test]
    fn work_labels_avoid_the_times() {
        let rect = |x: f64, y: f64, w: f64, h: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        assert_eq!((work_label(""), work_label(" Telefonat ")), ("Sonstiges", "Telefonat"));
        // Mittig, wenn frei; sonst am Rand; zu schmal: nichts.
        let at = work_label_rect(rect(0.0, 100.0, 120.0, 40.0), &[], 50.0).unwrap();
        assert_eq!((at.origin.y, at.size.height), (114.0, 12.0));
        let times = [rect(7.0, 102.0, 80.0, 15.0)];
        assert_eq!(work_label_rect(rect(0.0, 100.0, 120.0, 40.0), &times, 50.0).unwrap().origin.y, 126.5);
        assert!(work_label_rect(rect(0.0, 100.0, 50.0, 40.0), &[], 50.0).is_none());
        // Zu niedrig: rechts neben der Uhrzeit, wenn Platz ist.
        let times = [rect(7.0, 100.0, 60.0, 17.0)];
        let at = work_label_rect(rect(0.0, 100.0, 140.0, 10.0), &times, 40.0).unwrap();
        assert_eq!((at.origin.x, at.origin.y), (95.0, 101.0));
        assert!(work_label_rect(rect(0.0, 100.0, 100.0, 10.0), &times, 40.0).is_none());
    }

    #[test]
    fn durations_follow_shown_minutes() {
        // 17:00:19 – 17:30:00 zeigt 17:00 – 17:30 und damit 30 Min.
        assert_eq!(fmt_span(Span::new(h(17, 0) + 19, h(17, 30))), "17:00 – 17:30 Uhr (30 Min.)");
        assert_eq!(fmt_span(Span::new(h(14, 45) + 27, h(15, 40))), "14:45 – 15:40 Uhr (55 Min.)");
        assert_eq!(records::shown_len(h(9, 0) + 59, h(9, 0) + 30), 0);
    }

    #[test]
    fn rule_warnings_per_day() {
        // 10:30 Std. ohne Pause: Pause fehlt und Höchstarbeitszeit überschritten.
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(18, 30));
        // Am Samstag gearbeitet.
        let mut sat = Day::new(date(2026, 11, 21));
        sat.add_period(h(10, 0), h(11, 0));
        let cols = columns(
            &snap(vec![d, sat], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false),
            date(2026, 11, 16),
        );
        assert_eq!(cols[1].warnings.len(), 2);
        assert!(cols[1].warnings[0].contains("45 Min. Pause"));
        assert!(cols[1].warnings[1].contains("mehr als 10 Std."));
        assert_eq!(cols[5].warnings, vec![Warning::Weekend.text()]);
        assert_eq!(warnings_summary(&cols), "Hinweise an 2 Tagen");
        assert_eq!(warnings_summary(&cols[..2]), "Hinweise an 1 Tag");
        assert_eq!(warnings_summary(&cols[2..5]), "");
    }

    #[test]
    fn running_block_today() {
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(9, 30));
        let s = snap(vec![d], date(2026, 11, 17).and_hms_opt(9, 30, 0).unwrap(), true);
        let cols = columns(&s, date(2026, 11, 16));
        assert!(cols[1].blocks[0].running);
        assert_eq!(week_summary(&s, date(2026, 11, 16), &cols), "Woche: 1:30 h  ·  Pausen: 0:00 h  ·  Saldo −6:18 h");
        assert_eq!(week_summary(&s, date(2026, 11, 23), &columns(&s, date(2026, 11, 23))), "Woche: –");
    }
}
