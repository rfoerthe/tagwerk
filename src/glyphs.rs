//! Symbole der Menüleiste: neun Symbolsätze mit je vier Zuständen – drei sachliche
//! (Tagbogen, Schlossuhr, Tagesring) und sechs verspielte (Bauarbeiter, Handwerkerin,
//! Kaffeetasse, Hamsterrad, Roboter, Kaninchen). Gezeichnet wird vektoriell mit AppKit, damit die Symbole in jeder
//! Größe scharf sind.
//!
//! Die Formen sind in Punkten für eine Bezugsgröße von 18 pt entworfen (Strichstärke
//! 2 pt, kräftig wie die übrigen Symbole der Menüleiste). Alle Sätze haben dieselbe Strichstärke; die Höhe der Figur folgt einer
//! optischen Größe je Satz (breite Formen wirken größer und werden daher niedriger
//! gezeichnet – wie bei den SF Symbols). Die Breite ist für alle Zustände eines Satzes
//! gleich, damit nichts springt.
//!
//! Einfarbige Zustände sind Template-Bilder (Farbe und Abblenden übernimmt macOS).
//! „Läuft“ (grün) und „Pause“ (orange) tragen eine satte Farbfläche mit weißem Detail –
//! halbtransparente Flächen würden auf farbigem Hintergrund trüb. Die übrigen Teile
//! werden dann in `textColor` gezeichnet, und zwar bei jedem Zeichnen neu, damit sie
//! dem Erscheinungsbild der Menüleiste folgen. Die Entwürfe liegen unter
//! `design/icon-vorschlaege/`.

use crate::rules::IconSet;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSBezierPath, NSColor, NSImage, NSImageCacheMode, NSLineCapStyle, NSLineJoinStyle};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::f64::consts::PI;

/// Zustand, den das Symbol zeigt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Break,
    Outside,
    Closed,
}

impl State {
    pub const ALL: [State; 4] = [State::Running, State::Break, State::Outside, State::Closed];

    /// Farbakzent des Zustands (ohne Akzent: Template-Bild).
    fn accent(self) -> Option<fn() -> Retained<NSColor>> {
        match self {
            State::Running => Some(NSColor::systemGreenColor),
            State::Break => Some(NSColor::systemOrangeColor),
            State::Outside | State::Closed => None,
        }
    }
}

/// Bezugsgröße des Entwurfs und Standard-Strichstärke darin.
const REF_H: f64 = 18.0;
const W: f64 = 2.0;
/// Strichstärke kleiner Details (Zeiger, Haken, kleine Ringe).
const W_DETAIL: f64 = 1.8;

/// Optische Größe: Höhe der Figur im Verhältnis zur Symbolgröße. Breite Formen
/// (Bogen) werden niedriger, schmale (Schloss) etwas höher gezeichnet – so wirken alle
/// neben den Systemsymbolen gleich groß.
fn optical(set: IconSet) -> f64 {
    match set {
        IconSet::Tagbogen => 0.71,
        IconSet::Schlossuhr => 1.06,
        IconSet::Tagesring => 1.0,
        IconSet::Bauarbeiter | IconSet::Handwerkerin => 1.06,
        IconSet::Kaffeetasse => 1.08,
        IconSet::Hamsterrad => 1.0,
        IconSet::Roboter => 1.0,
        IconSet::Kaninchen => 1.06,
    }
}

/// Womit gezeichnet wird: Grundfarbe oder Farbakzent, jeweils mit Deckkraft.
#[derive(Clone, Copy, PartialEq)]
enum Ink {
    Base(f64),
    Accent(f64),
    /// Weiß auf der Farbfläche
    White,
}

/// Zeichenstift: misst die Figur aus (`draw == false`) oder zeichnet sie.
struct Pen {
    draw: bool,
    /// Maßstab der Geometrie, Maßstab der Strichstärken, Versatz.
    s: f64,
    k: f64,
    ox: f64,
    oy: f64,
    ink: Ink,
    /// Farbakzent des Zustands; ohne Akzent wird schwarz (Template) gezeichnet.
    accent: Option<fn() -> Retained<NSColor>>,
    min: (f64, f64),
    max: (f64, f64),
}

/// Punkt auf einem Kreis; Winkel mathematisch (Grad, gegen den Uhrzeigersinn), y nach unten.
fn on_circle(cx: f64, cy: f64, r: f64, deg: f64) -> (f64, f64) {
    let a = deg.to_radians();
    (cx + r * a.cos(), cy - r * a.sin())
}

impl Pen {
    fn measure() -> Self {
        Pen {
            draw: false,
            s: 1.0,
            k: 1.0,
            ox: 0.0,
            oy: 0.0,
            ink: Ink::Base(1.0),
            accent: None,
            min: (f64::MAX, f64::MAX),
            max: (f64::MIN, f64::MIN),
        }
    }

    fn base(&mut self) {
        self.ink = Ink::Base(1.0);
    }

    fn accent(&mut self) {
        self.ink = Ink::Accent(1.0);
    }

    fn white(&mut self) {
        self.ink = Ink::White;
    }

    fn add(&mut self, (x, y): (f64, f64)) {
        self.min = (self.min.0.min(x), self.min.1.min(y));
        self.max = (self.max.0.max(x), self.max.1.max(y));
    }

    fn p(&self, (x, y): (f64, f64)) -> NSPoint {
        NSPoint::new(self.ox + x * self.s, self.oy + y * self.s)
    }

    fn color(&self) {
        let color = match (self.accent, self.ink) {
            // Template: nur die Deckkraft zählt
            (None, Ink::Base(a) | Ink::Accent(a)) => NSColor::colorWithWhite_alpha(0.0, a),
            (_, Ink::White) => NSColor::whiteColor(),
            // deckend wie die Template-Symbole (labelColor ist leicht transparent)
            (Some(_), Ink::Base(a)) if a >= 1.0 => NSColor::textColor(),
            (Some(_), Ink::Base(a)) => NSColor::textColor().colorWithAlphaComponent(a),
            (Some(accent), Ink::Accent(a)) if a >= 1.0 => accent(),
            (Some(accent), Ink::Accent(a)) => accent().colorWithAlphaComponent(a),
        };
        color.set();
    }

    /// Strich mit runden Enden; `dots` = Abstand der Punkte einer gepunkteten Linie.
    fn stroke(&self, path: &NSBezierPath, w: f64, dots: Option<f64>) {
        path.setLineWidth(w * self.k);
        path.setLineCapStyle(NSLineCapStyle::Round);
        path.setLineJoinStyle(NSLineJoinStyle::Round);
        if let Some(gap) = dots {
            let pattern = [0.001, gap * self.s];
            unsafe { path.setLineDash_count_phase(pattern.as_ptr(), 2, 0.0) };
        }
        self.color();
        path.stroke();
    }

    /// Kreisbogen von `a1` nach `a2` (Grad) als kubische Bézierkurven anhängen.
    fn append_arc(&self, path: &NSBezierPath, c: (f64, f64), r: f64, a1: f64, a2: f64) {
        let n = ((a1 - a2).abs() / 90.0).ceil().max(1.0) as usize;
        let step = (a2 - a1) / n as f64;
        let t = 4.0 / 3.0 * (step.to_radians() / 4.0).tan();
        for i in 0..n {
            let (a, b) = (a1 + step * i as f64, a1 + step * (i + 1) as f64);
            let (ar, br) = (a.to_radians(), b.to_radians());
            let (ax, ay) = on_circle(c.0, c.1, r, a);
            let (bx, by) = on_circle(c.0, c.1, r, b);
            let c1 = (ax - t * r * ar.sin(), ay - t * r * ar.cos());
            let c2 = (bx + t * r * br.sin(), by + t * r * br.cos());
            path.curveToPoint_controlPoint1_controlPoint2(self.p((bx, by)), self.p(c1), self.p(c2));
        }
    }

    fn line(&mut self, pts: &[(f64, f64)], w: f64) {
        if !self.draw {
            // Striche dicker als `W` ragen über den Rand hinaus, den `layout` vorsieht
            let e = ((w - W) / 2.0).max(0.0);
            for &(x, y) in pts {
                self.add((x - e, y - e));
                self.add((x + e, y + e));
            }
            return;
        }
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.p(pts[0]));
        for &q in &pts[1..] {
            path.lineToPoint(self.p(q));
        }
        self.stroke(&path, w, None);
    }

    /// Bogen im Uhrzeigersinn von `a1` nach `a2` (`a1 > a2`).
    fn arc(&mut self, c: (f64, f64), r: f64, a1: f64, a2: f64, w: f64, dots: Option<f64>) {
        if !self.draw {
            let n = ((a1 - a2) / 2.0).ceil() as usize;
            for i in 0..=n {
                self.add(on_circle(c.0, c.1, r, a1 + (a2 - a1) * i as f64 / n as f64));
            }
            return;
        }
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.p(on_circle(c.0, c.1, r, a1)));
        self.append_arc(&path, c, r, a1, a2);
        self.stroke(&path, w, dots);
    }

    fn ring(&mut self, c: (f64, f64), r: f64, w: f64, dots: Option<f64>) {
        if !self.draw {
            self.add((c.0 - r, c.1 - r));
            self.add((c.0 + r, c.1 + r));
            return;
        }
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.p(on_circle(c.0, c.1, r, 90.0)));
        self.append_arc(&path, c, r, 90.0, -270.0);
        if dots.is_none() {
            path.closePath();
        }
        self.stroke(&path, w, dots);
    }

    fn disc(&mut self, c: (f64, f64), r: f64) {
        if !self.draw {
            self.add((c.0 - r, c.1 - r));
            self.add((c.0 + r, c.1 + r));
            return;
        }
        let o = self.p((c.0 - r, c.1 - r));
        let d = 2.0 * r * self.s;
        self.color();
        NSBezierPath::bezierPathWithOvalInRect(NSRect::new(o, NSSize::new(d, d))).fill();
    }

    /// Obere Hälfte einer Scheibe (Sonne auf dem Horizont).
    fn half_disc(&mut self, c: (f64, f64), r: f64) {
        if !self.draw {
            self.add((c.0 - r, c.1 - r));
            self.add((c.0 + r, c.1));
            return;
        }
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.p((c.0 - r, c.1)));
        self.append_arc(&path, c, r, 180.0, 0.0);
        path.closePath();
        self.color();
        path.fill();
    }

    /// Abgerundetes Rechteck, gefüllt (`stroke == None`) oder als Linie.
    fn rrect(&mut self, (x, y): (f64, f64), (w, h): (f64, f64), r: f64, stroke: Option<f64>, dots: Option<f64>) {
        if !self.draw {
            self.add((x, y));
            self.add((x + w, y + h));
            return;
        }
        let rect = NSRect::new(self.p((x, y)), NSSize::new(w * self.s, h * self.s));
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, r * self.s, r * self.s);
        match stroke {
            Some(sw) => self.stroke(&path, sw, dots),
            None => {
                self.color();
                path.fill();
            }
        }
    }

    /// Gefülltes Vieleck.
    fn poly(&mut self, pts: &[(f64, f64)]) {
        if !self.draw {
            pts.iter().for_each(|&q| self.add(q));
            return;
        }
        let path = NSBezierPath::bezierPath();
        path.moveToPoint(self.p(pts[0]));
        for &q in &pts[1..] {
            path.lineToPoint(self.p(q));
        }
        path.closePath();
        self.color();
        path.fill();
    }

    /// Dampf: eine kleine S-Kurve, oben bei `(x, y)`.
    fn steam(&mut self, x: f64, y: f64, r: f64) {
        self.arc((x, y + r), r, 90.0, -90.0, W_DETAIL, None);
        self.arc((x, y + 3.0 * r), r, 270.0, 90.0, W_DETAIL, None);
    }

    /// „z“ mit linker oberer Ecke `(x, y)` und Kantenlänge `a`.
    fn zee(&mut self, x: f64, y: f64, a: f64, w: f64) {
        self.line(&[(x, y), (x + a, y), (x, y + a), (x + a, y + a)], w);
    }

    /// Fröhliches Auge „^“ um `(x, y)`.
    fn happy_eye(&mut self, x: f64, y: f64, a: f64) {
        self.line(&[(x - a, y + a * 0.8), (x, y - a * 0.2), (x + a, y + a * 0.8)], W_DETAIL);
    }
}

// ---------- Symbolsätze ----------

/// A – Tagbogen: der Arbeitstag als Sonnenbogen über dem Horizont.
fn tagbogen(pen: &mut Pen, state: State) {
    const R: f64 = 15.0;
    const SUN: f64 = 52.0;
    let c = (0.0, 0.0);
    match state {
        State::Running | State::Break => {
            // Kuppel als Farbfläche, darin die Sonne in Weiß
            pen.accent();
            pen.half_disc(c, R + W / 2.0);
            pen.white();
            let sun = on_circle(0.0, 0.0, R - 5.6, SUN);
            if state == State::Running {
                pen.disc(sun, 3.0);
            } else {
                pen.ring(sun, 2.4, W_DETAIL, None);
            }
            pen.base();
        }
        State::Outside => pen.arc(c, R, 180.0, 0.0, W, Some(PI * R / 13.0)),
        State::Closed => {
            // Feierabend: die Sonne geht am Horizont unter
            pen.half_disc(c, 8.0);
            for a in [30.0, 90.0, 150.0] {
                pen.line(&[on_circle(0.0, 0.0, 10.4, a), on_circle(0.0, 0.0, 13.8, a)], W);
            }
        }
    }
    // Horizont zuletzt, über der Kuppel
    pen.line(&[(-R - 1.5, 0.0), (R + 1.5, 0.0)], W);
}

/// B – Schlossuhr: entsperrt = Arbeitszeit, gesperrt = Pause.
fn schlossuhr(pen: &mut Pen, state: State) {
    const R: f64 = 6.4;
    const LEG: f64 = 3.7;
    const LEG_END: f64 = -5.0; // Beine enden im Ring
    let c = (0.0, 0.0);
    let open = matches!(state, State::Running | State::Outside);
    let top = if open { -11.0 } else { -9.0 };
    pen.line(&[(-LEG, LEG_END), (-LEG, top)], W);
    pen.arc((0.0, top), LEG, 180.0, 0.0, W, None);
    pen.line(&[(LEG, top), (LEG, if open { -9.2 } else { LEG_END })], W);
    match state {
        State::Running | State::Break => {
            // Gehäuse als Farbfläche, Zeiger in Weiß
            pen.accent();
            pen.disc(c, R + W / 2.0);
            pen.white();
            pen.line(&[(0.0, -3.6), (0.0, 0.0), (2.9, 0.0)], W_DETAIL);
            pen.base();
        }
        State::Outside => pen.ring(c, R, W, Some(2.0 * PI * R / 12.0)),
        State::Closed => {
            pen.ring(c, R, W, None);
            pen.line(&[(-2.9, 0.1), (-0.8, 2.2), (3.0, -1.9)], W_DETAIL);
        }
    }
}

/// C – Tagesring: Fortschritt zum Tagessoll.
fn tagesring(pen: &mut Pen, state: State) {
    const R: f64 = 8.2;
    let c = (0.0, 0.0);
    match state {
        State::Running | State::Break => {
            // Fortschritt im Farbakzent, innen eine Farbfläche mit weißem Kern
            pen.ink = Ink::Base(0.3);
            pen.ring(c, R, W, None);
            pen.accent();
            pen.arc(c, R, 90.0, 90.0 - 0.68 * 360.0, W, None);
            pen.disc(c, R - 2.9);
            pen.white();
            if state == State::Running {
                pen.disc(c, 1.9);
            } else {
                pen.ring(c, 1.9, 1.5, None);
            }
            pen.base();
        }
        State::Outside => pen.ring(c, R, W, Some(2.0 * PI * R / 14.0)),
        State::Closed => {
            pen.ring(c, R, W, None);
            pen.line(&[(-3.0, 0.1), (-0.9, 2.2), (3.1, -2.0)], W);
        }
    }
}

/// Kopfbedeckung der Figuren.
#[derive(Clone, Copy, PartialEq)]
enum Figure {
    /// Bauarbeiter: Helm mit Krempe, Hammer
    Worker,
    /// Handwerkerin: Dutt mit Schleife, Schraubenschlüssel
    Craftswoman,
}

/// D/E – Figur (Kopf und Schultern) mit Werkzeug, Tasse, „Zz“ oder Winken rechts daneben.
fn figure(pen: &mut Pen, state: State, who: Figure) {
    let colored = matches!(state, State::Running | State::Break);
    // Schultern (im Farbakzent: das Hemd)
    if colored {
        pen.accent();
    }
    pen.half_disc((0.0, 10.0), 7.1);
    pen.base();
    // Kopf
    pen.disc((0.0, -1.8), 3.1);
    match who {
        Figure::Worker => {
            if colored {
                pen.accent();
            }
            pen.half_disc((0.0, -6.6), 4.4);
            pen.line(&[(-6.0, -6.6), (6.0, -6.6)], W);
            if colored {
                pen.white();
                pen.line(&[(0.0, -10.0), (0.0, -7.8)], 1.3);
            }
            pen.base();
        }
        Figure::Craftswoman => {
            // Dutt mit Schleife
            pen.disc((0.0, -7.2), 2.2);
            if colored {
                pen.accent();
            }
            pen.poly(&[(0.0, -7.6), (-4.6, -10.2), (-4.6, -5.2)]);
            pen.poly(&[(0.0, -7.6), (4.6, -10.2), (4.6, -5.2)]);
            pen.base();
        }
    }
    match state {
        State::Running => match who {
            Figure::Worker => {
                // erhobener Hammer
                pen.line(&[(6.8, 6.0), (10.2, -1.2)], W_DETAIL);
                pen.line(&[(8.2, -3.0), (12.6, -0.9)], 2.8);
            }
            Figure::Craftswoman => {
                // Schraubenschlüssel
                pen.line(&[(6.8, 6.0), (9.8, 0.0)], W_DETAIL + 0.4);
                pen.arc((10.6, -1.8), 2.0, 380.0, 75.0, W_DETAIL, None);
            }
        },
        State::Break => {
            // Kaffeebecher mit Dampf
            pen.rrect((6.6, 0.4), (4.4, 4.8), 1.2, None, None);
            pen.arc((11.0, 2.6), 1.4, 90.0, -90.0, 1.4, None);
            pen.steam(8.8, -4.6, 0.85);
        }
        State::Outside => {
            pen.zee(7.0, -4.2, 3.2, W_DETAIL);
            pen.zee(10.8, -9.6, 2.2, 1.5);
        }
        State::Closed => {
            // Winken: Feierabend!
            pen.line(&[(5.6, 3.6), (9.0, -3.2)], W);
            pen.disc((9.4, -4.4), 1.8);
            pen.arc((9.4, -4.4), 4.0, 75.0, 15.0, 1.4, None);
        }
    }
}

/// F – Kaffeetasse mit Gesicht.
fn kaffeetasse(pen: &mut Pen, state: State) {
    // Becher groß, Dampf kurz – sonst wirkt die Tasse neben den Systemsymbolen klein
    let (x0, y0, w, h) = (-7.0, -3.5, 12.0, 12.5);
    let cx = x0 + w / 2.0;
    let (el, er, ey) = (cx - 2.4, cx + 2.4, 1.7);
    // Henkel
    pen.arc((5.0, 2.6), 3.0, 75.0, -75.0, W, None);
    match state {
        State::Running | State::Break => {
            pen.steam(cx - 2.2, -8.6, 0.75);
            pen.steam(cx + 2.2, -8.6, 0.75);
            pen.accent();
            pen.rrect((x0 - 0.5, y0 - 0.5), (w + 1.0, h + 1.0), 3.2, None, None);
            pen.white();
            if state == State::Running {
                pen.disc((el, ey), 1.1);
                pen.disc((er, ey), 1.1);
                pen.arc((cx, 3.9), 2.5, 340.0, 200.0, W_DETAIL, None);
            } else {
                pen.happy_eye(el, ey, 1.3);
                pen.happy_eye(er, ey, 1.3);
                pen.arc((cx, 3.7), 2.7, 350.0, 190.0, W_DETAIL, None);
            }
            pen.base();
        }
        State::Outside => {
            pen.rrect((x0, y0), (w, h), 2.8, Some(W), None);
            // müde Augen, kleiner Mund, „z“
            pen.line(&[(el - 1.2, ey + 0.4), (el + 1.2, ey + 0.4)], W_DETAIL);
            pen.line(&[(er - 1.2, ey + 0.4), (er + 1.2, ey + 0.4)], W_DETAIL);
            pen.line(&[(cx - 1.0, 5.4), (cx + 1.0, 5.4)], W_DETAIL);
            pen.zee(3.2, -8.8, 2.4, 1.4);
        }
        State::Closed => {
            pen.rrect((x0, y0), (w, h), 2.8, Some(W), None);
            pen.line(&[(cx - 3.0, 3.2), (cx - 0.9, 5.4), (cx + 3.0, 1.2)], W_DETAIL);
        }
    }
}

/// G – Hamster im Hamsterrad.
fn hamsterrad(pen: &mut Pen, state: State) {
    let c = (0.0, -1.0);
    const R: f64 = 7.5;
    // Gestell
    pen.line(&[(-2.8, 6.4), (-4.8, 9.6)], W);
    pen.line(&[(2.8, 6.4), (4.8, 9.6)], W);
    pen.line(&[(-6.6, 9.6), (6.6, 9.6)], W);
    // Rad mit Speichen
    match state {
        State::Outside => pen.ring(c, R, W, Some(2.0 * PI * R / 16.0)),
        _ => pen.ring(c, R, W, None),
    }
    pen.disc(c, 0.9);
    match state {
        State::Running => {
            // Hamster rennt, Tempostriche dahinter
            pen.line(&[(-5.2, 0.0), (-3.6, 0.0)], 1.4);
            pen.line(&[(-5.0, 2.4), (-3.6, 2.4)], 1.4);
            pen.accent();
            pen.disc((0.2, 2.4), 3.2);
            pen.disc((3.2, 0.8), 2.3);
            pen.disc((2.3, -1.3), 1.0);
            pen.white();
            pen.disc((3.9, 0.4), 0.6);
            pen.base();
        }
        State::Break => {
            // Hamster rollt sich ein und schläft
            pen.accent();
            pen.disc((-0.4, 2.5), 3.3);
            pen.disc((2.4, 2.6), 2.2);
            pen.disc((1.8, 0.5), 0.9);
            pen.white();
            pen.line(&[(2.3, 2.3), (3.6, 2.3)], 1.0);
            pen.base();
        }
        // leeres Rad mit Speichen: außerhalb gepunktet, Feierabend durchgezogen
        State::Outside | State::Closed => {
            pen.ink = Ink::Base(0.5);
            for a in [90.0, 210.0, 330.0] {
                pen.line(&[c, on_circle(c.0, c.1, R - 0.8, a)], 1.2);
            }
            pen.base();
        }
    }
}

/// H – Roboter.
fn roboter(pen: &mut Pen, state: State) {
    let (x0, y0, w, h) = (-6.5, -4.5, 13.0, 11.0);
    let (el, er, ey) = (-2.8, 2.8, 0.4);
    // Antenne und Ohren
    pen.line(&[(0.0, y0), (0.0, -7.4)], W_DETAIL);
    pen.rrect((-8.6, -0.4), (1.6, 3.6), 0.7, None, None);
    pen.rrect((7.0, -0.4), (1.6, 3.6), 0.7, None, None);
    match state {
        State::Running | State::Break => {
            pen.accent();
            pen.disc((0.0, -8.6), 1.6);
            pen.rrect((x0 - 0.5, y0 - 0.5), (w + 1.0, h + 1.0), 3.4, None, None);
            pen.white();
            if state == State::Running {
                pen.disc((el, ey), 1.7);
                pen.disc((er, ey), 1.7);
                pen.line(&[(-2.6, 3.8), (2.6, 3.8)], W_DETAIL);
            } else {
                // schläfrig, gähnt
                pen.line(&[(el - 1.4, ey + 0.6), (el + 1.4, ey + 0.6)], W_DETAIL);
                pen.line(&[(er - 1.4, ey + 0.6), (er + 1.4, ey + 0.6)], W_DETAIL);
                pen.ring((0.0, 3.8), 1.0, 1.3, None);
            }
            pen.base();
        }
        State::Outside => {
            pen.ring((0.0, -8.6), 1.3, 1.3, None);
            pen.rrect((x0, y0), (w, h), 3.0, Some(W), Some(2.6));
            pen.line(&[(el - 1.0, ey + 0.4), (el + 1.0, ey + 0.4)], W_DETAIL);
            pen.line(&[(er - 1.0, ey + 0.4), (er + 1.0, ey + 0.4)], W_DETAIL);
        }
        State::Closed => {
            pen.disc((0.0, -8.6), 1.6);
            pen.rrect((x0, y0), (w, h), 3.0, Some(W), None);
            pen.happy_eye(el, ey + 0.2, 1.4);
            pen.happy_eye(er, ey + 0.2, 1.4);
            pen.arc((0.0, 2.4), 2.4, 340.0, 200.0, W_DETAIL, None);
        }
    }
}

/// J – Kaninchen: Ohren hoch bei der Arbeit, eins knickt in der Pause um.
fn kaninchen(pen: &mut Pen, state: State) {
    let head = (0.0, 3.4);
    const R: f64 = 5.6;
    const EAR: f64 = 3.0;
    const BASE: f64 = -2.3; // Ohransatz (endet am Kopf)
    let (el, er, ey) = (-2.2, 2.2, 2.6);
    let left_ear = |pen: &mut Pen| pen.line(&[(-2.3, BASE), (-3.2, -9.0)], EAR);
    let right_ear = |pen: &mut Pen| pen.line(&[(2.3, BASE), (3.2, -9.0)], EAR);
    let closed_eyes = |pen: &mut Pen| {
        pen.line(&[(el - 0.9, ey + 0.3), (el + 0.9, ey + 0.3)], 1.4);
        pen.line(&[(er - 0.9, ey + 0.3), (er + 0.9, ey + 0.3)], 1.4);
    };
    match state {
        State::Running | State::Break => {
            pen.accent();
            left_ear(pen);
            if state == State::Running {
                right_ear(pen);
            } else {
                // umgeknicktes Ohr
                pen.line(&[(2.3, BASE), (3.0, -5.6), (5.8, -3.8)], EAR);
            }
            pen.disc(head, R);
            pen.white();
            pen.line(&[(-2.5, -3.2), (-3.0, -7.6)], 1.0);
            if state == State::Running {
                pen.line(&[(2.5, -3.2), (3.0, -7.6)], 1.0);
                pen.disc((el, ey), 0.95);
                pen.disc((er, ey), 0.95);
            } else {
                closed_eyes(pen);
            }
            pen.disc((0.0, 4.9), 0.8);
            pen.base();
        }
        State::Outside => {
            // schläft: Augen zu, gepunkteter Kopf, „z“
            left_ear(pen);
            right_ear(pen);
            pen.ring(head, R, W, Some(2.0 * PI * R / 12.0));
            closed_eyes(pen);
            pen.disc((0.0, 5.0), 0.8);
            pen.zee(5.9, -9.0, 2.2, 1.4);
        }
        State::Closed => {
            left_ear(pen);
            right_ear(pen);
            pen.ring(head, R, W, None);
            pen.line(&[(-2.6, 3.4), (-0.8, 5.2), (2.8, 1.6)], W_DETAIL);
        }
    }
}

fn paint(set: IconSet, state: State, pen: &mut Pen) {
    match set {
        IconSet::Tagbogen => tagbogen(pen, state),
        IconSet::Schlossuhr => schlossuhr(pen, state),
        IconSet::Tagesring => tagesring(pen, state),
        IconSet::Bauarbeiter => figure(pen, state, Figure::Worker),
        IconSet::Handwerkerin => figure(pen, state, Figure::Craftswoman),
        IconSet::Kaffeetasse => kaffeetasse(pen, state),
        IconSet::Hamsterrad => hamsterrad(pen, state),
        IconSet::Roboter => roboter(pen, state),
        IconSet::Kaninchen => kaninchen(pen, state),
    }
}

/// Gemeinsame Ausdehnung aller Zustände eines Satzes (ohne Strichstärke).
fn bounds(set: IconSet) -> ((f64, f64), (f64, f64)) {
    let mut pen = Pen::measure();
    for state in State::ALL {
        paint(set, state, &mut pen);
    }
    (pen.min, pen.max)
}

/// Lage der Figur in einem Symbol der Größe `size`.
struct Layout {
    /// Maßstab der Geometrie und der Strichstärke
    s: f64,
    k: f64,
    /// Breite und Höhe des Bildes (gerundet), Breite der Figur ohne Strich
    width: f64,
    height: f64,
    figure_w: f64,
    min_x: f64,
    oy: f64,
}

fn layout(set: IconSet, size: f64) -> Layout {
    let (min, max) = bounds(set);
    let k = size / REF_H;
    let pad = W * k / 2.0;
    let s = (size * optical(set) - 2.0 * pad) / (max.1 - min.1);
    let figure_w = (max.0 - min.0) * s;
    // höher als `size` nur, wenn die Figur es verlangt
    let height = (size * optical(set).max(1.0)).ceil();
    Layout {
        s,
        k,
        width: (figure_w + 2.0 * pad).ceil(),
        height,
        figure_w,
        min_x: min.0,
        // senkrecht mittig im Bild
        oy: (height - (max.1 - min.1) * s) / 2.0 - min.1 * s,
    }
}

/// Breite der Symbole eines Satzes bei der Symbolgröße `size`.
pub fn width(set: IconSet, size: f64) -> f64 {
    layout(set, size).width
}

/// Bild eines Zustands zur Symbolgröße `size` (Höhe des Bildes: `size`, bei schmalen
/// Figuren etwas mehr); mindestens `min_width` breit (die Figur steht dann mittig).
pub fn image(set: IconSet, state: State, size: f64, min_width: f64, description: &str) -> Retained<NSImage> {
    let Layout { s, k, width, height, figure_w, min_x, oy } = layout(set, size);
    let width = width.max(min_width.ceil());
    let ox = (width - figure_w) / 2.0 - min_x * s;
    let accent = state.accent();
    let handler = RcBlock::new(move |_rect: NSRect| -> Bool {
        let mut pen = Pen { draw: true, s, k, ox, oy, accent, ..Pen::measure() };
        paint(set, state, &mut pen);
        Bool::YES
    });
    let image = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(width, height), true, &handler);
    if accent.is_some() {
        // Grundfarbe bei jedem Zeichnen neu auflösen (hell/dunkel der Menüleiste)
        image.setCacheMode(NSImageCacheMode::Never);
    } else {
        image.setTemplate(true);
    }
    image.setAccessibilityDescription(Some(&NSString::from_str(description)));
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBitmapImageFileType, NSBitmapImageRep,
        NSDeviceRGBColorSpace, NSGraphicsContext,
    };
    use objc2_foundation::NSDictionary;

    #[test]
    fn bounds_are_finite_and_shapes_fit_their_height() {
        for set in IconSet::ALL {
            let (min, max) = bounds(set);
            let (w, h) = (max.0 - min.0, max.1 - min.1);
            assert!(w > 0.0 && h > 0.0, "{set:?}");
            // Seitenverhältnisse wie im Entwurf: Bogen breit, Schloss hoch, Ring rund
            let ratio = w / h;
            match set {
                IconSet::Tagbogen => assert!((1.8..2.4).contains(&ratio), "{ratio}"),
                IconSet::Schlossuhr => assert!((0.5..0.8).contains(&ratio), "{ratio}"),
                IconSet::Tagesring => assert!((ratio - 1.0).abs() < 1e-6, "{ratio}"),
                _ => assert!((0.6..1.6).contains(&ratio), "{set:?}: {ratio}"),
            }
        }
    }

    #[test]
    fn widths_stay_moderate() {
        // bei 18 pt: kein Satz breiter als ein typisches breites Systemsymbol
        for set in IconSet::ALL {
            assert!(width(set, 18.0) <= 28.0, "{set:?}: {}", width(set, 18.0));
        }
    }

    /// Vorschau aller Symbole als PNG (@2x) auf hellem und dunklem Grund:
    /// `TAGWERK_GLYPH_DIR=/pfad cargo test render_glyphs -- --ignored`
    /// (Symbolgröße über `TAGWERK_GLYPH_SIZE`, Standard 18 pt)
    #[test]
    #[ignore]
    fn render_glyphs() {
        let dir = std::env::var("TAGWERK_GLYPH_DIR").expect("TAGWERK_GLYPH_DIR");
        let size: f64 = std::env::var("TAGWERK_GLYPH_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(18.0);
        for (mode, name) in [("light", unsafe { NSAppearanceNameAqua }), ("dark", unsafe { NSAppearanceNameDarkAqua })] {
            let appearance = NSAppearance::appearanceNamed(name).unwrap();
            #[allow(deprecated)]
            unsafe {
                NSAppearance::setCurrentAppearance(Some(&appearance))
            };
            for set in IconSet::ALL {
                for (i, state) in State::ALL.into_iter().enumerate() {
                    let img = image(set, state, size, 0.0, "");
                    let size = img.size();
                    let (w, h) = ((size.width * 2.0) as isize, (size.height * 2.0) as isize);
                    let rep = unsafe {
                        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                            NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false, NSDeviceRGBColorSpace, 0, 0,
                        )
                    }
                    .unwrap();
                    NSGraphicsContext::saveGraphicsState_class();
                    NSGraphicsContext::setCurrentContext(NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).as_deref());
                    img.drawInRect(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w as f64, h as f64)));
                    NSGraphicsContext::restoreGraphicsState_class();
                    let data =
                        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
                            .unwrap();
                    let tpl = if img.isTemplate() { "t" } else { "c" };
                    std::fs::write(format!("{dir}/{mode}_{}_{i}_{tpl}.png", set.name()), data.to_vec()).unwrap();
                }
            }
        }
    }
}
