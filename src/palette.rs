//! Farben, die in Menü und Fenstern dasselbe bedeuten: grün = im Plus bzw. läuft, rot = im
//! Minus bzw. Fehler, orange = Pause bzw. Hinweis, indigo = Feierabend, türkis = Abwesenheit,
//! magenta = Tag in Bearbeitung (Wochenansicht).
//!
//! Die Systemfarben allein sind auf hellem Grund zu kontrastarm; deshalb nimmt jede Farbe im
//! hellen Erscheinungsbild einen dunkleren Ton und im dunklen den hellen Systemton.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::Message;
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSColor, NSFont, NSFontAttributeName,
    NSForegroundColorAttributeName,
};
use objc2_foundation::{NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSString};

fn adaptive(light: (f64, f64, f64), dark: fn() -> Retained<NSColor>) -> Retained<NSColor> {
    let light = NSColor::colorWithSRGBRed_green_blue_alpha(light.0, light.1, light.2, 1.0);
    let dark = dark();
    let provider = block2::RcBlock::new(move |appearance: std::ptr::NonNull<NSAppearance>| {
        let names = NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe { NSAppearanceNameDarkAqua }]);
        let best = unsafe { appearance.as_ref() }.bestMatchFromAppearancesWithNames(&names);
        let is_dark = best.is_some_and(|n| &*n == unsafe { NSAppearanceNameDarkAqua });
        // Beide Farben gehören dem Block und leben so lange wie die dynamische Farbe.
        std::ptr::NonNull::from(if is_dark { &*dark } else { &*light })
    });
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

pub fn green() -> Retained<NSColor> {
    adaptive((0.09, 0.50, 0.20), NSColor::systemGreenColor)
}

pub fn orange() -> Retained<NSColor> {
    adaptive((0.74, 0.36, 0.0), NSColor::systemOrangeColor)
}

/// Feierabend: Indigo statt Blau, damit der Text auf der (meist blauen) Markierung
/// beim Überfahren mit der Maus lesbar bleibt.
pub fn indigo() -> Retained<NSColor> {
    adaptive((0.30, 0.24, 0.78), NSColor::systemIndigoColor)
}

/// Abwesenheit (Urlaub, Sonderurlaub, Krank, Feiertag, Zeitausgleich) in der Wochenansicht.
pub fn absence() -> Retained<NSColor> {
    adaptive((0.0, 0.45, 0.52), NSColor::systemTealColor)
}

/// Tag in Bearbeitung in der Wochenansicht: hebt sich von Blau (Arbeit, heute), Türkis
/// (Abwesenheit), Orange und Rot ab.
pub fn selection() -> Retained<NSColor> {
    // Ein echtes Magenta: Das Pink des Systems wirkt im Dunkeln rötlich, nah am Minus.
    fn dark() -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(0.89, 0.42, 0.85, 1.0)
    }
    adaptive((0.66, 0.13, 0.58), dark)
}

pub fn red() -> Retained<NSColor> {
    adaptive((0.78, 0.12, 0.12), NSColor::systemRedColor)
}

/// Ist der Saldo im Minus? Unter einer Minute gilt er als ausgeglichen (angezeigt wird
/// dann `+0:00 h`).
pub fn is_minus(balance_secs: i64) -> bool {
    balance_secs < -59
}

/// Farbe eines Saldos: grün im Plus (und ausgeglichen), rot im Minus – überall gleich.
pub fn balance(balance_secs: i64) -> Retained<NSColor> {
    if is_minus(balance_secs) {
        red()
    } else {
        green()
    }
}

/// Text aus Teilen mit je eigener Farbe in einer Schrift – für Zeilen, in denen nur der
/// Saldo farbig ist („3 Arbeitstage · Saldo −0:30 h“).
pub fn colored(parts: &[(&str, &NSColor)], font: &NSFont) -> Retained<NSAttributedString> {
    let out = NSMutableAttributedString::new();
    for &(text, color) in parts {
        let keys = unsafe { [NSForegroundColorAttributeName, NSFontAttributeName] };
        let values: [Retained<AnyObject>; 2] = [
            Retained::into_super(Retained::into_super(color.retain())),
            Retained::into_super(Retained::into_super(font.retain())),
        ];
        let attrs = NSDictionary::from_retained_objects(&keys, &values);
        // Farbe und Schrift sind gültige Werte für diese Schlüssel.
        let part = unsafe { NSAttributedString::new_with_attributes(&NSString::from_str(text), &attrs) };
        out.appendAttributedString(&part);
    }
    Retained::into_super(out)
}
