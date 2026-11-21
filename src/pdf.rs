//! Export als PDF: die Übersicht eines Zeitraums ([`store::Report`]) als Tabelle mit
//! Wochensummen, Gesamtsumme und Hinweisen, auf A4 und über mehrere Seiten.
//!
//! Der Text wird als ein formatierter Text mit Tabulatoren gesetzt und über das Drucksystem
//! von macOS als PDF gesichert – so übernimmt macOS den Seitenumbruch. Gesetzt wird immer
//! schwarz auf weiß, unabhängig vom Erscheinungsbild.

use crate::day::fmt_clock;
use crate::rules::{self, Settings};
use crate::store::{self, Report};
use chrono::{Datelike, NaiveDateTime};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, AnyThread, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSApplication, NSColor, NSFont,
    NSFontAttributeName, NSFontWeight, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSPrintInfo, NSPrintJobSavingURL, NSPrintOperation,
    NSPrintSaveJob, NSPrintingPaginationMode, NSTextAlignment, NSTextTab, NSTextView,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::path::Path;

/// A4 in Punkten und der Rand ringsum.
const PAGE: (f64, f64) = (595.0, 842.0);
const MARGIN: f64 = 50.0;
/// Rechte Kanten der Spalten Beginn, Ende, Arbeitszeit, Pausen und Saldo.
const COLUMNS: [f64; 5] = [215.0, 280.0, 365.0, 430.0, PAGE.0 - 2.0 * MARGIN];
/// Beginn des Texts in der Liste der Hinweise.
const HINT_INDENT: f64 = 105.0;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Absatzformat: rechtsbündige Tabulatoren an `tabs` (bzw. ein linksbündiger für die
/// Hinweise), Abstand davor und Einzug der Folgezeilen.
fn paragraph(tabs: &[f64], left: bool, before: f64, indent: f64) -> Retained<NSMutableParagraphStyle> {
    let style = NSMutableParagraphStyle::new();
    let align = if left { NSTextAlignment::Left } else { NSTextAlignment::Right };
    let stops: Vec<Retained<NSTextTab>> = tabs
        .iter()
        .map(|&at| unsafe {
            NSTextTab::initWithTextAlignment_location_options(NSTextTab::alloc(), align, at, &NSDictionary::new())
        })
        .collect();
    style.setTabStops(Some(&NSArray::from_retained_slice(&stops)));
    style.setParagraphSpacingBefore(before);
    style.setHeadIndent(indent);
    style
}

struct Text {
    out: Retained<NSMutableAttributedString>,
}

impl Text {
    fn line(&self, text: &str, size: f64, weight: NSFontWeight, gray: bool, style: &NSMutableParagraphStyle) {
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(size, weight);
        let color = if gray { NSColor::colorWithWhite_alpha(0.4, 1.0) } else { NSColor::blackColor() };
        let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName, NSParagraphStyleAttributeName] };
        let values: [Retained<AnyObject>; 3] = [
            Retained::into_super(Retained::into_super(font)),
            Retained::into_super(Retained::into_super(color)),
            Retained::into_super(Retained::into_super(Retained::into_super(style.retain()))),
        ];
        let attrs = NSDictionary::from_retained_objects(&keys, &values);
        let part = unsafe { NSAttributedString::new_with_attributes(&ns(&format!("{text}\n")), &attrs) };
        self.out.appendAttributedString(&part);
    }
}

/// `tagwerk`-Übersicht als formatierter Text.
fn compose(report: &Report, settings: &Settings, now: NaiveDateTime) -> Retained<NSMutableAttributedString> {
    let (regular, bold) = unsafe { (NSFontWeightRegular, NSFontWeightSemibold) };
    let t = Text { out: NSMutableAttributedString::new() };
    let plain = paragraph(&[], false, 0.0, 0.0);
    t.line(
        &format!(
            "Tagwerk – Arbeitszeiten {} bis {}",
            store::fmt_date_short(report.from),
            store::fmt_date_short(report.to)
        ),
        16.0,
        bold,
        false,
        &plain,
    );
    // Die Regeln des Zeitraums – bei einem Wechsel eine Zeile je Stand.
    let summaries = settings.summary_between(report.from, report.to);
    let single = summaries.len() == 1;
    let exported = format!("Exportiert am {} um {} Uhr", store::fmt_date_short(now.date()), now.format("%H:%M"));
    let head = if single { format!("{exported}  ·  {}", summaries[0]) } else { exported };
    t.line(&head, 8.5, regular, true, &paragraph(&[], false, 3.0, 0.0));
    if !single {
        for line in &summaries {
            t.line(line, 8.5, regular, true, &paragraph(&[], false, 1.0, 0.0));
        }
    }

    let row = |cells: [&str; 6]| cells.join("\t");
    t.line(
        &row(["Datum", "Beginn", "Ende", "Arbeitszeit", "Pausen", "Saldo"]),
        8.5,
        bold,
        true,
        &paragraph(&COLUMNS, false, 16.0, 0.0),
    );
    let day_style = paragraph(&COLUMNS, false, 2.0, 0.0);
    let week_style = paragraph(&COLUMNS, false, 3.0, 0.0);
    let first_of_week = paragraph(&COLUMNS, false, 9.0, 0.0);
    let mut i = 0;
    while i < report.days.len() {
        let week = report.days[i].date.iso_week();
        let j = report.days[i..].iter().position(|d| d.date.iso_week() != week).map_or(report.days.len(), |n| i + n);
        for (k, d) in report.days[i..j].iter().enumerate() {
            let mut date = store::fmt_date(d.date);
            if !d.absence.is_empty() {
                date.push_str(&format!("  {}", d.absence));
            }
            if d.corrected {
                date.push_str("  ✎");
            }
            if !d.warnings.is_empty() {
                date.push_str("  ⚠");
            }
            // Ohne Arbeitszeit gibt es weder Beginn noch Ende – und einen Saldo nur mit Soll.
            let worked = d.work > 0;
            let clock = |t: i64| if worked { fmt_clock(t) } else { "–".to_string() };
            let balance = if d.has_balance() { rules::fmt_balance(d.balance()) } else { "–".to_string() };
            t.line(
                &row([
                    &date,
                    &clock(d.first_start),
                    &clock(d.last_end),
                    &store::fmt_hm(d.work),
                    &store::fmt_hm(d.pause),
                    &balance,
                ]),
                10.0,
                regular,
                false,
                if k == 0 { &first_of_week } else { &day_style },
            );
        }
        let (work, pause, balance) =
            report.days[i..j].iter().fold((0, 0, 0), |(w, p, b), d| (w + d.work, p + d.pause, b + d.balance()));
        t.line(
            &row([
                &format!("KW {}", week.week()),
                "",
                "",
                &store::fmt_hm(work),
                &store::fmt_hm(pause),
                &rules::fmt_balance(balance),
            ]),
            9.0,
            bold,
            true,
            &week_style,
        );
        i = j;
    }
    let (work, pause, target, count) = report.totals();
    let days = if count == 1 { "Arbeitstag" } else { "Arbeitstage" };
    t.line(
        &row([
            &format!("Summe: {count} {days}"),
            "",
            "",
            &store::fmt_hm(work),
            &store::fmt_hm(pause),
            &rules::fmt_balance(report.balance()),
        ]),
        10.0,
        bold,
        false,
        &paragraph(&COLUMNS, false, 12.0, 0.0),
    );
    t.line(
        &format!(
            "Soll: {} ({}; nur Tage mit Arbeitszeit).  ✎ manuell korrigiert  ·  ⚠ siehe Hinweise",
            store::fmt_hm(target),
            settings.target_basis(report.from, report.to)
        ),
        8.5,
        regular,
        true,
        &paragraph(&[], false, 4.0, 0.0),
    );

    t.line("Hinweise", 12.0, bold, false, &paragraph(&[], false, 20.0, 0.0));
    let hint_style = paragraph(&[HINT_INDENT], true, 3.0, HINT_INDENT);
    let mut any = false;
    for d in &report.days {
        for w in &d.warnings {
            t.line(&format!("{}\t{w}", store::fmt_date(d.date)), 9.5, regular, false, &hint_style);
            any = true;
        }
    }
    if !any {
        t.line(
            "Keine Verstöße gegen Arbeitszeitgesetz oder Arbeitszeitrahmen.",
            9.5,
            regular,
            false,
            &paragraph(&[], false, 3.0, 0.0),
        );
    }
    t.out
}

/// Schreibt die Übersicht als PDF nach `path`. Nur im Hauptthread.
pub fn write(report: &Report, settings: &Settings, now: NaiveDateTime, path: &Path) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("Das PDF muss im Hauptthread entstehen.")?;
    // Ohne laufende App (Kommandozeile) braucht das Drucksystem trotzdem die Anwendung.
    let _app = NSApplication::sharedApplication(mtm);

    let width = PAGE.0 - 2.0 * MARGIN;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, PAGE.1 - 2.0 * MARGIN));
    let view = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    view.setAppearance(NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).as_deref());
    view.setDrawsBackground(true);
    view.setBackgroundColor(&NSColor::whiteColor());
    view.setTextContainerInset(NSSize::new(0.0, 0.0));
    if let Some(container) = unsafe { view.textContainer() } {
        container.setLineFragmentPadding(0.0);
    }
    let storage = unsafe { view.textStorage() }.ok_or("Der Text ließ sich nicht setzen.")?;
    storage.setAttributedString(&compose(report, settings, now));
    view.sizeToFit();

    let info = NSPrintInfo::new();
    info.setPaperSize(NSSize::new(PAGE.0, PAGE.1));
    info.setTopMargin(MARGIN);
    info.setBottomMargin(MARGIN);
    info.setLeftMargin(MARGIN);
    info.setRightMargin(MARGIN);
    info.setHorizontallyCentered(false);
    info.setVerticallyCentered(false);
    info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
    info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
    info.setJobDisposition(unsafe { NSPrintSaveJob });
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    unsafe {
        let options = info.dictionary();
        let _: () = msg_send![&*options, setObject: &*url, forKey: NSPrintJobSavingURL];
    }

    let operation = NSPrintOperation::printOperationWithView_printInfo(&view, &info);
    operation.setShowsPrintPanel(false);
    operation.setShowsProgressPanel(false);
    if operation.runOperation() && path.exists() {
        Ok(())
    } else {
        Err(format!("{} konnte nicht geschrieben werden.", path.display()))
    }
}
