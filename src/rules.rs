//! Einstellungen und Regeln der Zeiterfassung.
//!
//! Einstellbar sind der Arbeitszeitrahmen, die Regelwochenarbeitszeit, die Arbeitstage und die
//! Mindestdauer einer Pause ([`WorkRules`]) – je Stand ab einem Datum, damit eine Änderung
//! frühere Tage nicht neu rechnet ([`Settings::rules`] liefert die Regeln eines Tages). Fest
//! sind die Regeln aus Arbeitszeitgesetz und Betriebsvereinbarung:
//!
//! - **Arbeitszeitrahmen** – Arbeit nur im eingestellten Zeitraum (Standard 07:00–22:00), an
//!   jedem Tag; außerhalb wird weder Arbeitszeit noch Pause gezählt, es gibt aber einen Hinweis.
//!   Arbeit an Tagen ohne Soll (Wochenende, abgewählte Arbeitstage) wird gemeldet.
//! - **Freie Tage** – Wochenende und abgewählte Arbeitstage haben kein Soll. Arbeit dort
//!   wirkt sich standardmäßig nicht auf Saldo und Zeitkonto aus; je Stand lässt sie sich als
//!   Mehrarbeit zählen ([`WorkRules::free_days_count`]).
//! - **Sollarbeitszeit** – Regelwochenarbeitszeit verteilt auf die Arbeitstage
//!   (39 Std. = 7:48 Std. pro Tag), je Tag in ganzen Minuten ([`WorkRules::daily_target_on`]);
//!   daraus ergibt sich der Saldo.
//! - **Pausen** (§ 4 ArbZG) – mehr als 6 Std. Arbeit: 30 Min., mehr als 9 Std.: 45 Min.;
//!   gezählt werden Abschnitte von mindestens 15 Min. Länger als 6 Std. am Stück ohne
//!   solche Pause sind ebenfalls ein Verstoß.
//! - **Ruhezeit** (§ 5 ArbZG) – mindestens 11 Std. zwischen zwei Arbeitstagen.
//!
//! Pausen, Höchstarbeitszeit und Ruhezeit werden zuerst mit der gezählten Arbeitszeit geprüft
//! und dann mit der tatsächlichen – samt der Zeit außerhalb des Arbeitszeitrahmens
//! ([`Day::actual_spans`]); der Hinweis sagt dann, dass sie mitgerechnet ist.
//! - **Höchstarbeitszeit** – mehr als 10 Std. am Tag gelten als Verstoß; bei 9:45 und 10 Std.
//!   kommt eine Mitteilung (siehe [`LimitWatch`]).

use crate::day::{fmt_clock, Absence, Day, DayRecord, Span, DAY_END};
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

/// Arbeitszeitregeln: was an einem Tag als Arbeitszeit, Pause und Soll zählt (alle Zeiten in
/// Sekunden). Sie gelten ab einem Tag bis zum nächsten Stand (siehe [`Settings::work`]) –
/// eine Änderung rechnet frühere Tage nicht neu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkRules {
    /// Mindestdauer einer Pause: kürzere Sperrzeiten zählen zur Arbeitszeit.
    pub min_pause: i64,
    /// Regelwochenarbeitszeit.
    pub weekly_target: i64,
    /// Beginn des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_from: i64,
    /// Ende des Arbeitszeitrahmens (Sekunden seit Mitternacht).
    pub frame_to: i64,
    /// Arbeitstage der Woche als Bits (Bit 0 = Montag … Bit 4 = Freitag); das Wochensoll
    /// verteilt sich gleichmäßig auf sie.
    pub workdays: u8,
    /// Arbeit an freien Tagen (Wochenende, abgewählte Arbeitstage) zählt als Mehrarbeit in
    /// Saldo und Zeitkonto. Standard: nein – sie wird angezeigt, ändert aber keinen Saldo.
    pub free_days_count: bool,
}

impl WorkRules {
    pub const DEFAULT: WorkRules = WorkRules {
        min_pause: 15 * 60,
        weekly_target: 39 * 3600,
        frame_from: 7 * 3600,
        frame_to: 22 * 3600,
        workdays: ALL_WORKDAYS,
        free_days_count: false,
    };

    /// Werte in gültige Grenzen bringen.
    pub fn sanitized(self) -> Self {
        let d = Self::DEFAULT;
        let mut r = Self {
            min_pause: self.min_pause.clamp(MIN_PAUSE_RANGE.0, MIN_PAUSE_RANGE.1),
            // In ganzen Minuten – so wie sie sich eingeben lässt und wie alle Salden rechnen.
            weekly_target: self.weekly_target.clamp(WEEKLY_RANGE.0, WEEKLY_RANGE.1) / 60 * 60,
            frame_from: self.frame_from.clamp(0, DAY_END),
            frame_to: self.frame_to.clamp(0, DAY_END),
            // Ohne Arbeitstag gäbe es kein Soll: dann wieder Montag bis Freitag.
            workdays: if self.workdays & ALL_WORKDAYS == 0 { ALL_WORKDAYS } else { self.workdays & ALL_WORKDAYS },
            free_days_count: self.free_days_count,
        };
        if r.frame_to <= r.frame_from {
            r.frame_from = d.frame_from;
            r.frame_to = d.frame_to;
        }
        r
    }

    /// Ist `date` einer der eingestellten Arbeitstage (für ihn gibt es ein Soll)?
    pub fn is_workday(&self, date: NaiveDate) -> bool {
        let day = date.weekday().num_days_from_monday();
        day < 5 && self.workdays & (1 << day) != 0
    }

    /// Anzahl der Arbeitstage pro Woche (1 bis 5).
    pub fn workday_count(&self) -> i64 {
        ((self.workdays & ALL_WORKDAYS).count_ones() as i64).max(1)
    }

    /// Sollarbeitszeit des Tages `date` in ganzen Minuten (an freien Tagen 0). Geht die
    /// Wochenarbeitszeit nicht in vollen Minuten auf (38,5 Std. an vier Tagen = 9:37:30),
    /// bekommen die Arbeitstage der Woche der Reihe nach 9:37, 9:38, 9:37 und 9:38 Std. –
    /// zusammen genau die Wochenarbeitszeit. So ist jeder Tagessaldo eine ganze Minutenzahl,
    /// und Tage summieren sich zur Woche ohne Rundungsreste.
    pub fn daily_target_on(&self, date: NaiveDate) -> i64 {
        if !self.is_workday(date) {
            return 0;
        }
        let weekday = date.weekday().num_days_from_monday();
        // Arbeitstage der Woche vor diesem Tag.
        let before = (self.workdays & ((1u8 << weekday) - 1)).count_ones() as i64;
        let (week, n) = (self.weekly_target / 60, self.workday_count());
        ((before + 1) * week / n - before * week / n) * 60
    }

    /// Kürzeste und längste Sollarbeitszeit eines Arbeitstags (siehe
    /// [`WorkRules::daily_target_on`]) – meist gleich, sonst eine Minute auseinander.
    pub fn daily_target_range(&self) -> (i64, i64) {
        let (week, n) = (self.weekly_target / 60, self.workday_count());
        let low = week / n * 60;
        (low, if week % n == 0 { low } else { low + 60 })
    }

    /// `7:48 Std.` bzw. `9:37 bis 9:38 Std.` – die Sollarbeitszeit eines Arbeitstags zum
    /// Anzeigen.
    pub fn daily_target_text(&self) -> String {
        let hm = |secs: i64| format!("{}:{:02}", secs / 3600, secs % 3600 / 60);
        match self.daily_target_range() {
            (low, high) if low == high => format!("{} Std.", hm(low)),
            (low, high) => format!("{} bis {} Std.", hm(low), hm(high)),
        }
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
        let free = if self.free_days_count { "  ·  Arbeit an freien Tagen im Saldo" } else { "" };
        format!(
            "Arbeitszeitrahmen {}  ·  Regelarbeitszeit {} Std./Woche ({})  ·  Pausen ab {} Min.{free}",
            self.frame_text(),
            fmt_hours(self.weekly_target),
            self.workdays_text(),
            self.min_pause / 60
        )
    }
}

impl Default for WorkRules {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Ein Stand der Arbeitszeitregeln: gilt ab `from` (`None`: seit jeher) bis zum nächsten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RulesPeriod {
    pub from: Option<NaiveDate>,
    pub rules: WorkRules,
}

/// Vom Benutzer einstellbare Werte (alle Zeiten in Sekunden).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Arbeitszeitregeln mit ihren Gültigkeiten: nach Datum sortiert, nie leer, der erste
    /// Stand gilt seit jeher (`from: None`), zwei aufeinanderfolgende sind verschieden
    /// (siehe [`Settings::sanitized`]). Abfragen über [`Settings::rules`].
    pub work: Vec<RulesPeriod>,
    /// Mindestdauer von Arbeit ohne Mac beim Eintragen von Hand (unabhängig von der
    /// Mindestpause; prüft nur Eingaben und gilt deshalb ohne Datum).
    pub min_work: i64,
    /// Symbolsatz der Menüleiste.
    pub icon_set: IconSet,
    /// Arbeitszeit von heute neben dem Symbol in der Menüleiste zeigen.
    pub show_time: bool,
    /// Globale Kurzbefehle (wirken in jeder App).
    pub hotkeys: bool,
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
            work: vec![RulesPeriod { from: None, rules: WorkRules::DEFAULT }],
            min_work: 15 * 60,
            icon_set: IconSet::default(),
            show_time: false,
            hotkeys: false,
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
        Self {
            work: normalized(self.work),
            min_work: self.min_work.clamp(MIN_WORK_RANGE.0, MIN_WORK_RANGE.1),
            idle_secs: if self.idle_secs <= 0 { 0 } else { self.idle_secs.clamp(IDLE_RANGE.0, IDLE_RANGE.1) },
            account_carry: self.account_carry.clamp(-CARRY_MAX, CARRY_MAX),
            ..self
        }
    }

    /// Dieselben Einstellungen mit nur einem Stand der Arbeitszeitregeln, der seit jeher gilt.
    #[cfg(test)]
    pub fn with_rules(self, rules: WorkRules) -> Self {
        Self { work: vec![RulesPeriod { from: None, rules }], ..self }.sanitized()
    }

    /// Die Arbeitszeitregeln, die am Tag `date` gelten.
    pub fn rules(&self, date: NaiveDate) -> &WorkRules {
        self.work
            .iter()
            .rev()
            .find(|p| p.from.is_none_or(|f| f <= date))
            .map_or(&WorkRules::DEFAULT, |p| &p.rules)
    }

    /// Stand ab `from` (`None`: seit jeher) setzen; ein vorhandener Stand mit demselben Datum
    /// wird ersetzt, ebenso der Stand ab `replaces` (beim Verschieben eines Stands auf ein
    /// anderes Datum – in einem Schritt, damit dazwischen nichts zusammengefasst wird). Gleicht
    /// ein Stand dem davor, entfällt er.
    pub fn set_rules(&mut self, from: Option<NaiveDate>, rules: WorkRules, replaces: Option<NaiveDate>) {
        self.work.retain(|p| p.from != from && (replaces.is_none() || p.from != replaces));
        self.work.push(RulesPeriod { from, rules: rules.sanitized() });
        self.work = normalized(std::mem::take(&mut self.work));
    }

    /// Den Stand ab `from` löschen – ab dann gilt wieder der Stand davor. Der erste Stand
    /// (seit jeher) bleibt immer.
    pub fn remove_rules(&mut self, from: NaiveDate) {
        self.work.retain(|p| p.from != Some(from));
        self.work = normalized(std::mem::take(&mut self.work));
    }

    /// Die Stände, die zwischen `from` und `to` gelten: jeweils der erste Tag im Zeitraum, an
    /// dem der Stand gilt, und die Regeln.
    pub fn rules_between(&self, from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, WorkRules)> {
        let mut out = vec![(from, *self.rules(from))];
        out.extend(self.work.iter().filter_map(|p| p.from.filter(|f| *f > from && *f <= to).map(|f| (f, p.rules))));
        out
    }

    /// Kurzfassung der Regeln von `from` bis `to` für Fußzeilen und Export – bei einem Wechsel
    /// im Zeitraum ein Absatz je Stand (`bis 30.11.2026: …`, `ab 01.12.2026: …`).
    pub fn summary_between(&self, from: NaiveDate, to: NaiveDate) -> Vec<String> {
        let parts = self.rules_between(from, to);
        if parts.len() == 1 {
            return vec![parts[0].1.summary()];
        }
        parts
            .iter()
            .enumerate()
            .map(|(i, (start, r))| match parts.get(i + 1) {
                None => format!("Ab {}: {}", fmt_day(*start), r.summary()),
                Some(_) if i == 0 => format!("Bis {}: {}", fmt_day(parts[1].0 - chrono::Duration::days(1)), r.summary()),
                Some((next, _)) => {
                    format!("{} bis {}: {}", fmt_day(*start), fmt_day(*next - chrono::Duration::days(1)), r.summary())
                }
            })
            .collect()
    }

    /// Grundlage des Solls von `from` bis `to` für Fußzeilen: `39 Std./Woche, Mo–Fr` – bei
    /// einem Wechsel im Zeitraum der Verweis auf die Regeln je Stand (siehe
    /// [`Settings::summary_between`]).
    pub fn target_basis(&self, from: NaiveDate, to: NaiveDate) -> String {
        match self.rules_between(from, to).as_slice() {
            [(_, r)] => format!("{} Std./Woche, {}", fmt_hours(r.weekly_target), r.workdays_text()),
            _ => "nach den Regeln, die am jeweiligen Tag galten".to_string(),
        }
    }

    /// Samstag oder Sonntag?
    pub fn is_weekend(date: NaiveDate) -> bool {
        matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
    }

    /// Ist `date` ein Arbeitstag nach den Regeln, die an ihm gelten (für ihn gibt es ein Soll)?
    pub fn is_workday(&self, date: NaiveDate) -> bool {
        self.rules(date).is_workday(date)
    }

    /// Zählt die automatische Erfassung an `date` von selbst? Sonst markiert die Erfassung den
    /// Tag beim ersten Entsperren als nicht gezählt, bis „Heute erfassen“ gewählt ist.
    pub fn auto_counts_on(&self, date: NaiveDate) -> bool {
        !self.auto_workdays_only || self.is_workday(date)
    }

    /// Geht die Arbeitszeit am Tag `date` in Saldo und Zeitkonto ein? An Arbeitstagen immer,
    /// an freien Tagen nur, wenn der Stand, der an ihm gilt, es vorsieht
    /// ([`WorkRules::free_days_count`]).
    pub fn work_counts_on(&self, date: NaiveDate) -> bool {
        let r = self.rules(date);
        r.free_days_count || r.is_workday(date)
    }

    /// Arbeitszeit `work` am Tag `date`, soweit sie in den Saldo eingeht (siehe
    /// [`Settings::work_counts_on`]; sonst 0).
    pub fn counted_work(&self, date: NaiveDate, work: i64) -> i64 {
        if self.work_counts_on(date) {
            work
        } else {
            0
        }
    }

    /// Saldo des Tages `day` mit seinen Kennzahlen `rec` (`today`: heute): angezeigte
    /// Arbeitszeit, soweit sie zählt, minus Soll (siehe [`Settings::counted_target`]).
    /// `None`, wenn es keinen gibt – weder Arbeitszeit, die zählt, noch Soll (z. B. Arbeit an
    /// einem freien Tag, die nicht zählt).
    pub fn day_balance(&self, day: &Day, rec: &DayRecord, today: NaiveDate) -> Option<i64> {
        let target = self.counted_target(day, rec, today);
        (self.counted_work(day.date, rec.work_secs) > 0 || target > 0)
            .then(|| self.counted_work(day.date, rec.shown_work_secs) - target)
    }

    /// Sollarbeitszeit eines Tages (an freien Tagen 0) nach den Regeln, die an ihm gelten.
    pub fn daily_target(&self, date: NaiveDate) -> i64 {
        self.rules(date).daily_target_on(date)
    }

    /// Sollarbeitszeit des Tages `date` mit der Abwesenheit `absence`: Urlaub, Sonderurlaub,
    /// Krankheit und Feiertag senken sie auf null bzw. (halber Tag) auf die Hälfte – auf ganze
    /// Minuten abgerundet, damit auch dieser Tagessaldo ohne Sekunden bleibt –, Zeitausgleich
    /// nicht.
    pub fn day_target(&self, date: NaiveDate, absence: Option<Absence>) -> i64 {
        let full = self.daily_target(date);
        match absence {
            Some(a) if !a.kind.reduces_target() => full,
            Some(a) if a.half => full / 120 * 60,
            Some(_) => 0,
            None => full,
        }
    }

    /// Soll, das für `day` mit seinen Kennzahlen `rec` in den Saldo eingeht (`today`: heute).
    /// Bis gestern zählt jeder Arbeitstag, auch ohne Arbeitszeit – bis eine Abwesenheit
    /// eingetragen ist, ist er ein Minus. Mit Abwesenheit gilt, was sie besagt: halber
    /// Urlaubstag = halbes Soll, Zeitausgleich = volles Soll. Künftige Tage (eine geplante
    /// Abwesenheit) zählen noch gar nicht.
    ///
    /// Heute zählt das Soll nur, soweit es schon erarbeitet ist: Der laufende Tag steht bei
    /// null, bis das Soll erreicht ist, und danach im Plus – Wochensaldo und Zeitkonto fallen
    /// also nicht morgens um das Tagessoll, um sich bis zum Abend wieder zu erholen. Was fehlt,
    /// zählt erst mit dem Feierabend (dann ist der Tag zu Ende) oder ab morgen. Zeitausgleich
    /// zählt sofort – dort soll das Zeitkonto sinken.
    pub fn counted_target(&self, day: &Day, rec: &DayRecord, today: NaiveDate) -> i64 {
        let target = self.day_target(day.date, day.absence);
        if day.date > today {
            0
        } else if day.date < today || day.end.is_some() || day.absence.is_some_and(|a| !a.kind.reduces_target()) {
            target
        } else {
            target.min(self.counted_work(day.date, rec.shown_work_secs))
        }
    }

    /// Ab so langer Sperre am Tag `date` wird beim Entsperren nachgefragt (nie unter der
    /// Mindestpause).
    pub fn ask_after(&self, date: NaiveDate) -> i64 {
        ASK_AFTER_LOCK.max(self.rules(date).min_pause)
    }
}

/// Stände sortieren, bereinigen und zusammenfassen: Der erste gilt seit jeher, je Datum gibt es
/// höchstens einen, und ein Stand, der dem davor gleicht, entfällt.
fn normalized(mut work: Vec<RulesPeriod>) -> Vec<RulesPeriod> {
    // Stabil sortiert: Bei gleichem Datum gewinnt der zuletzt eingetragene.
    work.sort_by_key(|p| p.from);
    let mut out: Vec<RulesPeriod> = Vec::with_capacity(work.len());
    for p in work {
        let p = RulesPeriod { rules: p.rules.sanitized(), ..p };
        match out.last_mut() {
            Some(last) if last.from == p.from => *last = p,
            Some(last) if last.rules == p.rules => {}
            _ => out.push(p),
        }
    }
    match out.first_mut() {
        Some(first) => first.from = None,
        None => out.push(RulesPeriod { from: None, rules: WorkRules::DEFAULT }),
    }
    out
}

/// `01.12.2026`
pub fn fmt_day(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
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
    /// Zu wenig Pause für die Arbeitszeit (`required` Sekunden vorgeschrieben). `outside` hier
    /// und bei den nächsten drei: Der Verstoß ergibt sich erst mit der Arbeit außerhalb des
    /// Arbeitszeitrahmens, die nicht gezählt wird.
    Breaks { required: i64, outside: bool },
    /// Mehr als 6 Stunden am Stück ohne Pause gearbeitet (`work` Sekunden).
    LongStretch { work: i64, outside: bool },
    /// Mehr als 10 Stunden Arbeit.
    MaxHours { work: i64, outside: bool },
    /// Ruhezeit seit dem letzten Arbeitstag zu kurz.
    Rest { rest: i64, outside: bool },
    /// Arbeitszeit außerhalb des Arbeitszeitrahmens (nicht gezählt): wann und in welchem Rahmen.
    OutsideFrame { spans: Vec<Span>, frame: String },
    /// Arbeit am Wochenende (`counted`: Sie zählt als Mehrarbeit, siehe
    /// [`WorkRules::free_days_count`]).
    Weekend { counted: bool },
    /// Arbeit an einem freien Wochentag (abgewählter Arbeitstag); `counted` wie bei
    /// [`Warning::Weekend`].
    FreeWeekday { counted: bool },
}

impl Warning {
    pub fn text(&self) -> String {
        // Der Verstoß ergibt sich erst mit der Zeit, die nicht gezählt wird: dazusagen, sonst
        // passt der Hinweis nicht zur angezeigten Arbeitszeit.
        let with = |outside: &bool| if *outside { ", mit der Zeit außerhalb des Arbeitszeitrahmens" } else { "" };
        match self {
            Warning::Breaks { required, outside } => format!(
                "Ab {} Std. Arbeit sind {} Min. Pause vorgeschrieben (§ 4 ArbZG){}",
                if *required >= 45 * 60 { 9 } else { 6 },
                required / 60,
                with(outside)
            ),
            Warning::LongStretch { work, outside } => format!(
                "{} ohne Pause – erlaubt sind höchstens 6 Std. am Stück (§ 4 ArbZG){}",
                fmt_hm(*work),
                with(outside)
            ),
            Warning::MaxHours { work, outside } => {
                format!("{} Arbeitszeit – mehr als 10 Std. am Tag (§ 3 ArbZG){}", fmt_hm(*work), with(outside))
            }
            Warning::Rest { rest, outside } => {
                format!("Ruhezeit nur {} – vorgeschrieben sind 11 Std. (§ 5 ArbZG){}", fmt_hm(*rest), with(outside))
            }
            Warning::OutsideFrame { spans, frame } => {
                format!("{} außerhalb des Arbeitszeitrahmens ({frame}) – nicht gezählt", fmt_spans(spans))
            }
            Warning::Weekend { counted: true } => "Arbeit am Wochenende – zählt ohne Soll als Mehrarbeit".to_string(),
            Warning::Weekend { counted: false } => "Arbeit am Wochenende – zählt nicht im Saldo".to_string(),
            Warning::FreeWeekday { counted: true } => {
                "Arbeit an einem freien Tag – zählt ohne Soll als Mehrarbeit".to_string()
            }
            Warning::FreeWeekday { counted: false } => "Arbeit an einem freien Tag – zählt nicht im Saldo".to_string(),
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

/// Verstöße gegen Pausen und Höchstarbeitszeit in den Arbeitsabschnitten `spans` (`outside`:
/// samt der Zeit außerhalb des Arbeitszeitrahmens): zu wenig Pause, zu lange am Stück, mehr als
/// 10 Stunden.
fn legal_checks(spans: &[Span], outside: bool) -> [Option<Warning>; 3] {
    let work: i64 = spans.iter().map(Span::len).sum();
    let pause = Day::legal_pause_secs(spans);
    let breaks = if work > 9 * 3600 && pause < 45 * 60 {
        Some(Warning::Breaks { required: 45 * 60, outside })
    } else if work > 6 * 3600 && pause < 30 * 60 {
        Some(Warning::Breaks { required: 30 * 60, outside })
    } else {
        None
    };
    // Fehlt die Pause ganz, sagt der Hinweis oben schon alles; sonst zählt auch, wann sie lag.
    let no_break_at_all = pause == 0 && breaks.is_some();
    let stretch = Day::longest_stretch_secs(spans);
    let stretch = (stretch > MAX_STRETCH && !no_break_at_all).then_some(Warning::LongStretch { work: stretch, outside });
    let max = (work > MAX_DAILY_WORK).then_some(Warning::MaxHours { work, outside });
    [breaks, stretch, max]
}

/// Hinweise für den Tag `date`. `days` muss nach Datum sortiert sein; der vorige
/// Arbeitstag wird für die Ruhezeit gebraucht.
///
/// Pausen, Höchstarbeitszeit und Ruhezeit werden zweimal geprüft: mit der gezählten Arbeitszeit
/// und – wenn das nichts ergibt – mit der tatsächlichen samt der Zeit außerhalb des
/// Arbeitszeitrahmens ([`Day::actual_spans`]). Das Gesetz fragt nicht danach, was gezählt wird.
pub fn day_warnings(days: &[Day], date: NaiveDate, s: &Settings) -> Vec<Warning> {
    let Some(idx) = days.iter().position(|d| d.date == date) else { return Vec::new() };
    let day = &days[idx];
    let mut out = Vec::new();
    let spans = day.spans(s);
    let work: i64 = spans.iter().map(|x| x.len()).sum();
    let actual = day.actual_spans(s);

    let counted = legal_checks(&spans, false);
    let with_outside = if actual == spans { [None, None, None] } else { legal_checks(&actual, true) };
    out.extend(counted.into_iter().zip(with_outside).filter_map(|(c, a)| c.or(a)));

    // Ruhezeit seit dem Ende des letzten Tages mit Arbeit – gezählt bzw. tatsächlich.
    let rest = |first: Option<&Span>, prev: Option<(NaiveDate, i64)>| {
        let (first, (prev_date, prev_end)) = (first?, prev?);
        Some((date - prev_date).num_days() * DAY_END + first.from - prev_end)
    };
    let before = &days[..idx];
    let counted_rest = rest(
        spans.first(),
        before.iter().rev().find_map(|d| d.record(s).filter(|r| r.work_secs > 0).map(|r| (d.date, r.last_end))),
    );
    let actual_rest =
        rest(actual.first(), before.iter().rev().find_map(|d| d.actual_spans(s).last().map(|l| (d.date, l.to))));
    match (counted_rest, actual_rest) {
        (Some(rest), _) if rest < MIN_REST => out.push(Warning::Rest { rest, outside: false }),
        (_, Some(rest)) if rest < MIN_REST => out.push(Warning::Rest { rest, outside: true }),
        _ => {}
    }
    // Arbeit an einem Tag ohne Soll: Sie geht nur in den Saldo ein, wenn der Stand es vorsieht –
    // am Wochenende wie an einem abgewählten Arbeitstag.
    if work > 0 && !s.is_workday(date) {
        let counted = s.work_counts_on(date);
        out.push(if Settings::is_weekend(date) { Warning::Weekend { counted } } else { Warning::FreeWeekday { counted } });
    }
    // Abschnitte unter einer Minute (z. B. kurz entsperrt in der Nacht) lohnen keinen Hinweis.
    let outside: Vec<Span> = day.outside_frame_spans(s).into_iter().filter(|x| x.len() >= 60).collect();
    if !outside.is_empty() {
        out.push(Warning::OutsideFrame { spans: outside, frame: s.rules(date).frame_text() });
    }
    out
}

/// Arbeitstage von `from` bis `to` ohne jeden Eintrag (`days`: alle Tage, nach Datum
/// sortiert). Sie zählen trotzdem ins Soll – ab dem ersten erfassten Tag (davor gibt es keine
/// Daten) bis gestern (heute zählt erst mit Arbeitszeit, siehe [`Settings::counted_target`]).
/// Ein Eintrag ist, was eine Zeile ergibt ([`Day::record`]): erfasste Zeit, eine Korrektur
/// oder eine Abwesenheit. Die bloße Entscheidung zur Erfassung an einem freien Tag ist keiner –
/// sonst fiele der Tag ganz aus dem Soll, wenn er nachträglich Arbeitstag wird.
pub fn missing_workdays(days: &[Day], from: NaiveDate, to: NaiveDate, today: NaiveDate, s: &Settings) -> Vec<NaiveDate> {
    let entry = |d: &&Day| d.has_entries() || d.absence.is_some();
    let Some(first) = days.iter().find(entry).map(|d| d.date) else { return Vec::new() };
    let Some(yesterday) = today.pred_opt() else { return Vec::new() };
    let (from, to) = (from.max(first), to.min(yesterday));
    if from > to {
        return Vec::new();
    }
    let recorded: std::collections::HashSet<NaiveDate> =
        days.iter().filter(|d| d.date >= from && d.date <= to).filter(entry).map(|d| d.date).collect();
    from.iter_days().take_while(|d| *d <= to).filter(|d| s.is_workday(*d) && !recorded.contains(d)).collect()
}

/// Arbeitszeit, die in den Saldo eingeht (wie angezeigt, in ganzen Minuten), und Soll der Tage
/// von `from` bis `to` (`days`: alle Tage, `today`: heute); der Saldo ist die Differenz. Arbeit
/// an freien Tagen zählt nur, wenn der Stand es vorsieht ([`Settings::counted_work`]). Das Soll
/// zählt nach [`Settings::counted_target`] – auch für Arbeitstage ganz ohne Eintrag
/// ([`missing_workdays`]).
pub fn balance(days: &[Day], from: NaiveDate, to: NaiveDate, today: NaiveDate, s: &Settings) -> (i64, i64) {
    let (work, target) = days
        .iter()
        .filter(|d| d.date >= from && d.date <= to)
        .filter_map(|d| d.record(s).map(|r| (d, r)))
        .fold((0, 0), |(w, t), (d, r)| {
            (w + s.counted_work(d.date, r.shown_work_secs), t + s.counted_target(d, &r, today))
        });
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
        assert_eq!(s.rules(date(16)).frame_text(), "07:00–22:00 Uhr");
        let bad = WorkRules { frame_from: h(22, 0), frame_to: h(7, 0), min_pause: 0, weekly_target: 0, workdays: 0, free_days_count: true };
        let fixed = s.clone().with_rules(bad);
        let r = fixed.rules(date(16));
        assert_eq!((r.frame_from, r.frame_to), (h(7, 0), h(22, 0)));
        assert_eq!(r.min_pause, 60);
        assert_eq!(r.weekly_target, 3600);
        assert_eq!(r.workdays, ALL_WORKDAYS);
        assert_eq!(fixed.min_work, s.min_work);
        // Die Mindestdauer von Arbeit ohne Mac hängt nicht an der Mindestpause.
        let s20 = Settings { min_work: 5 * 60, ..s.clone() }.with_rules(WorkRules { min_pause: 20 * 60, ..WorkRules::DEFAULT });
        assert_eq!((s20.rules(date(16)).min_pause, s20.min_work), (20 * 60, 5 * 60));
        assert_eq!(Settings { min_work: 0, ..s.clone() }.sanitized().min_work, MIN_WORK_RANGE.0);
        assert_eq!(Settings { min_work: 999 * 60, ..s }.sanitized().min_work, MIN_WORK_RANGE.1);
    }

    #[test]
    fn rules_apply_from_their_date() {
        let mut s = Settings::default();
        let part = WorkRules { weekly_target: h(32, 0), workdays: 0b0_1111, ..WorkRules::DEFAULT };
        // Ab Donnerstag 32 Std. an Mo–Do: davor gilt weiter der Stand von Beginn an.
        s.set_rules(Some(date(19)), part, None);
        assert_eq!(s.work.len(), 2);
        assert_eq!(s.rules(date(18)), &WorkRules::DEFAULT);
        assert_eq!(s.rules(date(19)), &part);
        assert_eq!(s.rules(NaiveDate::MIN), &WorkRules::DEFAULT);
        assert_eq!(s.daily_target(date(18)), h(7, 48)); // Mittwoch, alter Stand
        assert_eq!(s.daily_target(date(19)), h(8, 0)); // Donnerstag, neuer Stand
        assert_eq!(s.daily_target(date(20)), 0); // Freitag ist kein Arbeitstag mehr
        assert!(s.auto_counts_on(date(18)) && !s.auto_counts_on(date(20)));

        // Derselbe Tag ersetzt den Stand; ein Stand wie der davor entfällt.
        let part30 = WorkRules { weekly_target: h(30, 0), ..part };
        s.set_rules(Some(date(19)), part30, None);
        assert_eq!(s.work.len(), 2);
        assert_eq!(s.rules(date(25)), &part30);
        s.set_rules(Some(date(23)), part30, None);
        assert_eq!(s.work.len(), 2, "gleicht dem Stand davor");

        // Verschieben: Der Stand ab Donnerstag zieht auf Dienstag um, ohne dass ein späterer
        // Stand mit den Regeln von Beginn dabei verloren geht.
        s.set_rules(Some(date(26)), WorkRules::DEFAULT, None);
        assert_eq!(s.work.len(), 3);
        s.set_rules(Some(date(17)), part30, Some(date(19)));
        assert_eq!(s.work.iter().map(|p| p.from).collect::<Vec<_>>(), vec![None, Some(date(17)), Some(date(26))]);

        // Löschen: Ab dann gilt der Stand davor – hier gleicht er dem danach, beide fallen zusammen.
        s.remove_rules(date(17));
        assert_eq!(s.work, vec![RulesPeriod { from: None, rules: WorkRules::DEFAULT }]);
        // Der erste Stand bleibt immer und gilt seit jeher.
        s.set_rules(None, part, None);
        assert_eq!(s.work, vec![RulesPeriod { from: None, rules: part }]);
        let odd = Settings { work: vec![], ..Settings::default() }.sanitized();
        assert_eq!(odd.work, Settings::default().work);
        let late = Settings { work: vec![RulesPeriod { from: Some(date(5)), rules: part }], ..Settings::default() }.sanitized();
        assert_eq!(late.work, vec![RulesPeriod { from: None, rules: part }]);
    }

    #[test]
    fn earlier_days_keep_their_rules() {
        // Mo–Mi 39 Std./5 Tage, ab Donnerstag 32 Std./4 Tage (Mo–Do), Rahmen ab 8 Uhr,
        // Mindestpause 30 Min.
        let mut s = Settings::default();
        let part = WorkRules { weekly_target: h(32, 0), workdays: 0b0_1111, frame_from: h(8, 0), min_pause: 30 * 60, ..WorkRules::DEFAULT };
        s.set_rules(Some(date(19)), part, None);
        let mut days = Vec::new();
        for d in 16..=20 {
            let mut day = Day::new(date(d));
            // 7:30 bis 16:00 mit 20 Min. Sperre am Mittag
            day.add_period(h(7, 30), h(12, 0));
            day.add_period(h(12, 20), h(16, 0));
            days.push(day);
        }
        let rec = |d: u32| days.iter().find(|x| x.date == date(d)).unwrap().record(&s).unwrap();
        // Mittwoch: alter Rahmen (ab 7 Uhr) und alte Mindestpause (20 Min. sind eine Pause).
        assert_eq!((rec(18).work_secs, rec(18).pause_secs()), (h(8, 10), h(0, 20)));
        // Donnerstag: ab 8 Uhr, die 20 Min. zählen zur Arbeitszeit.
        assert_eq!((rec(19).work_secs, rec(19).pause_secs()), (h(8, 0), 0));
        // Soll der Woche: 3 × 7:48 + 8:00 (Freitag frei – seine Arbeit zählt nicht im Saldo).
        let (work, target) = balance(&days, date(16), date(22), date(23), &s);
        assert_eq!(target, 3 * h(7, 48) + h(8, 0));
        assert_eq!(work, 3 * h(8, 10) + h(8, 0));
        // Ein ganz fehlender Arbeitstag zählt mit dem Soll, das an ihm galt.
        let missing: Vec<Day> = days.iter().filter(|d| d.date != date(17) && d.date != date(19)).cloned().collect();
        let (_, t) = balance(&missing, date(16), date(22), date(23), &s);
        assert_eq!(t, 3 * h(7, 48) + h(8, 0));
        // Hinweis „außerhalb des Rahmens“ nur, wo der Rahmen es sagt.
        assert!(day_warnings(&days, date(18), &s).iter().all(|w| !matches!(w, Warning::OutsideFrame { .. })));
        assert!(day_warnings(&days, date(19), &s).contains(&Warning::OutsideFrame {
            spans: vec![Span::new(h(7, 30), h(8, 0))],
            frame: "08:00–22:00 Uhr".to_string()
        }));
        // Fußzeilen: je Stand ein Absatz, sonst einer.
        assert_eq!(s.summary_between(date(16), date(18)), vec![WorkRules::DEFAULT.summary()]);
        assert_eq!(s.target_basis(date(19), date(22)), "32 Std./Woche, Mo, Di, Mi, Do");
        let parts = s.summary_between(date(16), date(22));
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("Bis 18.11.2026: Arbeitszeitrahmen 07:00"), "{}", parts[0]);
        assert!(parts[1].starts_with("Ab 19.11.2026: Arbeitszeitrahmen 08:00"), "{}", parts[1]);
        s.set_rules(Some(date(21)), WorkRules::DEFAULT, None);
        assert!(s.summary_between(date(16), date(22))[1].starts_with("19.11.2026 bis 20.11.2026: "));
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
        assert!(w.contains(&Warning::Breaks { required: 45 * 60, outside: false }), "{w:?}");
        assert!(!w.iter().any(|x| matches!(x, Warning::MaxHours { .. })), "genau 10 Std. sind erlaubt");

        let w = day_warnings(&days, date(17), &s);
        assert!(w.contains(&Warning::Rest { rest: h(9, 0), outside: false }), "{w:?}");
        let outside = Warning::OutsideFrame { spans: vec![Span::new(h(6, 30), h(7, 0))], frame: s.rules(date(17)).frame_text() };
        assert!(w.contains(&outside), "{w:?}");
        assert_eq!(
            outside.text(),
            "06:30–07:00 Uhr außerhalb des Arbeitszeitrahmens (07:00–22:00 Uhr) – nicht gezählt"
        );

        assert_eq!(day_warnings(&days, date(21), &s), vec![Warning::Weekend { counted: false }]);

        let mut long = Day::new(date(18));
        long.add_period(h(7, 0), h(12, 0));
        long.add_period(h(13, 0), h(19, 0));
        let w = day_warnings(&[long], date(18), &s);
        assert_eq!(w, vec![Warning::MaxHours { work: h(11, 0), outside: false }]);
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
        assert_eq!(w, vec![Warning::LongStretch { work: h(7, 0), outside: false }]);
        assert_eq!(w[0].text(), "7:00 Std. ohne Pause – erlaubt sind höchstens 6 Std. am Stück (§ 4 ArbZG)");
        // Genau 6 Std. am Stück sind erlaubt.
        let ok = day(&[(h(7, 0), h(13, 0)), (h(13, 30), h(15, 0))]);
        assert!(day_warnings(&[ok], date(18), &s).is_empty());
        // Ganz ohne Pause genügt der Hinweis auf die fehlende Pause.
        let none = day(&[(h(7, 0), h(14, 0))]);
        assert_eq!(day_warnings(&[none], date(18), &s), vec![Warning::Breaks { required: 30 * 60, outside: false }]);
        // Zu wenig Pause und zu spät: beide Hinweise.
        let both = day(&[(h(7, 0), h(13, 10)), (h(13, 30), h(14, 30))]);
        assert_eq!(
            day_warnings(&[both], date(18), &s),
            vec![Warning::Breaks { required: 30 * 60, outside: false }, Warning::LongStretch { work: h(6, 10), outside: false }]
        );
        // Mindestpause 5 Min.: Eine Unterbrechung von 10 Min. ist keine Pause nach dem Gesetz –
        // die Arbeit davor und danach zählt zusammen (ohne die Unterbrechung).
        let short = Settings { min_work: 5 * 60, ..s.clone() }.with_rules(WorkRules { min_pause: 5 * 60, ..WorkRules::DEFAULT });
        let split = day(&[(h(7, 0), h(10, 0)), (h(10, 10), h(13, 30)), (h(14, 0), h(15, 0))]);
        assert_eq!(day_warnings(&[split], date(18), &short), vec![Warning::LongStretch { work: h(6, 20), outside: false }]);
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
            frame: WorkRules::DEFAULT.frame_text(),
        };
        assert!(two.text().starts_with("06:30–07:00 und 22:00–24:00 Uhr außerhalb"), "{}", two.text());
        let many = Warning::OutsideFrame { spans: vec![Span::new(0, 600); 4], frame: WorkRules::DEFAULT.frame_text() };
        assert!(many.text().starts_with("4 Zeiträume (0:40 Std.) außerhalb"), "{}", many.text());
        let late = Warning::OutsideFrame { spans: vec![Span::new(h(23, 22) + 34, DAY_END - 1)], frame: WorkRules::DEFAULT.frame_text() };
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
        let d = WorkRules::DEFAULT;
        assert_eq!((d.workday_count(), d.workdays_text(), d.week_text()), (5, "Mo–Fr".to_string(), "5-Tage-Woche".to_string()));
        // 4-Tage-Woche ohne Freitag: 32 Std. sind 8 Std. an Mo–Do, freitags kein Soll.
        let r4 = WorkRules { workdays: 0b0_1111, weekly_target: h(32, 0), ..d };
        let four = s.clone().with_rules(r4);
        assert_eq!((r4.workday_count(), r4.workdays_text(), r4.week_text()), (4, "Mo, Di, Mi, Do".to_string(), "4-Tage-Woche".to_string()));
        assert_eq!(four.daily_target(date(16)), h(8, 0)); // Montag
        assert_eq!(four.daily_target(date(20)), 0); // Freitag
        assert!(four.is_workday(date(19)) && !four.is_workday(date(20)) && !four.is_workday(date(21)));
        assert_eq!(r4.summary(), "Arbeitszeitrahmen 07:00–22:00 Uhr  ·  Regelarbeitszeit 32 Std./Woche (Mo, Di, Mi, Do)  ·  Pausen ab 15 Min.");
        // Arbeit am freien Freitag zählt nicht im Saldo – das sagt ein Hinweis wie am
        // Wochenende; mit dem Schalter im Stand ist sie Mehrarbeit.
        let mut fri = Day::new(date(20));
        fri.add_period(h(9, 0), h(11, 0));
        let hint = day_warnings(&[fri.clone()], date(20), &four);
        assert_eq!(hint, vec![Warning::FreeWeekday { counted: false }]);
        assert_eq!(hint[0].text(), "Arbeit an einem freien Tag – zählt nicht im Saldo");
        assert_eq!(balance(&[fri.clone()], date(16), date(22), date(23), &four), (0, 0));
        let four_counted = s.clone().with_rules(WorkRules { free_days_count: true, ..r4 });
        assert_eq!(day_warnings(&[fri.clone()], date(20), &four_counted), vec![Warning::FreeWeekday { counted: true }]);
        assert_eq!(balance(&[fri], date(16), date(22), date(23), &four_counted), (h(2, 0), 0));
        // Ungültige Angaben: ohne Arbeitstag wieder Mo–Fr, Samstag und Sonntag gibt es nicht.
        assert_eq!(WorkRules { workdays: 0, ..d }.sanitized().workdays, ALL_WORKDAYS);
        assert_eq!(WorkRules { workdays: 0b110_0001, ..d }.sanitized().workdays, 0b0_0001);
        assert_eq!(s.clone().with_rules(WorkRules { workdays: 0b0_0100, weekly_target: h(8, 0), ..d }).daily_target(date(18)), h(8, 0));
        // Inaktivität: aus oder in Grenzen.
        assert_eq!(Settings { idle_secs: -5, ..s.clone() }.sanitized().idle_secs, 0);
        assert_eq!(Settings { idle_secs: 10, ..s.clone() }.sanitized().idle_secs, IDLE_RANGE.0);
        assert_eq!(s.clone().with_rules(WorkRules { min_pause: h(0, 45), ..d }).ask_after(date(16)), h(0, 45));
        assert_eq!(s.ask_after(date(16)), ASK_AFTER_LOCK);
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
        assert_eq!(s.counted_target(&half, &half.record(&s).unwrap(), date(23)), full / 2);
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
        assert_eq!(s.counted_target(&days[2], &days[2].record(&s).unwrap(), date(17)), 0);
        assert_eq!(s.counted_target(&days[2], &days[2].record(&s).unwrap(), date(18)), full);
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
    fn special_leave_is_always_a_full_day() {
        use crate::day::AbsenceKind::*;
        let s = Settings::default();
        let a = Absence::new(SpecialLeave, true);
        assert!(!a.half, "halben Sonderurlaub gibt es nicht");
        assert!(SpecialLeave.reduces_target() && !SpecialLeave.allows_half());
        assert!(Vacation.allows_half() && !TimeOff.allows_half());
        // Senkt das Soll ganz, wie ein ganzer Urlaubstag.
        assert_eq!(s.day_target(date(16), Some(a)), 0);
        let mut day = Day::new(date(16));
        day.absence = Some(a);
        assert_eq!(balance(&[day], date(16), date(16), date(23), &s), (0, 0));
        // Texte, Kennung und zurück – „Sonderurlaub“ ist kein „Urlaub“.
        assert_eq!((a.label(), a.short(), a.code()), ("Sonderurlaub".to_string(), "Sonderurlaub".to_string(), "SU".to_string()));
        assert_eq!(Absence::parse("Sonderurlaub"), Some(a));
        assert_eq!(Absence::parse("Sonderurlaub (halber Tag)"), Some(a));
        assert_eq!(Absence::parse("Urlaub"), Some(Absence::new(Vacation, false)));
        assert_eq!(crate::day::AbsenceKind::from_id(SpecialLeave.id()), Some(SpecialLeave));
        // Ganztags: nicht neben anderen Einträgen.
        let mut busy = Day::new(date(17));
        busy.add_period(h(8, 0), h(9, 0));
        assert!(!busy.accepts_absence(Some(a), false));
    }

    #[test]
    fn free_day_work_counts_only_if_the_rules_say_so() {
        // Bis Mittwoch der Standard (zählt nicht), ab Donnerstag zählt Arbeit an freien Tagen.
        let mut s = Settings::default();
        s.set_rules(Some(date(19)), WorkRules { free_days_count: true, ..WorkRules::DEFAULT }, None);
        assert_eq!(s.work.len(), 2, "nur der Schalter unterscheidet die Stände");
        let day = |d: u32| {
            let mut x = Day::new(date(d));
            x.add_period(h(10, 0), h(12, 0));
            x
        };
        // Sonntag davor zählt nicht, Samstag danach schon; Arbeitstage zählen immer.
        let (sun, mon, sat) = (day(15), day(16), day(21));
        assert!(!s.work_counts_on(date(15)) && s.work_counts_on(date(16)) && s.work_counts_on(date(21)));
        let rec = |d: &Day| d.record(&s).unwrap();
        assert_eq!(s.day_balance(&sun, &rec(&sun), date(23)), None, "kein Saldo");
        assert_eq!(s.day_balance(&sat, &rec(&sat), date(23)), Some(h(2, 0)));
        assert_eq!(s.day_balance(&mon, &rec(&mon), date(23)), Some(h(2, 0) - h(7, 48)));
        let days = vec![sun.clone(), mon, sat];
        assert_eq!(balance(&days, date(15), date(15), date(23), &s), (0, 0));
        assert_eq!(balance(&days, date(21), date(21), date(23), &s), (h(2, 0), 0));
        // Hinweis am Wochenende je nachdem, ob es zählt.
        assert_eq!(day_warnings(&days, date(15), &s), vec![Warning::Weekend { counted: false }]);
        assert_eq!(day_warnings(&days, date(21), &s), vec![Warning::Weekend { counted: true }]);
        assert_eq!(Warning::Weekend { counted: false }.text(), "Arbeit am Wochenende – zählt nicht im Saldo");
        // Ein Feiertag an einem Arbeitstag ist kein freier Tag: Arbeit zählt als Mehrarbeit.
        let mut holiday = day(17);
        holiday.absence = Some(Absence::new(crate::day::AbsenceKind::Holiday, false));
        assert_eq!(s.day_balance(&holiday, &holiday.record(&s).unwrap(), date(23)), Some(h(2, 0)));
        // Die Kurzfassung nennt den Schalter nur, wenn er an ist.
        assert!(!s.rules(date(16)).summary().contains("freien Tagen"));
        assert!(s.rules(date(19)).summary().ends_with("Arbeit an freien Tagen im Saldo"));
        // Der Standard zählt nicht.
        assert!(!WorkRules::DEFAULT.free_days_count && !Settings::default().work_counts_on(date(15)));
        // `sanitized` lässt den Schalter stehen.
        assert!(WorkRules { free_days_count: true, ..WorkRules::DEFAULT }.sanitized().free_days_count);
    }

    #[test]
    fn account_adds_carry_and_counts_from_start() {
        let s = Settings::default();
        let day = |d: u32, from: i64, to: i64| {
            let mut x = Day::new(date(d));
            x.add_period(from, to);
            x
        };
        // Mo 9 Std. (+1:12), Di 7 Std. (−0:48), Mi bis Fr genau das Soll, Sa 1 Std. (zählt
        // nicht – erst mit dem Schalter im Stand als Mehrarbeit)
        let mut days = vec![day(16, h(8, 0), h(17, 0)), day(17, h(8, 0), h(15, 0))];
        days.extend((18..=20).map(|d| day(d, h(8, 0), h(15, 48))));
        days.push(day(21, h(10, 0), h(11, 0)));
        assert_eq!(account(&days, date(22), &s), h(0, 24));
        let s = s.with_rules(WorkRules { free_days_count: true, ..WorkRules::DEFAULT });
        assert_eq!(account(&days, date(22), &s), h(1, 24));
        // Stand an einem früheren Tag: spätere Tage zählen noch nicht.
        assert_eq!(account(&days, date(16), &s), h(1, 12));
        // Ab Dienstag, mit 5 Std. Übertrag aus der Zeit davor.
        let from_tue = Settings { account_from: Some(date(17)), account_carry: h(5, 0), ..s.clone() };
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
        sat.add_period(h(10, 0), h(11, 0)); // Wochenende: kein Soll, zählt nicht
        let days = [mon, sat];
        // Stand Dienstag: Heute zählt erst mit Arbeitszeit.
        assert_eq!(balance(&days, date(16), date(22), date(17), &s), (h(9, 0), h(7, 48)));
        assert!(missing_workdays(&days, date(16), date(22), date(17), &s).is_empty());
        // Stand Montag darauf: Di bis Fr ohne Eintrag zählen mit ihrem Soll.
        assert_eq!(missing_workdays(&days, date(16), date(22), date(23), &s), (17..=20).map(date).collect::<Vec<_>>());
        assert_eq!(balance(&days, date(16), date(22), date(23), &s), (h(9, 0), 5 * h(7, 48)));
        // Vor dem ersten erfassten Tag zählt nichts; ohne Einträge gar nichts.
        assert!(missing_workdays(&days, date(2), date(13), date(23), &s).is_empty());
        assert!(missing_workdays(&[], date(16), date(22), date(23), &s).is_empty());
        // Ein Tag nur mit Pause o. Ä., aber ohne Arbeitszeit, zählt sein Soll ebenso.
        let mut tue = Day::new(date(17));
        tue.add_break(h(12, 0), h(12, 30));
        assert_eq!(s.counted_target(&tue, &tue.record(&s).unwrap(), date(23)), h(7, 48));
        assert_eq!(s.counted_target(&tue, &tue.record(&s).unwrap(), date(17)), 0, "heute noch nicht");
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

    #[test]
    fn daily_targets_are_whole_minutes_and_add_up() {
        // 38,5 Std. an vier Tagen: 9:37:30 je Tag ginge nicht in Minuten auf.
        let r = WorkRules { workdays: 0b0_1111, weekly_target: h(38, 30), ..WorkRules::DEFAULT };
        let s = Settings::default().with_rules(r);
        let targets: Vec<i64> = (16..=22).map(|d| s.daily_target(date(d))).collect();
        assert_eq!(targets, [h(9, 37), h(9, 38), h(9, 37), h(9, 38), 0, 0, 0]);
        assert_eq!(targets.iter().sum::<i64>(), h(38, 30));
        assert_eq!(r.daily_target_range(), (h(9, 37), h(9, 38)));
        assert_eq!(r.daily_target_text(), "9:37 bis 9:38 Std.");
        assert_eq!(WorkRules::DEFAULT.daily_target_text(), "7:48 Std.");
        assert!((16..=20).all(|d| Settings::default().daily_target(date(d)) == h(7, 48)));
        // Di und Do als Arbeitstage: der zweite bekommt die Minute mehr.
        let two = WorkRules { workdays: 0b0_1010, weekly_target: h(19, 15), ..WorkRules::DEFAULT };
        assert_eq!((two.daily_target_on(date(17)), two.daily_target_on(date(19))), (h(9, 37), h(9, 38)));
        assert_eq!(two.daily_target_on(date(16)), 0);
        // Vier Tage mit je 9:37:30 Arbeit: Jeder Tagessaldo ist eine ganze Minutenzahl, und
        // zusammen ergeben sie genau den Wochensaldo.
        let days: Vec<Day> = (16..=19)
            .map(|d| {
                let mut day = Day::new(date(d));
                day.add_period(h(8, 0), h(17, 37) + 30);
                day
            })
            .collect();
        let per_day: Vec<i64> = days.iter().map(|d| s.day_balance(d, &d.record(&s).unwrap(), date(23)).unwrap()).collect();
        assert_eq!(per_day, [0, -60, 0, -60]);
        let (work, target) = balance(&days, date(16), date(22), date(23), &s);
        assert_eq!(work - target, per_day.iter().sum::<i64>());
        assert_eq!(fmt_balance(work - target), "−0:02 h");
        // Halber Tag: die Hälfte, auf ganze Minuten abgerundet.
        let half = Some(Absence::new(crate::day::AbsenceKind::Vacation, true));
        assert_eq!(s.day_target(date(16), half), h(4, 48));
        assert_eq!(Settings::default().day_target(date(16), half), h(3, 54));
        // Eine Wochenarbeitszeit mit Sekunden (nur aus einer fremden Datenbank) wird abgerundet.
        assert_eq!(WorkRules { weekly_target: h(39, 0) + 59, ..WorkRules::DEFAULT }.sanitized().weekly_target, h(39, 0));
    }

    #[test]
    fn todays_target_counts_only_as_far_as_worked() {
        let s = Settings::default();
        let mut mon = Day::new(date(16));
        mon.add_period(h(8, 0), h(16, 0)); // 8:00 Std.: +0:12
        let mut tue = Day::new(date(17));
        tue.add_period(h(8, 0), h(8, 1));
        let week = |days: &[Day], today: u32| {
            let (work, target) = balance(days, date(16), date(22), date(today), &s);
            work - target
        };
        let day_balance = |d: &Day, today: u32| s.day_balance(d, &d.record(&s).unwrap(), date(today));
        // Dienstag vor dem ersten Entsperren und um 08:01 Uhr: Der Wochensaldo bleibt stehen.
        assert_eq!(week(&[mon.clone()], 17), h(0, 12));
        assert_eq!(week(&[mon.clone(), tue.clone()], 17), h(0, 12));
        assert_eq!(day_balance(&tue, 17), Some(0));
        // Mittags ebenso; über dem Soll wächst er.
        tue.add_period(h(8, 0), h(12, 0));
        assert_eq!(week(&[mon.clone(), tue.clone()], 17), h(0, 12));
        let mut long = tue.clone();
        long.add_period(h(12, 0), h(16, 30));
        assert_eq!(day_balance(&long, 17), Some(h(0, 42)));
        // Feierabend um 12:00 Uhr: Der Tag ist zu Ende, der Rest fehlt.
        let mut closed = tue.clone();
        closed.end = Some(h(12, 0));
        assert_eq!(day_balance(&closed, 17), Some(h(4, 0) - h(7, 48)));
        // Ohne Feierabend fehlt er ab morgen.
        assert_eq!(day_balance(&tue, 18), Some(h(4, 0) - h(7, 48)));
        assert_eq!(week(&[mon.clone(), tue.clone()], 18), h(0, 12) + h(4, 0) - h(7, 48));
        // Halber Urlaubstag heute: das halbe Soll, ebenfalls nur soweit erarbeitet.
        let mut half = tue.clone();
        half.absence = Some(Absence::new(crate::day::AbsenceKind::Vacation, true));
        assert_eq!(day_balance(&half, 17), Some(h(0, 6)));
        let mut half_only = Day::new(date(17));
        half_only.absence = half.absence;
        assert_eq!(day_balance(&half_only, 17), None);
        assert_eq!(day_balance(&half_only, 18), Some(-h(3, 54)));
        // Zeitausgleich zählt sofort: Dort soll das Zeitkonto sinken.
        let mut off = Day::new(date(17));
        off.absence = Some(Absence::new(crate::day::AbsenceKind::TimeOff, false));
        assert_eq!(day_balance(&off, 17), Some(-h(7, 48)));
    }

    #[test]
    fn legal_checks_see_work_outside_the_frame() {
        let s = Settings::default();
        // Montag bis 23:30 Uhr am Mac (gezählt bis 22:00), Dienstag ab 09:15 Uhr.
        let mut mon = Day::new(date(16));
        mon.add_period(h(8, 0), h(12, 0));
        mon.add_period(h(13, 0), h(23, 30));
        let mut tue = Day::new(date(17));
        tue.add_period(h(9, 15), h(12, 0));
        let w = day_warnings(&[mon, tue.clone()], date(17), &s);
        assert_eq!(w, vec![Warning::Rest { rest: h(9, 45), outside: true }]);
        assert_eq!(
            w[0].text(),
            "Ruhezeit nur 9:45 Std. – vorgeschrieben sind 11 Std. (§ 5 ArbZG), mit der Zeit außerhalb des Arbeitszeitrahmens"
        );
        // Mittwoch 05:00–11:00 und 11:45–16:00 Uhr: gezählt 8:15, tatsächlich 10:15 Std.
        let mut wed = Day::new(date(18));
        wed.add_period(h(5, 0), h(11, 0));
        wed.add_period(h(11, 45), h(16, 0));
        let w = day_warnings(&[wed.clone()], date(18), &s);
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[0], Warning::MaxHours { work: h(10, 15), outside: true });
        assert!(matches!(w[1], Warning::OutsideFrame { .. }));
        // Ein Verstoß schon in der gezählten Zeit wird nicht doppelt gemeldet.
        let mut long = Day::new(date(18));
        long.add_period(h(5, 0), h(11, 0));
        long.add_period(h(11, 45), h(19, 0));
        let w = day_warnings(&[long], date(18), &s);
        let max: Vec<&Warning> = w.iter().filter(|x| matches!(x, Warning::MaxHours { .. })).collect();
        assert_eq!(max, [&Warning::MaxHours { work: h(11, 15), outside: false }]);
        // Ein kurzer Blick auf den Mac in der Nacht (unter einer Minute) verkürzt keine Ruhezeit.
        let mut thu = Day::new(date(19));
        thu.add_period(h(2, 0), h(2, 0) + 30);
        thu.add_period(h(8, 0), h(12, 0));
        assert!(day_warnings(&[wed, thu], date(19), &s).is_empty());
        // Sonntagabend erfasst, aber nicht gezählt: keine Arbeit – also auch keine Ruhezeit.
        let mut sun = Day::new(date(22));
        sun.add_period(h(20, 0), h(23, 0));
        sun.free_day = Some(crate::day::FreeDay::Uncounted);
        let mut next = Day::new(date(23));
        next.add_period(h(7, 0), h(12, 0));
        assert!(day_warnings(&[sun, next], date(23), &s).is_empty());
    }

    #[test]
    fn a_day_with_only_the_free_day_mark_is_no_entry() {
        let s = Settings::default();
        let mut first = Day::new(date(16));
        first.add_period(h(8, 0), h(16, 0));
        // Nur die Entscheidung zur Erfassung, an einem Tag, der (inzwischen) Arbeitstag ist.
        let mut marked = Day::new(date(17));
        marked.free_day = Some(crate::day::FreeDay::Stopped);
        let days = [first, marked];
        assert_eq!(missing_workdays(&days, date(16), date(22), date(19), &s), [date(17), date(18)]);
        assert_eq!(balance(&days, date(16), date(22), date(19), &s), (h(8, 0), 3 * h(7, 48)));
        // Ein solcher Tag allein ist auch kein erster erfasster Tag.
        assert!(missing_workdays(&days[1..], date(16), date(22), date(19), &s).is_empty());
    }
}
