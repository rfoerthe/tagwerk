//! Fenster „Wochenansicht“: eine Kalenderwoche als Kalender (wie Termine in Outlook
//! oder Kalender). Jeder Tag ist eine Spalte, jeder Arbeitsabschnitt ein Block von
//! Beginn bis Ende. Gezeigt wird das Ergebnis aus automatischer Erfassung und manuellen
//! Korrekturen (Arbeitsbeginn, Feierabend, Pausen) – also genau die Abschnitte, die als
//! Arbeitszeit zählen. Arbeit außerhalb des Arbeitszeitrahmens erscheint orange als
//! „nicht gezählt“, Tage mit Regelverstößen tragen ein Warnschild im Tageskopf.
//!
//! Das Fenster läuft in der Menüleisten-App (wie „Einstellungen“) und bekommt von ihr
//! den aktuellen Stand samt laufender Periode. Blättern mit ‹ › oder den Pfeiltasten,
//! „Heute“ (⌘T) springt zur aktuellen Woche. Ein Klick auf die Uhrzeit von Beginn oder
//! Ende eines Abschnitts kopiert sie, ein Klick auf einen Tageskopf mit Warnschild zeigt
//! die Hinweise, ein Doppelklick auf einen Tag öffnet ihn im Fenster „Aufzeichnungen“.

use crate::day::{fmt_clock, secs_of, Day, Span};
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

/// Stand, den die Menüleisten-App übergibt.
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
    /// Welche Grenzen von manuellen Korrekturen stammen.
    notes: Vec<&'static str>,
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
                return Column { date, blocks: Vec::new(), work_secs: 0, pause_secs: 0, warnings: Vec::new() };
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
            Column { date, blocks, work_secs, pause_secs, warnings }
        })
        .collect()
}

/// Hinweise, welche Grenzen eines Abschnitts manuell festgelegt wurden.
fn notes(day: &Day, s: &Settings, sp: Span, first: bool, last: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if first && day.start.is_some_and(|x| x.clamp(s.frame_from, s.frame_to) == sp.from) {
        out.push("Arbeitsbeginn manuell festgelegt");
    }
    if day.breaks.iter().any(|b| b.to == sp.from) {
        out.push("beginnt nach einer manuellen Pause");
    }
    if day.breaks.iter().any(|b| b.from == sp.to) {
        out.push("endet mit einer manuellen Pause");
    }
    if last && day.end.is_some_and(|x| x.clamp(s.frame_from, s.frame_to) == sp.to) {
        out.push("Feierabend manuell festgelegt");
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

    fn x(&self, col: usize) -> f64 {
        GUTTER_W + col as f64 * self.col_w
    }

    fn block_rect(&self, col: usize, b: &Block) -> NSRect {
        let top = self.y(b.from.max(self.h0 * 3600));
        let bottom = self.y(b.to.min(self.h1 * 3600)).max(top + 3.0);
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
        Kind::Work => store::fmt_hm(b.to - b.from),
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
    window: Retained<NSWindow>,
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
    static OPEN_DAY: Cell<Option<fn(NaiveDate)>> = const { Cell::new(None) };
    static FLASH: RefCell<Option<Flash>> = const { RefCell::new(None) };
    static FLASH_GEN: Cell<u64> = const { Cell::new(0) };
    static POPOVER: RefCell<Option<Retained<NSPopover>>> = const { RefCell::new(None) };
}

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
            DATA.with(|d| {
                if let Ok(d) = d.try_borrow() {
                    draw(self.bounds(), &d, flash.as_ref());
                }
            });
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

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let cmd = event.modifierFlags().contains(NSEventModifierFlags::Command);
            let key = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
            match (cmd, key.as_str()) {
                (true, "w") => {
                    if let Some(w) = self.window() {
                        w.performClose(None);
                    }
                    true
                }
                (true, "t") => {
                    go_today();
                    true
                }
                _ => unsafe { msg_send![super(self), performKeyEquivalent: event] },
            }
        }

        /// Klick auf eine Uhrzeit: kopieren. Klick auf einen Tageskopf mit Warnschild:
        /// Hinweise zeigen. Doppelklick auf einen Tag: im Fenster „Aufzeichnungen“ öffnen.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            match (hit_test(self, p), event.clickCount()) {
                (Some(Hit::Time { flash, text, .. }), 1) => copy_time(self, flash, &text),
                (Some(Hit::Time { .. }), _) => {}
                (Some(Hit::Warnings { date, warnings, rect }), 1) => show_warnings(self, date, &warnings, rect),
                (_, 2) => {
                    let date = DATA.with(|d| {
                        let d = d.try_borrow().ok()?;
                        let geo = Geo::new(self.bounds(), d.hours);
                        geo.column_at(p.x).and_then(|c| d.cols.get(c)).map(|c| c.date)
                    });
                    if let (Some(date), Some(open)) = (date, OPEN_DAY.with(|o| o.get())) {
                        open(date);
                    }
                }
                _ => {}
            }
        }
    }
);

impl WeekView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
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
        let rect = geo.header_rect(col);
        (!c.warnings.is_empty() && contains(rect, p))
            .then(|| Hit::Warnings { date: c.date, warnings: c.warnings.clone(), rect })
    })
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

fn hline(x0: f64, x1: f64, y: f64, color: &NSColor) {
    fill(NSRect::new(NSPoint::new(x0, y.floor()), NSSize::new(x1 - x0, 1.0)), color);
}

fn vline(x: f64, y0: f64, y1: f64, color: &NSColor) {
    fill(NSRect::new(NSPoint::new(x.floor(), y0), NSSize::new(1.0, y1 - y0)), color);
}

fn draw(bounds: NSRect, d: &Data, flash: Option<&Flash>) {
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
    for bx in block_boxes(&geo, d, &fonts) {
        let b = &bx.block;
        let (tint, bar) = match b.kind {
            Kind::Work if b.running => (accent.colorWithAlphaComponent(0.13), &accent),
            Kind::Work => (accent.colorWithAlphaComponent(0.22), &accent),
            Kind::Outside => (orange.colorWithAlphaComponent(0.16), &orange),
        };
        round_fill(bx.rect, 4.0, &tint);
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
                    format!("{}\n{} Uhr – {end}\n{}", fmt_day(c.date), fmt_clock(b.from), store::fmt_hm(b.to - b.from));
                if b.kind == Kind::Outside {
                    text.push_str("\naußerhalb des Arbeitszeitrahmens – nicht gezählt");
                }
                for n in &b.notes {
                    text.push_str(&format!("\n{n}"));
                }
                text.push_str("\nKlick auf eine Uhrzeit kopiert sie.");
                out.push((geo.block_rect(i, b), text));
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
        Some((u.view.clone(), u.window.clone()))
    });
    if let Some((v, w)) = view {
        v.setNeedsDisplay(true);
        update_tooltips(&v);
        w.invalidateCursorRectsForView(&v);
    }
}

/// Ist das Fenster offen (dann soll die App es regelmäßig aktualisieren)?
pub fn is_open() -> bool {
    UI.with(|u| u.borrow().as_ref().is_some_and(|u| u.window.isVisible()))
}

/// Neuen Stand übernehmen (die angezeigte Woche bleibt).
pub fn update(snap: Snapshot) {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
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

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(980.0, 680.0)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&ns("Wochenansicht"));
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.setContentMinSize(NSSize::new(720.0, 460.0));
    let root = window.contentView().expect("contentView");
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
    window.center();
    window.setFrameAutosaveName(&ns("Wochenansicht"));

    Ui { window, view, title, warn_row, warn_label, summary, _controller: controller }
}

/// Öffnet die Wochenansicht (oder holt sie nach vorn). Ein neu geöffnetes Fenster zeigt
/// die aktuelle Woche. `open_day` öffnet einen Tag im Fenster „Aufzeichnungen“.
pub fn show(snap: Snapshot, open_day: fn(NaiveDate)) {
    show_inner(snap, open_day, true);
}

fn show_inner(snap: Snapshot, open_day: fn(NaiveDate), activate: bool) {
    OPEN_DAY.with(|o| o.set(Some(open_day)));
    let fresh = !is_open();
    if UI.with(|u| u.borrow().is_none()) {
        let ui = build();
        UI.with(|u| *u.borrow_mut() = Some(ui));
    }
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        if fresh {
            d.monday = monday_of(snap.now.date());
        }
        d.snap = Some(snap);
        d.rebuild();
    });
    render();
    let ui = UI.with(|u| u.borrow().as_ref().map(|u| (u.window.clone(), u.view.clone())));
    if let Some((window, view)) = ui {
        if activate {
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm()).activateIgnoringOtherApps(true);
            window.makeKeyAndOrderFront(None);
        } else {
            window.orderFront(None);
        }
        window.makeFirstResponder(Some(&view));
    }
}

/// Zum Testen: Wochenansicht mit dem Stand der Datenbank öffnen, ein Bild speichern und
/// beenden (`Tagwerk --week-screenshot <png>`; Woche über `TAGWERK_SELECT=TT.MM.JJJJ`,
/// Rückmeldung „kopiert“ für den Beginn des ersten Abschnitts mit `TAGWERK_FLASH=1`).
pub fn screenshot(path: &std::path::Path) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Das Fenster muss im Hauptthread laufen.")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if let Ok(a) = std::env::var("TAGWERK_APPEARANCE") {
        let name = if a == "dark" { unsafe { NSAppearanceNameDarkAqua } } else { unsafe { NSAppearanceNameAqua } };
        app.setAppearance(NSAppearance::appearanceNamed(name).as_deref());
    }
    let db = store::Db::open().map_err(|e| e.to_string())?;
    let days = db.load().map_err(|e| e.to_string())?;
    let settings = db.load_settings().map_err(|e| e.to_string())?;
    let now = chrono::Local::now().naive_local();
    let running = store::read_status() == Some(store::Status::Running);
    show_inner(Snapshot { days, settings, now, running }, |_| {}, false);
    if let Some(d) = std::env::var("TAGWERK_SELECT").ok().and_then(|s| store::parse_date(&s)) {
        DATA.with(|data| {
            let mut data = data.borrow_mut();
            data.monday = monday_of(d);
            data.rebuild();
        });
        render();
    }

    let window = UI.with(|u| u.borrow().as_ref().map(|u| u.window.clone())).ok_or("kein Fenster")?;
    let content = window.contentView().ok_or("kein Inhalt")?;
    let frame_view = unsafe { content.superview() }.unwrap_or(content);
    frame_view.layoutSubtreeIfNeeded();
    if std::env::var("TAGWERK_FLASH").is_ok() {
        let flash = DATA.with(|d| {
            let d = d.borrow();
            d.cols.iter().find_map(|c| {
                let b = c.blocks.iter().find(|b| b.kind == Kind::Work)?;
                Some(Flash { date: c.date, from: b.from, kind: b.kind, part: Part::Start, generation: 0 })
            })
        });
        FLASH.with(|f| *f.borrow_mut() = flash);
        frame_view.setNeedsDisplay(true);
    }
    frame_view.displayIfNeeded();
    let bounds = frame_view.bounds();
    let rep = frame_view.bitmapImageRepForCachingDisplayInRect(bounds).ok_or("kein Bildpuffer")?;
    frame_view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let props = NSDictionary::new();
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }
        .ok_or("PNG konnte nicht erzeugt werden")?;
    std::fs::write(path, data.to_vec()).map_err(|e| e.to_string())
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
        assert_eq!(cols[1].blocks[2].notes, vec!["Feierabend manuell festgelegt"]);
        assert!(cols[1].warnings.is_empty());
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
