//! Tagwerk – erfasst die Arbeitszeit anhand der Zeit, in der der Mac entsperrt ist.
//!
//! Aufruf:
//!   Tagwerk                       Menüleisten-App (Standard)
//!   Tagwerk --records             Fenster "Aufzeichnungen"
//!   Tagwerk --screenshot <png>    Fenster "Aufzeichnungen" öffnen, Bild speichern, beenden
//!   Tagwerk --render-icon <png>   App-Symbol zeichnen (für build.sh)

mod day;
mod icon;
#[cfg(target_os = "macos")]
mod menubar;
mod records;
mod store;
mod tracker;

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    store::migrate_legacy();
    let arg_after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };

    if let Some(path) = arg_after("--render-icon") {
        if let Err(e) = icon::render(1024).save(&path) {
            eprintln!("Symbol konnte nicht gespeichert werden: {e}");
            std::process::exit(1);
        }
        return;
    }

    if args.iter().any(|a| a == "--screenshot") {
        let Some(path) = arg_after("--screenshot") else {
            eprintln!("Aufruf: Tagwerk --screenshot <datei.png>");
            std::process::exit(2);
        };
        if let Err(e) = records::run(Some(path)) {
            eprintln!("Fehler: {e}");
            std::process::exit(1);
        }
        return;
    }

    if args.iter().any(|a| a == "--records") {
        if let Err(e) = records::run(None) {
            eprintln!("Fehler: {e}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(target_os = "macos")]
    menubar::run();

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Die Menüleisten-App gibt es nur für macOS – zeige die Aufzeichnungen.");
        let _ = records::run(None);
    }
}
