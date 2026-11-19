//! Tagwerk – erfasst die Arbeitszeit anhand der Zeit, in der der Mac entsperrt ist.
//!
//! Aufruf:
//!   Tagwerk                       Menüleisten-App (Standard)
//!   Tagwerk --records             nur das Fenster "Aufzeichnungen" (ohne Menüleisten-App)
//!   Tagwerk --export [--from TT.MM.JJJJ] [--to TT.MM.JJJJ] [-o datei.md]
//!                                 Aufzeichnungen als Markdown exportieren
//!                                 (Standard: die letzten 28 Tage, Ausgabe auf stdout)
//!   Tagwerk --import <datei.md>   Tage aus einem Markdown-Export übernehmen
//!   Tagwerk --screenshot <png>    Fenster "Aufzeichnungen" öffnen, Bild speichern, beenden
//!   Tagwerk --week-screenshot <png>
//!                                 Fenster "Aufzeichnungen" in der Ansicht "Woche" öffnen,
//!                                 Bild speichern, beenden
//!   Tagwerk --settings-screenshot <png>
//!                                 Fenster "Einstellungen" öffnen, Bild speichern, beenden

#[cfg(target_os = "macos")]
mod about;
mod day;
#[cfg(target_os = "macos")]
mod glyphs;
#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod login_item;
#[cfg(target_os = "macos")]
mod menubar;
#[cfg(target_os = "macos")]
mod notify;
#[cfg(target_os = "macos")]
mod records;
mod rules;
#[cfg(target_os = "macos")]
mod settings_window;
mod store;
mod tracker;
#[cfg(target_os = "macos")]
mod week_view;

use chrono::{Duration, Local};
use std::path::PathBuf;

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("Fehler: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let arg_after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if has("--export") {
        export(arg_after("--from"), arg_after("--to"), arg_after("-o").or_else(|| arg_after("--output")));
        return;
    }

    if has("--import") {
        let Some(path) = arg_after("--import") else {
            eprintln!("Aufruf: Tagwerk --import <datei.md>");
            std::process::exit(2);
        };
        import(PathBuf::from(path));
        return;
    }

    if has("--screenshot") {
        let Some(path) = arg_after("--screenshot") else {
            eprintln!("Aufruf: Tagwerk --screenshot <datei.png>");
            std::process::exit(2);
        };
        #[cfg(target_os = "macos")]
        if let Err(e) = records::run(Some(PathBuf::from(path)), false) {
            fail(e);
        }
        return;
    }

    if has("--week-screenshot") {
        let Some(path) = arg_after("--week-screenshot") else {
            eprintln!("Aufruf: Tagwerk --week-screenshot <datei.png>");
            std::process::exit(2);
        };
        #[cfg(target_os = "macos")]
        if let Err(e) = records::run(Some(PathBuf::from(path)), true) {
            fail(e);
        }
        return;
    }

    if has("--settings-screenshot") {
        let Some(path) = arg_after("--settings-screenshot") else {
            eprintln!("Aufruf: Tagwerk --settings-screenshot <datei.png>");
            std::process::exit(2);
        };
        #[cfg(target_os = "macos")]
        if let Err(e) = settings_window::screenshot(std::path::Path::new(&path)) {
            fail(e);
        }
        return;
    }

    if has("--records") {
        #[cfg(target_os = "macos")]
        if let Err(e) = records::run(None, false) {
            fail(e);
        }
        return;
    }

    #[cfg(target_os = "macos")]
    menubar::run();

    #[cfg(not(target_os = "macos"))]
    fail("Die Menüleisten-App und das Fenster „Aufzeichnungen“ gibt es nur für macOS.");
}

fn export(from: Option<String>, to: Option<String>, output: Option<String>) {
    let now = Local::now().naive_local();
    let (default_from, default_to) = store::default_export_range(now.date());
    let date = |arg: Option<String>, default| match arg {
        None => default,
        Some(s) => store::parse_date(&s).unwrap_or_else(|| fail(format!("„{s}“ ist kein Datum (TT.MM.JJJJ)."))),
    };
    let (from, to) = (date(from, default_from), date(to, default_to));
    if from > to {
        fail("Der Beginn des Zeitraums liegt nach dem Ende.");
    }
    let db = store::Db::open().unwrap_or_else(|e| fail(e));
    // Vortag für die Prüfung der Ruhezeit am ersten Tag
    let days = db.load_range(from - Duration::days(1), to).unwrap_or_else(|e| fail(e));
    let settings = db.load_settings().unwrap_or_else(|e| fail(e));
    let md = store::export_markdown(&days, from, to, now, &settings);
    match output {
        None => print!("{md}"),
        Some(path) => {
            if let Err(e) = std::fs::write(&path, md) {
                fail(format!("{path} konnte nicht geschrieben werden: {e}"));
            }
            eprintln!(
                "{} Tag(e) vom {} bis {} nach {path} exportiert.",
                days.iter().filter(|d| d.date >= from).count(),
                store::fmt_date_short(from),
                store::fmt_date_short(to)
            );
        }
    }
}

fn import(path: PathBuf) {
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| fail(format!("{} konnte nicht gelesen werden: {e}", path.display())));
    let days = store::parse_file(&text);
    if days.is_empty() {
        fail(format!("{} enthält keine Aufzeichnungen.", path.display()));
    }
    let mut db = store::Db::open().unwrap_or_else(|e| fail(e));
    if let Err(e) = db.save_days(&days, &[], Local::now().naive_local()) {
        fail(e);
    }
    eprintln!(
        "{} Tag(e) vom {} bis {} übernommen (vorhandene Einträge dieser Tage ersetzt).",
        days.len(),
        store::fmt_date_short(days[0].date),
        store::fmt_date_short(days[days.len() - 1].date)
    );
}
