//! Bilder eigener Fenster, wie sie auf dem Bildschirm stehen – für die Bildschirmfoto-Aufrufe
//! (`--screenshot` und Verwandte).
//!
//! Eine Ansicht in ein Bild zu zeichnen (`cacheDisplayInRect:`) genügt dafür nicht mehr: Was
//! macOS erst beim Zusammensetzen des Fensters ergänzt – etwa das gewählte Segment eines
//! Umschalters oder die Reiterleiste –, fehlt in einem solchen Bild. Deshalb liefert hier der
//! Fensterserver das fertige Fenster. Für eigene Fenster braucht das keine Freigabe zur
//! Bildschirmaufnahme.

use objc2::encode::{Encoding, RefEncode};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, AnyThread};
use objc2_app_kit::{NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString};
use std::ffi::{c_char, c_void};

/// `CGImage` (nur als Zeiger benutzt).
#[repr(C)]
struct CGImage {
    _private: [u8; 0],
}

unsafe impl RefEncode for CGImage {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGImage", &[]));
}

type CreateImage = unsafe extern "C" fn(bounds: NSRect, list: u32, window: u32, image: u32) -> *mut CGImage;

extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGImageRelease(image: *mut CGImage);
    fn CGImageCreateWithImageInRect(image: *mut CGImage, rect: NSRect) -> *mut CGImage;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut AnyObject;
}

/// `kCGWindowListOptionOnScreenOnly`
const ON_SCREEN_ONLY: u32 = 1 << 0;
/// `kCGWindowListOptionIncludingWindow`
const INCLUDING_WINDOW: u32 = 1 << 3;
/// `kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution`: ohne Schatten, in der
/// Auflösung des Bildschirms.
const IMAGE_OPTIONS: u32 = (1 << 0) | (1 << 3);

/// Rand in Pixeln, der beim Beschneiden um den Inhalt stehen bleibt (für den Schatten).
const TRIM_MARGIN: isize = 24;

/// Das Fenster mit der Nummer `window` als PNG (`None`, wenn macOS es nicht oder nur leer
/// liefert – dann bleibt dem Aufrufer das Zeichnen der Ansicht). Mit `trim` wird der durchsichtige Rand
/// bis auf einen schmalen Saum abgeschnitten: Ein Menü ist als Fenster viel größer als das,
/// was man von ihm sieht.
pub fn window_png(window: u32, trim: bool) -> Option<Vec<u8>> {
    // Zur Laufzeit gesucht: Die Funktion gilt als veraltet und könnte einem künftigen SDK
    // fehlen – dann soll Tagwerk trotzdem bauen und starten.
    let create = unsafe { dlsym(-2isize as *mut c_void, c"CGWindowListCreateImage".as_ptr()) };
    if create.is_null() {
        return None;
    }
    let create: CreateImage = unsafe { std::mem::transmute(create) };
    // `CGRectNull`: der Bereich des Fensters selbst.
    let null = NSRect::new(NSPoint::new(f64::INFINITY, f64::INFINITY), NSSize::new(0.0, 0.0));
    let image = unsafe { create(null, INCLUDING_WINDOW, window, IMAGE_OPTIONS) };
    if image.is_null() {
        return None;
    }
    let mut rep: Option<Retained<NSBitmapImageRep>> =
        unsafe { msg_send![NSBitmapImageRep::alloc(), initWithCGImage: image] };
    match rep.as_deref().map(opaque_bounds) {
        // Bei gesperrtem Bildschirm liefert macOS ein Bild ohne Inhalt. Das ist kein
        // Bildschirmfoto – lieber keins als ein leeres.
        Some(Opaque::Nothing) => rep = None,
        Some(Opaque::Area(bounds)) if trim => {
            let cropped = unsafe { CGImageCreateWithImageInRect(image, bounds) };
            if !cropped.is_null() {
                rep = unsafe { msg_send![NSBitmapImageRep::alloc(), initWithCGImage: cropped] };
                unsafe { CGImageRelease(cropped) };
            }
        }
        _ => {}
    }
    unsafe { CGImageRelease(image) };
    let rep = rep?;
    if rep.pixelsWide() < 2 || rep.pixelsHigh() < 2 {
        return None;
    }
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(data.to_vec())
}

/// Was ein Bild an Deckendem enthält.
enum Opaque {
    /// Nicht zu beurteilen: kein gewöhnliches Bild mit vier Werten je Pixel.
    Unknown,
    /// Nichts – das Bild ist vollständig durchsichtig.
    Nothing,
    /// Der Bereich der deckenden Pixel, um [`TRIM_MARGIN`] erweitert.
    Area(NSRect),
}

/// Sucht den Bereich der deckenden Pixel eines Bildes.
fn opaque_bounds(rep: &NSBitmapImageRep) -> Opaque {
    let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
    let (samples, row) = (rep.samplesPerPixel(), rep.bytesPerRow());
    let data = rep.bitmapData();
    if data.is_null() || samples != 4 || rep.bitsPerPixel() != 32 || w < 1 || h < 1 {
        return Opaque::Unknown;
    }
    // Wo die Deckkraft im Pixel steht, hängt vom Format ab: vorn oder hinten, und bei
    // umgekehrter Bytefolge gerade andersherum.
    let format = rep.bitmapFormat();
    let first = format.contains(NSBitmapFormat::AlphaFirst);
    let reversed = format.contains(NSBitmapFormat::ThirtyTwoBitLittleEndian);
    let at = if first != reversed { 0 } else { 3 };
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, -1, -1);
    for y in 0..h {
        for x in 0..w {
            let alpha = unsafe { *data.offset(y * row + x * 4 + at) };
            if alpha > 240 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x1 < x0 || y1 < y0 {
        return Opaque::Nothing;
    }
    let (x0, y0) = ((x0 - TRIM_MARGIN).max(0), (y0 - TRIM_MARGIN).max(0));
    let (x1, y1) = ((x1 + TRIM_MARGIN).min(w - 1), (y1 + TRIM_MARGIN).min(h - 1));
    Opaque::Area(NSRect::new(
        NSPoint::new(x0 as f64, y0 as f64),
        NSSize::new((x1 - x0 + 1) as f64, (y1 - y0 + 1) as f64),
    ))
}

/// Fenster dieses Programms auf dem Bildschirm: Nummer, Ebene und Größe (von vorn nach hinten).
pub fn own_windows() -> Vec<(u32, i64, NSSize)> {
    let raw = unsafe { CGWindowListCopyWindowInfo(ON_SCREEN_ONLY, 0) };
    let Some(list) = (unsafe { Retained::from_raw(raw.cast::<NSArray<NSDictionary<NSString, AnyObject>>>()) }) else {
        return Vec::new();
    };
    let pid = std::process::id() as i64;
    let number = |dict: &NSDictionary<NSString, AnyObject>, key: &str| -> Option<f64> {
        let value = dict.objectForKey(&NSString::from_str(key))?;
        Some(unsafe { msg_send![&*value, doubleValue] })
    };
    list.iter()
        .filter_map(|w| {
            if number(&w, "kCGWindowOwnerPID")? as i64 != pid {
                return None;
            }
            let bounds = w.objectForKey(&NSString::from_str("kCGWindowBounds"))?;
            let side = |key: &str| -> Option<f64> {
                let value: Option<Retained<AnyObject>> =
                    unsafe { msg_send![&*bounds, objectForKey: &*NSString::from_str(key)] };
                Some(unsafe { msg_send![&*value?, doubleValue] })
            };
            Some((
                number(&w, "kCGWindowNumber")? as u32,
                number(&w, "kCGWindowLayer")? as i64,
                NSSize::new(side("Width")?, side("Height")?),
            ))
        })
        .collect()
}
