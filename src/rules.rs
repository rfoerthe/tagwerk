//! Einstellungen und Regeln der Zeiterfassung.
//!
//! Einstellbar sind der Arbeitszeitrahmen, die Regelwochenarbeitszeit und die Mindestdauer
//! einer Pause. Fest sind die Regeln aus Arbeitszeitgesetz und Betriebsvereinbarung:
//!
//! - **Arbeitszeitrahmen** – Arbeit nur Mo–Fr im eingestellten Zeitraum (Standard 07:00–22:00);
//!   außerhalb wird weder Arbeitszeit noch Pause gezählt, es gibt aber einen Hinweis.
//! - **Sollarbeitszeit** – Regelwochenarbeitszeit verteilt auf fünf Arbeitstage
//!   (39 Std. = 7:48 Std. pro Tag); daraus ergibt sich der Saldo.
//! - **Pausen** (§ 4 ArbZG) – mehr als 6 Std. Arbeit: 30 Min., mehr als 9 Std.: 45 Min.;
//!   gezählt werden Abschnitte von mindestens 15 Min.
//! - **Ruhezeit** (§ 5 ArbZG) – mindestens 11 Std. zwischen zwei Arbeitstagen.
//! - **Höchstarbeitszeit** – mehr als 10 Std. am Tag gelten als Verstoß.

use crate::day::{fmt_clock, Day, Span, DAY_END};
use chrono::{Datelike, NaiveDate, Weekday};

/// Gesetzlich zählen nur Pausenabschnitte von mindestens 15 Minuten.
pub const LEGAL_BREAK_SEGMENT: i64 = 15 * 60;
/// Mehr als 10 Stunden Arbeit am Tag sind ein Verstoß.
pub const MAX_DAILY_WORK: i64 = 10 * 3600;
/// Mindestruhezeit zwischen zwei Arbeitstagen.
pub const MIN_REST: i64 = 11 * 3600;
/// Arbeitstage pro Woche (Montag bis Freitag).
pub const WORKDAYS_PER_WEEK: i64 = 5;

/// Grenzen der Einstellungen.
pub const MIN_PAUSE_RANGE: (i64, i64) = (60, 120 * 60);
pub const WEEKLY_RANGE: (i64, i64) = (3600, 60 * 3600);
/// Höchstwert für die Mindestdauer von Arbeit ohne Mac (Untergrenze: die Mindestpause).
pub const MIN_WORK_MAX: i64 = 240 * 60;

/// Symbolsatz für die Menüleiste.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IconSet {
    /// Der Arbeitstag als Sonnenbogen über dem Horizont.
    #[default]
    Tagbogen,
    /// Vorhängeschloss mit Uhr: entsperrt = Arbeitszeit.
    Schlossuhr,
    /// Fortschrittsring zum Tagessoll.
    Tagesring,
    /// Bauarbeiter mit Helm und Hammer.
    Bauarbeiter,
    /// Handwerkerin mit Dutt, Schleife und Schraubenschlüssel.
    Handwerkerin,
    /// Kaffeetasse mit Gesicht.
    Kaffeetasse,
    /// Hamster im Hamsterrad.
    Hamsterrad,
    /// Roboter.
    Roboter,
    /// Kaninchen mit langen Ohren.
    Kaninchen,
}

impl IconSet {
    pub const ALL: [IconSet; 9] = [
        IconSet::Tagbogen,
        IconSet::Schlossuhr,
        IconSet::Tagesring,
        IconSet::Bauarbeiter,
        IconSet::Handwerkerin,
        IconSet::Kaffeetasse,
        IconSet::Hamsterrad,
        IconSet::Roboter,
        IconSet::Kaninchen,
    ];

    pub fn name(self) -> &'static str {
        match self {
            IconSet::Tagbogen => "Tagbogen",
            IconSet::Schlossuhr => "Schlossuhr",
            IconSet::Tagesring => "Tagesring",
            IconSet::Bauarbeiter => "Bauarbeiter",
            IconSet::Handwerkerin => "Handwerkerin",
            IconSet::Kaffeetasse => "Kaffeetasse",
            IconSet::Hamsterrad => "Hamsterrad",
            IconSet::Roboter => "Roboter",
            IconSet::Kaninchen => "Kaninchen",
        }
    }

    /// Wert für die Datenbank.
    pub fn id(self) -> i64 {
        match self {
            IconSet::Tagbogen => 0,
            IconSet::Schlossuhr => 1,
            IconSet::Tagesring => 2,
            IconSet::Bauarbeiter => 3,
            IconSet::Handwerkerin => 4,
            IconSet::Kaffeetasse => 5,
            IconSet::Hamsterrad => 6,
            IconSet::Roboter => 7,
            IconSet::Kaninchen => 8,
        }
    }

    /// Unbekannte Werte ergeben den Standard.
    pub fn from_id(id: i64) -> Self {
        Self::ALL.into_iter().find(|s| s.id() == id).unwrap_or_default()
    }
}

/// Vom Benutzer einstellbare Werte (alle Zeiten in Sekunden).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Mindestdauer einer Pause: kürzere Sperrzeiten zählen zur Arbeitszeit.
    pub min_pause: i64,
    /// Mindestdauer von Arbeit ohne Mac beim Eintragen (nie kürzer als `min_pause`).
    pub min_work: i64,
    /// Regelwochenarbeitszeit.
    pub weekly_target: i64,
    /// Beginn des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_from: i64,
    /// Ende des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_to: i64,
    /// Symbolsatz der Menüleiste.
    pub icon_set: IconSet,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            min_pause: 15 * 60,
            min_work: 15 * 60,
            weekly_target: 39 * 3600,
            frame_from: 7 * 3600,
            frame_to: 22 * 3600,
            icon_set: IconSet::default(),
        }
    }
}

impl Settings {
    /// Werte in gültige Grenzen bringen (z. B. nach dem Laden aus der Datenbank).
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let min_pause = self.min_pause.clamp(MIN_PAUSE_RANGE.0, MIN_PAUSE_RANGE.1);
        let mut s = Self {
            min_pause,
            min_work: self.min_work.clamp(min_pause, MIN_WORK_MAX),
            weekly_target: self.weekly_target.clamp(WEEKLY_RANGE.0, WEEKLY_RANGE.1),
            frame_from: self.frame_from.clamp(0, DAY_END),
            frame_to: self.frame_to.clamp(0, DAY_END),
            icon_set: self.icon_set,
        };
        if s.frame_to <= s.frame_from {
            s.frame_from = d.frame_from;
            s.frame_to = d.frame_to;
        }
        s
    }

    /// Montag bis Freitag.
    pub fn is_workday(date: NaiveDate) -> bool {
        !matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Sollarbeitszeit eines Tages (am Wochenende 0).
    pub fn daily_target(&self, date: NaiveDate) -> i64 {
        if Self::is_workday(date) {
            self.daily_target_workday()
        } else {
            0
        }
    }

    /// Sollarbeitszeit eines Arbeitstags (Wochenarbeitszeit / 5).
    pub fn daily_target_workday(&self) -> i64 {
        self.weekly_target / WORKDAYS_PER_WEEK
    }

    /// Liegt die Uhrzeit `secs` im Arbeitszeitrahmen?
    pub fn in_frame(&self, secs: i64) -> bool {
        (self.frame_from..self.frame_to).contains(&secs)
    }

    /// `07:00–22:00 Uhr`
    pub fn frame_text(&self) -> String {
        format!("{}–{} Uhr", fmt_clock(self.frame_from), fmt_clock(self.frame_to))
    }

    /// Kurzfassung für Fußzeilen und Export.
    pub fn summary(&self) -> String {
        format!(
            "Arbeitszeitrahmen Mo–Fr {}  ·  Regelarbeitszeit {} Std./Woche  ·  Pausen ab {} Min.",
            self.frame_text(),
            fmt_hours(self.weekly_target),
            self.min_pause / 60
        )
    }
}

/// `39` bzw. `38,5` (Stunden, höchstens zwei Nachkommastellen).
pub fn fmt_hours(secs: i64) -> String {
    let h = secs as f64 / 3600.0;
    let s = format!("{h:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.replace('.', ",")
}

/// Liest `39`, `38,5` oder `38.5` (Stunden) und gibt Sekunden zurück.
pub fn parse_hours(s: &str) -> Option<i64> {
    let h: f64 = s.trim().replace(',', ".").parse().ok()?;
    h.is_finite().then(|| (h * 60.0).round() as i64 * 60)
}

/// Hinweis bei einem Verstoß gegen eine Regel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// Zu wenig Pause für die Arbeitszeit (`required` Sekunden vorgeschrieben).
    Breaks { required: i64 },
    /// Mehr als 10 Stunden Arbeit.
    MaxHours { work: i64 },
    /// Ruhezeit seit dem letzten Arbeitstag zu kurz.
    Rest { rest: i64 },
    /// Arbeitszeit außerhalb des Arbeitszeitrahmens (nicht gezählt): wann und in welchem Rahmen.
    OutsideFrame { spans: Vec<Span>, frame: String },
    /// Arbeit am Wochenende.
    Weekend,
}

impl Warning {
    pub fn text(&self) -> String {
        match self {
            Warning::Breaks { required } => format!(
                "Ab {} Std. Arbeit sind {} Min. Pause vorgeschrieben (§ 4 ArbZG)",
                if *required >= 45 * 60 { 9 } else { 6 },
                required / 60
            ),
            Warning::MaxHours { work } => {
                format!("{} Arbeitszeit – mehr als 10 Std. am Tag (§ 3 ArbZG)", fmt_hm(*work))
            }
            Warning::Rest { rest } => {
                format!("Ruhezeit nur {} – vorgeschrieben sind 11 Std. (§ 5 ArbZG)", fmt_hm(*rest))
            }
            Warning::OutsideFrame { spans, frame } => {
                format!("{} außerhalb des Arbeitszeitrahmens ({frame}) – nicht gezählt", fmt_spans(spans))
            }
            Warning::Weekend => "Arbeit am Wochenende – außerhalb des Arbeitszeitrahmens (Mo–Fr)".to_string(),
        }
    }
}

/// `00:00–00:24 Uhr`, `06:30–07:00 und 22:00–23:15 Uhr` bzw. bei vielen Abschnitten
/// `4 Zeiträume (1:05 Std.)`.
fn fmt_spans(spans: &[Span]) -> String {
    // auf volle Minuten gerundet (23:59:59 wird 24:00)
    let round = |t: i64| fmt_clock((t + 30) / 60 * 60);
    let parts: Vec<String> = spans.iter().map(|s| format!("{}–{}", round(s.from), round(s.to))).collect();
    match parts.as_slice() {
        [] => String::new(),
        [one] => format!("{one} Uhr"),
        [rest @ .., last] if parts.len() <= 3 => format!("{} und {last} Uhr", rest.join(", ")),
        _ => format!(
            "{} Zeiträume ({})",
            parts.len(),
            fmt_hm(spans.iter().map(Span::len).sum())
        ),
    }
}

fn fmt_hm(secs: i64) -> String {
    let m = secs.max(0) / 60;
    format!("{}:{:02} Std.", m / 60, m % 60)
}

/// Hinweise für den Tag `date`. `days` muss nach Datum sortiert sein; der vorige
/// Arbeitstag wird für die Ruhezeit gebraucht.
pub fn day_warnings(days: &[Day], date: NaiveDate, s: &Settings) -> Vec<Warning> {
    let Some(idx) = days.iter().position(|d| d.date == date) else { return Vec::new() };
    let day = &days[idx];
    let mut out = Vec::new();
    let spans = day.spans(s);
    let work: i64 = spans.iter().map(|x| x.len()).sum();

    let pause = Day::legal_pause_secs(&spans);
    if work > 9 * 3600 && pause < 45 * 60 {
        out.push(Warning::Breaks { required: 45 * 60 });
    } else if work > 6 * 3600 && pause < 30 * 60 {
        out.push(Warning::Breaks { required: 30 * 60 });
    }
    if work > MAX_DAILY_WORK {
        out.push(Warning::MaxHours { work });
    }
    if let (Some(first), Some(prev)) = (
        spans.first(),
        days[..idx].iter().rev().find_map(|d| d.record(s).filter(|r| r.work_secs > 0)),
    ) {
        let rest = (date - prev.date).num_days() * DAY_END + first.from - prev.last_end;
        if rest < MIN_REST {
            out.push(Warning::Rest { rest });
        }
    }
    if work > 0 && !Settings::is_workday(date) {
        out.push(Warning::Weekend);
    }
    // Abschnitte unter einer Minute (z. B. kurz entsperrt in der Nacht) lohnen keinen Hinweis.
    let outside: Vec<Span> = day.outside_frame_spans(s).into_iter().filter(|x| x.len() >= 60).collect();
    if !outside.is_empty() {
        out.push(Warning::OutsideFrame { spans: outside, frame: s.frame_text() });
    }
    out
}

/// Arbeitszeit und Soll der Tage von `from` bis `to`. Soll gibt es nur für Tage, an denen
/// gearbeitet wurde – Urlaub, Feiertage oder Krankheit drücken den Saldo also nicht.
pub fn balance(days: &[Day], from: NaiveDate, to: NaiveDate, s: &Settings) -> (i64, i64) {
    days.iter()
        .filter(|d| d.date >= from && d.date <= to)
        .filter_map(|d| d.record(s))
        .filter(|r| r.work_secs > 0)
        .fold((0, 0), |(w, t), r| (w + r.work_secs, t + s.daily_target(r.date)))
}

/// `+1:05 h` bzw. `−0:30 h`
pub fn fmt_balance(secs: i64) -> String {
    let m = secs.abs() / 60;
    let sign = if secs < 0 && m > 0 { "−" } else { "+" };
    format!("{sign}{}:{:02} h", m / 60, m % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn defaults_and_target() {
        let s = Settings::default();
        assert_eq!(s.daily_target(date(16)), h(7, 48)); // Montag
        assert_eq!(s.daily_target(date(15)), 0); // Sonntag
        assert_eq!(s.frame_text(), "07:00–22:00 Uhr");
        let bad = Settings { frame_from: h(22, 0), frame_to: h(7, 0), min_pause: 0, weekly_target: 0, ..s };
        let fixed = bad.sanitized();
        assert_eq!((fixed.frame_from, fixed.frame_to), (h(7, 0), h(22, 0)));
        assert_eq!(fixed.min_pause, 60);
        assert_eq!(fixed.weekly_target, 3600);
        // Arbeit ohne Mac nie kürzer als die Mindestpause.
        assert_eq!(fixed.min_work, s.min_work);
        let s20 = Settings { min_pause: 20 * 60, min_work: 15 * 60, ..s }.sanitized();
        assert_eq!(s20.min_work, 20 * 60);
        assert_eq!(Settings { min_work: 999 * 60, ..s }.sanitized().min_work, MIN_WORK_MAX);
    }

    #[test]
    fn icon_set_ids() {
        assert_eq!(IconSet::default(), IconSet::Tagbogen);
        for s in IconSet::ALL {
            assert_eq!(IconSet::from_id(s.id()), s);
        }
        assert_eq!(IconSet::from_id(99), IconSet::Tagbogen);
    }

    #[test]
    fn hours_text() {
        assert_eq!(fmt_hours(h(39, 0)), "39");
        assert_eq!(fmt_hours(h(38, 30)), "38,5");
        assert_eq!(parse_hours("38,5"), Some(h(38, 30)));
        assert_eq!(parse_hours(" 40 "), Some(h(40, 0)));
        assert_eq!(parse_hours("x"), None);
        assert_eq!(fmt_balance(-h(0, 30)), "−0:30 h");
        assert_eq!(fmt_balance(h(1, 5)), "+1:05 h");
        assert_eq!(fmt_balance(-20), "+0:00 h");
    }

    #[test]
    fn warnings() {
        let s = Settings::default();
        // Montag bis 22:00, Dienstag ab 07:00: nur 9 Std. Ruhezeit
        let mut mon = Day::new(date(16));
        mon.add_period(h(12, 0), h(22, 0));
        let mut tue = Day::new(date(17));
        tue.add_period(h(6, 30), h(12, 0));
        let mut sat = Day::new(date(21));
        sat.add_period(h(10, 0), h(11, 0));
        let days = vec![mon, tue, sat];

        let w = day_warnings(&days, date(16), &s);
        assert!(w.contains(&Warning::Breaks { required: 45 * 60 }), "{w:?}");
        assert!(!w.iter().any(|x| matches!(x, Warning::MaxHours { .. })), "genau 10 Std. sind erlaubt");

        let w = day_warnings(&days, date(17), &s);
        assert!(w.contains(&Warning::Rest { rest: h(9, 0) }), "{w:?}");
        let outside = Warning::OutsideFrame { spans: vec![Span::new(h(6, 30), h(7, 0))], frame: s.frame_text() };
        assert!(w.contains(&outside), "{w:?}");
        assert_eq!(
            outside.text(),
            "06:30–07:00 Uhr außerhalb des Arbeitszeitrahmens (07:00–22:00 Uhr) – nicht gezählt"
        );

        assert_eq!(day_warnings(&days, date(21), &s), vec![Warning::Weekend]);

        let mut long = Day::new(date(18));
        long.add_period(h(7, 0), h(12, 0));
        long.add_period(h(13, 0), h(19, 0));
        let w = day_warnings(&[long], date(18), &s);
        assert_eq!(w, vec![Warning::MaxHours { work: h(11, 0) }]);
    }

    #[test]
    fn outside_frame_text_lists_times() {
        let s = Settings::default();
        // wie in der Nacht: entsperrt 00:00–00:02 und 00:15–00:24 (13 Min. Sperre zählt mit),
        // 30 Sekunden um 02:03 werden nicht erwähnt, danach Arbeit im Rahmen
        let mut d = Day::new(date(17));
        d.add_period(0, 2 * 60 + 39);
        d.add_period(h(0, 15) + 50, h(0, 24) + 26);
        d.add_period(h(2, 3), h(2, 3) + 30);
        d.add_period(h(12, 8), h(12, 23));
        let w = day_warnings(&[d], date(17), &s);
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(
            w[0].text(),
            "00:00–00:24 Uhr außerhalb des Arbeitszeitrahmens (07:00–22:00 Uhr) – nicht gezählt"
        );
        let two = Warning::OutsideFrame {
            spans: vec![Span::new(h(6, 30), h(7, 0)), Span::new(h(22, 0), DAY_END)],
            frame: s.frame_text(),
        };
        assert!(two.text().starts_with("06:30–07:00 und 22:00–24:00 Uhr außerhalb"), "{}", two.text());
        let many = Warning::OutsideFrame { spans: vec![Span::new(0, 600); 4], frame: s.frame_text() };
        assert!(many.text().starts_with("4 Zeiträume (0:40 Std.) außerhalb"), "{}", many.text());
        let late = Warning::OutsideFrame { spans: vec![Span::new(h(23, 22) + 34, DAY_END - 1)], frame: s.frame_text() };
        assert!(late.text().starts_with("23:23–24:00 Uhr außerhalb"), "{}", late.text());
    }

    #[test]
    fn balance_counts_only_worked_days() {
        let s = Settings::default();
        let mut mon = Day::new(date(16));
        mon.add_period(h(8, 0), h(17, 0)); // 9 Std.
        let mut sat = Day::new(date(21));
        sat.add_period(h(10, 0), h(11, 0)); // Wochenende: kein Soll
        let (work, target) = balance(&[mon, sat], date(16), date(22), &s);
        assert_eq!((work, target), (h(10, 0), h(7, 48)));
    }
}
