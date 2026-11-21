//! Einstellungen und Regeln der Zeiterfassung.
//!
//! Einstellbar sind der Arbeitszeitrahmen, die Regelwochenarbeitszeit und die Mindestdauer
//! einer Pause. Fest sind die Regeln aus Arbeitszeitgesetz und Betriebsvereinbarung:
//!
//! - **Arbeitszeitrahmen** – Arbeit nur im eingestellten Zeitraum (Standard 07:00–22:00), an
//!   jedem Tag; außerhalb wird weder Arbeitszeit noch Pause gezählt, es gibt aber einen Hinweis.
//!   Arbeitstage sind Mo–Fr: Arbeit am Wochenende zählt ohne Soll als Mehrarbeit und wird
//!   ebenfalls gemeldet.
//! - **Sollarbeitszeit** – Regelwochenarbeitszeit verteilt auf fünf Arbeitstage
//!   (39 Std. = 7:48 Std. pro Tag); daraus ergibt sich der Saldo.
//! - **Pausen** (§ 4 ArbZG) – mehr als 6 Std. Arbeit: 30 Min., mehr als 9 Std.: 45 Min.;
//!   gezählt werden Abschnitte von mindestens 15 Min. Länger als 6 Std. am Stück ohne
//!   solche Pause sind ebenfalls ein Verstoß.
//! - **Ruhezeit** (§ 5 ArbZG) – mindestens 11 Std. zwischen zwei Arbeitstagen.
//! - **Höchstarbeitszeit** – mehr als 10 Std. am Tag gelten als Verstoß; bei 9:45 und 10 Std.
//!   kommt eine Mitteilung (siehe [`LimitWatch`]).

use crate::day::{fmt_clock, Absence, Day, Span, DAY_END};
use chrono::{Datelike, NaiveDate, Weekday};

/// Gesetzlich zählen nur Pausenabschnitte von mindestens 15 Minuten.
pub const LEGAL_BREAK_SEGMENT: i64 = 15 * 60;
/// Länger als 6 Stunden am Stück darf niemand ohne Pause arbeiten.
pub const MAX_STRETCH: i64 = 6 * 3600;
/// Mehr als 10 Stunden Arbeit am Tag sind ein Verstoß.
pub const MAX_DAILY_WORK: i64 = 10 * 3600;
/// Mindestruhezeit zwischen zwei Arbeitstagen.
pub const MIN_REST: i64 = 11 * 3600;
/// Alle Arbeitstage: Montag bis Freitag (Bit 0 = Montag … Bit 4 = Freitag).
pub const ALL_WORKDAYS: u8 = 0b1_1111;
/// Grenzen der Inaktivität, ab der die Erfassung anhält (0 = aus).
pub const IDLE_RANGE: (i64, i64) = (60, 120 * 60);
/// Höchster Betrag des Übertrags im Zeitkonto.
pub const CARRY_MAX: i64 = 10_000 * 3600;
/// Ab dieser Sperrdauer fragt Tagwerk beim Entsperren nach, was die Zeit war (sofern
/// eingeschaltet) – nie kürzer als die Mindestpause.
pub const ASK_AFTER_LOCK: i64 = 30 * 60;

/// Grenzen der Einstellungen.
pub const MIN_PAUSE_RANGE: (i64, i64) = (60, 120 * 60);
pub const WEEKLY_RANGE: (i64, i64) = (3600, 60 * 3600);
/// Grenzen für die Mindestdauer von Arbeit ohne Mac beim Eintragen. Mit der Mindestpause hat
/// sie nichts zu tun: Auch ein kurzer Termin in einer langen Pause oder vor dem Arbeitsbeginn
/// ist Arbeit; die Grenze schützt nur vor versehentlichen Einträgen.
pub const MIN_WORK_RANGE: (i64, i64) = (60, 240 * 60);

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
    /// Mindestdauer von Arbeit ohne Mac beim Eintragen von Hand (unabhängig von `min_pause`).
    pub min_work: i64,
    /// Regelwochenarbeitszeit.
    pub weekly_target: i64,
    /// Beginn des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_from: i64,
    /// Ende des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_to: i64,
    /// Symbolsatz der Menüleiste.
    pub icon_set: IconSet,
    /// Arbeitszeit von heute neben dem Symbol in der Menüleiste zeigen.
    pub show_time: bool,
    /// Globale Kurzbefehle (wirken in jeder App).
    pub hotkeys: bool,
    /// Arbeitstage der Woche als Bits (Bit 0 = Montag … Bit 4 = Freitag); das Wochensoll
    /// verteilt sich gleichmäßig auf sie.
    pub workdays: u8,
    /// Automatische Erfassung zählt nur an Arbeitstagen: An Samstag, Sonntag und freien
    /// Wochentagen wird sie aufgezeichnet, zählt aber erst nach „Heute erfassen“ (siehe
    /// [`crate::day::Day::free_day`]).
    pub auto_workdays_only: bool,
    /// Ohne Eingabe so lange (Sekunden) gilt der Mac als verlassen, die Erfassung hält an
    /// (0 = aus).
    pub idle_secs: i64,
    /// Beim Entsperren nach langer Sperre nachfragen, ob es Pause, Arbeit oder Feierabend war.
    pub ask_after_lock: bool,
    /// Mitteilungen zur Höchstarbeitszeit (9:45 und 10 Std.).
    pub notify_limit: bool,
    /// Mitteilungen zur Pause (5:45 und 6 Std. Arbeit am Stück).
    pub notify_break: bool,
    /// Das Zeitkonto zählt ab diesem Tag (`None`: alle erfassten Tage).
    pub account_from: Option<NaiveDate>,
    /// Übertrag im Zeitkonto: Saldo aus der Zeit davor (Sekunden, auch negativ).
    pub account_carry: i64,
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
            show_time: false,
            hotkeys: false,
            workdays: ALL_WORKDAYS,
            auto_workdays_only: true,
            idle_secs: 0,
            ask_after_lock: false,
            notify_limit: true,
            notify_break: true,
            account_from: None,
            account_carry: 0,
        }
    }
}

impl Settings {
    /// Werte in gültige Grenzen bringen (z. B. nach dem Laden aus der Datenbank).
    pub fn sanitized(self) -> Self {
        let d = Self::default();
        let mut s = Self {
            min_pause: self.min_pause.clamp(MIN_PAUSE_RANGE.0, MIN_PAUSE_RANGE.1),
            min_work: self.min_work.clamp(MIN_WORK_RANGE.0, MIN_WORK_RANGE.1),
            weekly_target: self.weekly_target.clamp(WEEKLY_RANGE.0, WEEKLY_RANGE.1),
            frame_from: self.frame_from.clamp(0, DAY_END),
            frame_to: self.frame_to.clamp(0, DAY_END),
            icon_set: self.icon_set,
            show_time: self.show_time,
            hotkeys: self.hotkeys,
            // Ohne Arbeitstag gäbe es kein Soll: dann wieder Montag bis Freitag.
            workdays: if self.workdays & ALL_WORKDAYS == 0 { ALL_WORKDAYS } else { self.workdays & ALL_WORKDAYS },
            auto_workdays_only: self.auto_workdays_only,
            idle_secs: if self.idle_secs <= 0 { 0 } else { self.idle_secs.clamp(IDLE_RANGE.0, IDLE_RANGE.1) },
            ask_after_lock: self.ask_after_lock,
            notify_limit: self.notify_limit,
            notify_break: self.notify_break,
            account_from: self.account_from,
            account_carry: self.account_carry.clamp(-CARRY_MAX, CARRY_MAX),
        };
        if s.frame_to <= s.frame_from {
            s.frame_from = d.frame_from;
            s.frame_to = d.frame_to;
        }
        s
    }

    /// Samstag oder Sonntag?
    pub fn is_weekend(date: NaiveDate) -> bool {
        matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Ist `date` einer der eingestellten Arbeitstage (für ihn gibt es ein Soll)?
    pub fn is_workday(&self, date: NaiveDate) -> bool {
        let day = date.weekday().num_days_from_monday();
        day < 5 && self.workdays & (1 << day) != 0
    }

    /// Zählt die automatische Erfassung an `date` von selbst? Sonst markiert die Erfassung den
    /// Tag beim ersten Entsperren als nicht gezählt, bis „Heute erfassen“ gewählt ist.
    pub fn auto_counts_on(&self, date: NaiveDate) -> bool {
        !self.auto_workdays_only || self.is_workday(date)
    }

    /// Anzahl der Arbeitstage pro Woche (1 bis 5).
    pub fn workday_count(&self) -> i64 {
        ((self.workdays & ALL_WORKDAYS).count_ones() as i64).max(1)
    }

    /// Sollarbeitszeit eines Tages (an freien Tagen 0).
    pub fn daily_target(&self, date: NaiveDate) -> i64 {
        if self.is_workday(date) {
            self.daily_target_workday()
        } else {
            0
        }
    }

    /// Sollarbeitszeit des Tages `date` mit der Abwesenheit `absence`: Urlaub, Krankheit und
    /// Feiertag senken sie auf null bzw. (halber Tag) auf die Hälfte, Zeitausgleich nicht.
    pub fn day_target(&self, date: NaiveDate, absence: Option<Absence>) -> i64 {
        let full = self.daily_target(date);
        match absence {
            Some(a) if !a.kind.reduces_target() => full,
            Some(a) if a.half => full / 2,
            Some(_) => 0,
            None => full,
        }
    }

    /// Soll, das für `day` in den Saldo eingeht (`today`: heute). Bis gestern zählt jeder
    /// Arbeitstag, auch ohne Arbeitszeit – bis eine Abwesenheit eingetragen ist, ist er ein
    /// Minus. Heute (und später) zählt das Soll erst mit Arbeitszeit oder Abwesenheit, damit
    /// der Tag nicht schon morgens im Minus beginnt; künftige Tage (eine geplante Abwesenheit)
    /// zählen noch gar nicht. Mit Abwesenheit gilt, was sie besagt: halber Urlaubstag =
    /// halbes Soll, Zeitausgleich = volles Soll.
    pub fn counted_target(&self, day: &Day, work_secs: i64, today: NaiveDate) -> i64 {
        if day.date > today || (day.absence.is_none() && work_secs <= 0 && day.date >= today) {
            0
        } else {
            self.day_target(day.date, day.absence)
        }
    }

    /// Sollarbeitszeit eines Arbeitstags (Wochenarbeitszeit / Arbeitstage).
    pub fn daily_target_workday(&self) -> i64 {
        self.weekly_target / self.workday_count()
    }

    /// `Mo–Fr` bzw. `Mo, Di, Do` – die eingestellten Arbeitstage.
    pub fn workdays_text(&self) -> String {
        const NAMES: [&str; 5] = ["Mo", "Di", "Mi", "Do", "Fr"];
        if self.workdays & ALL_WORKDAYS == ALL_WORKDAYS {
            return "Mo–Fr".to_string();
        }
        let days: Vec<&str> = NAMES.iter().enumerate().filter(|(i, _)| self.workdays & (1 << i) != 0).map(|(_, n)| *n).collect();
        days.join(", ")
    }

    /// `5-Tage-Woche`
    pub fn week_text(&self) -> String {
        format!("{}-Tage-Woche", self.workday_count())
    }

    /// Ab so langer Sperre wird beim Entsperren nachgefragt (nie unter der Mindestpause).
    pub fn ask_after(&self) -> i64 {
        ASK_AFTER_LOCK.max(self.min_pause)
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
            "Arbeitszeitrahmen {}  ·  Regelarbeitszeit {} Std./Woche ({})  ·  Pausen ab {} Min.",
            self.frame_text(),
            fmt_hours(self.weekly_target),
            self.workdays_text(),
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
    /// Mehr als 6 Stunden am Stück ohne Pause gearbeitet (`work` Sekunden).
    LongStretch { work: i64 },
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
            Warning::LongStretch { work } => {
                format!("{} ohne Pause – erlaubt sind höchstens 6 Std. am Stück (§ 4 ArbZG)", fmt_hm(*work))
            }
            Warning::MaxHours { work } => {
                format!("{} Arbeitszeit – mehr als 10 Std. am Tag (§ 3 ArbZG)", fmt_hm(*work))
            }
            Warning::Rest { rest } => {
                format!("Ruhezeit nur {} – vorgeschrieben sind 11 Std. (§ 5 ArbZG)", fmt_hm(*rest))
            }
            Warning::OutsideFrame { spans, frame } => {
                format!("{} außerhalb des Arbeitszeitrahmens ({frame}) – nicht gezählt", fmt_spans(spans))
            }
            Warning::Weekend => "Arbeit am Wochenende – zählt ohne Soll als Mehrarbeit".to_string(),
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
    // Fehlt die Pause ganz, sagt der Hinweis oben schon alles; sonst zählt auch, wann sie lag.
    let no_break_at_all = pause == 0 && !out.is_empty();
    let stretch = Day::longest_stretch_secs(&spans);
    if stretch > MAX_STRETCH && !no_break_at_all {
        out.push(Warning::LongStretch { work: stretch });
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
    if work > 0 && Settings::is_weekend(date) {
        out.push(Warning::Weekend);
    }
    // Abschnitte unter einer Minute (z. B. kurz entsperrt in der Nacht) lohnen keinen Hinweis.
    let outside: Vec<Span> = day.outside_frame_spans(s).into_iter().filter(|x| x.len() >= 60).collect();
    if !outside.is_empty() {
        out.push(Warning::OutsideFrame { spans: outside, frame: s.frame_text() });
    }
    out
}

/// Arbeitstage von `from` bis `to` ohne jeden Eintrag (`days`: alle Tage, nach Datum
/// sortiert). Sie zählen trotzdem ins Soll – ab dem ersten erfassten Tag (davor gibt es keine
/// Daten) bis gestern (heute zählt erst mit Arbeitszeit, siehe [`Settings::counted_target`]).
pub fn missing_workdays(days: &[Day], from: NaiveDate, to: NaiveDate, today: NaiveDate, s: &Settings) -> Vec<NaiveDate> {
    let Some(first) = days.iter().find(|d| !d.is_empty()).map(|d| d.date) else { return Vec::new() };
    let Some(yesterday) = today.pred_opt() else { return Vec::new() };
    let (from, to) = (from.max(first), to.min(yesterday));
    if from > to {
        return Vec::new();
    }
    let recorded: std::collections::HashSet<NaiveDate> =
        days.iter().filter(|d| d.date >= from && d.date <= to && !d.is_empty()).map(|d| d.date).collect();
    from.iter_days().take_while(|d| *d <= to).filter(|d| s.is_workday(*d) && !recorded.contains(d)).collect()
}

/// Arbeitszeit (wie angezeigt, in ganzen Minuten) und Soll der Tage von `from` bis `to`
/// (`days`: alle Tage, `today`: heute). Das Soll zählt nach [`Settings::counted_target`] –
/// auch für Arbeitstage ganz ohne Eintrag ([`missing_workdays`]); freie Tage zählen nur mit
/// Zeitausgleich.
pub fn balance(days: &[Day], from: NaiveDate, to: NaiveDate, today: NaiveDate, s: &Settings) -> (i64, i64) {
    let (work, target) = days
        .iter()
        .filter(|d| d.date >= from && d.date <= to)
        .filter_map(|d| d.record(s).map(|r| (d, r)))
        .fold((0, 0), |(w, t), (d, r)| (w + r.shown_work_secs, t + s.counted_target(d, r.work_secs, today)));
    let missing: i64 = missing_workdays(days, from, to, today, s).iter().map(|d| s.daily_target(*d)).sum();
    (work, target + missing)
}

/// Stand des Zeitkontos am Tag `today`: Übertrag plus Saldo aller Tage seit dem eingestellten
/// Beginn (ohne Beginn: seit dem ersten erfassten Tag), siehe [`balance`].
pub fn account(days: &[Day], today: NaiveDate, s: &Settings) -> i64 {
    let (work, target) = balance(days, s.account_from.unwrap_or(NaiveDate::MIN), today, today, s);
    s.account_carry + work - target
}

/// Vorwarnung vor der Höchstarbeitszeit: so lange vorher kommt eine Mitteilung.
pub const MAX_DAILY_WORK_NOTICE: i64 = 15 * 60;

/// Mitteilung zur Höchstarbeitszeit (in der Mitteilungszentrale von macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LimitAlert {
    /// Noch 15 Minuten bis 10 Stunden (ab 9:45 Std.).
    Soon,
    /// 10 Stunden erreicht.
    Reached,
}

impl LimitAlert {
    /// Höchste Stufe, die `work` Sekunden Arbeit erreicht haben.
    pub fn level(work: i64) -> Option<Self> {
        if work >= MAX_DAILY_WORK {
            Some(LimitAlert::Reached)
        } else if work >= MAX_DAILY_WORK - MAX_DAILY_WORK_NOTICE {
            Some(LimitAlert::Soon)
        } else {
            None
        }
    }

    /// Kennung der Mitteilung (eine neue ersetzt eine ältere mit derselben Kennung).
    pub fn id(self) -> &'static str {
        match self {
            LimitAlert::Soon => "max-hours-soon",
            LimitAlert::Reached => "max-hours-reached",
        }
    }

    pub fn title(self) -> String {
        match self {
            LimitAlert::Soon => format!("Noch {} Min. bis 10 Std. Arbeitszeit", MAX_DAILY_WORK_NOTICE / 60),
            LimitAlert::Reached => "10 Std. Arbeitszeit erreicht".to_string(),
        }
    }

    pub fn body(self) -> String {
        match self {
            LimitAlert::Soon => format!(
                "Heute {} gearbeitet – mehr als 10 Std. am Tag sind nicht erlaubt (§ 3 ArbZG).",
                fmt_hm(MAX_DAILY_WORK - MAX_DAILY_WORK_NOTICE)
            ),
            LimitAlert::Reached => {
                "Die Höchstarbeitszeit ist erreicht – mehr als 10 Std. am Tag sind nicht erlaubt (§ 3 ArbZG).".to_string()
            }
        }
    }
}

/// Vorwarnung vor 6 Std. Arbeit am Stück: so lange vorher kommt eine Mitteilung.
pub const MAX_STRETCH_NOTICE: i64 = 15 * 60;

/// Erinnerung an die Pause (in der Mitteilungszentrale von macOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BreakAlert {
    /// Noch 15 Minuten bis 6 Stunden am Stück (ab 5:45 Std.).
    Soon,
    /// 6 Stunden am Stück erreicht.
    Reached,
}

impl BreakAlert {
    /// Höchste Stufe, die `stretch` Sekunden Arbeit am Stück erreicht haben.
    pub fn level(stretch: i64) -> Option<Self> {
        if stretch >= MAX_STRETCH {
            Some(BreakAlert::Reached)
        } else if stretch >= MAX_STRETCH - MAX_STRETCH_NOTICE {
            Some(BreakAlert::Soon)
        } else {
            None
        }
    }

    /// Kennung der Mitteilung (eine neue ersetzt eine ältere mit derselben Kennung).
    pub fn id(self) -> &'static str {
        match self {
            BreakAlert::Soon => "break-soon",
            BreakAlert::Reached => "break-reached",
        }
    }

    pub fn title(self) -> String {
        match self {
            BreakAlert::Soon => format!("Noch {} Min. bis 6 Std. ohne Pause", MAX_STRETCH_NOTICE / 60),
            BreakAlert::Reached => "6 Std. ohne Pause gearbeitet".to_string(),
        }
    }

    pub fn body(self) -> String {
        match self {
            BreakAlert::Soon => format!(
                "Seit {} ohne Pause – nach spätestens 6 Std. am Stück ist eine Pause von mindestens 15 Min. \
                 vorgeschrieben (§ 4 ArbZG).",
                fmt_hm(MAX_STRETCH - MAX_STRETCH_NOTICE)
            ),
            BreakAlert::Reached => {
                "Jetzt ist eine Pause von mindestens 15 Min. vorgeschrieben (§ 4 ArbZG).".to_string()
            }
        }
    }
}

/// Merkt sich je Tag die höchste schon erreichte Stufe, damit jede Schwelle nur einmal
/// gemeldet wird – beim Überschreiten, nicht schon beim Start der App.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Watch<A> {
    date: Option<NaiveDate>,
    level: Option<A>,
}

impl<A: Copy + Ord> Watch<A> {
    /// Neue Stufe von heute. Gibt sie zurück, wenn sie gerade erreicht wurde und gerade
    /// gearbeitet wird (`working`). Sinkt der Wert wieder unter eine Schwelle, wird sie beim
    /// nächsten Mal erneut gemeldet; springt er über zwei Schwellen, kommt nur die höhere.
    fn update(&mut self, date: NaiveDate, level: Option<A>, working: bool) -> Option<A> {
        if self.date != Some(date) {
            *self = Self { date: Some(date), level: None };
        }
        let prev = std::mem::replace(&mut self.level, level);
        (working && level > prev).then_some(level).flatten()
    }
}

/// Mitteilungen zur Höchstarbeitszeit: welche heute schon kam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitWatch(Watch<LimitAlert>);

impl LimitWatch {
    /// Stand ohne Mitteilung übernehmen (beim Start: bereits erreichte Schwellen nicht melden).
    pub fn init(date: NaiveDate, work: i64) -> Self {
        Self(Watch { date: Some(date), level: LimitAlert::level(work) })
    }

    /// Neue Arbeitszeit von heute. Gibt die fällige Mitteilung zurück, wenn eine Schwelle
    /// gerade überschritten wurde und gerade gearbeitet wird (`working`). Wer z. B. im Fenster
    /// „Aufzeichnungen“ nachträglich Zeit einträgt, bekommt keine Mitteilung. Sinkt die
    /// Arbeitszeit wieder unter eine Schwelle (Korrektur), wird sie erneut gemeldet.
    /// Springt die Zeit über beide Schwellen, kommt nur die Mitteilung „erreicht“.
    pub fn update(&mut self, date: NaiveDate, work: i64, working: bool) -> Option<LimitAlert> {
        self.0.update(date, LimitAlert::level(work), working)
    }
}

/// Erinnerungen an die Pause: welche für die laufende Arbeit am Stück schon kam. Nach einer
/// Pause beginnt die Arbeit am Stück neu, und die Erinnerungen kommen wieder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakWatch(Watch<BreakAlert>);

impl BreakWatch {
    /// Stand ohne Mitteilung übernehmen (beim Start: bereits erreichte Schwellen nicht melden).
    pub fn init(date: NaiveDate, stretch: i64) -> Self {
        Self(Watch { date: Some(date), level: BreakAlert::level(stretch) })
    }

    /// Neue Dauer der Arbeit am Stück (`stretch` Sekunden, siehe
    /// [`Day::current_stretch_secs`]). Gibt die fällige Erinnerung zurück.
    pub fn update(&mut self, date: NaiveDate, stretch: i64, working: bool) -> Option<BreakAlert> {
        self.0.update(date, BreakAlert::level(stretch), working)
    }
}

/// `+12:30` bzw. `−3:15` – ein Saldo ohne Einheit, für Eingabefelder.
pub fn fmt_signed_hm(secs: i64) -> String {
    fmt_balance(secs).trim_end_matches(" h").to_string()
}

/// Liest einen Saldo: `+12:30`, `-3:15`, `−3:15`, `12:30` oder in Stunden `12,5` bzw. `-0.25`.
pub fn parse_balance(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('h').trim();
    let (negative, rest) = match s.strip_prefix(['-', '−', '–']) {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let rest = rest.trim();
    let secs = match rest.split_once(':') {
        Some((h, m)) => {
            let (h, m): (i64, i64) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
            if h < 0 || !(0..60).contains(&m) {
                return None;
            }
            h.checked_mul(3600)? + m * 60
        }
        None => parse_hours(rest).filter(|v| *v >= 0)?,
    };
    (secs <= CARRY_MAX).then_some(if negative { -secs } else { secs })
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
        assert_eq!(fixed.min_work, s.min_work);
        // Die Mindestdauer von Arbeit ohne Mac hängt nicht an der Mindestpause.
        let s20 = Settings { min_pause: 20 * 60, min_work: 5 * 60, ..s }.sanitized();
        assert_eq!((s20.min_pause, s20.min_work), (20 * 60, 5 * 60));
        assert_eq!(Settings { min_work: 0, ..s }.sanitized().min_work, MIN_WORK_RANGE.0);
        assert_eq!(Settings { min_work: 999 * 60, ..s }.sanitized().min_work, MIN_WORK_RANGE.1);
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
    fn long_stretch_without_break() {
        let s = Settings::default();
        let day = |periods: &[(i64, i64)]| {
            let mut d = Day::new(date(18));
            for &(from, to) in periods {
                d.add_period(from, to);
            }
            d
        };
        // 7 Std. am Stück, die Pause kommt erst danach: genug Pause, aber zu spät.
        let late = day(&[(h(7, 0), h(14, 0)), (h(14, 30), h(15, 30))]);
        let w = day_warnings(&[late], date(18), &s);
        assert_eq!(w, vec![Warning::LongStretch { work: h(7, 0) }]);
        assert_eq!(w[0].text(), "7:00 Std. ohne Pause – erlaubt sind höchstens 6 Std. am Stück (§ 4 ArbZG)");
        // Genau 6 Std. am Stück sind erlaubt.
        let ok = day(&[(h(7, 0), h(13, 0)), (h(13, 30), h(15, 0))]);
        assert!(day_warnings(&[ok], date(18), &s).is_empty());
        // Ganz ohne Pause genügt der Hinweis auf die fehlende Pause.
        let none = day(&[(h(7, 0), h(14, 0))]);
        assert_eq!(day_warnings(&[none], date(18), &s), vec![Warning::Breaks { required: 30 * 60 }]);
        // Zu wenig Pause und zu spät: beide Hinweise.
        let both = day(&[(h(7, 0), h(13, 10)), (h(13, 30), h(14, 30))]);
        assert_eq!(
            day_warnings(&[both], date(18), &s),
            vec![Warning::Breaks { required: 30 * 60 }, Warning::LongStretch { work: h(6, 10) }]
        );
        // Mindestpause 5 Min.: Eine Unterbrechung von 10 Min. ist keine Pause nach dem Gesetz –
        // die Arbeit davor und danach zählt zusammen (ohne die Unterbrechung).
        let short = Settings { min_pause: 5 * 60, min_work: 5 * 60, ..s };
        let split = day(&[(h(7, 0), h(10, 0)), (h(10, 10), h(13, 30)), (h(14, 0), h(15, 0))]);
        assert_eq!(day_warnings(&[split], date(18), &short), vec![Warning::LongStretch { work: h(6, 20) }]);
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
    fn limit_alerts_at_9_45_and_10_hours() {
        assert_eq!(LimitAlert::level(h(9, 44)), None);
        assert_eq!(LimitAlert::level(h(9, 45)), Some(LimitAlert::Soon));
        assert_eq!(LimitAlert::level(h(10, 0)), Some(LimitAlert::Reached));
        assert_eq!(LimitAlert::Soon.title(), "Noch 15 Min. bis 10 Std. Arbeitszeit");
        assert!(LimitAlert::Soon.body().starts_with("Heute 9:45 Std. gearbeitet"), "{}", LimitAlert::Soon.body());

        let mut w = LimitWatch::init(date(16), h(9, 0));
        assert_eq!(w.update(date(16), h(9, 44), true), None);
        assert_eq!(w.update(date(16), h(9, 45), true), Some(LimitAlert::Soon));
        assert_eq!(w.update(date(16), h(9, 50), true), None, "nur einmal");
        assert_eq!(w.update(date(16), h(10, 0), true), Some(LimitAlert::Reached));
        assert_eq!(w.update(date(16), h(10, 30), true), None);
        // Korrektur nach unten, danach wieder erreicht
        assert_eq!(w.update(date(16), h(9, 30), false), None);
        assert_eq!(w.update(date(16), h(10, 0), true), Some(LimitAlert::Reached), "Sprung: nur „erreicht“");
        // Neuer Tag
        assert_eq!(w.update(date(17), h(9, 45), true), Some(LimitAlert::Soon));
        // Nachgetragen, während nicht gearbeitet wird: keine Mitteilung, auch nicht später
        assert_eq!(w.update(date(17), h(10, 5), false), None);
        assert_eq!(w.update(date(17), h(10, 6), true), None);
        // Beim Start schon über 9:45 Std.: nur noch „erreicht“
        let mut w = LimitWatch::init(date(18), h(9, 50));
        assert_eq!(w.update(date(18), h(9, 55), true), None);
        assert_eq!(w.update(date(18), h(10, 0), true), Some(LimitAlert::Reached));
    }

    #[test]
    fn carry_input() {
        assert_eq!(parse_balance("+12:30"), Some(h(12, 30)));
        assert_eq!(parse_balance("-3:15"), Some(-h(3, 15)));
        assert_eq!(parse_balance(" −3:15 h "), Some(-h(3, 15)));
        assert_eq!(parse_balance("0:05"), Some(h(0, 5)));
        assert_eq!(parse_balance("12,5"), Some(h(12, 30)));
        assert_eq!(parse_balance("-0.25"), Some(-h(0, 15)));
        assert_eq!(parse_balance("0"), Some(0));
        for bad in ["", "abc", "1:60", "1:-5", "--2", "99999999:00"] {
            assert_eq!(parse_balance(bad), None, "{bad}");
        }
        assert_eq!(fmt_signed_hm(h(12, 30)), "+12:30");
        assert_eq!(fmt_signed_hm(-h(3, 15)), "−3:15");
        assert_eq!(parse_balance(&fmt_signed_hm(-h(3, 15))), Some(-h(3, 15)));
    }

    #[test]
    fn workdays_spread_the_weekly_target() {
        let s = Settings::default();
        assert_eq!((s.workday_count(), s.workdays_text(), s.week_text()), (5, "Mo–Fr".to_string(), "5-Tage-Woche".to_string()));
        // 4-Tage-Woche ohne Freitag: 32 Std. sind 8 Std. an Mo–Do, freitags kein Soll.
        let four = Settings { workdays: 0b0_1111, weekly_target: h(32, 0), ..s };
        assert_eq!((four.workday_count(), four.workdays_text(), four.week_text()), (4, "Mo, Di, Mi, Do".to_string(), "4-Tage-Woche".to_string()));
        assert_eq!(four.daily_target(date(16)), h(8, 0)); // Montag
        assert_eq!(four.daily_target(date(20)), 0); // Freitag
        assert!(four.is_workday(date(19)) && !four.is_workday(date(20)) && !four.is_workday(date(21)));
        assert_eq!(four.summary(), "Arbeitszeitrahmen 07:00–22:00 Uhr  ·  Regelarbeitszeit 32 Std./Woche (Mo, Di, Mi, Do)  ·  Pausen ab 15 Min.");
        // Arbeit am freien Freitag ist Mehrarbeit, aber kein Hinweis wie am Wochenende.
        let mut fri = Day::new(date(20));
        fri.add_period(h(9, 0), h(11, 0));
        assert!(day_warnings(&[fri.clone()], date(20), &four).is_empty());
        assert_eq!(balance(&[fri], date(16), date(22), date(23), &four), (h(2, 0), 0));
        // Ungültige Angaben: ohne Arbeitstag wieder Mo–Fr, Samstag und Sonntag gibt es nicht.
        assert_eq!(Settings { workdays: 0, ..s }.sanitized().workdays, ALL_WORKDAYS);
        assert_eq!(Settings { workdays: 0b110_0001, ..s }.sanitized().workdays, 0b0_0001);
        assert_eq!(Settings { workdays: 0b0_0100, weekly_target: h(8, 0), ..s }.daily_target(date(18)), h(8, 0));
        // Inaktivität: aus oder in Grenzen.
        assert_eq!(Settings { idle_secs: -5, ..s }.sanitized().idle_secs, 0);
        assert_eq!(Settings { idle_secs: 10, ..s }.sanitized().idle_secs, IDLE_RANGE.0);
        assert_eq!(Settings { min_pause: h(0, 45), ..s }.ask_after(), h(0, 45));
        assert_eq!(s.ask_after(), ASK_AFTER_LOCK);
    }

    #[test]
    fn absences_change_the_target() {
        use crate::day::AbsenceKind::*;
        let s = Settings::default();
        let day = |d: u32, hours: i64, absence: Option<Absence>| {
            let mut x = Day::new(date(d));
            if hours > 0 {
                x.add_period(h(8, 0), h(8 + hours, 0));
            }
            x.absence = absence;
            x
        };
        let full = h(7, 48);
        assert_eq!(s.day_target(date(16), None), full);
        assert_eq!(s.day_target(date(16), Some(Absence::new(Vacation, true))), full / 2);
        assert_eq!(s.day_target(date(16), Some(Absence::new(Sick, false))), 0);
        assert_eq!(s.day_target(date(16), Some(Absence::new(TimeOff, false))), full);
        assert_eq!(s.day_target(date(21), Some(Absence::new(TimeOff, false))), 0, "Samstag: kein Soll");
        assert!(!Absence::new(TimeOff, true).half, "halber Zeitausgleich gibt es nicht");

        // Halber Urlaubstag mit 4 Std. Arbeit: +0:06 statt −3:48.
        let half = day(16, 4, Some(Absence::new(Vacation, true)));
        assert_eq!(s.counted_target(&half, h(4, 0), date(23)), full / 2);
        // Ganzer Urlaubstag, trotzdem 1 Std. gearbeitet: Mehrarbeit.
        let vacation = day(17, 1, Some(Absence::new(Vacation, false)));
        // Zeitausgleich ohne Arbeitszeit: das volle Soll zählt.
        let off = day(18, 0, Some(Absence::new(TimeOff, false)));
        // Feiertag ohne Arbeitszeit: kein Soll.
        let holiday = day(19, 0, Some(Absence::new(Holiday, false)));
        let days = vec![half, vacation, off, holiday];
        // Stand Freitag: Der Freitag ohne Eintrag zählt noch nicht …
        let (work, target) = balance(&days, date(16), date(22), date(20), &s);
        assert_eq!((work, target), (h(5, 0), full / 2 + full));
        assert_eq!(fmt_balance(account(&days, date(20), &s)), "−6:42 h");
        // … ab Samstag schon: ein Arbeitstag ohne Abwesenheit, das volle Soll fehlt.
        assert_eq!(balance(&days, date(16), date(22), date(21), &s), (h(5, 0), full / 2 + 2 * full));
        assert_eq!(fmt_balance(account(&days, date(22), &s)), "−14:30 h");
        // Geplanter Zeitausgleich zählt erst an seinem Tag.
        assert_eq!(s.counted_target(&days[2], 0, date(17)), 0);
        assert_eq!(s.counted_target(&days[2], 0, date(18)), full);
        // Die Abwesenheit allein macht aus dem Tag einen Eintrag.
        assert!(!days[2].is_empty() && days[2].has_corrections());
        // Texte für Export und Tabelle – und zurück.
        let a = Absence::new(Vacation, true);
        assert_eq!((a.label(), a.short(), a.code()), ("Urlaub (halber Tag)".to_string(), "½ Urlaub".to_string(), "½ U".to_string()));
        assert_eq!(Absence::new(TimeOff, false).code(), "ZA");
        assert_eq!(Absence::parse("Urlaub (halber Tag)"), Some(a));
        assert_eq!(Absence::parse(" zeitausgleich "), Some(Absence::new(TimeOff, false)));
        assert_eq!(Absence::parse("Dienstreise"), None);
    }

    #[test]
    fn account_adds_carry_and_counts_from_start() {
        let s = Settings::default();
        let day = |d: u32, from: i64, to: i64| {
            let mut x = Day::new(date(d));
            x.add_period(from, to);
            x
        };
        // Mo 9 Std. (+1:12), Di 7 Std. (−0:48), Mi bis Fr genau das Soll, Sa 1 Std. (+1:00)
        let mut days = vec![day(16, h(8, 0), h(17, 0)), day(17, h(8, 0), h(15, 0))];
        days.extend((18..=20).map(|d| day(d, h(8, 0), h(15, 48))));
        days.push(day(21, h(10, 0), h(11, 0)));
        assert_eq!(account(&days, date(22), &s), h(1, 24));
        // Stand an einem früheren Tag: spätere Tage zählen noch nicht.
        assert_eq!(account(&days, date(16), &s), h(1, 12));
        // Ab Dienstag, mit 5 Std. Übertrag aus der Zeit davor.
        let from_tue = Settings { account_from: Some(date(17)), account_carry: h(5, 0), ..s };
        assert_eq!(account(&days, date(22), &from_tue), h(5, 0) - h(0, 48) + h(1, 0));
        let minus = Settings { account_carry: -h(2, 0), ..s };
        assert_eq!(fmt_balance(account(&days, date(22), &minus)), "−0:36 h");
    }

    #[test]
    fn break_reminders_at_5_45_and_6_hours() {
        assert_eq!(BreakAlert::level(h(5, 44)), None);
        assert_eq!(BreakAlert::level(h(5, 45)), Some(BreakAlert::Soon));
        assert_eq!(BreakAlert::level(h(6, 0)), Some(BreakAlert::Reached));
        assert_eq!(BreakAlert::Soon.title(), "Noch 15 Min. bis 6 Std. ohne Pause");
        assert!(BreakAlert::Soon.body().starts_with("Seit 5:45 Std. ohne Pause"), "{}", BreakAlert::Soon.body());

        let mut w = BreakWatch::init(date(16), h(2, 0));
        assert_eq!(w.update(date(16), h(5, 45), true), Some(BreakAlert::Soon));
        assert_eq!(w.update(date(16), h(5, 50), true), None, "nur einmal");
        assert_eq!(w.update(date(16), h(6, 0), true), Some(BreakAlert::Reached));
        // Nach der Pause beginnt die Arbeit am Stück neu – und die Erinnerung kommt wieder.
        assert_eq!(w.update(date(16), h(0, 10), true), None);
        assert_eq!(w.update(date(16), h(5, 45), true), Some(BreakAlert::Soon));
        // Beim Start schon über 6 Std. am Stück: keine Erinnerung mehr.
        let mut w = BreakWatch::init(date(17), h(6, 30));
        assert_eq!(w.update(date(17), h(6, 31), true), None);
    }

    #[test]
    fn balance_counts_workdays_without_entries() {
        let s = Settings::default();
        let mut mon = Day::new(date(16));
        mon.add_period(h(8, 0), h(17, 0)); // 9 Std.
        let mut sat = Day::new(date(21));
        sat.add_period(h(10, 0), h(11, 0)); // Wochenende: kein Soll
        let days = [mon, sat];
        // Stand Dienstag: Heute zählt erst mit Arbeitszeit.
        assert_eq!(balance(&days, date(16), date(22), date(17), &s), (h(10, 0), h(7, 48)));
        assert!(missing_workdays(&days, date(16), date(22), date(17), &s).is_empty());
        // Stand Montag darauf: Di bis Fr ohne Eintrag zählen mit ihrem Soll.
        assert_eq!(missing_workdays(&days, date(16), date(22), date(23), &s), (17..=20).map(date).collect::<Vec<_>>());
        assert_eq!(balance(&days, date(16), date(22), date(23), &s), (h(10, 0), 5 * h(7, 48)));
        // Vor dem ersten erfassten Tag zählt nichts; ohne Einträge gar nichts.
        assert!(missing_workdays(&days, date(2), date(13), date(23), &s).is_empty());
        assert!(missing_workdays(&[], date(16), date(22), date(23), &s).is_empty());
        // Ein Tag nur mit Pause o. Ä., aber ohne Arbeitszeit, zählt sein Soll ebenso.
        let mut tue = Day::new(date(17));
        tue.add_break(h(12, 0), h(12, 30));
        assert_eq!(s.counted_target(&tue, 0, date(23)), h(7, 48));
        assert_eq!(s.counted_target(&tue, 0, date(17)), 0, "heute noch nicht");
    }

    #[test]
    fn balance_adds_up_shown_minutes() {
        // Zwei Tage mit je 7:34:40 Std.: angezeigt je 7:34 h und −0:14 h, die Woche also
        // 15:08 h und −0:28 h – nicht 15:09 h und −0:26 h aus den Sekunden.
        let s = Settings::default();
        let days: Vec<Day> = [16, 17]
            .iter()
            .map(|&n| {
                let mut d = Day::new(date(n));
                d.add_period(h(8, 0) + 10, h(15, 34) + 50);
                d
            })
            .collect();
        let rec = days[0].record(&s).unwrap();
        assert_eq!(fmt_balance(rec.shown_work_secs - s.daily_target(rec.date)), "−0:14 h");
        let (work, target) = balance(&days, date(16), date(22), date(18), &s);
        assert_eq!(work, h(15, 8));
        assert_eq!(fmt_balance(work - target), "−0:28 h");
    }
}
