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
//! dem Arbeitsbeginn und nach dem Feierabend eine halbe Stunde ab dem Klick. Über das
//! Kontextmenü lässt sich für einen Arbeitstag eine Abwesenheit eintragen (Urlaub, Krank,
//! Feiertag, Zeitausgleich); sie liegt als Band über dem Arbeitszeitrahmen des Tages. Der Tag
//! in Bearbeitung (in „Tage“ ausgewählt, siehe [`Selection`]) trägt sein Datum auf einer
//! Pille und einen Rahmen um die Spalte; ein Klick in eine Spalte wählt ihn.
//!
//! Arbeitsbeginn und Feierabend lassen sich mit der Maus verschieben: An der Oberkante des
//! ersten bzw. der Unterkante des letzten Abschnitts erscheint ein Griff. Beim Ziehen rastet
//! die Zeit auf 5 Minuten ein (mit ⌥ auf die Minute) und am erfassten Wert – dorthin
//! gezogen, gilt wieder die automatische Erfassung. Esc bricht ab. Arbeitsbeginn und
//! Feierabend tragen eine türkise Linie, manuell festgelegte kräftiger und mit einem Punkt.

use crate::day::{fmt_clock, secs_of, Absence, AbsenceKind, Activity, Day, Span, DAY_END};
use crate::palette;
use crate::records;
use crate::rules::{self, Settings, Warning};
use crate::store;
use block2::RcBlock;
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSCopying, NSDictionary, NSUserDefaults, NSEdgeInsets, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSTimer};
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
    /// Abwesenheit: kurz für den Tageskopf (`½ Urlaub`) und ausgeschrieben.
    absence: Option<(String, String)>,
    /// Arbeitstag ohne Arbeitszeit und ohne Abwesenheit, der trotzdem sein Soll zählt
    /// (siehe [`rules::missing_workdays`]): das Soll, das im Saldo fehlt.
    missing: Option<i64>,
    /// Ganztägige Abwesenheit: Arbeit ohne Mac lässt sich weder eintragen noch löschen,
    /// bis die Abwesenheit auf „Keine“ steht (siehe [`Day::locks_entries`]).
    locked: bool,
    /// Arbeitsbeginn und Feierabend (nur mit Arbeitszeit).
    edges: Option<Edges>,
}

/// Arbeitsbeginn oder Feierabend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    Start,
    End,
}

/// Arbeitsbeginn und Feierabend einer Tagesspalte, wie angezeigt (zum Verschieben).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edges {
    start: i64,
    end: i64,
    /// Ohne manuelle Angabe (dort rastet das Ziehen ein; dorthin gezogen, gilt sie nicht mehr).
    auto_start: Option<i64>,
    auto_end: Option<i64>,
    manual_start: bool,
    manual_end: bool,
    /// Der letzte Abschnitt läuft noch – sein Ende ist „jetzt“, kein Feierabend.
    running: bool,
}

impl Edges {
    /// Angezeigter Wert, Wert ohne manuelle Angabe und ob er manuell festgelegt ist.
    fn get(&self, bound: Bound) -> (i64, Option<i64>, bool) {
        match bound {
            Bound::Start => (self.start, self.auto_start, self.manual_start),
            Bound::End => (self.end, self.auto_end, self.manual_end),
        }
    }
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

/// So viele Stunden zeigt das kompakte Raster mindestens.
const MIN_HOURS: i64 = 9;
/// Breite eines freien Tages ohne Einträge im Verhältnis zu einem Arbeitstag.
const NARROW: f64 = 0.45;

/// Volle Stunden, die das Raster zeigt.
///
/// Kompakt (`compact`): die Zeit, in der in dieser Woche gearbeitet wurde, mit einer Stunde
/// Luft davor und danach – so füllen die Abschnitte das Raster, statt sich in einem Teil
/// davon zu drängen. `now` (Sekunden seit Mitternacht, nur in der laufenden Woche) gehört
/// dazu, mindestens sind es [`MIN_HOURS`] Stunden. Die Luft und das Auffüllen bleiben im
/// Arbeitszeitrahmen; Arbeit außerhalb gehört immer dazu.
///
/// Sonst, und in einer Woche ohne Arbeit: der ganze Arbeitszeitrahmen und – falls außerhalb
/// gearbeitet wurde – auch diese Zeit.
fn hour_range(s: &Settings, cols: &[Column], now: Option<i64>, compact: bool) -> (i64, i64) {
    let blocks = cols.iter().flat_map(|c| &c.blocks);
    let first = blocks.clone().map(|b| b.from).min();
    let last = blocks.map(|b| b.to).max();
    let hours = |from: i64, to: i64| {
        let h0 = (from / 3600).clamp(0, 23);
        (h0, ((to + 3599) / 3600).clamp(h0 + 1, 24))
    };
    let (frame0, frame1) = hours(s.frame_from, s.frame_to);
    let (Some(first), Some(last), true) = (first, last, compact) else {
        return hours(first.map_or(s.frame_from, |f| f.min(s.frame_from)), last.map_or(s.frame_to, |l| l.max(s.frame_to)));
    };
    let now = now.map(|n| n.clamp(s.frame_from, s.frame_to));
    let (used0, used1) = hours(now.map_or(first, |n| n.min(first)), now.map_or(last, |n| n.max(last)));
    // Eine Stunde Luft, aber nicht über den Rahmen hinaus (außer die Arbeit liegt dort).
    let mut h0 = (used0 - 1).max(frame0.min(used0));
    let mut h1 = (used1 + 1).min(frame1.max(used1));
    // Auf die Mindesthöhe auffüllen: abwechselnd unten und oben, solange der Rahmen reicht.
    while h1 - h0 < MIN_HOURS && (h1 < frame1 || h0 > frame0) {
        if h1 < frame1 {
            h1 += 1;
        }
        if h1 - h0 < MIN_HOURS && h0 > frame0 {
            h0 -= 1;
        }
    }
    (h0, h1)
}

/// Breite der Tagesspalten im Verhältnis zueinander: Freie Tage ohne jeden Eintrag (meist
/// das Wochenende) sind schmal, damit die Arbeitstage den Platz bekommen.
fn column_weights(s: &Settings, cols: &[Column]) -> [f64; 7] {
    let mut weights = [1.0; 7];
    for (w, c) in weights.iter_mut().zip(cols) {
        if !s.is_workday(c.date) && c.blocks.is_empty() && c.absence.is_none() {
            *w = NARROW;
        }
    }
    weights
}

/// Spalten der Woche ab `monday`.
fn columns(snap: &Snapshot, monday: NaiveDate) -> Vec<Column> {
    (0..7).map(|i| column(&snap.days, snap, monday + Duration::days(i))).collect()
}

/// Spalte des Tages `date` aus `days` (sonst `snap.days`; beim Ziehen eine Kopie mit dem
/// neuen Arbeitsbeginn bzw. Feierabend).
fn column(days: &[Day], snap: &Snapshot, date: NaiveDate) -> Column {
    let s = &snap.settings;
    let today = snap.now.date();
    let Some(day) = days.iter().find(|d| d.date == date) else {
        let missing = !rules::missing_workdays(days, date, date, today, s).is_empty();
        return Column {
            date,
            blocks: Vec::new(),
            work_secs: 0,
            pause_secs: 0,
            warnings: Vec::new(),
            manual: Vec::new(),
            gaps: Vec::new(),
            absence: None,
            missing: missing.then(|| s.daily_target(date)),
            locked: false,
            edges: None,
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
    let edges = edges(day, s, &blocks);
    // Wie bei den Hinweisen: Abschnitte unter einer Minute lohnen keine Anzeige.
    blocks.extend(day.outside_frame_spans(s).into_iter().filter(|x| x.len() >= 60).map(|sp| Block {
        from: sp.from,
        to: sp.to,
        kind: Kind::Outside,
        running: false,
        notes: Vec::new(),
    }));
    let (work_secs, pause_secs) = day.record(s).map(|r| (r.shown_work_secs, r.shown_pause_secs)).unwrap_or((0, 0));
    let warnings = rules::day_warnings(days, date, s).iter().map(Warning::text).collect();
    let manual = day
        .counted_work(s)
        .into_iter()
        .map(|(span, w)| Manual { span, block: w.span(), note: w.note.clone() })
        .collect();
    let gaps = day.pauses(s);
    let absence = day.absence.map(|a| (a.short(), a.label()));
    // Einträge, aber keine Arbeitszeit (z. B. nur eine Pause): Das Soll zählt trotzdem.
    let raw_work = day.record(s).map_or(0, |r| r.work_secs);
    let missing = (day.absence.is_none() && raw_work <= 0)
        .then(|| s.counted_target(day, raw_work, today))
        .filter(|t| *t > 0);
    let locked = day.locks_entries();
    Column { date, blocks, work_secs, pause_secs, warnings, manual, gaps, absence, missing, locked, edges }
}

/// Arbeitsbeginn und Feierabend aus den Abschnitten, die zählen (`work`, in Reihenfolge),
/// samt den Werten ohne manuelle Angabe.
fn edges(day: &Day, s: &Settings, work: &[Block]) -> Option<Edges> {
    let (first, last) = (work.first()?, work.last()?);
    let without = |f: fn(&mut Day)| {
        let mut d = day.clone();
        f(&mut d);
        d.spans(s)
    };
    Some(Edges {
        start: first.from,
        end: last.to,
        auto_start: without(|d| d.start = None).first().map(|sp| sp.from),
        auto_end: without(|d| d.end = None).last().map(|sp| sp.to),
        manual_start: day.start.is_some(),
        manual_end: day.end.is_some(),
        running: last.running,
    })
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
    let (head, balance) = week_summary_parts(snap, monday, cols);
    match balance {
        Some(b) => format!("{head}{}", rules::fmt_balance(b)),
        None => head,
    }
}

/// Wie [`week_summary`], aber der Saldo getrennt (er wird farbig gesetzt): Der Text endet
/// dann mit „Saldo “.
fn week_summary_parts(snap: &Snapshot, monday: NaiveDate, cols: &[Column]) -> (String, Option<i64>) {
    let total: i64 = cols.iter().map(|c| c.work_secs).sum();
    let pauses: i64 = cols.iter().map(|c| c.pause_secs).sum();
    let today = snap.now.date();
    if monday > today {
        return ("Woche: –".to_string(), None);
    }
    let head = format!("Woche: {}  ·  Pausen: {}", store::fmt_hm(total), store::fmt_hm(pauses));
    let to = (monday + Duration::days(6)).min(today);
    let (work, target) = rules::balance(&snap.days, monday, to, today, &snap.settings);
    // Ohne Arbeitszeit und ohne Soll (z. B. Zeitausgleich) gibt es keinen Saldo.
    if work == 0 && target == 0 {
        return (head, None);
    }
    (format!("{head}  ·  Saldo "), Some(work - target))
}

/// Tooltip eines Arbeitstags ohne Arbeitszeit und Abwesenheit (`target`: das fehlende Soll).
fn missing_text(target: i64) -> String {
    format!(
        "Keine Arbeitszeit: Das Soll von {} zählt als Minus. Zeiten oder eine Abwesenheit \
         nachtragen (Rechtsklick).",
        store::fmt_hm(target)
    )
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
    /// Breite der Spalten im Verhältnis zueinander (siehe [`column_weights`]).
    weights: [f64; 7],
    /// Den ganzen Arbeitszeitrahmen zeigen statt nur der Arbeitszeit der Woche.
    full_frame: bool,
}

impl Data {
    fn rebuild(&mut self) {
        if let Some(snap) = &self.snap {
            self.cols = columns(snap, self.monday);
            // „Jetzt“ gehört nur in der laufenden Woche ins Raster.
            let now = (monday_of(snap.now.date()) == self.monday).then(|| secs_of(snap.now.time()));
            self.hours = hour_range(&snap.settings, &self.cols, now, !self.full_frame);
            self.weights = column_weights(&snap.settings, &self.cols);
        }
        // Ein neuer Stand während des Ziehens: Die Vorschau bleibt.
        DRAG.with(|g| {
            if let Some(g) = g.borrow_mut().as_mut().filter(|g| g.active) {
                preview(self, g);
            }
        });
    }
}

// ---------- Geometrie ----------

struct Geo {
    w: f64,
    h: f64,
    /// Linke Kanten der sieben Spalten und die rechte Kante der letzten.
    xs: [f64; 8],
    hour_h: f64,
    h0: i64,
    h1: i64,
}

impl Geo {
    fn new(bounds: NSRect, d: &Data) -> Self {
        let (w, h) = (bounds.size.width, bounds.size.height);
        let n = (d.hours.1 - d.hours.0).max(1) as f64;
        let unit = ((w - GUTTER_W) / d.weights.iter().sum::<f64>()).max(1.0);
        let mut xs = [GUTTER_W; 8];
        for i in 0..7 {
            xs[i + 1] = xs[i] + d.weights[i] * unit;
        }
        Geo {
            w,
            h,
            xs,
            hour_h: ((h - HEADER_H - GRID_PAD_TOP - GRID_PAD_BOTTOM) / n).max(1.0),
            h0: d.hours.0,
            h1: d.hours.1,
        }
    }

    /// Breite der Spalte `col`.
    fn col_w(&self, col: usize) -> f64 {
        self.xs[col.min(6) + 1] - self.xs[col.min(6)]
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

    /// Uhrzeit an der y-Koordinate, auch außerhalb des Rasters (zum Ziehen).
    fn secs_raw(&self, y: f64) -> i64 {
        self.h0 * 3600 + ((y - self.grid_top()) / self.hour_h * 3600.0).round() as i64
    }

    /// Linke Kante der Spalte `col` (7 = rechte Kante der letzten).
    fn x(&self, col: usize) -> f64 {
        self.xs[col.min(7)]
    }

    fn block_rect(&self, col: usize, b: &Block) -> NSRect {
        let top = self.y(b.from.max(self.h0 * 3600));
        let bottom = self.y(b.to.min(self.h1 * 3600)).max(top + 3.0);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top + 0.5), NSSize::new(self.col_w(col) - 6.0, bottom - top - 1.0))
    }

    /// Bereich einer Pause zwischen zwei Abschnitten.
    fn gap_rect(&self, col: usize, gap: Span) -> NSRect {
        let top = self.y(gap.from.max(self.h0 * 3600));
        let bottom = self.y(gap.to.min(self.h1 * 3600)).max(top);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top), NSSize::new(self.col_w(col) - 6.0, bottom - top))
    }

    /// Bereich von Arbeit ohne Mac (innerhalb ihres Blocks).
    fn span_rect(&self, col: usize, sp: Span) -> NSRect {
        let top = self.y(sp.from.max(self.h0 * 3600));
        let bottom = self.y(sp.to.min(self.h1 * 3600)).max(top + 1.0);
        NSRect::new(NSPoint::new(self.x(col) + 3.0, top + 0.5), NSSize::new(self.col_w(col) - 6.0, bottom - top - 1.0))
    }

    fn header_rect(&self, col: usize) -> NSRect {
        NSRect::new(NSPoint::new(self.x(col), 0.0), NSSize::new(self.col_w(col), HEADER_H))
    }

    fn column_at(&self, x: f64) -> Option<usize> {
        if x < GUTTER_W || x >= self.w {
            return None;
        }
        Some((1..=7).position(|i| x < self.xs[i]).unwrap_or(6))
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
        weights: [1.0; 7],
        full_frame: false,
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
    /// Abwesenheit eintragen oder entfernen – stellt „Aufzeichnungen“ bereit.
    static SET_ABSENCE: Cell<Option<SetAbsence>> = const { Cell::new(None) };
    /// Alle Einträge eines Tages löschen (nach Rückfrage) – stellt „Aufzeichnungen“ bereit.
    static CLEAR_DAY: Cell<Option<ClearDay>> = const { Cell::new(None) };
    /// Einen Tag in der Ansicht „Tage“ öffnen – stellt „Aufzeichnungen“ bereit.
    static OPEN_DAY: Cell<Option<OpenDay>> = const { Cell::new(None) };
    /// Der Tag in Bearbeitung (in „Aufzeichnungen“ ausgewählt) und wie er sich wählen lässt.
    static SELECTION: Cell<Option<Selection>> = const { Cell::new(None) };
    /// Arbeitsbeginn bzw. Feierabend festlegen (Tag, Grenze, Uhrzeit oder `None` für
    /// automatisch) – stellt „Aufzeichnungen“ bereit.
    static SET_BOUND: Cell<Option<SetBound>> = const { Cell::new(None) };
    /// Kante unter der Maus (Spalte, Grenze) – dort erscheint der Griff.
    static HOVER_EDGE: Cell<Option<(usize, Bound)>> = const { Cell::new(None) };
    /// Arbeitsbeginn oder Feierabend, der gerade gezogen wird.
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
    /// Worauf sich das gerade offene Kontextmenü bezieht.
    static PENDING: RefCell<Option<Context>> = const { RefCell::new(None) };
    /// Zähler für den Stand der Anzeige (für die Elemente der Bedienungshilfen).
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// Elemente für VoiceOver und der Stand, für den sie gelten.
    static AX: RefCell<Option<(AxKey, Retained<NSArray<AnyObject>>)>> = const { RefCell::new(None) };
}

/// Stand, für den die Elemente der Bedienungshilfen gelten: Anzeige und Lage auf dem Bildschirm.
type AxKey = (u64, [i64; 4]);

/// Einen Tag in der Ansicht „Tage“ zeigen.
pub type OpenDay = fn(NaiveDate);

/// Der Tag in Bearbeitung – geteilt mit der Ansicht „Tage“ (dort zeigt ihn der Inspektor).
#[derive(Clone, Copy)]
pub struct Selection {
    /// Der ausgewählte Tag.
    pub get: fn() -> Option<NaiveDate>,
    /// Einen Tag auswählen (Klick in seine Spalte).
    pub set: fn(NaiveDate),
}

fn selected_day() -> Option<NaiveDate> {
    SELECTION.with(|c| c.get()).and_then(|s| (s.get)())
}

/// Den Tag `date` auswählen und neu zeichnen, wenn er wechselt.
fn select_day(view: &WeekView, date: NaiveDate) {
    let Some(sel) = SELECTION.with(|c| c.get()) else { return };
    if (sel.get)() != Some(date) {
        (sel.set)(date);
        view.setNeedsDisplay(true);
    }
}

/// Was an einer Stelle des Rasters möglich ist (für Kontextmenü und Doppelklick).
#[derive(Clone)]
struct Context {
    date: NaiveDate,
    /// Arbeit ohne Mac unter dem Zeiger: der eingetragene Block und seine Notiz.
    work: Option<(Span, String)>,
    /// Freier Bereich unter dem Zeiger: Vorschlag für Arbeit ohne Mac.
    preset: Option<Span>,
    /// Uhrzeit unter dem Zeiger (zum Kopieren).
    time: Option<(Flash, String)>,
    /// Abwesenheit lässt sich ändern: die eingetragene (`Some(None)`: keine).
    absence: Option<Option<Absence>>,
    /// Hat der Tag Einträge außer der Abwesenheit? Dann geht nur ein halber Tag.
    busy: bool,
    /// Der Tag lässt sich leeren („Alle Einträge löschen …“): Er hat Einträge außer der
    /// Abwesenheit.
    clearable: bool,
    /// Arbeitsbeginn und Feierabend des Tages und welche davon sich hier ändern lassen.
    edges: Option<Edges>,
    bounds: Vec<Bound>,
}

/// Schlüssel in den Benutzereinstellungen: den ganzen Arbeitszeitrahmen zeigen.
const FULL_FRAME_KEY: &str = "TagwerkWeekFullFrame";

/// Arbeit ohne Mac eintragen: Tag, von, bis, Grund.
pub type AddWork = fn(NaiveDate, i64, i64, Activity);

/// Arbeit ohne Mac löschen: Tag, von, bis (der eingetragene Block).
pub type RemoveWork = fn(NaiveDate, i64, i64);

/// Abwesenheit eines Tages eintragen, ändern oder (`None`) entfernen.
pub type SetAbsence = fn(NaiveDate, Option<Absence>);

/// Alle Einträge eines Tages löschen (fragt vorher nach).
pub type ClearDay = fn(NaiveDate);
/// Arbeitsbeginn bzw. Feierabend festlegen: Tag, Grenze, Uhrzeit (`None`: wieder automatisch).
pub type SetBound = fn(NaiveDate, Bound, Option<i64>);

/// Fangbereich der Kanten außerhalb bzw. innerhalb des Abschnitts (Punkte).
const EDGE_OUT: f64 = 6.0;
const EDGE_IN: f64 = 2.5;
/// Erst ab dieser Mausbewegung wird gezogen (Punkte) – sonst ist es ein Klick.
const DRAG_SLOP: f64 = 3.0;
/// So nah am erfassten Wert rastet die Kante dort ein (Punkte).
const MAGNET: f64 = 6.0;
/// Kürzester Abstand zwischen Arbeitsbeginn und Feierabend beim Verschieben.
const MIN_DAY: i64 = 5 * 60;

/// Was beim Ziehen zusätzlich gezeichnet wird.
struct DragShow {
    col: usize,
    bound: Bound,
    /// Schild an der Kante (siehe [`drag_label`]).
    label: String,
    /// Arbeit ohne Mac, die entfiele.
    cut: Vec<Span>,
}

/// Arbeitsbeginn oder Feierabend, der gerade gezogen wird.
struct Drag {
    col: usize,
    date: NaiveDate,
    bound: Bound,
    /// Stand beim Drücken der Maustaste.
    edges: Edges,
    /// y beim Drücken und Abstand des Zeigers zur Kante (damit sie nicht springt).
    y0: f64,
    offset: f64,
    /// Die Maus hat sich weit genug bewegt (sonst war es ein Klick).
    active: bool,
    /// Uhrzeit, auf der die Kante gerade steht, und ob sie am erfassten Wert eingerastet ist.
    at: i64,
    snapped: bool,
    /// Arbeit ohne Mac, die mit dem neuen Wert entfiele.
    cut: Vec<Span>,
    /// Alle Tage (für die Hinweise) mit dem Tag in der Vorschau; `day` ist der Tag vorher.
    days: Vec<Day>,
    day: Day,
}

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

        /// „Ganzer Rahmen“: den ganzen Arbeitszeitrahmen zeigen oder nur die Arbeitszeit.
        #[unsafe(method(toggleFrame:))]
        fn toggle_frame(&self, sender: Option<&NSButton>) {
            let full = sender.is_some_and(|b| b.state() == NSControlStateValueOn);
            NSUserDefaults::standardUserDefaults().setBool_forKey(full, &ns(FULL_FRAME_KEY));
            DATA.with(|d| {
                let mut d = d.borrow_mut();
                d.full_frame = full;
                d.rebuild();
            });
            render();
        }

        /// Kontextmenü: Arbeit ohne Mac löschen.
        #[unsafe(method(removeWork:))]
        fn remove_work(&self, _sender: Option<&NSMenuItem>) {
            let pending = PENDING.with(|p| p.borrow_mut().take());
            if let (Some(Context { date, work: Some((block, _)), .. }), Some(f)) = (pending, REMOVE_WORK.with(|c| c.get())) {
                f(date, block.from, block.to);
            }
        }

        /// Kontextmenü: den Tag in der Ansicht „Tage“ bearbeiten.
        #[unsafe(method(openDay:))]
        fn open_day(&self, _sender: Option<&NSMenuItem>) {
            if let Some(ctx) = PENDING.with(|p| p.borrow_mut().take()) {
                open_day(ctx.date);
            }
        }

        /// Kontextmenü: Arbeit ohne Mac im freien Bereich eintragen.
        #[unsafe(method(addWorkHere:))]
        fn add_work_here(&self, _sender: Option<&NSMenuItem>) {
            if let Some(Context { date, preset: Some(preset), .. }) = PENDING.with(|p| p.borrow_mut().take()) {
                show_add_work(date, preset, None);
            }
        }

        /// Kontextmenü „Abwesenheit“: Tag des Menüpunkts siehe [`absence_entries`].
        #[unsafe(method(setAbsence:))]
        fn set_absence(&self, sender: Option<&NSMenuItem>) {
            let pending = PENDING.with(|p| p.borrow_mut().take());
            let choice = sender.and_then(|s| absence_of_tag(s.tag()));
            if let (Some(ctx), Some(absence), Some(f)) = (pending, choice, SET_ABSENCE.with(|c| c.get())) {
                f(ctx.date, absence);
            }
        }

        /// Kontextmenü: alle Einträge des Tages löschen (nach Rückfrage).
        #[unsafe(method(clearDay:))]
        fn clear_day(&self, _sender: Option<&NSMenuItem>) {
            if let (Some(ctx), Some(f)) = (PENDING.with(|p| p.borrow_mut().take()), CLEAR_DAY.with(|c| c.get())) {
                f(ctx.date);
            }
        }

        /// Kontextmenü: Uhrzeit kopieren.
        #[unsafe(method(copyTime:))]
        fn copy_time_action(&self, _sender: Option<&NSMenuItem>) {
            let pending = PENDING.with(|p| p.borrow_mut().take());
            let view = UI.with(|u| u.borrow().as_ref().map(|u| u.view.clone()));
            if let (Some(Context { time: Some((flash, text)), .. }), Some(view)) = (pending, view) {
                copy_time(&view, flash, &text);
            }
        }

        /// Kontextmenü: Arbeitsbeginn bzw. Feierabend ändern oder zurücksetzen.
        #[unsafe(method(changeStart:))]
        fn change_start(&self, _sender: Option<&NSMenuItem>) {
            pending_bound(Bound::Start, false);
        }

        #[unsafe(method(changeEnd:))]
        fn change_end(&self, _sender: Option<&NSMenuItem>) {
            pending_bound(Bound::End, false);
        }

        #[unsafe(method(resetStart:))]
        fn reset_start(&self, _sender: Option<&NSMenuItem>) {
            pending_bound(Bound::Start, true);
        }

        #[unsafe(method(resetEnd:))]
        fn reset_end(&self, _sender: Option<&NSMenuItem>) {
            pending_bound(Bound::End, true);
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
    /// Arbeit ohne Mac (schraffiert); die Einzelheiten liefert [`context_at`].
    Work,
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
            // Beim Ziehen kein Rahmen für Arbeit ohne Mac: Die Kante wandert unter der Maus weg.
            let dragging = DRAG.with(|g| g.borrow().is_some());
            let p = HOVER.with(|h| h.get()).filter(|_| !dragging);
            let hover = hover_at(self, p);
            HOVER_SPAN.with(|h| h.set(hover));
            let edge = p.and_then(|p| edge_at(self, p));
            HOVER_EDGE.with(|h| h.set(edge));
            let drag = DRAG.with(|g| {
                g.borrow().as_ref().filter(|g| g.active).map(|g| DragShow {
                    col: g.col,
                    bound: g.bound,
                    label: drag_label(g.bound, g.at, g.edges.get(g.bound).0, g.edges.get(g.bound).1),
                    cut: g.cut.clone(),
                })
            });
            let grip = drag.as_ref().map(|g| (g.col, g.bound)).or(edge);
            DATA.with(|d| {
                if let Ok(d) = d.try_borrow() {
                    draw(self.bounds(), &d, flash.as_ref(), hover, grip, drag.as_ref());
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

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            drag_moved(self, p, event.modifierFlags().contains(NSEventModifierFlags::Option));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            end_drag(self, p);
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
            // Kanten von Arbeitsbeginn und Feierabend: verschieben.
            let resize = resize_cursor();
            for r in edge_cursor_rects(self) {
                self.addCursorRect_cursor(r, &resize);
            }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            match event.keyCode() {
                53 if DRAG.with(|g| g.borrow().as_ref().is_some_and(|g| g.active)) => cancel_drag(self), // Esc
                123 => step(-1), // ←
                124 => step(1),  // →
                _ => unsafe { msg_send![super(self), keyDown: event] },
            }
        }

        /// Klick auf eine Uhrzeit: kopieren. Klick auf einen Tageskopf mit Warnschild:
        /// Hinweise zeigen. Klick auf einen freien Bereich: Arbeit ohne Mac eintragen. Klick
        /// auf Arbeit ohne Mac: ihr Menü. Doppelklick auf einen Tageskopf oder Abschnitt:
        /// den Tag in „Tage“ bearbeiten. Weitere Klicks eines Mehrfachklicks tun nichts.
        /// Auf der Kante von Arbeitsbeginn oder Feierabend beginnt das Ziehen; ohne Bewegung
        /// gilt beim Loslassen der Klick.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            // Ein Klick in eine Tagesspalte macht diesen Tag zum Tag in Bearbeitung.
            if let Some(ctx) = context_at(self, p) {
                select_day(self, ctx.date);
            }
            match event.clickCount() {
                1 => {
                    if !begin_drag(self, p) {
                        click(self, p);
                    }
                }
                // Nur dort, wo der erste Klick nichts ausgelöst hat (außer den Hinweisen).
                2 if matches!(hit_test(self, p), None | Some(Hit::Warnings { .. })) => {
                    if let Some(ctx) = context_at(self, p).filter(can_open) {
                        open_day(ctx.date);
                    }
                }
                _ => {}
            }
        }

        /// Rechtsklick: alles, was an dieser Stelle geht, als Menü.
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, event: &NSEvent) -> Option<Retained<NSMenu>> {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            context_at(self, p).and_then(|ctx| {
                select_day(self, ctx.date);
                let menu = context_menu(&ctx)?;
                PENDING.with(|c| *c.borrow_mut() = Some(ctx));
                Some(menu)
            })
        }

        // ---------- Bedienungshilfen (VoiceOver) ----------

        /// Das Raster ist selbst gezeichnet; für VoiceOver ist es eine Gruppe mit einem
        /// Element je Tageskopf, Abschnitt und Pause.
        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn accessibility_role(&self) -> Option<Retained<NSString>> {
            Some(unsafe { NSAccessibilityGroupRole }.copy())
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Option<Retained<NSString>> {
            DATA.with(|d| {
                let d = d.try_borrow().ok()?;
                let summary = d.snap.as_ref().map(|s| week_summary(s, d.monday, &d.cols)).unwrap_or_default();
                Some(ns(&format!("Woche {}. {}", spoken(&week_title(d.monday)), spoken(&summary))))
            })
        }

        #[unsafe(method_id(accessibilityChildren))]
        fn accessibility_children(&self) -> Option<Retained<NSArray<AnyObject>>> {
            Some(ax_children(self))
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

/// Freier Bereich unter `p` mit dem Vorschlag für Arbeit ohne Mac (für den Rahmen). Auf
/// einer Kante gibt es keinen Rahmen, dort erscheint der Griff.
fn hover_at(view: &WeekView, p: Option<NSPoint>) -> Option<(NaiveDate, Span)> {
    let p = p?;
    if edge_at(view, p).is_some() {
        return None;
    }
    match hit_test(view, p) {
        Some(Hit::Free { date, preset }) => Some((date, preset)),
        _ => None,
    }
}

fn set_hover(view: &WeekView, p: Option<NSPoint>) {
    HOVER.with(|h| h.set(p));
    let edge = p.and_then(|p| edge_at(view, p));
    if hover_at(view, p) != HOVER_SPAN.with(|h| h.get()) || edge != HOVER_EDGE.with(|h| h.get()) {
        view.setNeedsDisplay(true);
    }
}

/// Einfacher Klick: Uhrzeit kopieren, Hinweise zeigen, Arbeit ohne Mac eintragen oder ihr Menü.
fn click(view: &WeekView, p: NSPoint) {
    match hit_test(view, p) {
        Some(Hit::Time { flash, text, .. }) => copy_time(view, flash, &text),
        Some(Hit::Warnings { date, warnings, rect }) => show_warnings(view, date, &warnings, rect),
        Some(Hit::Free { date, preset }) => show_add_work(date, preset, None),
        Some(Hit::Work) => {
            if let Some(ctx) = context_at(view, p) {
                show_context_menu(view, ctx, p);
            }
        }
        None => {}
    }
}

// ---------- Arbeitsbeginn und Feierabend verschieben ----------

/// Lassen sich Arbeitsbeginn und Feierabend ändern (nicht im Bildschirmfoto-Modus)?
fn bounds_editable() -> bool {
    editable() && SET_BOUND.with(|c| c.get()).is_some()
}

/// Bereich, in dem sich `bound` verschieben lässt: im Arbeitszeitrahmen (nur dort zählt
/// Arbeit) und im sichtbaren Raster `grid`, mindestens [`MIN_DAY`] vom anderen Ende entfernt,
/// heute höchstens bis jetzt (`limit`, siehe [`free_limit`]). Auf ganze Minuten.
fn drag_limits(e: &Edges, bound: Bound, s: &Settings, limit: Option<i64>, grid: (i64, i64)) -> Option<(i64, i64)> {
    let limit = limit?;
    let (lo, hi) = match bound {
        Bound::Start => (s.frame_from.max(grid.0), (e.end - MIN_DAY).min(s.frame_to).min(limit)),
        Bound::End => ((e.start + MIN_DAY).max(s.frame_from), s.frame_to.min(grid.1).min(limit)),
    };
    let (lo, hi) = ((lo + 59).div_euclid(60) * 60, hi.div_euclid(60) * 60);
    (lo <= hi).then_some((lo, hi))
}

/// Uhrzeit beim Ziehen an der Stelle `raw`: auf 5 Minuten (`fine`: auf die Minute) innerhalb
/// von `limits`; näher als `magnet` am Wert ohne manuelle Angabe (`auto`) rastet sie dort ein
/// (zweiter Wert `true`).
fn drag_value(raw: i64, auto: Option<i64>, magnet: i64, fine: bool, (lo, hi): (i64, i64)) -> (i64, bool) {
    if let Some(a) = auto.filter(|a| (lo..=hi).contains(a) && (raw - a).abs() <= magnet) {
        return (a, true);
    }
    let step = if fine { 60 } else { 300 };
    (((raw + step / 2).div_euclid(step) * step).clamp(lo, hi), false)
}

/// Was für `bound` gespeichert wird, wenn die Kante auf `at` steht: `None` = nichts ändern,
/// `Some(None)` = wieder automatisch (am Wert ohne manuelle Angabe), sonst die Uhrzeit.
fn drag_result(e: &Edges, bound: Bound, at: i64) -> Option<Option<i64>> {
    let (current, auto, manual) = e.get(bound);
    if Some(at) == auto {
        return manual.then_some(None);
    }
    if manual && at == current {
        return None;
    }
    Some(Some(at))
}

/// `Beginn 07:30 · +25 Min.` – die Uhrzeit beim Ziehen und wie weit sie vom Wert vorher
/// (`before`) abweicht; am Wert ohne manuelle Angabe `· automatisch`.
fn drag_label(bound: Bound, at: i64, before: i64, auto: Option<i64>) -> String {
    let what = match bound {
        Bound::Start => "Beginn",
        Bound::End => "Feierabend",
    };
    let mut text = format!("{what} {}", fmt_clock(at));
    let diff = at / 60 - before / 60;
    if Some(at) == auto {
        text.push_str(" · automatisch");
    } else if diff != 0 {
        let sign = if diff > 0 { "+" } else { "−" };
        let m = diff.abs();
        text.push_str(&if m < 60 { format!(" · {sign}{m} Min.") } else { format!(" · {sign}{}:{:02} h", m / 60, m % 60) });
    }
    text
}

/// Den Tag `day` auf den neuen Wert für `bound` setzen; gibt zurück, welche Arbeit ohne Mac
/// dabei entfällt.
fn apply_bound(day: &mut Day, bound: Bound, at: Option<i64>) -> Vec<Span> {
    let cut = match bound {
        Bound::Start => day.set_start(at),
        Bound::End => day.set_end(at),
    };
    cut.iter().map(|w| w.span()).collect()
}

/// Die Spalte des gezogenen Tages mit dem aktuellen Wert neu berechnen.
fn preview(d: &mut Data, g: &mut Drag) {
    let Some(i) = g.days.iter().position(|x| x.date == g.date) else { return };
    let mut day = g.day.clone();
    g.cut = match drag_result(&g.edges, g.bound, g.at) {
        Some(at) => apply_bound(&mut day, g.bound, at),
        None => Vec::new(),
    };
    g.days[i] = day;
    let Some(snap) = d.snap.as_ref() else { return };
    let col = column(&g.days, snap, g.date);
    if let Some(c) = d.cols.get_mut(g.col) {
        *c = col;
    }
}

/// Fangbereiche der Kanten: Oberkante des ersten und Unterkante des letzten Abschnitts, der
/// zählt (nicht das „jetzt“ eines laufenden Abschnitts und nicht, wo sich nichts ändern lässt).
fn edge_rects(geo: &Geo, d: &Data) -> Vec<(usize, Bound, NSRect)> {
    let Some(snap) = d.snap.as_ref() else { return Vec::new() };
    let grid = (geo.h0 * 3600, geo.h1 * 3600);
    let mut out = Vec::new();
    for (i, c) in d.cols.iter().enumerate() {
        // Ganztägig abwesend: Arbeitsbeginn und Feierabend sind gesperrt (siehe `Day::locks_entries`).
        let Some(e) = c.edges.filter(|_| !c.locked) else { continue };
        let limit = free_limit(c.date, snap.now);
        let (x, w) = (geo.x(i) + 3.0, geo.col_w(i) - 6.0);
        if drag_limits(&e, Bound::Start, &snap.settings, limit, grid).is_some() {
            let top = geo.y(e.start) + 0.5;
            out.push((i, Bound::Start, NSRect::new(NSPoint::new(x, top - EDGE_OUT), NSSize::new(w, EDGE_OUT + EDGE_IN))));
        }
        if !e.running && drag_limits(&e, Bound::End, &snap.settings, limit, grid).is_some() {
            let bottom = geo.y(e.end) - 0.5;
            out.push((i, Bound::End, NSRect::new(NSPoint::new(x, bottom - EDGE_IN), NSSize::new(w, EDGE_IN + EDGE_OUT))));
        }
    }
    out
}

/// Kante unter `p` (Spalte und Grenze).
fn edge_at(view: &WeekView, p: NSPoint) -> Option<(usize, Bound)> {
    if !bounds_editable() {
        return None;
    }
    DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let geo = Geo::new(view.bounds(), &d);
        edge_rects(&geo, &d).into_iter().find(|(_, _, r)| contains(*r, p)).map(|(i, b, _)| (i, b))
    })
}

/// Maustaste auf einer Kante gedrückt: Ziehen vorbereiten (`false`: keine Kante).
fn begin_drag(view: &WeekView, p: NSPoint) -> bool {
    let Some((col, bound)) = edge_at(view, p) else { return false };
    let drag = DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let geo = Geo::new(view.bounds(), &d);
        let snap = d.snap.as_ref()?;
        let c = d.cols.get(col)?;
        let edges = c.edges?;
        let at = edges.get(bound).0;
        Some(Drag {
            col,
            date: c.date,
            bound,
            edges,
            y0: p.y,
            offset: p.y - geo.y(at),
            active: false,
            at,
            snapped: false,
            cut: Vec::new(),
            day: snap.days.iter().find(|x| x.date == c.date)?.clone(),
            days: snap.days.clone(),
        })
    });
    let started = drag.is_some();
    DRAG.with(|g| *g.borrow_mut() = drag);
    started
}

/// Maus mit gedrückter Taste bewegt: die Kante folgt (`fine`: ⌥ gedrückt, minutengenau).
fn drag_moved(view: &WeekView, p: NSPoint, fine: bool) {
    // Während des Ziehens kommen keine Mausbewegungen: die Stelle hier merken.
    HOVER.with(|h| h.set(Some(p)));
    let started = DRAG.with(|g| {
        let mut g = g.borrow_mut();
        let g = g.as_mut()?;
        if g.active {
            return Some(false);
        }
        if (p.y - g.y0).abs() < DRAG_SLOP {
            return None;
        }
        g.active = true;
        Some(true)
    });
    match started {
        None => return,
        Some(true) => {
            // Während des Ziehens überall der Pfeil nach oben und unten.
            if let Some(w) = view.window() {
                w.disableCursorRects();
            }
            resize_cursor().set();
        }
        Some(false) => {}
    }
    let snapped = DATA.with(|d| {
        let mut d = d.try_borrow_mut().ok()?;
        let geo = Geo::new(view.bounds(), &d);
        let (settings, now) = d.snap.as_ref().map(|s| (s.settings, s.now))?;
        DRAG.with(|g| {
            let mut g = g.borrow_mut();
            let g = g.as_mut()?;
            let limit = free_limit(g.date, now);
            let limits = drag_limits(&g.edges, g.bound, &settings, limit, (geo.h0 * 3600, geo.h1 * 3600))?;
            let magnet = (MAGNET / geo.hour_h * 3600.0) as i64;
            let (at, snapped) = drag_value(geo.secs_raw(p.y - g.offset), g.edges.get(g.bound).1, magnet, fine, limits);
            let newly = snapped && !g.snapped;
            g.at = at;
            g.snapped = snapped;
            preview(&mut d, g);
            Some(newly)
        })
    });
    if snapped == Some(true) {
        // Spürbar einrasten (auf dem Trackpad).
        NSHapticFeedbackManager::defaultPerformer()
            .performFeedbackPattern_performanceTime(NSHapticFeedbackPattern::Alignment, NSHapticFeedbackPerformanceTime::Now);
    }
    view.setNeedsDisplay(true);
}

/// Maustaste losgelassen: den neuen Wert speichern – oder, ohne Bewegung, der Klick.
fn end_drag(view: &WeekView, p: NSPoint) {
    let Some(g) = DRAG.with(|g| g.borrow_mut().take()) else {
        // Nach Esc: Rahmen bzw. Griff dort, wo die Maus jetzt ist.
        set_hover(view, Some(p));
        return;
    };
    if !g.active {
        click(view, p);
        return;
    }
    restore_cursor(view);
    if let Some(at) = drag_result(&g.edges, g.bound, g.at) {
        set_bound(g.date, g.bound, at);
    }
    // Ohne Änderung (oder falls das Speichern scheiterte) wieder der gespeicherte Stand.
    DATA.with(|d| d.borrow_mut().rebuild());
    render();
    set_hover(view, Some(p));
}

/// Esc während des Ziehens: alles wie vorher.
fn cancel_drag(view: &WeekView) {
    DRAG.with(|g| g.borrow_mut().take());
    restore_cursor(view);
    DATA.with(|d| d.borrow_mut().rebuild());
    render();
}

/// Zeiger zum Verschieben einer Kante nach oben oder unten.
fn resize_cursor() -> Retained<NSCursor> {
    // Die Nachfolger (`frameResizeCursorFromPosition:inDirections:` …) gibt es erst ab
    // macOS 15; Tagwerk läuft ab macOS 13.
    #[allow(deprecated)]
    NSCursor::resizeUpDownCursor()
}

fn restore_cursor(view: &WeekView) {
    if let Some(w) = view.window() {
        w.enableCursorRects();
        w.invalidateCursorRectsForView(view);
    }
}

/// Arbeitsbeginn bzw. Feierabend speichern (`None`: wieder automatisch).
fn set_bound(date: NaiveDate, bound: Bound, at: Option<i64>) {
    if let Some(f) = SET_BOUND.with(|c| c.get()) {
        f(date, bound, at);
    }
}

/// Arbeitsbeginn und Feierabend des Tages `date` in der angezeigten Woche.
fn edges_of(date: NaiveDate) -> Option<Edges> {
    DATA.with(|d| d.try_borrow().ok()?.cols.iter().find(|c| c.date == date)?.edges)
}

/// Kontextmenü: Arbeitsbeginn bzw. Feierabend ändern (Dialog) oder zurücksetzen.
fn pending_bound(bound: Bound, reset: bool) {
    let Some(ctx) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    if reset {
        set_bound(ctx.date, bound, None);
    } else {
        show_bound_dialog(ctx.date, bound);
    }
}

/// VoiceOver: Arbeitsbeginn bzw. Feierabend um 5 Minuten verschieben (`later`: später), auf
/// das 5-Minuten-Raster. `false`, wenn es nicht weiter geht.
fn nudge(date: NaiveDate, bound: Bound, later: bool) -> bool {
    let found = DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let snap = d.snap.as_ref()?;
        let e = d.cols.iter().find(|c| c.date == date)?.edges?;
        let limits = drag_limits(&e, bound, &snap.settings, free_limit(date, snap.now), (0, DAY_END))?;
        Some((e, limits))
    });
    let Some((e, (lo, hi))) = found else { return false };
    let current = e.get(bound).0;
    let at = if later { (current.div_euclid(300) + 1) * 300 } else { ((current + 299).div_euclid(300) - 1) * 300 };
    let at = at.clamp(lo, hi);
    if at == current {
        return false;
    }
    if let Some(at) = drag_result(&e, bound, at) {
        set_bound(date, bound, at);
    }
    true
}

/// Dialog „Arbeitsbeginn ändern“ bzw. „Feierabend ändern“ als Sheet am Fenster.
fn show_bound_dialog(date: NaiveDate, bound: Bound) {
    let Some(window) = UI.with(|u| u.borrow().as_ref().and_then(|u| u.view.window())) else { return };
    let Some(e) = edges_of(date) else { return };
    if window.attachedSheet().is_some() {
        return;
    }
    let mtm = mtm();
    let field = records::time_field(date, e.get(bound).0);
    let today = DATA.with(|d| d.try_borrow().ok()?.snap.as_ref().map(|s| s.now.date())) == Some(date);
    let (title, text) = match bound {
        Bound::Start => (
            "Arbeitsbeginn ändern",
            "Ohne Angabe beginnt der Tag mit dem ersten Entsperren. Arbeit ohne Mac davor entfällt – \
             die letzte Angabe gilt.",
        ),
        Bound::End => (
            "Feierabend ändern",
            "Ohne Angabe endet der Tag mit dem letzten Sperren. Arbeit ohne Mac danach entfällt – \
             die letzte Angabe gilt.",
        ),
    };
    let mut info = format!("{}: {text}", fmt_day(date));
    if today && bound == Bound::End {
        info.push_str(" Mit Feierabend wird heute nichts mehr erfasst.");
    }
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns(title));
    alert.setInformativeText(&ns(&info));
    alert.addButtonWithTitle(&ns("Übernehmen"));
    let cancel = alert.addButtonWithTitle(&ns("Abbrechen"));
    cancel.setKeyEquivalent(&ns("\u{1b}"));
    field.setFrameSize(field.fittingSize());
    alert.setAccessoryView(Some(&field));
    alert.layout();
    alert.window().setInitialFirstResponder(Some(&field));
    let win = window.clone();
    let handler = RcBlock::new(move |response: NSModalResponse| {
        if response != NSAlertFirstButtonReturn {
            return;
        }
        let Some(at) = records::picker_secs(&field) else { return };
        let problem = match bound {
            Bound::Start if at >= e.end => Some("Der Arbeitsbeginn muss vor dem Feierabend liegen."),
            Bound::End if at <= e.start => Some("Der Feierabend muss nach dem Arbeitsbeginn liegen."),
            _ => None,
        };
        if let Some(text) = problem {
            // Erst wenn das Sheet geschlossen ist.
            let win = win.clone();
            let block = RcBlock::new(move |_timer| records::show_info_sheet(&win, title, text));
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &block) };
            return;
        }
        if let Some(at) = drag_result(&e, bound, at) {
            set_bound(date, bound, at);
        }
    });
    alert.beginSheetModalForWindow_completionHandler(&window, Some(&handler));
}

fn hit_test(view: &WeekView, p: NSPoint) -> Option<Hit> {
    DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let geo = Geo::new(view.bounds(), &d);
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
            if !c.locked && c.manual.iter().any(|m| contains(geo.span_rect(col, m.span), p)) {
                return Some(Hit::Work);
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
    if c.locked {
        return None;
    }
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
    let Some(limit) = free_limit(c.date, now).filter(|_| !c.locked) else { return Vec::new() };
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
/// `error` zeigt rot über den Feldern einen Hinweis zur vorigen, abgelehnten Eingabe (dann
/// sind die Felder schon gefüllt) – wie im Inspektor.
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
    if let Some((text, ..)) = error {
        // So breit wie die Felder darunter, damit der Dialog nicht breiter wird.
        let width = times.fittingSize().width.max(why.fittingSize().width);
        let problem = NSTextField::wrappingLabelWithString(&ns(text), mtm);
        problem.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        problem.setTextColor(Some(&NSColor::systemRedColor()));
        problem.setSelectable(false);
        problem.setPreferredMaxLayoutWidth(width);
        problem.widthAnchor().constraintLessThanOrEqualToConstant(width).setActive(true);
        stack.addArrangedSubview(&problem);
    }
    stack.addArrangedSubview(&times);
    stack.addArrangedSubview(&why);
    // Felder bündig unter „Von“ bzw. „Grund“ (erst jetzt: beide brauchen eine gemeinsame
    // Ansicht, sonst wirft AppKit eine Ausnahme).
    grund.widthAnchor().constraintEqualToAnchor(&von.widthAnchor()).setActive(true);
    stack.setFrameSize(stack.fittingSize());

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns("Arbeit ohne Mac eintragen"));
    alert.setInformativeText(&ns(&format!(
        "{}: Telefonat, Meeting oder Termin bei gesperrtem Mac zählt als Arbeitszeit. \
         Eine Pause in dieser Zeit entfällt.",
        fmt_day(date)
    )));
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

/// Was an der Stelle `p` möglich ist (`None` neben den Tagesspalten).
fn context_at(view: &WeekView, p: NSPoint) -> Option<Context> {
    DATA.with(|d| {
        let d = d.try_borrow().ok()?;
        let geo = Geo::new(view.bounds(), &d);
        let col = geo.column_at(p.x)?;
        let c = d.cols.get(col)?;
        let label = NSColor::labelColor();
        let fonts = Fonts::new(&label, &label);
        let time = block_boxes(&geo, &d, &fonts).into_iter().filter(|bx| bx.col == col).find_map(|bx| {
            let piece = bx.pieces.iter().find(|piece| piece.part.is_some() && contains(hit_rect(piece), p))?;
            let flash = Flash { date: c.date, from: bx.block.from, kind: bx.block.kind, part: piece.part?, generation: 0 };
            Some((flash, piece.text.clone()))
        });
        let day = d.snap.as_ref().and_then(|snap| snap.days.iter().find(|x| x.date == c.date));
        let absence = SET_ABSENCE.with(|c| c.get()).and_then(|_| {
            let snap = d.snap.as_ref()?;
            let current = day.and_then(|x| x.absence);
            // An freien Tagen gibt es kein Soll – höchstens eine Abwesenheit zum Entfernen.
            (snap.settings.is_workday(c.date) || current.is_some()).then_some(current)
        });
        // Heute zählen für die Abwesenheit nur Einträge von Hand (siehe `Day::accepts_absence`).
        let today = d.snap.as_ref().is_some_and(|s| s.now.date() == c.date);
        let busy = day.is_some_and(|x| x.blocks_full_absence(today));
        // Nur mit Einträgen außer der Abwesenheit – die allein entfernt „Abwesenheit → Keine“.
        let clearable = CLEAR_DAY.with(|c| c.get()).is_some() && day.is_some_and(Day::has_entries);
        let mut ctx = Context {
            date: c.date,
            work: None,
            preset: None,
            time,
            absence,
            busy,
            clearable,
            edges: c.edges,
            bounds: Vec::new(),
        };
        // Arbeitsbeginn auf dem ersten Abschnitt, Feierabend auf dem letzten (oder an der Kante).
        if bounds_editable() {
            let zones = edge_rects(&geo, &d);
            let mut work = c.blocks.iter().filter(|b| b.kind == Kind::Work);
            let first = work.next();
            let last = work.next_back().or(first);
            for (bound, block) in [(Bound::Start, first), (Bound::End, last)] {
                let zone = zones.iter().find(|(i, b, _)| *i == col && *b == bound).map(|(.., r)| *r);
                let on_block = block.is_some_and(|b| contains(geo.block_rect(col, b), p));
                if zone.is_some_and(|r| on_block || contains(r, p)) {
                    ctx.bounds.push(bound);
                }
            }
        }
        if editable() {
            ctx.work = c.manual.iter().filter(|_| !c.locked).find(|m| contains(geo.span_rect(col, m.span), p)).map(|m| (m.block, m.note.clone()));
            if ctx.work.is_none() {
                let snap = d.snap.as_ref()?;
                ctx.preset = geo.secs_at(p.y).and_then(|at| free_preset(c, at, snap.now, snap.settings.min_work));
            }
        }
        Some(ctx)
    })
}

/// Lässt sich der Tag in „Tage“ bearbeiten? Nur mit Fenster „Aufzeichnungen“ und nicht in
/// der Zukunft.
fn can_open(ctx: &Context) -> bool {
    let today = DATA.with(|d| d.try_borrow().ok().and_then(|d| d.snap.as_ref().map(|s| s.now.date())));
    OPEN_DAY.with(|c| c.get()).is_some() && today.is_some_and(|t| ctx.date <= t)
}

/// Den Tag in der Ansicht „Tage“ zeigen (schließt vorher die Hinweise).
fn open_day(date: NaiveDate) {
    if let Some(p) = POPOVER.with(|p| p.borrow_mut().take()) {
        p.close();
    }
    if let Some(f) = OPEN_DAY.with(|c| c.get()) {
        f(date);
    }
}

/// Einträge des Kontextmenüs für `ctx`: Titel und Aktion (`None` = nur Anzeige, leerer Titel
/// = Trennlinie). Leer, wenn es an der Stelle nichts zu tun gibt.
fn context_entries(ctx: &Context, can_open: bool) -> Vec<(String, Option<Sel>)> {
    let mut out: Vec<(String, Option<Sel>)> = Vec::new();
    if can_open {
        out.push(("Tag bearbeiten".to_string(), Some(sel!(openDay:))));
    }
    if let Some((_, text)) = &ctx.time {
        out.push((format!("Uhrzeit {text} kopieren"), Some(sel!(copyTime:))));
    }
    if ctx.preset.is_some() {
        out.push(("Arbeit ohne Mac eintragen …".to_string(), Some(sel!(addWorkHere:))));
    }
    if ctx.absence.is_some() {
        // Wird zum Untermenü mit den Arten (siehe `context_menu`).
        out.push((ABSENCE_MENU.to_string(), Some(sel!(setAbsence:))));
    }
    if ctx.clearable {
        out.push(("Alle Einträge löschen …".to_string(), Some(sel!(clearDay:))));
    }
    if let Some((block, note)) = &ctx.work {
        if !out.is_empty() {
            out.push((String::new(), None));
        }
        let what = if note.is_empty() { "Arbeit ohne Mac" } else { note };
        out.push((format!("{what} {}", fmt_span(*block)), None));
        out.push(("Arbeit ohne Mac löschen".to_string(), Some(sel!(removeWork:))));
    }
    if let (Some(e), false) = (ctx.edges, ctx.bounds.is_empty()) {
        if !out.is_empty() {
            out.push((String::new(), None));
        }
        for &bound in &ctx.bounds {
            let (what, change, reset) = match bound {
                Bound::Start => ("Arbeitsbeginn", sel!(changeStart:), sel!(resetStart:)),
                Bound::End => ("Feierabend", sel!(changeEnd:), sel!(resetEnd:)),
            };
            out.push((format!("{what} ändern …"), Some(change)));
            if let (_, Some(auto), true) = e.get(bound) {
                out.push((format!("{what} zurücksetzen ({} Uhr)", fmt_clock(auto)), Some(reset)));
            }
        }
    }
    if out.is_empty() {
        return out;
    }
    // Der Tag als Überschrift.
    let mut all = vec![(fmt_day(ctx.date), None), (String::new(), None)];
    all.append(&mut out);
    all
}

/// Titel des Untermenüs zur Abwesenheit.
const ABSENCE_MENU: &str = "Abwesenheit";

/// Punkt im Untermenü „Abwesenheit“.
#[derive(Clone, Debug, PartialEq)]
struct AbsenceItem {
    /// Leer = Trennlinie.
    title: String,
    /// Siehe [`absence_of_tag`] (negativ: nur Anzeige).
    tag: isize,
    /// Die eingetragene Abwesenheit (Häkchen).
    checked: bool,
    enabled: bool,
}

/// Hinweis im Untermenü, wenn der Tag Einträge hat.
const ABSENCE_BLOCKED: &str = "Ganztags erst nach „Alle Einträge löschen“";

/// Punkte im Untermenü „Abwesenheit“ (`busy`: Der Tag hat andere Einträge – dann gehen
/// ganze Tage erst, wenn sie gelöscht sind, siehe [`Day::accepts_absence`]).
fn absence_entries(current: Option<Absence>, busy: bool) -> Vec<AbsenceItem> {
    let item = |title: String, tag: isize, checked: bool, enabled: bool| AbsenceItem { title, tag, checked, enabled };
    let mut out = vec![item("Keine".to_string(), 0, current.is_none(), true), item(String::new(), -1, false, false)];
    for (i, kind) in AbsenceKind::ALL.iter().enumerate() {
        // Halbe Tage gibt es bei Urlaub, Krankheit und Feiertag.
        let halves: &[bool] = if kind.reduces_target() { &[false, true] } else { &[false] };
        for &half in halves {
            let a = Absence::new(*kind, half);
            let checked = current == Some(a);
            out.push(item(a.label(), 1 + 2 * i as isize + half as isize, checked, checked || half || !busy));
        }
    }
    if busy {
        out.push(item(String::new(), -1, false, false));
        out.push(item(ABSENCE_BLOCKED.to_string(), -1, false, false));
    }
    out
}

/// Wahl im Untermenü „Abwesenheit“ (`Some(None)`: keine).
fn absence_of_tag(tag: isize) -> Option<Option<Absence>> {
    if tag == 0 {
        return Some(None);
    }
    let kind = AbsenceKind::ALL.get(usize::try_from((tag - 1) / 2).ok()?)?;
    (tag > 0).then(|| Some(Absence::new(*kind, (tag - 1) % 2 == 1)))
}

/// Kontextmenü für `ctx` (`None`, wenn es an der Stelle nichts zu tun gibt).
fn context_menu(ctx: &Context) -> Option<Retained<NSMenu>> {
    let controller = UI.with(|u| u.borrow().as_ref().map(|u| u._controller.clone()))?;
    let entries = context_entries(ctx, can_open(ctx));
    if entries.is_empty() {
        return None;
    }
    let mtm = mtm();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    for (title, action) in entries {
        if title.is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(&title), action, &ns(""))
        };
        match action {
            Some(a) if a == sel!(setAbsence:) => {
                let sub = NSMenu::new(mtm);
                sub.setAutoenablesItems(false);
                for e in absence_entries(ctx.absence.flatten(), ctx.busy) {
                    if e.title.is_empty() {
                        sub.addItem(&NSMenuItem::separatorItem(mtm));
                        continue;
                    }
                    let action = (e.tag >= 0).then_some(a);
                    let choice = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(&e.title), action, &ns(""))
                    };
                    if action.is_some() {
                        unsafe { choice.setTarget(Some(&controller)) };
                    }
                    choice.setTag(e.tag);
                    choice.setEnabled(e.enabled);
                    choice.setState(if e.checked { NSControlStateValueOn } else { NSControlStateValueOff });
                    sub.addItem(&choice);
                }
                unsafe { item.setAction(None) };
                item.setSubmenu(Some(&sub));
            }
            Some(_) => unsafe { item.setTarget(Some(&controller)) },
            None => item.setEnabled(false),
        }
        menu.addItem(&item);
    }
    Some(menu)
}

/// Kontextmenü an der Stelle `p` zeigen (Klick auf Arbeit ohne Mac).
fn show_context_menu(view: &WeekView, ctx: Context, p: NSPoint) {
    let Some(menu) = context_menu(&ctx) else { return };
    PENDING.with(|c| *c.borrow_mut() = Some(ctx));
    menu.popUpMenuPositioningItem_atLocation_inView(None, p, Some(view));
}

// ---------- Bedienungshilfen ----------

const WEEKDAYS_LONG: [&str; 7] = ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"];

/// Text zum Vorlesen: ohne die Trennpunkte der Anzeige.
fn spoken(text: &str) -> String {
    text.replace("  ·  ", ", ")
}

/// `3 Std. 35 Min.` bzw. `45 Min.` (zum Vorlesen; `3:35 h` liest VoiceOver als Uhrzeit).
fn spoken_dur(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match (m / 60, m % 60) {
        (0, min) => format!("{min} Min."),
        (h, 0) => format!("{h} Std."),
        (h, min) => format!("{h} Std. {min} Min."),
    }
}

/// Hängt `more` als weiteren Satz an (ohne doppelten Punkt nach „Min.“).
fn add_sentence(text: &mut String, more: &str) {
    text.push_str(if text.ends_with('.') { " " } else { ". " });
    text.push_str(more);
}

/// Wofür ein Element der Bedienungshilfen steht.
#[derive(Clone, Copy, Debug, PartialEq)]
enum AxPart {
    /// Tageskopf.
    Header,
    /// Abschnitt, Pause oder Arbeit ohne Mac im Raster.
    Span(Span),
}

/// Was VoiceOver in der Woche vorliest: je Tag der Kopf mit Arbeitszeit, Pausen und
/// Hinweisen, danach der Reihe nach die Abschnitte, Pausen und Arbeit ohne Mac.
fn ax_items(cols: &[Column]) -> Vec<(usize, AxPart, String)> {
    let mut out = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        let day = format!(
            "{}, {}. {}",
            WEEKDAYS_LONG[c.date.weekday().num_days_from_monday() as usize],
            c.date.day(),
            MONTHS[c.date.month0() as usize]
        );
        let mut head = if c.blocks.is_empty() {
            format!("{day}: keine Arbeitszeit")
        } else {
            format!("{day}: Arbeitszeit {}, Pausen {}", spoken_dur(c.work_secs), spoken_dur(c.pause_secs))
        };
        if let Some((_, absence)) = &c.absence {
            add_sentence(&mut head, absence);
        }
        if let Some(target) = c.missing {
            add_sentence(&mut head, &format!("Nichts erfasst, Saldo minus {}", spoken_dur(target)));
        }
        for w in &c.warnings {
            add_sentence(&mut head, &format!("Hinweis: {w}"));
        }
        out.push((i, AxPart::Header, head));

        let range = |sp: Span| format!("{} bis {} Uhr", fmt_clock(sp.from), fmt_clock(sp.to));
        let mut parts: Vec<(Span, String)> = Vec::new();
        for b in &c.blocks {
            let sp = Span::new(b.from, b.to);
            let len = spoken_dur(records::shown_len(b.from, b.to));
            let mut text = match (b.kind, b.running) {
                (Kind::Outside, _) => format!("Nicht gezählt, außerhalb des Arbeitszeitrahmens: {}, {len}", range(sp)),
                (Kind::Work, true) => format!("Arbeit seit {} Uhr, läuft, bisher {len}", fmt_clock(b.from)),
                (Kind::Work, false) => format!("Arbeit {}, {len}", range(sp)),
            };
            // Arbeit ohne Mac ist ein eigenes Element (siehe unten).
            for n in b.notes.iter().filter(|n| !n.starts_with("ohne Mac")) {
                add_sentence(&mut text, n);
            }
            parts.push((sp, text));
        }
        for g in &c.gaps {
            parts.push((*g, format!("Pause {}, {}", range(*g), spoken_dur(records::shown_len(g.from, g.to)))));
        }
        for m in &c.manual {
            let what = if m.note.is_empty() { "Arbeit ohne Mac".to_string() } else { format!("Arbeit ohne Mac, {}", m.note) };
            parts.push((m.span, format!("{what}: {}", range(m.span))));
        }
        parts.sort_by_key(|(sp, _)| (sp.from, sp.to));
        out.extend(parts.into_iter().map(|(sp, text)| (i, AxPart::Span(sp), text)));
    }
    out
}

/// Elemente der Bedienungshilfen für den aktuellen Stand. Sie tragen ihre Lage auf dem
/// Bildschirm und werden deshalb neu gebaut, sobald sich Anzeige, Größe oder Lage ändern –
/// sonst bleiben es dieselben Objekte, damit VoiceOver seine Stelle behält.
fn ax_children(view: &WeekView) -> Retained<NSArray<AnyObject>> {
    let on_screen = |r: NSRect| match view.window() {
        Some(w) => w.convertRectToScreen(view.convertRect_toView(r, None)),
        None => r,
    };
    let frame = on_screen(view.bounds());
    let key: AxKey = (
        GENERATION.with(|g| g.get()),
        [frame.origin.x, frame.origin.y, frame.size.width, frame.size.height].map(|v| v.round() as i64),
    );
    if let Some(children) = AX.with(|a| a.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, c)| c.clone())) {
        return children;
    }
    let elements: Vec<Retained<AnyObject>> = DATA.with(|d| {
        let Ok(d) = d.try_borrow() else { return Vec::new() };
        let geo = Geo::new(view.bounds(), &d);
        let zones = if bounds_editable() { edge_rects(&geo, &d) } else { Vec::new() };
        ax_items(&d.cols)
            .into_iter()
            .map(|(col, part, text)| {
                let rect = match part {
                    AxPart::Header => geo.header_rect(col),
                    AxPart::Span(sp) => geo.span_rect(col, sp),
                };
                let element = unsafe {
                    NSAccessibilityElement::accessibilityElementWithRole_frame_label_parent(
                        NSAccessibilityStaticTextRole,
                        on_screen(rect),
                        Some(&ns(&text)),
                        Some(view),
                    )
                };
                // Erster und letzter Abschnitt: Arbeitsbeginn bzw. Feierabend als Aktionen.
                let c = &d.cols[col];
                if let (AxPart::Span(sp), Some(e)) = (part, c.edges) {
                    let work = |b: &&Block| b.kind == Kind::Work;
                    let is = |b: Option<&Block>| b.is_some_and(|b| Span::new(b.from, b.to) == sp);
                    let bounds: Vec<Bound> = [
                        (Bound::Start, is(c.blocks.iter().find(work))),
                        (Bound::End, is(c.blocks.iter().rfind(work))),
                    ]
                    .into_iter()
                    .filter(|&(b, on)| on && zones.iter().any(|&(i, zb, _)| i == col && zb == b))
                    .map(|(b, _)| b)
                    .collect();
                    if !bounds.is_empty() {
                        let actions = bound_actions(c.date, &bounds, &e);
                        let _: () = unsafe { msg_send![&*element, setAccessibilityCustomActions: &*actions] };
                    }
                }
                element
            })
            .collect()
    });
    let children = NSArray::from_retained_slice(&elements);
    AX.with(|a| *a.borrow_mut() = Some((key, children.clone())));
    children
}

/// Aktionen für VoiceOver an einem Abschnitt: Arbeitsbeginn bzw. Feierabend 5 Minuten früher
/// oder später, ändern (Dialog) und – wenn manuell festgelegt – zurücksetzen.
fn bound_actions(date: NaiveDate, bounds: &[Bound], e: &Edges) -> Retained<NSArray<NSAccessibilityCustomAction>> {
    let mut out = Vec::new();
    let mut add = |name: String, f: Box<dyn Fn() -> bool>| {
        let handler = RcBlock::new(move || Bool::new(f()));
        let alloc = <NSAccessibilityCustomAction as objc2::AnyThread>::alloc();
        out.push(NSAccessibilityCustomAction::initWithName_handler(alloc, &ns(&name), Some(&handler)));
    };
    for &bound in bounds {
        let what = match bound {
            Bound::Start => "Arbeitsbeginn",
            Bound::End => "Feierabend",
        };
        add(format!("{what} 5 Minuten früher"), Box::new(move || nudge(date, bound, false)));
        add(format!("{what} 5 Minuten später"), Box::new(move || nudge(date, bound, true)));
        add(
            format!("{what} ändern …"),
            Box::new(move || {
                show_bound_dialog(date, bound);
                true
            }),
        );
        if e.get(bound).2 {
            add(
                format!("{what} zurücksetzen"),
                Box::new(move || {
                    set_bound(date, bound, None);
                    true
                }),
            );
        }
    }
    NSArray::from_retained_slice(&out)
}

/// Anklickbare Bereiche (für die Zeigehand).
fn clickable_rects(view: &WeekView) -> Vec<NSRect> {
    DATA.with(|d| {
        let Ok(d) = d.try_borrow() else { return Vec::new() };
        let geo = Geo::new(view.bounds(), &d);
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
            // Freie Bereiche samt Pausen: Ein Klick trägt Arbeit ohne Mac ein (ohne die Kanten,
            // die sich verschieben lassen – dort gilt ein anderer Zeiger).
            if let Some(now) = d.snap.as_ref().map(|s| s.now) {
                let zones: Vec<NSRect> =
                    if bounds_editable() { edge_rects(&geo, &d).into_iter().map(|(.., r)| r).collect() } else { Vec::new() };
                let (geo, zones) = (&geo, &zones);
                out.extend(d.cols.iter().enumerate().flat_map(|(i, c)| {
                    free_spans(c, now)
                        .into_iter()
                        .map(move |sp| without_zones(geo.gap_rect(i, sp), zones))
                        .filter(|r| r.size.height > 0.0)
                }));
            }
            let geo = &geo;
            out.extend(
                d.cols
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| !c.locked)
                    .flat_map(|(i, c)| c.manual.iter().map(move |m| geo.span_rect(i, m.span))),
            );
        }
        out
    })
}

/// Bereiche der Kanten, die sich verschieben lassen (für den Zeiger nach oben und unten).
fn edge_cursor_rects(view: &WeekView) -> Vec<NSRect> {
    if !bounds_editable() {
        return Vec::new();
    }
    DATA.with(|d| {
        let Ok(d) = d.try_borrow() else { return Vec::new() };
        edge_rects(&Geo::new(view.bounds(), &d), &d).into_iter().map(|(.., r)| r).collect()
    })
}

/// `r` ohne die Teile oben und unten, die in einem der Bereiche `zones` liegen.
fn without_zones(r: NSRect, zones: &[NSRect]) -> NSRect {
    let (mut top, mut bottom) = (r.origin.y, r.origin.y + r.size.height);
    let (left, right) = (r.origin.x, r.origin.x + r.size.width);
    for z in zones {
        let (zt, zb) = (z.origin.y, z.origin.y + z.size.height);
        if z.origin.x >= right || z.origin.x + z.size.width <= left {
            continue;
        }
        if zt <= top && top < zb {
            top = zb;
        }
        if zt < bottom && bottom <= zb {
            bottom = zt;
        }
    }
    NSRect::new(NSPoint::new(r.origin.x, top), NSSize::new(r.size.width, (bottom - top).max(0.0)))
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

/// y der Kante `bound` in der Spalte `col` (dort sitzt der Griff).
fn edge_y(geo: &Geo, e: &Edges, bound: Bound) -> f64 {
    match bound {
        Bound::Start => geo.y(e.start) + 0.5,
        Bound::End => geo.y(e.end) - 0.5,
    }
}

/// Das Raster zeichnen. `grip`: Kante mit Griff (unter der Maus oder beim Ziehen), `drag`:
/// Schild und entfallende Arbeit ohne Mac beim Ziehen.
fn draw(
    bounds: NSRect,
    d: &Data,
    flash: Option<&Flash>,
    hover: Option<(NaiveDate, Span)>,
    grip: Option<(usize, Bound)>,
    drag: Option<&DragShow>,
) {
    let geo = Geo::new(bounds, d);
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

    // Arbeitstage nach den Einstellungen (ohne Stand: Montag bis Freitag).
    let is_workday =
        |date: NaiveDate| d.snap.as_ref().map_or(!Settings::is_weekend(date), |s| s.settings.is_workday(date));
    // Schattiert: freie Tage (kein Soll) und die Zeit außerhalb des Arbeitszeitrahmens
    // (wird nicht gezählt).
    for (i, c) in d.cols.iter().enumerate() {
        let x = geo.x(i);
        if !is_workday(c.date) {
            fill(NSRect::new(NSPoint::new(x, HEADER_H), NSSize::new(geo.col_w(i), geo.h - HEADER_H)), &shade);
        } else if let Some(s) = &d.snap {
            for (a, b) in [(geo.h0 * 3600, s.settings.frame_from), (s.settings.frame_to, geo.h1 * 3600)] {
                if b > a {
                    let (ya, yb) = (geo.y(a), geo.y(b));
                    fill(NSRect::new(NSPoint::new(x, ya), NSSize::new(geo.col_w(i), yb - ya)), &shade);
                }
            }
        }
        if Some(c.date) == today {
            fill(geo.header_rect(i), &accent.colorWithAlphaComponent(0.05));
            let body = NSRect::new(NSPoint::new(x, HEADER_H), NSSize::new(geo.col_w(i), geo.h - HEADER_H));
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
    let selected = selected_day();
    let on_accent = NSColor::whiteColor();
    let magenta = palette::selection();
    for (i, c) in d.cols.iter().enumerate() {
        let x = geo.x(i);
        let is_today = Some(c.date) == today;
        let is_selected = Some(c.date) == selected;
        let warn = !c.warnings.is_empty();
        // Tag in Bearbeitung: das Datum weiß auf einer Pille in Magenta.
        let color = if is_selected {
            &on_accent
        } else if is_today {
            &accent
        } else if is_workday(c.date) {
            &label
        } else {
            &secondary
        };
        let title = format!("{} {}", WEEKDAYS_SHORT[i], c.date.format("%d.%m."));
        let a = attrs(&font(12.0, if is_today { semibold } else { medium }), color, NSTextAlignment::Left);
        let tw = text_width(&title, &a);
        let icon_w = match (&icon, warn) {
            (Some(img), true) => img.size().width + 4.0,
            _ => 0.0,
        };
        // Die Pille ragt links und rechts über das Datum hinaus; das Warnschild steht davor –
        // mit demselben Abstand wie sonst zum Datum.
        let pad = if is_selected { 7.0 } else { 0.0 };
        let x0 = (x + (geo.col_w(i) - icon_w - tw - 2.0 * pad) / 2.0).max(x + 2.0).round();
        if let (Some(img), true) = (&icon, warn) {
            let sz = img.size();
            img.drawInRect(NSRect::new(NSPoint::new(x0, 7.0 + (15.0 - sz.height) / 2.0), sz));
        }
        let text_x = x0 + icon_w + pad;
        let title_rect = NSRect::new(NSPoint::new(text_x, 7.0), NSSize::new(geo.x(i + 1) - 2.0 - text_x, 16.0));
        if is_selected {
            let pill = NSRect::new(
                NSPoint::new(x0 + icon_w, 5.0),
                NSSize::new((tw + 2.0 * pad).min(geo.x(i + 1) - 2.0 - x0 - icon_w), 19.0),
            );
            round_fill(pill, 9.5, &magenta);
        }
        text_in(&title, title_rect, &a);
        // Ohne Arbeitszeit steht dort die Abwesenheit, an einem Arbeitstag ohne Eintrag das
        // fehlende Soll in Rot (sonst ein Strich).
        let total = match (&c.absence, c.work_secs > 0, c.missing) {
            (_, true, _) => store::fmt_hm(c.work_secs),
            (Some((short, _)), false, _) => short.clone(),
            (None, false, Some(target)) => rules::fmt_balance(-target),
            (None, false, None) => "–".to_string(),
        };
        let minus = palette::balance(-c.missing.unwrap_or(0));
        let total_color = if warn {
            &orange
        } else if c.work_secs == 0 && c.absence.is_none() && c.missing.is_some() {
            &minus
        } else {
            &secondary
        };
        let total_rect = NSRect::new(NSPoint::new(x + 2.0, 24.0), NSSize::new(geo.col_w(i) - 4.0, 15.0));
        // Daneben die Pausen mit Tasse (wie „Pausen heute“ im Menü) – nur wenn es Platz hat.
        let a = attrs(&digits(11.0, regular), total_color, NSTextAlignment::Left);
        let pa = attrs(&digits(11.0, regular), &secondary, NSTextAlignment::Left);
        let pause = store::fmt_hm(c.pause_secs);
        let (tw, pw) = (text_width(&total, &a), text_width(&pause, &pa));
        let cup_w = cup.as_ref().map_or(0.0, |img| img.size().width + 3.0);
        let width = tw + 12.0 + cup_w + pw;
        if c.work_secs > 0 && width <= total_rect.size.width {
            let x0 = (x + (geo.col_w(i) - width) / 2.0).round();
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

    // Abwesenheit: ein Band über den Arbeitszeitrahmen des Tages, mit gestricheltem Rand und
    // der Art oben; Abschnitte, an denen trotzdem gearbeitet wurde, liegen darüber.
    if let Some(s) = &d.snap {
        let teal = palette::absence();
        let band_attrs = attrs(&font(11.0, semibold), &teal, NSTextAlignment::Center);
        for (i, c) in d.cols.iter().enumerate() {
            let Some((short, long)) = &c.absence else { continue };
            let (from, to) = (s.settings.frame_from.max(geo.h0 * 3600), s.settings.frame_to.min(geo.h1 * 3600));
            if to <= from {
                continue;
            }
            let r = geo.span_rect(i, Span::new(from, to));
            round_fill(r, 4.0, &teal.colorWithAlphaComponent(0.12));
            let border = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inset(r, 0.5, 0.5), 4.0, 4.0);
            border.setLineWidth(1.0);
            let dash: [f64; 2] = [4.0, 3.0];
            unsafe { border.setLineDash_count_phase(dash.as_ptr(), 2, 0.0) };
            teal.colorWithAlphaComponent(0.6).setStroke();
            border.stroke();
            // Ausgeschrieben, wenn es passt („Urlaub (halber Tag)“), sonst kurz („½ Urlaub“).
            let room = r.size.width - 8.0;
            let text = if text_width(long, &band_attrs) <= room { long } else { short };
            text_in(text, NSRect::new(NSPoint::new(r.origin.x + 4.0, r.origin.y + 6.0), NSSize::new(room, 15.0)), &band_attrs);
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

    // Arbeitsbeginn und Feierabend als Linie in Textfarbe an der Kante (nicht das „jetzt“
    // eines laufenden Abschnitts) – neutral, damit sie sich vom türkisen Abwesenheitsband
    // eines halben Tages abhebt: erfasst dünn und gedämpft, manuell festgelegt kräftig und
    // mit einem Punkt am rechten Ende.
    let edge_color = NSColor::labelColor();
    let edge_auto = edge_color.colorWithAlphaComponent(0.55);
    for (i, c) in d.cols.iter().enumerate() {
        let Some(e) = c.edges else { continue };
        let (x, w) = (geo.x(i) + 3.0, geo.col_w(i) - 6.0);
        for (bound, manual) in [(Bound::Start, e.manual_start), (Bound::End, e.manual_end)] {
            if bound == Bound::End && e.running {
                continue;
            }
            let y = edge_y(&geo, &e, bound);
            let (thick, color) = if manual { (2.5, &edge_color) } else { (1.5, &edge_auto) };
            // Die Linie liegt innerhalb des Abschnitts.
            let (top, mid) = match bound {
                Bound::Start => (y, y + thick / 2.0),
                Bound::End => (y - thick, y - thick / 2.0),
            };
            round_fill(NSRect::new(NSPoint::new(x, top), NSSize::new(w, thick)), thick / 2.0, color);
            if manual {
                let r = 3.5;
                edge_color.setFill();
                NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                    NSPoint::new(x + w - 2.0 * r, mid - r),
                    NSSize::new(2.0 * r, 2.0 * r),
                ))
                .fill();
            }
        }
    }

    // Beim Ziehen: Arbeit ohne Mac, die mit dem neuen Wert entfiele, gestrichelt.
    if let Some(g) = drag {
        let cut_attrs = attrs(&font(10.0, medium), &orange, NSTextAlignment::Center);
        for sp in &g.cut {
            let r = inset(geo.span_rect(g.col, *sp), 0.75, 0.75);
            if r.size.height <= 0.0 {
                continue;
            }
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 4.0, 4.0);
            orange.colorWithAlphaComponent(0.08).setFill();
            path.fill();
            orange.setStroke();
            path.setLineWidth(1.0);
            let dash: [f64; 2] = [4.0, 3.0];
            // Das Muster hat zwei Einträge.
            unsafe { path.setLineDash_count_phase(dash.as_ptr(), 2, 0.0) };
            path.stroke();
            if r.size.height >= 14.0 {
                let y = r.origin.y + (r.size.height - 13.0) / 2.0;
                text_in("entfällt", NSRect::new(NSPoint::new(r.origin.x, y), NSSize::new(r.size.width, 13.0)), &cut_attrs);
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
            fill(NSRect::new(NSPoint::new(x, y - 0.75), NSSize::new(geo.col_w(i), 1.5)), &red);
            red.setFill();
            NSBezierPath::bezierPathWithOvalInRect(NSRect::new(NSPoint::new(x - 4.0, y - 4.0), NSSize::new(8.0, 8.0)))
                .fill();
        }
    }

    // Griff an der Kante unter der Maus bzw. an der, die gerade gezogen wird.
    if let Some((i, bound)) = grip {
        if let Some(e) = d.cols.get(i).and_then(|c| c.edges) {
            let y = edge_y(&geo, &e, bound);
            let cx = geo.x(i) + geo.col_w(i) / 2.0;
            let (w, h) = (32.0, 6.0);
            let r = NSRect::new(NSPoint::new(cx - w / 2.0, y - h / 2.0), NSSize::new(w, h));
            round_fill(inset(r, -1.5, -1.5), h / 2.0 + 1.5, &NSColor::textBackgroundColor());
            round_fill(r, h / 2.0, &edge_color);
        }
    }

    // Beim Ziehen: Uhrzeit und Abweichung als Schild an der Kante (über dem Beginn, unter dem
    // Feierabend).
    if let Some(g) = drag {
        if let Some(e) = d.cols.get(g.col).and_then(|c| c.edges) {
            let y = edge_y(&geo, &e, g.bound);
            let a = attrs(&digits(11.0, medium), &NSColor::textBackgroundColor(), NSTextAlignment::Center);
            let (w, h) = (text_width(&g.label, &a) + 16.0, 20.0);
            let cx = geo.x(g.col) + geo.col_w(g.col) / 2.0;
            let x = (cx - w / 2.0).clamp(GUTTER_W + 2.0, (geo.w - w - 2.0).max(GUTTER_W + 2.0));
            let top = match g.bound {
                Bound::Start => y - h - 8.0,
                Bound::End => y + 8.0,
            };
            let top = top.clamp(HEADER_H + 2.0, (geo.h - h - 2.0).max(HEADER_H + 2.0));
            let r = NSRect::new(NSPoint::new(x, top), NSSize::new(w, h));
            round_fill(r, h / 2.0, &NSColor::labelColor().colorWithAlphaComponent(0.85));
            text_in(&g.label, NSRect::new(NSPoint::new(x, top + 3.0), NSSize::new(w, 15.0)), &a);
        }
    }

    // Leere Woche – außer es gibt Abwesenheiten: Ihre Bänder sagen schon, warum (der Hinweis
    // läge sonst quer über ihnen).
    if d.cols.iter().all(|c| c.blocks.is_empty() && c.absence.is_none()) {
        let a = attrs(&font(13.0, regular), &secondary, NSTextAlignment::Center);
        let y = (grid_top + grid_bottom) / 2.0 - 9.0;
        text_in(
            "Keine Arbeitszeit in dieser Woche",
            NSRect::new(NSPoint::new(GUTTER_W, y), NSSize::new(geo.w - GUTTER_W, 18.0)),
            &a,
        );
    }

    // Tag in Bearbeitung: ein gestrichelter Rahmen in Magenta um die ganze Spalte (über den
    // Abschnitten) – anders als das türkise Band der Abwesenheit auch um den Tageskopf.
    if let Some(i) = d.cols.iter().position(|c| Some(c.date) == selected) {
        let r = NSRect::new(NSPoint::new(geo.x(i), 0.0), NSSize::new(geo.col_w(i), grid_bottom + GRID_PAD_BOTTOM));
        magenta.setStroke();
        let frame = NSBezierPath::bezierPathWithRect(inset(r, 0.75, 0.75));
        frame.setLineWidth(1.5);
        let dash: [f64; 2] = [5.0, 3.0];
        unsafe { frame.setLineDash_count_phase(dash.as_ptr(), 2, 0.0) };
        frame.stroke();
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
        let geo = Geo::new(view.bounds(), &d);
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
            if let Some((_, absence)) = &c.absence {
                text.push_str(&if text.is_empty() { format!("{}\n{absence}", fmt_day(c.date)) } else { format!("\n{absence}") });
                if c.locked && editable() {
                    text.push_str("\nAndere Einträge erst, wenn die Abwesenheit auf „Keine“ steht (Rechtsklick).");
                }
            }
            if let Some(target) = c.missing {
                text.push_str(&format!("{}\n{}", fmt_day(c.date), missing_text(target)));
            }
            if !c.warnings.is_empty() {
                text.push_str(&if text.is_empty() { format!("Hinweise für {}:", fmt_day(c.date)) } else { "\n\nHinweise:".to_string() });
                for w in &c.warnings {
                    text.push_str(&format!("\n• {w}"));
                }
            }
            let today = d.snap.as_ref().map(|s| s.now.date());
            if OPEN_DAY.with(|c| c.get()).is_some() && today.is_some_and(|t| c.date <= t) {
                text.push_str(if text.is_empty() { "Doppelklick: Tag bearbeiten" } else { "\n\nDoppelklick: Tag bearbeiten" });
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
                text.push_str("\nKlick auf eine Uhrzeit kopiert sie. Rechtsklick: alle Möglichkeiten.");
                if let (Some(e), true, Kind::Work) = (c.edges, bounds_editable() && !c.locked, b.kind) {
                    if b.from == e.start {
                        text.push_str("\nOberkante ziehen: Arbeitsbeginn verschieben.");
                    }
                    if b.to == e.end && !e.running {
                        text.push_str("\nUnterkante ziehen: Feierabend verschieben.");
                    }
                }
                if editable() && !c.locked && c.manual.iter().any(|m| m.span.from < b.to && m.span.to > b.from) {
                    text.push_str("\nKlick auf die Schraffur: Arbeit ohne Mac löschen.");
                }
                out.push((geo.block_rect(i, b), text));
            }
        }
        if editable() {
            for (i, gap, r) in gap_rects(&geo, &d).into_iter().filter(|(i, ..)| !d.cols[*i].locked) {
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
        let summary = d.snap.as_ref().map(|s| week_summary_parts(s, d.monday, &d.cols)).unwrap_or_default();
        (week_title(d.monday), summary, warnings_summary(&d.cols))
    });
    let view = UI.with(|u| {
        let u = u.borrow();
        let u = u.as_ref()?;
        u.title.setStringValue(&ns(&title));
        // Der Saldo farbig wie im Menü.
        let (head, balance) = &summary;
        let font = digits(NSFont::systemFontSize(), unsafe { NSFontWeightRegular });
        let secondary = NSColor::secondaryLabelColor();
        let value = balance.map(|b| (rules::fmt_balance(b), palette::balance(b)));
        let mut parts: Vec<(&str, &NSColor)> = vec![(head.as_str(), &secondary)];
        if let Some((text, color)) = &value {
            parts.push((text.as_str(), color));
        }
        u.summary.setAttributedStringValue(&palette::colored(&parts, &font));
        u.warn_label.setStringValue(&ns(&warn));
        u.warn_row.setHidden(warn.is_empty());
        Some(u.view.clone())
    });
    GENERATION.with(|g| g.set(g.get() + 1));
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
    // Das Raster zeigt sonst nur die Zeit, in der in der Woche gearbeitet wurde.
    let full = NSUserDefaults::standardUserDefaults().boolForKey(&ns(FULL_FRAME_KEY));
    DATA.with(|d| d.borrow_mut().full_frame = full);
    // Als Symbol: Ein Titel nähme der Wochenangabe daneben den Platz.
    let frame =
        unsafe { NSButton::buttonWithTitle_target_action(&ns("Ganzer Rahmen"), Some(target), Some(sel!(toggleFrame:)), mtm) };
    if let Some(img) =
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns("arrow.up.and.down"), Some(&ns("Ganzer Rahmen")))
    {
        frame.setImage(Some(&img));
        frame.setImagePosition(NSCellImagePosition::ImageOnly);
    }
    frame.setButtonType(NSButtonType::PushOnPushOff);
    frame.setState(if full { NSControlStateValueOn } else { NSControlStateValueOff });
    frame.setToolTip(Some(&ns(
        "Den ganzen Arbeitszeitrahmen zeigen – sonst nur die Zeit, in der in dieser Woche gearbeitet wurde",
    )));

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
    for v in [&*nav as &NSView, &today, &frame, &title, &warn_row, &summary] {
        bar.addArrangedSubview(v);
    }
    bar.setCustomSpacing_afterView(14.0, &frame);
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

/// Wie die Wochenansicht einen Tag in der Ansicht „Tage“ öffnet (`None`: gar nicht).
pub fn set_open_day(open_day: Option<OpenDay>) {
    OPEN_DAY.with(|c| c.set(open_day));
}

/// Wie die Wochenansicht den Tag in Bearbeitung erfährt und wählt (`None`: keine Markierung).
pub fn set_selection(selection: Option<Selection>) {
    SELECTION.with(|c| c.set(selection));
}

/// Wie sich Arbeit ohne Mac eintragen und löschen, Arbeitsbeginn und Feierabend festlegen,
/// die Abwesenheit wählen und ein Tag leeren lassen (ohne sie nur ansehen, etwa für
/// Bildschirmfotos).
pub fn set_actions(
    add_work: Option<AddWork>,
    remove_work: Option<RemoveWork>,
    set_bound: Option<SetBound>,
    set_absence: Option<SetAbsence>,
    clear_day: Option<ClearDay>,
) {
    ADD_WORK.with(|c| c.set(add_work));
    REMOVE_WORK.with(|c| c.set(remove_work));
    SET_ABSENCE.with(|c| c.set(set_absence));
    CLEAR_DAY.with(|c| c.set(clear_day));
    SET_BOUND.with(|c| c.set(set_bound));
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

/// Zum Testen: den Arbeitsbeginn des ersten Tages der Woche wie beim Ziehen 20 Minuten
/// später zeigen (`TAGWERK_DRAG=1` bei `--week-screenshot`).
pub fn drag_first_start() {
    DATA.with(|d| {
        let mut d = d.borrow_mut();
        let Some((col, c)) = d.cols.iter().enumerate().find(|(_, c)| c.edges.is_some()) else { return };
        let (Some(edges), Some(snap)) = (c.edges, d.snap.as_ref()) else { return };
        let Some(day) = snap.days.iter().find(|x| x.date == c.date).cloned() else { return };
        let mut g = Drag {
            col,
            date: c.date,
            bound: Bound::Start,
            edges,
            y0: 0.0,
            offset: 0.0,
            active: true,
            at: (edges.start + 20 * 60) / 300 * 300,
            snapped: false,
            cut: Vec::new(),
            days: snap.days.clone(),
            day,
        };
        preview(&mut d, &mut g);
        DRAG.with(|x| *x.borrow_mut() = Some(g));
    });
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
        assert_eq!(hour_range(&Settings::default(), &[], None, false), (7, 22));
        let s = Settings { frame_from: h(6, 30), frame_to: h(18, 15), ..Settings::default() };
        assert_eq!(hour_range(&s, &[], None, false), (6, 19));
        // Ohne Arbeit zeigt auch das kompakte Raster den ganzen Rahmen.
        assert_eq!(hour_range(&s, &[], Some(h(10, 0)), true), (6, 19));
        // Arbeit außerhalb des Rahmens erweitert das Raster.
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(5, 40), h(9, 0));
        d.add_period(h(21, 0), h(22, 30));
        let cols = columns(&snap(vec![d], date(2026, 11, 18).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(hour_range(&Settings::default(), &cols, None, false), (5, 23));
        // Kompakt reicht das Raster genauso weit – die Arbeit außerhalb gehört dazu.
        assert_eq!(hour_range(&Settings::default(), &cols, None, true), (5, 23));
        let outside: Vec<(i64, i64)> =
            cols[1].blocks.iter().filter(|b| b.kind == Kind::Outside).map(|b| (b.from, b.to)).collect();
        assert_eq!(outside, vec![(h(5, 40), h(7, 0)), (h(22, 0), h(22, 30))]);
        assert_eq!(cols[1].warnings.len(), 1);
        assert!(cols[1].warnings[0].contains("außerhalb des Arbeitszeitrahmens"));
    }

    #[test]
    fn compact_hours_follow_the_work() {
        let s = Settings::default();
        let week = |periods: &[(i64, i64)]| {
            let mut d = Day::new(date(2026, 11, 17));
            for &(from, to) in periods {
                d.add_period(from, to);
            }
            columns(&snap(vec![d], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16))
        };
        // 08:27–17:04: eine Stunde Luft davor (aber nicht vor dem Rahmen) und danach.
        let cols = week(&[(h(8, 27), h(12, 0)), (h(12, 45), h(17, 4))]);
        assert_eq!(hour_range(&s, &cols, None, true), (7, 19));
        assert_eq!(hour_range(&s, &cols, None, false), (7, 22), "ganzer Rahmen");
        // In der laufenden Woche gehört „jetzt“ dazu.
        assert_eq!(hour_range(&s, &cols, Some(h(20, 10)), true), (7, 22));
        // Ein kurzer Tag: auf die Mindesthöhe aufgefüllt, innerhalb des Rahmens.
        let short = week(&[(h(10, 0), h(12, 0))]);
        let (h0, h1) = hour_range(&s, &short, None, true);
        assert_eq!(h1 - h0, MIN_HOURS);
        assert!(h0 >= 7 && h0 <= 9 && h1 >= 13 && h1 <= 22, "{h0}–{h1}");
        // Freie Tage ohne Eintrag sind schmal, Arbeitstage und ein Samstag mit Arbeit nicht.
        assert_eq!(column_weights(&s, &cols), [1.0, 1.0, 1.0, 1.0, 1.0, NARROW, NARROW]);
        let mut sat = Day::new(date(2026, 11, 21));
        sat.add_period(h(10, 0), h(11, 0));
        let cols = columns(&snap(vec![sat], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(column_weights(&s, &cols), [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, NARROW]);
        // Vier-Tage-Woche: Der freie Freitag ist ebenfalls schmal.
        let four = Settings { workdays: 0b0_1111, ..s };
        assert_eq!(column_weights(&four, &cols)[4], NARROW);
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
    fn context_menu_offers_what_is_possible() {
        let d = date(2026, 11, 17);
        let titles = |ctx: &Context, can_open: bool| -> Vec<String> {
            context_entries(ctx, can_open).into_iter().map(|(t, _)| t).collect()
        };
        // Auf einem Abschnitt neben der Uhrzeit: nur der Tag.
        let plain = Context {
            date: d,
            work: None,
            preset: None,
            time: None,
            absence: None,
            busy: false,
            clearable: false,
            edges: None,
            bounds: Vec::new(),
        };
        assert_eq!(titles(&plain, true), ["Di, 17.11.2026", "", "Tag bearbeiten"]);
        // Ohne Fenster „Aufzeichnungen“ bzw. in der Zukunft gibt es dort nichts zu tun.
        assert!(context_entries(&plain, false).is_empty());
        // Freier Bereich: eintragen.
        let free = Context { preset: Some(Span::new(h(12, 0), h(12, 30))), ..plain.clone() };
        assert_eq!(titles(&free, true), ["Di, 17.11.2026", "", "Tag bearbeiten", "Arbeit ohne Mac eintragen …"]);
        // Arbeit ohne Mac: mit Zeitraum, löschen.
        let work = Context { work: Some((Span::new(h(14, 0), h(14, 45)), "Meeting".to_string())), ..plain.clone() };
        assert_eq!(
            titles(&work, true),
            ["Di, 17.11.2026", "", "Tag bearbeiten", "", "Meeting 14:00 – 14:45 Uhr (45 Min.)", "Arbeit ohne Mac löschen"]
        );
        let entries = context_entries(&work, false);
        assert_eq!(entries.len(), 4, "ohne „Tag bearbeiten“ keine zweite Trennlinie");
        assert!(entries[2].1.is_none() && entries[3].1.is_some(), "Zeitraum ist nur Anzeige");
        // Abwesenheit als Untermenü – auch für künftige Tage, an denen sonst nichts geht.
        let off = Context { absence: Some(None), ..plain.clone() };
        assert_eq!(titles(&off, false), ["Di, 17.11.2026", "", "Abwesenheit"]);
        assert_eq!(titles(&off, true), ["Di, 17.11.2026", "", "Tag bearbeiten", "Abwesenheit"]);
        // Ein Tag mit Einträgen lässt sich leeren.
        let full = Context { absence: Some(None), busy: true, clearable: true, ..plain.clone() };
        assert_eq!(titles(&full, true), ["Di, 17.11.2026", "", "Tag bearbeiten", "Abwesenheit", "Alle Einträge löschen …"]);
        let half = Absence::new(AbsenceKind::Vacation, true);
        let subs = absence_entries(Some(half), false);
        let names: Vec<&str> = subs.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            names,
            ["Keine", "", "Urlaub", "Urlaub (halber Tag)", "Krank", "Krank (halber Tag)", "Feiertag", "Feiertag (halber Tag)", "Zeitausgleich"]
        );
        assert_eq!(subs.iter().filter(|e| e.checked).map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Urlaub (halber Tag)"]);
        assert!(subs.iter().all(|e| e.enabled || e.title.is_empty()), "leerer Tag: alles wählbar");
        assert!(absence_entries(None, false)[0].checked, "„Keine“ abgehakt");
        // Jeder Punkt führt zu genau seiner Abwesenheit.
        for e in subs.iter().filter(|e| !e.title.is_empty()) {
            let choice = absence_of_tag(e.tag).unwrap();
            assert_eq!(choice.map_or("Keine".to_string(), Absence::label), e.title);
        }
        // Mit Einträgen: nur „Keine“ und halbe Tage, dazu der Hinweis.
        let busy = absence_entries(None, true);
        let enabled: Vec<&str> = busy.iter().filter(|e| e.enabled).map(|e| e.title.as_str()).collect();
        assert_eq!(enabled, ["Keine", "Urlaub (halber Tag)", "Krank (halber Tag)", "Feiertag (halber Tag)"]);
        assert_eq!(busy.last().map(|e| (e.title.as_str(), e.tag, e.enabled)), Some((ABSENCE_BLOCKED, -1, false)));
        assert_eq!(absence_of_tag(-1), None);
        assert_eq!(absence_of_tag(99), None);
        // Auf dem einzigen Abschnitt: Arbeitsbeginn (manuell, also auch zurücksetzen) und Feierabend.
        let e = Edges {
            start: h(7, 30),
            end: h(17, 0),
            auto_start: Some(h(8, 12)),
            auto_end: Some(h(17, 0)),
            manual_start: true,
            manual_end: false,
            running: false,
        };
        let both = Context { edges: Some(e), bounds: vec![Bound::Start, Bound::End], ..plain.clone() };
        assert_eq!(
            titles(&both, true),
            [
                "Di, 17.11.2026",
                "",
                "Tag bearbeiten",
                "",
                "Arbeitsbeginn ändern …",
                "Arbeitsbeginn zurücksetzen (08:12 Uhr)",
                "Feierabend ändern …",
            ]
        );
        let end = Context { bounds: vec![Bound::End], ..both.clone() };
        assert_eq!(titles(&end, false), ["Di, 17.11.2026", "", "Feierabend ändern …"]);
    }

    fn edges(start: i64, end: i64) -> Edges {
        Edges { start, end, auto_start: Some(start), auto_end: Some(end), manual_start: false, manual_end: false, running: false }
    }

    #[test]
    fn edges_of_a_day() {
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d.start = Some(h(7, 30));
        let cols = columns(&snap(vec![d], date(2026, 11, 17).and_hms_opt(20, 0, 0).unwrap(), false), date(2026, 11, 16));
        let e = cols[1].edges.unwrap();
        assert_eq!((e.start, e.auto_start, e.manual_start), (h(7, 30), Some(h(8, 12)), true));
        assert_eq!((e.end, e.auto_end, e.manual_end, e.running), (h(17, 30), Some(h(17, 30)), false, false));
        assert_eq!(e.get(Bound::Start), (h(7, 30), Some(h(8, 12)), true));
        assert!(cols[0].edges.is_none(), "ohne Arbeitszeit nichts zu verschieben");
        // Nur außerhalb des Rahmens gearbeitet: nichts, was zählt, also auch keine Kanten.
        let mut early = Day::new(date(2026, 11, 18));
        early.add_period(h(5, 0), h(6, 30));
        let cols = columns(&snap(vec![early], date(2026, 11, 18).and_hms_opt(20, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert!(cols[2].edges.is_none());
        // Läuft heute noch: Das Ende ist „jetzt“.
        let mut today = Day::new(date(2026, 11, 17));
        today.add_period(h(8, 0), h(9, 30));
        let cols = columns(&snap(vec![today], date(2026, 11, 17).and_hms_opt(9, 30, 0).unwrap(), true), date(2026, 11, 16));
        assert!(cols[1].edges.unwrap().running);
    }

    #[test]
    fn dragging_snaps_and_stays_in_limits() {
        let s = Settings::default(); // Rahmen 07:00–22:00
        let e = edges(h(8, 12), h(17, 0));
        let grid = (h(7, 0), h(19, 0));
        let past = Some(DAY_END - 60);
        assert_eq!(drag_limits(&e, Bound::Start, &s, past, grid), Some((h(7, 0), h(16, 55))));
        assert_eq!(drag_limits(&e, Bound::End, &s, past, grid), Some((h(8, 17), h(19, 0))));
        // Heute höchstens bis jetzt (auf die Minute), in der Zukunft gar nicht.
        assert_eq!(drag_limits(&e, Bound::End, &s, Some(h(17, 20) + 42), grid), Some((h(8, 17), h(17, 20))));
        assert_eq!(drag_limits(&e, Bound::Start, &s, None, grid), None);
        // Grenzen mit Sekunden: auf ganze Minuten nach innen.
        let odd = edges(h(8, 12) + 23, h(17, 0) + 41);
        assert_eq!(drag_limits(&odd, Bound::Start, &s, past, grid), Some((h(7, 0), h(16, 55))));
        assert_eq!(drag_limits(&odd, Bound::End, &s, past, grid), Some((h(8, 18), h(19, 0))));

        let limits = (h(7, 0), h(16, 55));
        let magnet = 5 * 60;
        // 5-Minuten-Raster, mit ⌥ minutengenau.
        assert_eq!(drag_value(h(7, 32) + 40, None, magnet, false, limits), (h(7, 35), false));
        assert_eq!(drag_value(h(7, 32) + 40, None, magnet, true, limits), (h(7, 33), false));
        // Nahe am erfassten Wert rastet die Kante dort ein (auch zwischen den Rasterlinien).
        assert_eq!(drag_value(h(8, 9), Some(h(8, 12) + 23), magnet, false, limits), (h(8, 12) + 23, true));
        assert_eq!(drag_value(h(8, 30), Some(h(8, 12) + 23), magnet, false, limits), (h(8, 30), false));
        // Nicht über die Grenzen hinaus.
        assert_eq!(drag_value(h(5, 0), None, magnet, false, limits), (h(7, 0), false));
        assert_eq!(drag_value(h(18, 0), None, magnet, false, limits), (h(16, 55), false));
    }

    #[test]
    fn drag_result_and_label() {
        let auto = edges(h(8, 12), h(17, 0));
        // Nicht manuell: am erfassten Wert ändert sich nichts, sonst die neue Uhrzeit.
        assert_eq!(drag_result(&auto, Bound::Start, h(8, 12)), None);
        assert_eq!(drag_result(&auto, Bound::Start, h(7, 30)), Some(Some(h(7, 30))));
        // Manuell: zurück auf den erfassten Wert heißt wieder automatisch; derselbe Wert nichts.
        let manual = Edges { start: h(7, 30), manual_start: true, ..auto };
        assert_eq!(drag_result(&manual, Bound::Start, h(8, 12)), Some(None));
        assert_eq!(drag_result(&manual, Bound::Start, h(7, 30)), None);
        assert_eq!(drag_result(&manual, Bound::Start, h(7, 45)), Some(Some(h(7, 45))));
        assert_eq!(drag_result(&manual, Bound::End, h(17, 30)), Some(Some(h(17, 30))));

        assert_eq!(drag_label(Bound::Start, h(7, 30), h(7, 5), Some(h(7, 5))), "Beginn 07:30 · +25 Min.");
        assert_eq!(drag_label(Bound::End, h(15, 55), h(17, 0), None), "Feierabend 15:55 · −1:05 h");
        assert_eq!(drag_label(Bound::Start, h(7, 5) + 12, h(7, 30), Some(h(7, 5) + 12)), "Beginn 07:05 · automatisch");
        assert_eq!(drag_label(Bound::End, h(17, 0), h(17, 0), None), "Feierabend 17:00");
    }

    #[test]
    fn drag_preview_moves_the_edge_and_shows_what_is_cut() {
        let day = date(2026, 11, 17);
        let mut d = Day::new(day);
        d.add_period(h(8, 0), h(12, 0));
        d.add_period(h(12, 45), h(16, 0));
        d.add_work(h(16, 0), h(17, 0), "Meeting");
        let s = snap(vec![d.clone()], date(2026, 11, 18).and_hms_opt(9, 0, 0).unwrap(), false);
        let mut data = Data {
            snap: Some(s.clone()),
            monday: date(2026, 11, 16),
            cols: Vec::new(),
            hours: (7, 22),
            weights: [1.0; 7],
            full_frame: false,
        };
        data.rebuild();
        let e = data.cols[1].edges.unwrap();
        assert_eq!((e.start, e.end), (h(8, 0), h(17, 0)), "Arbeit ohne Mac gehört zum Feierabend");
        let mut g = Drag {
            col: 1,
            date: day,
            bound: Bound::End,
            edges: e,
            y0: 0.0,
            offset: 0.0,
            active: true,
            at: h(16, 30),
            snapped: false,
            cut: Vec::new(),
            days: s.days.clone(),
            day: d,
        };
        preview(&mut data, &mut g);
        let c = &data.cols[1];
        assert_eq!(c.edges.unwrap().end, h(16, 30));
        assert!(c.edges.unwrap().manual_end);
        assert_eq!(c.blocks.last().map(|b| b.to), Some(h(16, 30)));
        assert_eq!(g.cut, vec![Span::new(h(16, 30), h(17, 0))], "das Ende des Meetings entfällt");
        // Zurück auf den alten Wert: wieder der gespeicherte Stand, nichts entfällt.
        g.at = h(17, 0);
        preview(&mut data, &mut g);
        assert_eq!(data.cols[1].edges.unwrap().end, h(17, 0));
        assert!(g.cut.is_empty());
    }

    #[test]
    fn free_areas_leave_room_for_the_edges() {
        let r = |y: f64, hh: f64| NSRect::new(NSPoint::new(10.0, y), NSSize::new(100.0, hh));
        // Freier Bereich über dem Arbeitsbeginn (Kante bei y = 100).
        let zone = r(95.0, 6.5);
        assert_eq!(without_zones(r(40.0, 60.0), &[zone]), r(40.0, 55.0));
        // Unter dem Feierabend.
        let end = r(198.5, 6.5);
        assert_eq!(without_zones(r(200.0, 50.0), &[end]), r(205.0, 45.0));
        // Andere Spalte: unverändert.
        let other = NSRect::new(NSPoint::new(200.0, 95.0), NSSize::new(100.0, 6.5));
        assert_eq!(without_zones(r(40.0, 60.0), &[other]), r(40.0, 60.0));
    }

    #[test]
    fn voiceover_reads_days_blocks_and_pauses() {
        // Dienstag: 08:00–12:00, Pause, 12:45–17:00 mit einem Meeting ohne Mac; Samstag frei.
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(12, 0));
        d.add_period(h(12, 45), h(15, 0));
        d.add_work(h(15, 0), h(17, 0), "Meeting");
        let cols = columns(&snap(vec![d], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        let items = ax_items(&cols);
        let of_day = |col: usize| -> Vec<&str> { items.iter().filter(|(c, ..)| *c == col).map(|(.., t)| t.as_str()).collect() };
        assert_eq!(of_day(0), ["Montag, 16. November: keine Arbeitszeit"]);
        assert_eq!(
            of_day(1),
            [
                "Dienstag, 17. November: Arbeitszeit 8 Std. 15 Min., Pausen 45 Min.",
                "Arbeit 08:00 bis 12:00 Uhr, 4 Std.",
                "Pause 12:00 bis 12:45 Uhr, 45 Min.",
                "Arbeit 12:45 bis 17:00 Uhr, 4 Std. 15 Min.",
                "Arbeit ohne Mac, Meeting: 15:00 bis 17:00 Uhr",
            ]
        );
        assert!(matches!(items[1].1, AxPart::Header) && matches!(items[2].1, AxPart::Span(sp) if sp == Span::new(h(8, 0), h(12, 0))));
        // Hinweise und manuelle Grenzen als eigene Sätze.
        let mut long = Day::new(date(2026, 11, 18));
        long.add_period(h(8, 0), h(18, 30));
        long.start = Some(h(7, 30));
        let cols = columns(&snap(vec![long], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        let items = ax_items(&cols);
        let texts: Vec<&str> = items.iter().filter(|(c, ..)| *c == 2).map(|(.., t)| t.as_str()).collect();
        assert!(texts[0].starts_with("Mittwoch, 18. November: Arbeitszeit 11 Std., Pausen 0 Min. Hinweis: Ab 9 Std."), "{}", texts[0]);
        assert_eq!(texts[1], "Arbeit 07:30 bis 18:30 Uhr, 11 Std. Arbeitsbeginn manuell festgelegt");
        // Abwesenheit im Tageskopf.
        let mut off = Day::new(date(2026, 11, 19));
        off.absence = Some(crate::day::Absence::new(crate::day::AbsenceKind::Vacation, true));
        let cols = columns(&snap(vec![off], date(2026, 11, 22).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(cols[3].absence, Some(("½ Urlaub".to_string(), "Urlaub (halber Tag)".to_string())));
        assert_eq!(ax_items(&cols)[3].2, "Donnerstag, 19. November: keine Arbeitszeit. Urlaub (halber Tag)");
        assert_eq!(spoken_dur(h(0, 5)), "5 Min.");
        assert_eq!(spoken("KW 47  ·  16. – 22. November 2026"), "KW 47, 16. – 22. November 2026");
    }

    #[test]
    fn workdays_without_entries_show_the_missing_target() {
        // Di gearbeitet, Mi nur eine Pause, Do Urlaub, Fr nichts; Stand: Montag darauf.
        let mut tue = Day::new(date(2026, 11, 17));
        tue.add_period(h(8, 0), h(16, 0));
        let mut wed = Day::new(date(2026, 11, 18));
        wed.add_break(h(12, 0), h(12, 30));
        let mut thu = Day::new(date(2026, 11, 19));
        thu.absence = Some(Absence::new(AbsenceKind::Vacation, false));
        let snapshot = snap(vec![tue, wed, thu], date(2026, 11, 23).and_hms_opt(9, 0, 0).unwrap(), false);
        let cols = columns(&snapshot, date(2026, 11, 16));
        let missing: Vec<Option<i64>> = cols.iter().map(|c| c.missing).collect();
        let full = h(7, 48);
        // Mo liegt vor dem ersten erfassten Tag, Sa und So sind frei.
        assert_eq!(missing, [None, None, Some(full), None, Some(full), None, None]);
        assert_eq!(ax_items(&cols).iter().find(|(c, ..)| *c == 4).map(|(.., t)| t.as_str()), Some("Freitag, 20. November: keine Arbeitszeit. Nichts erfasst, Saldo minus 7 Std. 48 Min."));
        // Der Wochensaldo zählt genau diese Tage: 8:00 − 3 × 7:48.
        assert_eq!(week_summary(&snapshot, date(2026, 11, 16), &cols), "Woche: 8:00 h  ·  Pausen: 0:00 h  ·  Saldo −15:24 h");
        // Heute zählt noch nicht.
        let cols = columns(&snap(snapshot.days.clone(), date(2026, 11, 20).and_hms_opt(9, 0, 0).unwrap(), false), date(2026, 11, 16));
        assert_eq!(cols[4].missing, None);
    }

    #[test]
    fn full_day_absence_locks_the_column() {
        // Ganzer Urlaubstag (mit Arbeit ohne Mac aus der Zeit davor) und ein halber.
        let mut full = Day::new(date(2026, 11, 17));
        full.add_work(h(9, 0), h(10, 0), "Meeting");
        full.absence = Some(Absence::new(AbsenceKind::Vacation, false));
        let mut half = Day::new(date(2026, 11, 18));
        half.add_period(h(8, 0), h(12, 0));
        half.absence = Some(Absence::new(AbsenceKind::Vacation, true));
        let now = date(2026, 11, 20).and_hms_opt(9, 0, 0).unwrap();
        let cols = columns(&snap(vec![full, half], now, false), date(2026, 11, 16));
        assert!(cols[1].locked && !cols[2].locked && !cols[0].locked);
        // Gesperrt: kein Vorschlag für Arbeit ohne Mac, keine freien Bereiche.
        assert_eq!(free_preset(&cols[1], h(14, 0), now, 15 * 60), None);
        assert!(free_spans(&cols[1], now).is_empty());
        assert!(free_preset(&cols[2], h(14, 0), now, 15 * 60).is_some());
        assert!(!free_spans(&cols[2], now).is_empty());
        // Auch Arbeitsbeginn und Feierabend lassen sich nur am halben Tag ziehen.
        let mut data = Data {
            snap: Some(snap(vec![full_with_work(), half_with_work()], now, false)),
            monday: date(2026, 11, 16),
            cols: Vec::new(),
            hours: (7, 22),
            weights: [1.0; 7],
            full_frame: false,
        };
        data.rebuild();
        assert!(data.cols[1].locked && data.cols[1].edges.is_some());
        let geo = Geo::new(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 700.0)), &data);
        let cols_with_edges: Vec<usize> = edge_rects(&geo, &data).into_iter().map(|(i, ..)| i).collect();
        assert_eq!(cols_with_edges, [2, 2], "nur der halbe Tag, Beginn und Feierabend");
    }

    /// Ganzer Urlaubstag, an dem trotzdem Zeit erfasst ist (aus der Zeit vor der Sperre).
    fn full_with_work() -> Day {
        let mut d = Day::new(date(2026, 11, 17));
        d.add_period(h(8, 0), h(12, 0));
        d.absence = Some(Absence::new(AbsenceKind::Vacation, false));
        d
    }

    fn half_with_work() -> Day {
        let mut d = Day::new(date(2026, 11, 18));
        d.add_period(h(8, 0), h(12, 0));
        d.absence = Some(Absence::new(AbsenceKind::Vacation, true));
        d
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
