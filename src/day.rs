//! Tagesmodell: automatisch erfasste Perioden plus manuelle Korrekturen.
//!
//! Die automatisch erfassten Perioden (Mac entsperrt) bleiben unverändert erhalten.
//! Manuelle Entscheidungen – Arbeitsbeginn, Pausen, Arbeit ohne Mac, Feierabend – liegen als
//! eigene Schicht darüber; die Arbeitszeit wird aus beidem und den Einstellungen berechnet:
//!
//! 1. Arbeit ohne Mac (Telefonat, Meeting …) zählt wie entsperrte Zeit; Lücken, die kürzer
//!    als die Mindestpause sind, zählen als Arbeit.
//! 2. Manueller Arbeitsbeginn und Feierabend verlängern bzw. kürzen den Tag. Für Arbeit ohne
//!    Mac gilt dabei die letzte Angabe: Wer Beginn oder Feierabend festlegt, kürzt Arbeit ohne
//!    Mac davor bzw. danach ([`Day::set_start`], [`Day::set_end`]); wer sie danach einträgt,
//!    lässt sie auch vor dem Beginn bzw. nach dem Feierabend zählen.
//! 3. Manuell eingetragene Pausen werden abgezogen.
//! 4. Gezählt wird nur, was im Arbeitszeitrahmen liegt.
//!
//! So lässt sich jede Korrektur nachvollziehen und wieder zurücknehmen; geänderte
//! Einstellungen wirken auch auf frühere Tage.
//!
//! Alle Uhrzeiten sind Sekunden seit Mitternacht (`0..=86_400`).

use crate::rules::{Settings, LEGAL_BREAK_SEGMENT};
use chrono::{NaiveDate, NaiveTime, Timelike};

/// Mitternacht am Ende des Tages (24:00:00).
pub const DAY_END: i64 = 86_400;

/// Ein Zeitabschnitt `[from, to)` innerhalb eines Tages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: i64,
    pub to: i64,
}

impl Span {
    pub fn new(from: i64, to: i64) -> Self {
        Self { from, to }
    }
    pub fn len(&self) -> i64 {
        (self.to - self.from).max(0)
    }
}

/// Eine manuell eingetragene Pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Break {
    pub from: i64,
    pub to: i64,
}

/// Nachgetragene Arbeit ohne Mac (z. B. Telefonat oder Meeting bei gesperrtem Mac).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkBlock {
    pub from: i64,
    pub to: i64,
    /// Freie Notiz, z. B. „Meeting“ (leer = keine).
    pub note: String,
}

impl WorkBlock {
    pub fn span(&self) -> Span {
        Span::new(self.from, self.to)
    }

    /// `Meeting 08:00–09:00` bzw. ohne Notiz `Arbeit ohne Mac 08:00–09:00`.
    pub fn describe(&self) -> String {
        let what = if self.note.is_empty() { "Arbeit ohne Mac" } else { &self.note };
        format!("{what} {}–{}", fmt_clock(self.from), fmt_clock(self.to))
    }
}

/// Neue Arbeit ohne Mac nach [`Day::join_work_gaps`]: der verlängerte Abschnitt und was
/// dazugerechnet wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joined {
    pub span: Span,
    /// Lücken zur benachbarten Arbeitszeit (erfasst oder Arbeit ohne Mac).
    pub gaps: Vec<Span>,
    /// Reste angeschnittener manueller Pausen.
    pub rests: Vec<Span>,
}

impl Joined {
    /// Hinweis nach dem Eintragen (`None`, wenn nichts Nennenswertes dazugerechnet wurde).
    ///
    /// Reste unter einer Minute bleiben unerwähnt: Sie entstehen nur, weil die Eingabe
    /// minutengenau ist, die Aufzeichnung aber sekundengenau (Eingabe bis 15:50, entsperrt
    /// um 15:50:40) – angezeigt wäre das eine „Lücke von 15:50 bis 15:50“.
    pub fn note(&self, min_pause: i64) -> Option<String> {
        let visible = |spans: &[Span]| spans.iter().copied().filter(|s| s.len() >= 60).collect::<Vec<_>>();
        let (gaps, rests) = (visible(&self.gaps), visible(&self.rests));
        let mins = |secs: i64| if secs < 60 { "unter 1 Min.".to_string() } else { format!("{} Min.", secs / 60) };
        let list = |spans: &[Span]| {
            spans
                .iter()
                .map(|g| format!("von {} bis {} ({})", fmt_clock(g.from), fmt_clock(g.to), mins(g.len())))
                .collect::<Vec<_>>()
                .join(" und ")
        };
        let mut out = Vec::new();
        match rests.len() {
            0 => {}
            1 => out.push(format!("Von der manuellen Pause bliebe nur {} übrig.", list(&rests))),
            _ => out.push(format!("Von den manuellen Pausen blieben nur {} übrig.", list(&rests))),
        }
        match gaps.len() {
            0 => {}
            1 => out.push(format!("Zur Arbeitszeit daneben bliebe eine Lücke {}.", list(&gaps))),
            _ => out.push(format!("Zur Arbeitszeit daneben blieben Lücken {}.", list(&gaps))),
        }
        if out.is_empty() {
            return None;
        }
        let that = if gaps.len() + rests.len() == 1 { "Das ist" } else { "Das ist jeweils" };
        out.push(format!(
            "{that} kürzer als die Mindestpause von {}; die Zeit wurde dem neuen Abschnitt \
             dazugerechnet. Er reicht jetzt von {} bis {} Uhr.",
            mins(min_pause),
            fmt_clock(self.span.from),
            fmt_clock(self.span.to)
        ));
        Some(out.join(" "))
    }
}

/// Grund für Arbeit ohne Mac (Menüleiste, Wochenansicht, „Aufzeichnungen“); ergibt die Notiz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Meeting,
    Call,
    Networking,
    Other,
}

impl Activity {
    /// In der Reihenfolge der Auswahl.
    pub const ALL: [Activity; 4] = [Activity::Meeting, Activity::Call, Activity::Networking, Activity::Other];

    /// Bezeichnung in Menüs und Auswahlboxen.
    pub fn label(self) -> &'static str {
        match self {
            Activity::Meeting => "Meeting",
            Activity::Call => "Telefonat",
            Activity::Networking => "Networking",
            Activity::Other => "Sonstiges",
        }
    }

    /// Notiz am Arbeitsblock (leer bei „Sonstiges“).
    pub fn note(self) -> &'static str {
        match self {
            Activity::Other => "",
            a => a.label(),
        }
    }

    pub fn id(self) -> isize {
        match self {
            Activity::Meeting => 1,
            Activity::Call => 2,
            Activity::Networking => 3,
            Activity::Other => 0,
        }
    }

    pub fn from_id(id: isize) -> Self {
        Self::ALL.into_iter().find(|a| a.id() == id).unwrap_or(Activity::Other)
    }

    /// Kurzname für die Statusdatei.
    pub fn key(self) -> &'static str {
        match self {
            Activity::Meeting => "meeting",
            Activity::Call => "call",
            Activity::Networking => "networking",
            Activity::Other => "other",
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|a| a.key() == key).unwrap_or(Activity::Other)
    }
}

/// Art einer Abwesenheit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbsenceKind {
    Vacation,
    Sick,
    Holiday,
    /// Freizeitausgleich für Mehrarbeit: Das Soll bleibt stehen, das Zeitkonto sinkt.
    TimeOff,
}

impl AbsenceKind {
    /// In der Reihenfolge der Auswahl.
    pub const ALL: [AbsenceKind; 4] =
        [AbsenceKind::Vacation, AbsenceKind::Sick, AbsenceKind::Holiday, AbsenceKind::TimeOff];

    pub fn label(self) -> &'static str {
        match self {
            AbsenceKind::Vacation => "Urlaub",
            AbsenceKind::Sick => "Krank",
            AbsenceKind::Holiday => "Feiertag",
            AbsenceKind::TimeOff => "Zeitausgleich",
        }
    }

    /// Wert für die Datenbank.
    pub fn id(self) -> i64 {
        match self {
            AbsenceKind::Vacation => 1,
            AbsenceKind::Sick => 2,
            AbsenceKind::Holiday => 3,
            AbsenceKind::TimeOff => 4,
        }
    }

    pub fn from_id(id: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Senkt die Abwesenheit das Soll? Zeitausgleich nicht – dort soll das Zeitkonto sinken.
    pub fn reduces_target(self) -> bool {
        self != AbsenceKind::TimeOff
    }
}

/// Abwesenheit an einem Tag: Urlaub, Krankheit und Feiertag senken sein Soll – ganz oder
/// (halber Tag) auf die Hälfte. Zeitausgleich lässt das Soll stehen, auch wenn an dem Tag
/// gar nicht gearbeitet wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Absence {
    pub kind: AbsenceKind,
    /// Nur der halbe Tag (bei Zeitausgleich ohne Bedeutung und immer `false`).
    pub half: bool,
}

impl Absence {
    pub fn new(kind: AbsenceKind, half: bool) -> Self {
        Self { kind, half: half && kind.reduces_target() }
    }

    /// `Urlaub` bzw. `Urlaub (halber Tag)` – so steht es auch im Export.
    pub fn label(self) -> String {
        if self.half {
            format!("{} (halber Tag)", self.kind.label())
        } else {
            self.kind.label().to_string()
        }
    }

    /// Kurz für Tabellen: `Urlaub` bzw. `½ Urlaub`.
    pub fn short(self) -> String {
        if self.half {
            format!("½ {}", self.kind.label())
        } else {
            self.kind.label().to_string()
        }
    }

    /// Kürzel für die Tabelle der Tage: `U`, `K`, `F`, `ZA`, halbe Tage mit `½`.
    pub fn code(self) -> String {
        let code = match self.kind {
            AbsenceKind::Vacation => "U",
            AbsenceKind::Sick => "K",
            AbsenceKind::Holiday => "F",
            AbsenceKind::TimeOff => "ZA",
        };
        if self.half {
            format!("½ {code}")
        } else {
            code.to_string()
        }
    }

    /// Liest [`Absence::label`] wieder ein (Groß- und Kleinschreibung egal).
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        let kind = AbsenceKind::ALL.into_iter().find(|k| text.starts_with(&k.label().to_lowercase()))?;
        Some(Self::new(kind, text.contains("halb") || text.contains('½')))
    }
}

/// Ein Arbeitstag mit Rohdaten und Korrekturen.
#[derive(Clone, Debug, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    /// Automatisch erfasst (Mac entsperrt), sortiert und überschneidungsfrei.
    pub periods: Vec<Span>,
    /// Manuell festgelegter Arbeitsbeginn.
    pub start: Option<i64>,
    /// Manuell festgelegter Feierabend.
    pub end: Option<i64>,
    /// Manuell eingetragene Pausen.
    pub breaks: Vec<Break>,
    /// Arbeit ohne Mac (nachgetragen oder über „Ohne Mac weiterarbeiten“), sortiert und
    /// überschneidungsfrei.
    pub work: Vec<WorkBlock>,
    /// Abwesenheit (Urlaub, krank, Feiertag, Zeitausgleich) – ändert nur das Soll.
    pub absence: Option<Absence>,
}

/// Zustand eines Tages aus Sicht der manuellen Entscheidungen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Normale, automatische Erfassung.
    Auto,
    /// Feierabend um die angegebene Uhrzeit.
    Closed(i64),
}

/// Kennzahlen eines Tages (das, was in der Tabelle steht).
#[derive(Clone, Debug, PartialEq)]
pub struct DayRecord {
    pub date: NaiveDate,
    /// Beginn (erstes Entsperren oder manuell festgelegt, frühestens Beginn des Rahmens).
    pub first_start: i64,
    /// Ende (letztes Sperren oder manuell festgelegt, spätestens Ende des Rahmens).
    pub last_end: i64,
    /// Arbeitszeit in Sekunden (sekundengenau, etwa für den Export und die Hinweise).
    pub work_secs: i64,
    /// Arbeitszeit zum Anzeigen: Summe der einzelnen Arbeitsabschnitte nach ihren angezeigten
    /// Uhrzeiten (siehe [`shown_len`]). Mit [`DayRecord::shown_pause_secs`] ergibt sie genau
    /// Ende − Beginn wie angezeigt, und Tage summieren sich zur Woche ohne Rundungsreste.
    pub shown_work_secs: i64,
    /// Pausen zum Anzeigen: Summe der einzelnen Pausen nach ihren angezeigten Uhrzeiten
    /// (siehe [`shown_len`]) – passt so zu den Pausen, die einzeln aufgelistet sind.
    pub shown_pause_secs: i64,
}

/// Dauer zwischen den angezeigten Uhrzeiten (ohne Sekunden): `17:00:19 – 17:30` ergibt
/// 30 Min. wie angezeigt, nicht 29.
pub fn shown_len(from: i64, to: i64) -> i64 {
    (to / 60 - from / 60).max(0) * 60
}

impl DayRecord {
    /// Pausen = Zeit zwischen Beginn und Ende, in der nicht gearbeitet wurde (sekundengenau,
    /// etwa für den Export; angezeigt wird [`DayRecord::shown_pause_secs`]).
    pub fn pause_secs(&self) -> i64 {
        ((self.last_end - self.first_start) - self.work_secs).max(0)
    }
}

impl Day {
    pub fn new(date: NaiveDate) -> Self {
        Self { date, periods: Vec::new(), start: None, end: None, breaks: Vec::new(), work: Vec::new(), absence: None }
    }

    pub fn has_corrections(&self) -> bool {
        self.start.is_some()
            || self.end.is_some()
            || !self.breaks.is_empty()
            || !self.work.is_empty()
            || self.absence.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.periods.is_empty() && !self.has_corrections()
    }

    /// Fügt eine automatisch erfasste Periode hinzu (überlappende werden zusammengefasst).
    pub fn add_period(&mut self, from: i64, to: i64) {
        if to < from {
            return;
        }
        self.periods.push(Span::new(from.clamp(0, DAY_END), to.clamp(0, DAY_END)));
        self.periods = union(std::mem::take(&mut self.periods));
    }

    pub fn mode(&self) -> Mode {
        match self.end {
            Some(e) => Mode::Closed(e),
            None => Mode::Auto,
        }
    }

    /// Arbeitsbeginn festlegen (`None`: wieder das erste Entsperren). Arbeit ohne Mac davor
    /// entfällt bzw. beginnt erst dort – die letzte Angabe gilt, wie bei Pausen. Gibt zurück,
    /// was entfallen ist.
    pub fn set_start(&mut self, at: Option<i64>) -> Vec<WorkBlock> {
        self.start = at;
        at.map(|at| self.take_work(0, at)).unwrap_or_default()
    }

    /// Feierabend festlegen (`None`: wieder das letzte Sperren). Arbeit ohne Mac danach
    /// entfällt bzw. endet dort. Gibt zurück, was entfallen ist.
    pub fn set_end(&mut self, at: Option<i64>) -> Vec<WorkBlock> {
        self.end = at;
        at.map(|at| self.take_work(at, DAY_END)).unwrap_or_default()
    }

    /// Arbeit ohne Mac zwischen `from` und `to` entfernen; gibt die entfernten Teile zurück.
    fn take_work(&mut self, from: i64, to: i64) -> Vec<WorkBlock> {
        let taken = self
            .work
            .iter()
            .filter_map(|w| {
                let (from, to) = (w.from.max(from), w.to.min(to));
                (to > from).then(|| WorkBlock { from, to, note: w.note.clone() })
            })
            .collect();
        self.cut_work(from, to);
        taken
    }

    pub fn sort_breaks(&mut self) {
        self.breaks.sort_by_key(|b| (b.from, b.to));
    }

    /// Manuelle Pause eintragen. Arbeit ohne Mac in dieser Zeit entfällt (die letzte
    /// Angabe gilt).
    pub fn add_break(&mut self, from: i64, to: i64) {
        let (from, to) = (from.clamp(0, DAY_END), to.clamp(0, DAY_END));
        if to <= from {
            return;
        }
        self.cut_work(from, to);
        self.breaks.push(Break { from, to });
        self.sort_breaks();
    }

    /// Arbeit ohne Mac zwischen `from` und `to` entfernen (Blöcke werden gekürzt oder geteilt).
    pub fn cut_work(&mut self, from: i64, to: i64) {
        let cut = Span::new(from, to);
        self.work = std::mem::take(&mut self.work)
            .into_iter()
            .flat_map(|w| {
                subtract(vec![w.span()], cut)
                    .into_iter()
                    .map(move |s| WorkBlock { from: s.from, to: s.to, note: w.note.clone() })
            })
            .collect();
    }

    /// Arbeit ohne Mac eintragen. Ältere Arbeit ohne Mac und manuelle Pausen in dieser Zeit
    /// entfallen (die letzte Angabe gilt); angrenzende Blöcke bleiben eigene Abschnitte.
    pub fn add_work(&mut self, from: i64, to: i64, note: &str) {
        let (from, to) = (from.clamp(0, DAY_END), to.clamp(0, DAY_END));
        if to <= from {
            return;
        }
        let cut = Span::new(from, to);
        self.breaks = std::mem::take(&mut self.breaks)
            .into_iter()
            .flat_map(|b| subtract(vec![Span::new(b.from, b.to)], cut))
            .map(|s| Break { from: s.from, to: s.to })
            .collect();
        self.cut_work(from, to);
        self.work.push(WorkBlock { from, to, note: note.trim().to_string() });
        self.work.sort_by_key(|w| (w.from, w.to));
    }

    /// Neue Arbeit ohne Mac `from`–`to` so verlängern, dass keine Lücke und keine manuelle
    /// Pause unter der Mindestpause `min_pause` übrig bleibt:
    /// - Schneidet sie eine manuelle Pause an, gehört ein Rest unter der Mindestpause zur
    ///   neuen Arbeit (sonst bliebe eine Pause, die sich so nicht eintragen lässt).
    /// - Eine Lücke zu benachbarter Arbeit ohne Mac unter der Mindestpause (ohne manuelle
    ///   Pause darin) gehört ebenfalls dazu.
    pub fn join_work_gaps(&self, from: i64, to: i64, min_pause: i64) -> Joined {
        let mut j = Joined { span: Span::new(from, to), gaps: Vec::new(), rests: Vec::new() };
        let short = |g: Span| g.len() > 0 && g.len() < min_pause;
        // Reste angeschnittener manueller Pausen
        for b in &self.breaks {
            let left = Span::new(b.from, from);
            if b.from < from && b.to > from && short(left) {
                j.span.from = j.span.from.min(b.from);
                j.rests.push(left);
            }
            let right = Span::new(to, b.to);
            if b.from < to && b.to > to && short(right) {
                j.span.to = j.span.to.max(b.to);
                j.rests.push(right);
            }
        }
        let (from, to) = (j.span.from, j.span.to);
        let free = |g: Span| short(g) && !self.breaks.iter().any(|b| b.from < g.to && b.to > g.from);
        // Ende der Arbeitszeit davor bzw. Beginn danach – erfasst oder Arbeit ohne Mac
        // (Überlappendes grenzt nach dem Eintragen direkt an).
        let spans = self.unframed_spans(min_pause);
        if let Some(prev) = spans.iter().filter(|w| w.from < from).map(|w| w.to.min(from)).max() {
            let g = Span::new(prev, from);
            if free(g) {
                j.span.from = prev;
                j.gaps.push(g);
            }
        }
        if let Some(next) = spans.iter().filter(|w| w.to > to).map(|w| w.from.max(to)).min() {
            let g = Span::new(to, next);
            if free(g) {
                j.span.to = next;
                j.gaps.push(g);
            }
        }
        j.rests.sort_by_key(|r| r.from);
        j
    }

    /// Pausen des Tages: Lücken zwischen den Arbeitsabschnitten (nach den Einstellungen `s`).
    pub fn pauses(&self, s: &Settings) -> Vec<Span> {
        self.spans(s).windows(2).map(|w| Span::new(w[0].to, w[1].from)).collect()
    }

    /// Arbeit ohne Mac, die im Arbeitszeitrahmen tatsächlich zählt (ohne manuelle Pausen und
    /// außerhalb des Feierabends), mit dem eingetragenen Block, aus dem sie stammt.
    pub fn counted_work(&self, s: &Settings) -> Vec<(Span, &WorkBlock)> {
        let spans = self.spans(s);
        self.work
            .iter()
            .flat_map(|w| {
                spans.iter().filter_map(move |sp| {
                    let c = Span::new(w.from.max(sp.from), w.to.min(sp.to));
                    (c.to > c.from).then_some((c, w))
                })
            })
            .collect()
    }

    /// Arbeitsabschnitte ohne den Arbeitszeitrahmen (Schritte 1–3).
    fn unframed_spans(&self, min_pause: i64) -> Vec<Span> {
        let manual: Vec<Span> = self.work.iter().map(WorkBlock::span).collect();
        let mut base = self.periods.clone();
        base.extend(manual.iter().copied());
        let mut spans = fill_gaps(base, min_pause);
        // Manuelle Angaben fügen nur vor dem ersten Entsperren bzw. nach dem letzten Sperren
        // Zeit hinzu – nie in Lücken dazwischen (das wäre z. B. die gesperrte Mittagszeit).
        // Bezug für Beginn und Feierabend ist das erste Entsperren bzw. letzte Sperren – auch
        // wenn davor bzw. danach Arbeit ohne Mac liegt (sonst verkürzte sie den Tag).
        let unlocked = fill_gaps(self.periods.clone(), min_pause);
        if let Some(s) = self.start {
            // Früherer Beginn: die Zeit bis zum ersten Entsperren zählt mit.
            let first = unlocked.first().or(spans.first()).map(|x| x.from).unwrap_or(self.end.unwrap_or(s));
            if s < first {
                spans.insert(0, Span::new(s, first));
            }
            spans = clip(spans, s, DAY_END);
        }
        if let Some(e) = self.end {
            // Späterer Feierabend: die Zeit nach dem letzten Sperren zählt mit.
            let last = unlocked
                .last()
                .or(spans.last())
                .map(|x| x.to.max(self.start.unwrap_or(0)))
                .unwrap_or(self.start.unwrap_or(e));
            if e > last {
                spans.push(Span::new(last, e));
            }
            spans = clip(spans, 0, e);
        }
        // Ausdrücklich eingetragene Arbeit ohne Mac zählt auch vor dem Beginn bzw. nach dem
        // Feierabend.
        spans.extend(manual);
        let mut spans = union(spans);
        for b in &self.breaks {
            spans = subtract(spans, Span::new(b.from, b.to));
        }
        spans
    }

    /// Effektive Arbeitsabschnitte nach den Einstellungen `s`.
    pub fn spans(&self, s: &Settings) -> Vec<Span> {
        clip(self.unframed_spans(s.min_pause), s.frame_from, s.frame_to)
    }

    /// Arbeitszeit außerhalb des Arbeitszeitrahmens (wird nicht gezählt).
    pub fn outside_frame_spans(&self, s: &Settings) -> Vec<Span> {
        let all = self.unframed_spans(s.min_pause);
        let mut out = clip(all.clone(), 0, s.frame_from);
        out.extend(clip(all, s.frame_to, DAY_END));
        out
    }

    /// Erstes Entsperren und letztes Sperren im Arbeitszeitrahmen (ohne manuelle Angaben;
    /// kurze Sperren zählen wie überall zur Arbeitszeit).
    pub fn auto_bounds_in_frame(&self, s: &Settings) -> Option<(i64, i64)> {
        let spans = clip(fill_gaps(self.periods.clone(), s.min_pause), s.frame_from, s.frame_to);
        Some((spans.first()?.from, spans.last()?.to))
    }

    /// Tabellenzeile für diesen Tag (`None`, wenn es nichts anzuzeigen gibt).
    pub fn record(&self, s: &Settings) -> Option<DayRecord> {
        if self.is_empty() {
            return None;
        }
        let spans = self.spans(s);
        let work: i64 = spans.iter().map(Span::len).sum();
        let (first, last) = match (spans.first(), spans.last()) {
            (Some(f), Some(l)) => (f.from, l.to),
            // Nichts im Rahmen: weder Arbeit noch Pause.
            _ => {
                let first = self
                    .start
                    .or(self.periods.first().map(|p| p.from))
                    .or(self.work.first().map(|w| w.from))
                    .or(self.end)
                    .or(self.breaks.first().map(|b| b.from))
                    .unwrap_or(0)
                    .clamp(s.frame_from, s.frame_to);
                (first, first)
            }
        };
        let shown_work_secs = spans.iter().map(|sp| shown_len(sp.from, sp.to)).sum();
        let shown_pause_secs = spans.windows(2).map(|w| shown_len(w[0].to, w[1].from)).sum();
        Some(DayRecord {
            date: self.date,
            first_start: first,
            last_end: last,
            work_secs: work,
            shown_work_secs,
            shown_pause_secs,
        })
    }

    /// Summe der Pausen von mindestens 15 Minuten zwischen Arbeitsabschnitten
    /// (so zählt das Arbeitszeitgesetz).
    pub fn legal_pause_secs(spans: &[Span]) -> i64 {
        spans
            .windows(2)
            .map(|w| w[1].from - w[0].to)
            .filter(|g| *g >= LEGAL_BREAK_SEGMENT)
            .sum()
    }

    /// Arbeitszeit am Stück bis zum Ende des letzten Abschnitts: seit der letzten Pause von
    /// mindestens 15 Minuten (wie [`Day::longest_stretch_secs`], aber nur der letzte Teil).
    pub fn current_stretch_secs(spans: &[Span]) -> i64 {
        let mut current = 0;
        for (i, sp) in spans.iter().enumerate() {
            if i > 0 && sp.from - spans[i - 1].to >= LEGAL_BREAK_SEGMENT {
                current = 0;
            }
            current += sp.len();
        }
        current
    }

    /// Längste Arbeitszeit am Stück: Arbeitsabschnitte, zwischen denen keine Pause von
    /// mindestens 15 Minuten liegt, zählen zusammen (kürzere Unterbrechungen sind nach dem
    /// Arbeitszeitgesetz keine Pause; mitgezählt wird nur die Arbeit, nicht die Unterbrechung).
    pub fn longest_stretch_secs(spans: &[Span]) -> i64 {
        let (mut longest, mut current) = (0, 0);
        for (i, sp) in spans.iter().enumerate() {
            if i > 0 && sp.from - spans[i - 1].to >= LEGAL_BREAK_SEGMENT {
                current = 0;
            }
            current += sp.len();
            longest = longest.max(current);
        }
        longest
    }
}

/// Schließt Lücken, die kürzer als `min_pause` sind: der Mac war nur kurz gesperrt,
/// die Zeit zählt zum laufenden Arbeitsblock.
pub fn fill_gaps(spans: Vec<Span>, min_pause: i64) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in union(spans) {
        match out.last_mut() {
            Some(last) if s.from - last.to < min_pause => last.to = last.to.max(s.to),
            _ => out.push(s),
        }
    }
    out
}

/// Holt (oder legt an) den Tag `date`; die Liste bleibt nach Datum sortiert.
pub fn day_mut(days: &mut Vec<Day>, date: NaiveDate) -> &mut Day {
    let idx = match days.binary_search_by_key(&date, |d| d.date) {
        Ok(i) => i,
        Err(i) => {
            days.insert(i, Day::new(date));
            i
        }
    };
    &mut days[idx]
}

/// Sortiert und fasst überlappende/angrenzende Abschnitte zusammen.
pub fn union(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by_key(|s| (s.from, s.to));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match out.last_mut() {
            Some(last) if s.from <= last.to => last.to = last.to.max(s.to),
            _ => out.push(s),
        }
    }
    out
}

fn clip(spans: Vec<Span>, lo: i64, hi: i64) -> Vec<Span> {
    spans
        .into_iter()
        .filter_map(|s| {
            let c = Span::new(s.from.max(lo), s.to.min(hi));
            (c.to > c.from).then_some(c)
        })
        .collect()
}

fn subtract(spans: Vec<Span>, cut: Span) -> Vec<Span> {
    if cut.to <= cut.from {
        return spans;
    }
    let mut out = Vec::with_capacity(spans.len() + 1);
    for s in spans {
        if s.to <= cut.from || s.from >= cut.to {
            out.push(s);
            continue;
        }
        if s.from < cut.from {
            out.push(Span::new(s.from, cut.from));
        }
        if s.to > cut.to {
            out.push(Span::new(cut.to, s.to));
        }
    }
    out
}

// ---------- Uhrzeiten ----------

pub fn secs_of(t: NaiveTime) -> i64 {
    t.num_seconds_from_midnight() as i64
}

/// `08:05` (24:00 möglich).
pub fn fmt_clock(secs: i64) -> String {
    let s = secs.clamp(0, DAY_END);
    format!("{:02}:{:02}", s / 3600, (s % 3600) / 60)
}

/// `08:05:30` (24:00:00 möglich).
pub fn fmt_clock_s(secs: i64) -> String {
    let s = secs.clamp(0, DAY_END);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Liest `8:05`, `08:05`, `08:05:30`, `8.05` oder `24:00`.
pub fn parse_clock(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches("Uhr").trim();
    let parts: Vec<&str> = s.split([':', '.']).collect();
    let nums: Vec<i64> = parts.iter().map(|p| p.trim().parse::<i64>().ok()).collect::<Option<_>>()?;
    let (h, m, sec) = match nums.as_slice() {
        [h, m] => (*h, *m, 0),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    if !(0..=24).contains(&h) || !(0..60).contains(&m) || !(0..60).contains(&sec) {
        return None;
    }
    let total = h * 3600 + m * 60 + sec;
    (total <= DAY_END).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(hh: i64, mm: i64) -> i64 {
        hh * 3600 + mm * 60
    }

    fn st() -> Settings {
        Settings::default()
    }

    fn day() -> Day {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(8, 12), h(11, 58));
        d.add_period(h(12, 41), h(17, 30));
        d
    }

    fn work(d: &Day) -> i64 {
        d.record(&st()).unwrap().work_secs
    }

    #[test]
    fn automatic_only() {
        let d = day();
        assert_eq!(work(&d), h(3, 46) + h(4, 49));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(8, 12), h(17, 30)));
        assert_eq!(r.pause_secs(), h(0, 43));
    }

    #[test]
    fn later_start_and_earlier_end_cut_work_without_mac() {
        // Entsperrt ab 09:10; davor ein Termin 08:00–09:00, abends einer 17:00–18:00.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(9, 10), h(16, 0));
        d.add_work(h(8, 0), h(9, 0), "Meeting");
        d.add_work(h(17, 0), h(18, 0), "Telefonat");
        assert_eq!(d.record(&st()).unwrap().first_start, h(8, 0));
        // Beginn 09:20: Der Termin davor entfällt, der Tag beginnt um 09:20.
        let cut = d.set_start(Some(h(9, 20)));
        assert_eq!(cut.iter().map(WorkBlock::describe).collect::<Vec<_>>(), ["Meeting 08:00–09:00"]);
        assert_eq!(d.work.len(), 1);
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(9, 20), h(18, 0)));
        // Feierabend 17:30: Der Termin danach endet dort.
        let cut = d.set_end(Some(h(17, 30)));
        assert_eq!(cut, vec![WorkBlock { from: h(17, 30), to: h(18, 0), note: "Telefonat".to_string() }]);
        assert_eq!(d.record(&st()).unwrap().last_end, h(17, 30));
        // Ein Termin, der über den Beginn reicht, beginnt erst dort.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 17).unwrap());
        d.add_work(h(8, 0), h(10, 0), "");
        assert_eq!(d.set_start(Some(h(9, 20)))[0].describe(), "Arbeit ohne Mac 08:00–09:20");
        assert_eq!(d.work[0].span(), Span::new(h(9, 20), h(10, 0)));
        // Die letzte Angabe gilt: Danach eingetragene Arbeit zählt auch vor dem Beginn.
        d.add_work(h(7, 30), h(8, 0), "Telefonat");
        assert_eq!(d.record(&st()).unwrap().first_start, h(7, 30));
        // Beginn oder Feierabend zurücknehmen kürzt nichts.
        assert!(d.set_start(None).is_empty() && d.set_end(None).is_empty());
        assert_eq!(d.work.len(), 2);
    }

    #[test]
    fn shown_pauses_follow_shown_minutes() {
        // Gesperrt 15:25:40 – 15:45:10: angezeigt 15:25 – 15:45, also 20 Min. Pause (nicht 19).
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap());
        d.add_period(h(8, 0), h(15, 25) + 40);
        d.add_period(h(15, 45) + 10, h(17, 0));
        let r = d.record(&st()).unwrap();
        assert_eq!(r.pause_secs(), h(0, 19) + 30);
        assert_eq!(r.shown_pause_secs, h(0, 20));
        assert_eq!(shown_len(h(17, 0) + 19, h(17, 30)), h(0, 30));
    }

    #[test]
    fn shown_work_follows_shown_minutes() {
        // 08:18:50 – 12:00:30 und 12:59:10 – 18:05:20: angezeigt 08:18 – 18:05 mit 59 Min. Pause,
        // also 8:48 Std. Arbeit – sekundengenau wären es nur 8:47:50.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap());
        d.add_period(h(8, 18) + 50, h(12, 0) + 30);
        d.add_period(h(12, 59) + 10, h(18, 5) + 20);
        let r = d.record(&st()).unwrap();
        assert_eq!(r.work_secs, h(8, 47) + 50);
        assert_eq!(r.shown_pause_secs, h(0, 59));
        assert_eq!(r.shown_work_secs, h(8, 48));
        assert_eq!(r.shown_work_secs + r.shown_pause_secs, shown_len(r.first_start, r.last_end));
    }

    #[test]
    fn short_locks_count_as_work() {
        let mut d = day();
        d.periods = vec![Span::new(h(8, 0), h(10, 0)), Span::new(h(10, 14), h(12, 0)), Span::new(h(12, 15), h(16, 0))];
        // 14 Min. gesperrt: Arbeit; 15 Min.: Pause
        let r = d.record(&st()).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(7, 45), h(0, 15)));
        // mit 20 Min. Mindestpause zählen beide Lücken als Arbeit
        let s = Settings { min_pause: 20 * 60, ..st() };
        let r = d.record(&s).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(8, 0), 0));
        // Rohdaten bleiben unverändert
        assert_eq!(d.periods.len(), 3);
    }

    #[test]
    fn frame_clips_work_and_pause() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(6, 0), h(8, 0));
        d.add_period(h(21, 0), h(23, 30));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(7, 0), h(22, 0)));
        assert_eq!(r.work_secs, h(2, 0));
        assert_eq!(
            d.outside_frame_spans(&st()),
            vec![Span::new(h(6, 0), h(7, 0)), Span::new(h(22, 0), h(23, 30))]
        );
        assert_eq!(d.auto_bounds_in_frame(&st()), Some((h(7, 0), h(22, 0))));
        // nur außerhalb: weder Arbeit noch Pause
        let mut late = Day::new(d.date);
        late.add_period(h(22, 30), h(23, 30));
        let r = late.record(&st()).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (0, 0));
        assert_eq!(late.auto_bounds_in_frame(&st()), None);
        // manueller Beginn vor dem Rahmen zählt erst ab Rahmenbeginn
        let mut early = day();
        early.start = Some(h(6, 30));
        assert_eq!(early.record(&st()).unwrap().first_start, h(7, 0));
        assert_eq!(early.outside_frame_spans(&st()), vec![Span::new(h(6, 30), h(7, 0))]);
    }

    #[test]
    fn earlier_start_adds_time() {
        let mut d = day();
        d.start = Some(h(7, 30));
        assert_eq!(work(&d), h(3, 46) + h(4, 49) + h(0, 42));
        assert_eq!(d.record(&st()).unwrap().first_start, h(7, 30));
    }

    #[test]
    fn later_start_cuts_time() {
        let mut d = day();
        d.start = Some(h(8, 30));
        assert_eq!(work(&d), h(3, 28) + h(4, 49));
    }

    #[test]
    fn start_and_end_inside_gap_add_no_time() {
        // Beginn in der Mittagslücke (gesperrt 11:58–12:41): Arbeit ab 12:41
        let mut d = day();
        d.start = Some(h(12, 30));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.work_secs), (h(12, 41), h(4, 49)));
        // Feierabend in der Mittagslücke: die gesperrte Zeit zählt nicht als Arbeit
        let mut d = day();
        d.end = Some(h(12, 20));
        let r = d.record(&st()).unwrap();
        assert_eq!((r.last_end, r.work_secs), (h(11, 58), h(3, 46)));
    }

    #[test]
    fn break_overlapping_lock_is_not_counted_twice() {
        let mut d = day();
        d.breaks.push(Break { from: h(12, 0), to: h(12, 45) });
        // gesperrt war ohnehin 11:58–12:41, zusätzlich entfällt nur 12:41–12:45
        assert_eq!(work(&d), h(3, 46) + h(4, 45));
    }

    #[test]
    fn end_clips_and_extends() {
        let mut d = day();
        d.end = Some(h(17, 0));
        assert_eq!(d.mode(), Mode::Closed(h(17, 0)));
        assert_eq!(work(&d), h(3, 46) + h(4, 19));
        d.end = Some(h(18, 0));
        assert_eq!(work(&d), h(3, 46) + h(5, 19));
        assert_eq!(d.record(&st()).unwrap().last_end, h(18, 0));
    }

    #[test]
    fn manual_day_without_periods() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap());
        d.start = Some(h(8, 0));
        d.end = Some(h(16, 0));
        d.breaks.push(Break { from: h(12, 0), to: h(12, 30) });
        let r = d.record(&st()).unwrap();
        assert_eq!(r.work_secs, h(7, 30));
        assert_eq!(r.pause_secs(), h(0, 30));
    }

    #[test]
    fn work_without_mac_fills_lock_gap() {
        // Mittags gesperrt 11:58–12:41, dabei 12:00–12:35 telefoniert: Die Reste (2 bzw.
        // 6 Min.) sind kürzer als die Mindestpause und zählen ebenfalls als Arbeit.
        let mut d = day();
        d.add_work(h(12, 0), h(12, 35), "Telefonat");
        let r = d.record(&st()).unwrap();
        assert_eq!((r.work_secs, r.pause_secs()), (h(9, 18), 0));
        assert!(d.has_corrections());
        assert!(d.pauses(&st()).is_empty());
        // Nur ein Teil der Mittagszeit: der Rest bleibt Pause.
        let mut d = day();
        d.add_work(h(12, 0), h(12, 20), "");
        let r = d.record(&st()).unwrap();
        assert_eq!(r.pause_secs(), h(0, 21));
        assert_eq!(d.pauses(&st()), vec![Span::new(h(12, 20), h(12, 41))]);
        let block = WorkBlock { from: h(12, 0), to: h(12, 20), note: String::new() };
        assert_eq!(d.counted_work(&st()), vec![(Span::new(h(12, 0), h(12, 20)), &block)]);
    }

    #[test]
    fn work_without_mac_outside_start_and_end() {
        let mut d = day();
        d.start = Some(h(8, 30));
        d.end = Some(h(17, 0));
        d.add_work(h(7, 45), h(8, 0), "Telefonat");
        d.add_work(h(19, 0), h(19, 30), "Meeting");
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.last_end), (h(7, 45), h(19, 30)));
        assert_eq!(r.work_secs, h(0, 15) + h(3, 28) + h(4, 19) + h(0, 30));
        // ganz ohne Entsperren
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 14).unwrap());
        d.add_work(h(10, 0), h(11, 0), "");
        assert!(!d.is_empty());
        let r = d.record(&st()).unwrap();
        assert_eq!((r.first_start, r.work_secs), (h(10, 0), h(1, 0)));
        // außerhalb des Rahmens: nicht gezählt
        d.add_work(h(22, 0), h(23, 0), "");
        assert_eq!(d.record(&st()).unwrap().work_secs, h(1, 0));
        assert_eq!(d.outside_frame_spans(&st()), vec![Span::new(h(22, 0), h(23, 0))]);
    }

    #[test]
    fn work_blocks_stay_separate_and_latest_entry_wins() {
        let mut d = day();
        d.add_work(h(12, 0), h(12, 20), "Meeting");
        d.add_work(h(12, 20), h(12, 30), "Telefonat");
        // angrenzend: zwei Abschnitte
        assert_eq!(
            d.work,
            vec![
                WorkBlock { from: h(12, 0), to: h(12, 20), note: "Meeting".into() },
                WorkBlock { from: h(12, 20), to: h(12, 30), note: "Telefonat".into() },
            ]
        );
        // überlappend: die letzte Angabe gilt, die älteren werden gekürzt
        d.add_work(h(12, 10), h(12, 25), "Networking");
        assert_eq!(
            d.work.iter().map(|w| (w.from, w.to, w.note.as_str())).collect::<Vec<_>>(),
            vec![(h(12, 0), h(12, 10), "Meeting"), (h(12, 10), h(12, 25), "Networking"), (h(12, 25), h(12, 30), "Telefonat")]
        );
        let mut d = day();
        d.add_work(h(12, 0), h(12, 30), "Meeting");
        // Pause darüber: die Arbeit wird geteilt
        d.add_break(h(12, 10), h(12, 15));
        assert_eq!(
            d.work.iter().map(|w| (w.from, w.to)).collect::<Vec<_>>(),
            vec![(h(12, 0), h(12, 10)), (h(12, 15), h(12, 30))]
        );
        assert_eq!(d.work[1].note, "Meeting");
        assert_eq!(d.breaks, vec![Break { from: h(12, 10), to: h(12, 15) }]);
        // Arbeit darüber: die Pause entfällt
        d.add_work(h(12, 5), h(12, 20), "");
        assert!(d.breaks.is_empty());
        assert_eq!(
            d.work.iter().map(|w| (w.from, w.to)).collect::<Vec<_>>(),
            vec![(h(12, 0), h(12, 5)), (h(12, 5), h(12, 20)), (h(12, 20), h(12, 30))]
        );
        // manuelle Pause teilweise überdeckt
        let mut d = day();
        d.add_break(h(10, 0), h(11, 0));
        d.add_work(h(10, 30), h(11, 30), "");
        assert_eq!(d.breaks, vec![Break { from: h(10, 0), to: h(10, 30) }]);
        // leere oder verkehrte Zeiten werden ignoriert
        let before = d.clone();
        d.add_work(h(9, 0), h(9, 0), "x");
        d.add_break(h(9, 0), h(8, 0));
        assert_eq!(d, before);
    }

    #[test]
    fn short_gaps_to_work_without_mac_join_the_new_block() {
        let mut d = day();
        d.add_work(h(18, 0), h(18, 30), "Meeting");
        d.add_work(h(19, 0), h(19, 30), "Telefonat");
        // 5 Min. Lücke davor, 10 Min. danach: beide kürzer als 15 Min.
        let j = d.join_work_gaps(h(18, 35), h(18, 50), 15 * 60);
        assert_eq!(j.span, Span::new(h(18, 30), h(19, 0)));
        assert_eq!(j.gaps, vec![Span::new(h(18, 30), h(18, 35)), Span::new(h(18, 50), h(19, 0))]);
        assert!(j.rests.is_empty());
        assert_eq!(
            j.note(15 * 60).unwrap(),
            "Zur Arbeitszeit daneben blieben Lücken von 18:30 bis 18:35 (5 Min.) und von 18:50 \
             bis 19:00 (10 Min.). Das ist jeweils kürzer als die Mindestpause von 15 Min.; die Zeit \
             wurde dem neuen Abschnitt dazugerechnet. Er reicht jetzt von 18:30 bis 19:00 Uhr."
        );
        d.add_work(j.span.from, j.span.to, "Networking");
        assert_eq!(d.work.len(), 3, "bleiben eigene Abschnitte");
        // Lücke ab der Mindestpause bleibt; direkt angrenzend gibt es nichts zu ergänzen.
        let j = d.join_work_gaps(h(19, 45), h(20, 0), 15 * 60);
        assert_eq!(j.span, Span::new(h(19, 45), h(20, 0)));
        assert_eq!(j.note(15 * 60), None);
        assert!(d.join_work_gaps(h(19, 30), h(19, 40), 15 * 60).gaps.is_empty());
        // Überlappt der neue Abschnitt einen Block, grenzt er nach dem Kürzen direkt an.
        let j = d.join_work_gaps(h(19, 20), h(19, 40), 15 * 60);
        assert_eq!((j.span, j.gaps.is_empty()), (Span::new(h(19, 20), h(19, 40)), true));
        // Eine ganze manuelle Pause in der Lücke bleibt bestehen.
        d.add_break(h(20, 30), h(20, 45));
        d.add_work(h(20, 45), h(21, 0), "");
        let j = d.join_work_gaps(h(20, 0), h(20, 30), 15 * 60);
        assert_eq!((j.span, j.gaps.is_empty(), j.rests.is_empty()), (Span::new(h(20, 0), h(20, 30)), true, true));
    }

    #[test]
    fn short_gaps_to_recorded_work_join_the_new_block() {
        // Pause 12:42–13:27, darin ein Telefonat 12:42–13:18: Die 9 Min. bis zum Entsperren
        // gehören zum neuen Abschnitt.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(11, 10), h(12, 42));
        d.add_period(h(13, 27), h(15, 55));
        let j = d.join_work_gaps(h(12, 42), h(13, 18), 15 * 60);
        assert_eq!(j.span, Span::new(h(12, 42), h(13, 27)));
        assert_eq!(j.gaps, vec![Span::new(h(13, 18), h(13, 27))]);
        assert_eq!(
            j.note(15 * 60).unwrap(),
            "Zur Arbeitszeit daneben bliebe eine Lücke von 13:18 bis 13:27 (9 Min.). Das ist kürzer \
             als die Mindestpause von 15 Min.; die Zeit wurde dem neuen Abschnitt dazugerechnet. Er \
             reicht jetzt von 12:42 bis 13:27 Uhr."
        );
        // Auch vor dem Abschnitt; ab der Mindestpause bleibt die Lücke eine Pause.
        let j = d.join_work_gaps(h(12, 50), h(13, 5), 15 * 60);
        assert_eq!((j.span, j.gaps.len()), (Span::new(h(12, 42), h(13, 5)), 1));
    }

    #[test]
    fn gaps_under_a_minute_join_without_a_note() {
        // Pause 15:29:10–15:50:40, eingetragen minutengenau 15:29–15:50: Die 40 Sek. bis
        // zum Entsperren gehören dazu, ein Hinweis „von 15:50 bis 15:50“ wäre aber verwirrend.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 17).unwrap());
        d.add_period(h(13, 13), h(15, 29) + 10);
        d.add_period(h(15, 50) + 40, h(17, 16));
        let j = d.join_work_gaps(h(15, 29), h(15, 50), 15 * 60);
        assert_eq!(j.span, Span::new(h(15, 29), h(15, 50) + 40));
        assert_eq!(j.gaps, vec![Span::new(h(15, 50), h(15, 50) + 40)]);
        assert_eq!(j.note(15 * 60), None);
        // Neben einer echten Lücke wird nur diese genannt.
        let j = d.join_work_gaps(h(15, 35), h(15, 50), 15 * 60);
        assert_eq!(j.gaps.len(), 2);
        assert_eq!(
            j.note(15 * 60).unwrap(),
            "Zur Arbeitszeit daneben bliebe eine Lücke von 15:29 bis 15:35 (5 Min.). Das ist kürzer \
             als die Mindestpause von 15 Min.; die Zeit wurde dem neuen Abschnitt dazugerechnet. Er \
             reicht jetzt von 15:29 bis 15:50 Uhr."
        );
    }

    #[test]
    fn rests_of_a_manual_break_join_the_new_block() {
        // Beginn 8:00 vorverlegt, erstes Entsperren 11:31, Pause 9:00–11:00, darin ein
        // Telefonat 9:10–10:50: Es dürfen keine Pausen von 10 Min. übrig bleiben.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap());
        d.add_period(h(11, 31), h(11, 38));
        d.start = Some(h(8, 0));
        d.add_break(h(9, 0), h(11, 0));
        assert_eq!(d.spans(&st()), vec![Span::new(h(8, 0), h(9, 0)), Span::new(h(11, 0), h(11, 38))]);
        let j = d.join_work_gaps(h(9, 10), h(10, 50), 15 * 60);
        assert_eq!(j.span, Span::new(h(9, 0), h(11, 0)));
        assert_eq!(j.rests, vec![Span::new(h(9, 0), h(9, 10)), Span::new(h(10, 50), h(11, 0))]);
        assert_eq!(
            j.note(15 * 60).unwrap(),
            "Von den manuellen Pausen blieben nur von 09:00 bis 09:10 (10 Min.) und von 10:50 bis \
             11:00 (10 Min.) übrig. Das ist jeweils kürzer als die Mindestpause von 15 Min.; die Zeit \
             wurde dem neuen Abschnitt dazugerechnet. Er reicht jetzt von 09:00 bis 11:00 Uhr."
        );
        d.add_work(j.span.from, j.span.to, "Telefonat");
        assert!(d.breaks.is_empty());
        // Die Zeit bis zum ersten Entsperren zählt weiter mit – keine Pause 11:00–11:31.
        assert_eq!(d.spans(&st()), vec![Span::new(h(8, 0), h(11, 38))]);
        // Ein Rest ab der Mindestpause bleibt als Pause stehen.
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 18).unwrap());
        d.add_break(h(9, 0), h(11, 0));
        let j = d.join_work_gaps(h(9, 20), h(10, 50), 15 * 60);
        assert_eq!((j.span, j.rests.clone()), (Span::new(h(9, 20), h(11, 0)), vec![Span::new(h(10, 50), h(11, 0))]));
        d.add_work(j.span.from, j.span.to, "");
        assert_eq!(d.breaks, vec![Break { from: h(9, 0), to: h(9, 20) }]);
    }

    #[test]
    fn manual_end_counts_from_last_lock_even_with_work_after_it() {
        // Letztes Sperren 17:30, danach ein Telefonat 18:00–18:20, Feierabend 19:00: Die ganze
        // Zeit nach dem letzten Sperren zählt (keine Pause 17:30–18:00).
        let mut d = day();
        d.add_work(h(18, 0), h(18, 20), "Telefonat");
        d.end = Some(h(19, 0));
        assert_eq!(d.spans(&st()).last(), Some(&Span::new(h(12, 41), h(19, 0))));
    }

    #[test]
    fn activity_ids_round_trip() {
        for a in Activity::ALL {
            assert_eq!(Activity::from_id(a.id()), a);
            assert_eq!(Activity::from_key(a.key()), a);
        }
        assert_eq!(Activity::from_key("unbekannt"), Activity::Other);
        let labels: Vec<&str> = Activity::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(labels, ["Meeting", "Telefonat", "Networking", "Sonstiges"]);
        assert_eq!(Activity::Networking.note(), "Networking");
        assert_eq!(Activity::Other.note(), "");
    }

    #[test]
    fn legal_pause_counts_segments_from_15_minutes() {
        let spans = vec![Span::new(h(8, 0), h(10, 0)), Span::new(h(10, 14), h(12, 0)), Span::new(h(12, 30), h(15, 0))];
        assert_eq!(Day::legal_pause_secs(&spans), h(0, 30));
    }

    #[test]
    fn periods_merge() {
        let mut d = Day::new(NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
        d.add_period(h(8, 0), h(9, 0));
        d.add_period(h(8, 0), h(9, 30)); // erneut gespeicherte laufende Periode
        d.add_period(h(10, 0), h(11, 0));
        assert_eq!(d.periods, vec![Span::new(h(8, 0), h(9, 30)), Span::new(h(10, 0), h(11, 0))]);
    }

    #[test]
    fn clock_parsing() {
        assert_eq!(parse_clock("8:05"), Some(h(8, 5)));
        assert_eq!(parse_clock("08.05 Uhr"), Some(h(8, 5)));
        assert_eq!(parse_clock("24:00:00"), Some(DAY_END));
        assert_eq!(parse_clock("24:01"), None);
        assert_eq!(parse_clock("abc"), None);
        assert_eq!(fmt_clock_s(DAY_END), "24:00:00");
    }
}
