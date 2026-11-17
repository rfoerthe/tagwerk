//! Fenster „Über Tagwerk“: das Standardfenster von macOS mit Symbol, Version, kurzer
//! Beschreibung, Link zum Projekt und Lizenz. Erreichbar aus dem Menü in der Menüleiste
//! und aus dem Menü „Tagwerk“ des Fensters „Aufzeichnungen“.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAboutPanelOptionApplicationIcon, NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSAboutPanelOptionVersion, NSApplication, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName, NSImage, NSLinkAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSTextAlignment,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString, NSString, NSURL};

const REPOSITORY: &str = "https://github.com/rfoerthe/tagwerk";

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Zeigt das Fenster „Über Tagwerk“ (oder holt es nach vorn).
pub fn show() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);

    let copyright = ns("Copyright");
    let mut keys: Vec<&NSString> = unsafe {
        vec![
            NSAboutPanelOptionApplicationName,
            NSAboutPanelOptionApplicationVersion,
            // Leer: keine Build-Nummer in Klammern hinter der Version
            NSAboutPanelOptionVersion,
            NSAboutPanelOptionCredits,
        ]
    };
    let mut values: Vec<Retained<AnyObject>> = vec![
        Retained::into_super(Retained::into_super(ns("Tagwerk"))),
        Retained::into_super(Retained::into_super(ns(env!("CARGO_PKG_VERSION")))),
        Retained::into_super(Retained::into_super(ns(""))),
        Retained::into_super(Retained::into_super(credits())),
    ];
    keys.push(&copyright);
    values.push(Retained::into_super(Retained::into_super(ns("© 2026 Roland Foerther · MIT-Lizenz"))));
    if let Some(icon) = dev_icon() {
        keys.push(unsafe { NSAboutPanelOptionApplicationIcon });
        values.push(Retained::into_super(Retained::into_super(icon)));
    }

    let options = NSDictionary::from_retained_objects(&keys, &values);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    // Die Werte haben die Typen, die das Fenster für diese Schlüssel erwartet.
    unsafe { app.orderFrontStandardAboutPanelWithOptions(&options) };
}

/// Beschreibung und Link zum Projekt, zentriert in kleiner Schrift.
fn credits() -> Retained<NSAttributedString> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    let font = NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
    let text = NSMutableAttributedString::new();
    let append = |s: &str, link: Option<&str>| {
        let mut keys = unsafe { vec![NSFontAttributeName, NSParagraphStyleAttributeName, NSForegroundColorAttributeName] };
        let mut values: Vec<Retained<AnyObject>> = vec![
            Retained::into_super(Retained::into_super(font.clone())),
            Retained::into_super(Retained::into_super(Retained::into_super(style.clone()))),
            Retained::into_super(Retained::into_super(NSColor::secondaryLabelColor())),
        ];
        if let Some(url) = link.and_then(|l| NSURL::URLWithString(&ns(l))) {
            // Links zeigt das Fenster in Akzentfarbe statt in der Textfarbe.
            keys.truncate(2);
            values.truncate(2);
            keys.push(unsafe { NSLinkAttributeName });
            values.push(Retained::into_super(Retained::into_super(url)));
        }
        let attrs = NSDictionary::from_retained_objects(&keys, &values);
        // Schrift, Absatzstil, Farbe und URL sind gültige Werte für diese Schlüssel.
        let part = unsafe { NSAttributedString::new_with_attributes(&ns(s), &attrs) };
        text.appendAttributedString(&part);
    };
    append(
        "Erfasst die Arbeitszeit anhand der Zeit, in der der Mac entsperrt ist. \
         Alle Daten bleiben lokal auf diesem Mac.\n\n",
        None,
    );
    append(REPOSITORY.trim_start_matches("https://"), Some(REPOSITORY));
    Retained::into_super(text)
}

/// Ohne App-Paket (`cargo run`) hat das Programm kein eigenes Symbol: dann das Symbol
/// aus dem Quellordner nehmen. Die installierte App nutzt das Symbol aus dem Paket.
fn dev_icon() -> Option<Retained<NSImage>> {
    let in_bundle = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"));
    if in_bundle {
        return None;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/AppIcon.png");
    NSImage::initWithContentsOfFile(NSImage::alloc(), &ns(path))
}
