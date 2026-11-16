//! Fenster "Aufzeichnungen": zeigt die Tabelle aus der Markdown-Datei und erlaubt,
//! einzelne Tage zu korrigieren (Arbeitsbeginn, Pausen, Feierabend).

use crate::day::{fmt_clock, parse_clock, secs_of, Break, Day, DayRecord};
use crate::store::{self, Status};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, NaiveDateTime};
use eframe::egui::{self, Color32, RichText};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(0x2f, 0x7d, 0xd1);
const GREEN: Color32 = Color32::from_rgb(0x2e, 0xa0, 0x5a);
const ORANGE: Color32 = Color32::from_rgb(0xe0, 0x8a, 0x1e);
const BROWN: Color32 = Color32::from_rgb(0xa8, 0x6b, 0x32);
const PURPLE: Color32 = Color32::from_rgb(0x7a, 0x5c, 0xc8);

/// Eingabefelder für den gerade bearbeiteten Tag.
struct Editor {
    date: NaiveDate,
    start: String,
    end: String,
    break_from: String,
    break_to: String,
    error: Option<String>,
}

impl Editor {
    fn new(date: NaiveDate, day: Option<&Day>) -> Self {
        Self {
            date,
            start: day.and_then(|d| d.start).map(fmt_clock).unwrap_or_default(),
            end: day.and_then(|d| d.end).map(fmt_clock).unwrap_or_default(),
            break_from: String::new(),
            break_to: String::new(),
            error: None,
        }
    }
}

/// Was im Editor geklickt wurde (wird nach dem Zeichnen ausgeführt).
enum Edit {
    SetStart(Option<i64>),
    SetEnd(Option<i64>),
    AddBreak(i64, i64),
    RemoveBreak(usize),
}

struct RecordsApp {
    days: Vec<Day>,
    status: Option<Status>,
    error: Option<String>,
    loaded_at: Instant,
    editor: Option<Editor>,
    goto_date: String,
    screenshot: Option<PathBuf>,
    frames: u32,
    shot_requested: bool,
    shot_done: Arc<AtomicBool>,
}

fn now() -> NaiveDateTime {
    Local::now().naive_local()
}

impl RecordsApp {
    fn new(screenshot: Option<PathBuf>) -> Self {
        let mut app = Self {
            days: Vec::new(),
            status: None,
            error: None,
            loaded_at: Instant::now(),
            editor: None,
            goto_date: String::new(),
            screenshot,
            frames: 0,
            shot_requested: false,
            shot_done: Arc::new(AtomicBool::new(false)),
        };
        app.reload();
        // Zum Testen/für Screenshots: TAGWERK_SELECT=latest oder =TT.MM.JJJJ
        if let Ok(sel) = std::env::var("TAGWERK_SELECT") {
            let date = if sel == "latest" { app.days.last().map(|d| d.date) } else { store::parse_date(&sel) };
            if let Some(date) = date {
                app.select(date);
            }
        }
        app
    }

    fn reload(&mut self) {
        match store::load() {
            Ok(d) => {
                self.days = d;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Datei konnte nicht gelesen werden: {e}")),
        }
        self.status = store::read_status();
        self.loaded_at = Instant::now();
    }

    fn day(&self, date: NaiveDate) -> Option<&Day> {
        self.days.iter().find(|d| d.date == date)
    }

    fn records(&self) -> Vec<(&Day, DayRecord)> {
        let t = now();
        self.days
            .iter()
            .filter_map(|d| d.record(store::open_until(d.date, t)).map(|r| (d, r)))
            .collect()
    }

    fn sum_where(&self, f: impl Fn(NaiveDate) -> bool) -> (i64, i64, usize) {
        self.records()
            .iter()
            .filter(|(_, r)| f(r.date))
            .fold((0, 0, 0), |(w, p, n), (_, r)| (w + r.work_secs, p + r.pause_secs(), n + 1))
    }

    fn select(&mut self, date: NaiveDate) {
        if self.editor.as_ref().is_some_and(|e| e.date == date) {
            self.editor = None;
        } else {
            self.editor = Some(Editor::new(date, self.day(date)));
        }
    }

    /// Prüft eine Uhrzeit-Eingabe für `date` (heute nicht in der Zukunft).
    fn check_time(date: NaiveDate, text: &str) -> Result<i64, String> {
        let secs = parse_clock(text).ok_or_else(|| format!("„{}“ ist keine Uhrzeit (z. B. 08:30).", text.trim()))?;
        let t = now();
        if date == t.date() && secs > secs_of(t.time()) {
            return Err(format!("{} Uhr liegt in der Zukunft.", fmt_clock(secs)));
        }
        Ok(secs)
    }

    fn apply(&mut self, edit: Edit) {
        let Some(date) = self.editor.as_ref().map(|e| e.date) else { return };
        let result = store::edit_day(date, now(), |d| match edit {
            Edit::SetStart(s) => d.start = s,
            Edit::SetEnd(e) => d.end = e,
            Edit::AddBreak(from, to) => {
                d.breaks.push(Break { from, to: Some(to) });
                d.sort_breaks();
            }
            Edit::RemoveBreak(i) => {
                if i < d.breaks.len() {
                    d.breaks.remove(i);
                }
            }
        });
        self.reload();
        let day = self.day(date).cloned();
        if let Some(ed) = self.editor.as_mut() {
            match result {
                Ok(()) => *ed = Editor::new(date, day.as_ref()),
                Err(e) => ed.error = Some(format!("Speichern fehlgeschlagen: {e}")),
            }
        }
    }

    fn editor_panel(&mut self, ui: &mut egui::Ui) -> Option<Edit> {
        let date = self.editor.as_ref()?.date;
        let day = self.day(date).cloned();
        let t = now();
        let weak = ui.visuals().weak_text_color();
        let mut edit = None;
        let mut close = false;
        let ed = self.editor.as_mut()?;

        ui.horizontal(|ui| {
            ui.label(RichText::new(store::fmt_date(date)).size(18.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Schließen").clicked() {
                    close = true;
                }
            });
        });
        match day.as_ref().and_then(|d| d.record(store::open_until(date, t))) {
            Some(r) => ui.label(format!(
                "Arbeitszeit {}  ·  Pausen {}",
                store::fmt_hm(r.work_secs),
                store::fmt_hm(r.pause_secs())
            )),
            None => ui.label(RichText::new("Noch keine Daten für diesen Tag.").color(weak)),
        };
        ui.add_space(8.0);

        ui.label(RichText::new("AUTOMATISCH ERFASST").small().color(weak));
        let periods: Vec<String> = day
            .as_ref()
            .map(|d| d.periods.iter().map(|p| format!("{}–{}", fmt_clock(p.from), fmt_clock(p.to))).collect())
            .unwrap_or_default();
        if periods.is_empty() {
            ui.label(RichText::new("keine (Mac war nicht entsperrt)").color(weak));
        } else {
            ui.label(RichText::new(periods.join(",  ")).monospace().color(weak));
        }
        ui.add_space(10.0);
        ui.separator();

        let field = |ui: &mut egui::Ui, text: &mut String, hint: &str| {
            ui.add(egui::TextEdit::singleline(text).desired_width(56.0).hint_text(hint.to_string()))
        };

        ui.label(RichText::new("ARBEITSBEGINN").small().color(weak));
        ui.horizontal(|ui| {
            let r = field(ui, &mut ed.start, "08:00");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Übernehmen").clicked() || enter {
                match Self::check_time(date, &ed.start) {
                    Ok(s) => edit = Some(Edit::SetStart(Some(s))),
                    Err(e) => ed.error = Some(e),
                }
            }
            if day.as_ref().is_some_and(|d| d.start.is_some()) && ui.button("Entfernen").clicked() {
                edit = Some(Edit::SetStart(None));
            }
        });
        let auto_first = day.as_ref().and_then(|d| d.periods.first()).map(|p| fmt_clock(p.from));
        ui.label(
            RichText::new(match auto_first {
                Some(a) => format!("Ohne Angabe: erstes Entsperren ({a} Uhr)"),
                None => "Ohne Angabe: erstes Entsperren".to_string(),
            })
            .small()
            .color(weak),
        );
        ui.add_space(8.0);

        ui.label(RichText::new("PAUSEN").small().color(weak));
        if let Some(d) = day.as_ref() {
            for (i, b) in d.breaks.iter().enumerate() {
                ui.horizontal(|ui| {
                    let to = b.to.map(fmt_clock).unwrap_or_else(|| "läuft".into());
                    ui.label(RichText::new(format!("{} – {}", fmt_clock(b.from), to)).monospace());
                    if ui.small_button("Entfernen").clicked() {
                        edit = Some(Edit::RemoveBreak(i));
                    }
                });
            }
        }
        ui.horizontal(|ui| {
            field(ui, &mut ed.break_from, "12:00");
            ui.label("–");
            field(ui, &mut ed.break_to, "12:30");
            if ui.button("Hinzufügen").clicked() {
                match (Self::check_time(date, &ed.break_from), Self::check_time(date, &ed.break_to)) {
                    (Ok(f), Ok(to)) if to > f => edit = Some(Edit::AddBreak(f, to)),
                    (Ok(_), Ok(_)) => ed.error = Some("Das Ende der Pause muss nach dem Beginn liegen.".into()),
                    (Err(e), _) | (_, Err(e)) => ed.error = Some(e),
                }
            }
        });
        ui.add_space(8.0);

        ui.label(RichText::new("FEIERABEND").small().color(weak));
        ui.horizontal(|ui| {
            let r = field(ui, &mut ed.end, "17:00");
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Übernehmen").clicked() || enter {
                match Self::check_time(date, &ed.end) {
                    Ok(s) => edit = Some(Edit::SetEnd(Some(s))),
                    Err(e) => ed.error = Some(e),
                }
            }
            if day.as_ref().is_some_and(|d| d.end.is_some()) && ui.button("Entfernen").clicked() {
                edit = Some(Edit::SetEnd(None));
            }
        });
        ui.label(
            RichText::new("Ohne Angabe: letztes Sperren. Ein späterer Feierabend wird mitgezählt.")
                .small()
                .color(weak),
        );

        if let Some(err) = &ed.error {
            ui.add_space(8.0);
            ui.colored_label(Color32::from_rgb(0xd0, 0x3b, 0x3b), err);
        }
        if close {
            self.editor = None;
        } else if edit.is_some() {
            ed.error = None;
        }
        edit
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frames += 1;
        ctx.request_repaint();
        if self.shot_done.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.frames >= 120 && !self.shot_requested {
            self.shot_requested = true;
            let done = self.shot_done.clone();
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                    Some(buf) => {
                        if let Err(e) = buf.save(&path) {
                            eprintln!("Screenshot konnte nicht gespeichert werden: {e}");
                        }
                    }
                    None => eprintln!("Screenshot: ungültige Bildgröße"),
                }
                done.store(true, Ordering::SeqCst);
            });
        }
    }
}

fn card(ui: &mut egui::Ui, title: &str, work: i64, pause: i64, extra: &str) {
    egui::Frame::group(ui.style())
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(150.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).small().color(ui.visuals().weak_text_color()));
                ui.label(RichText::new(store::fmt_hm(work)).size(22.0).strong());
                ui.label(RichText::new(format!("Pausen {}", store::fmt_hm(pause))).small());
                if !extra.is_empty() {
                    ui.label(RichText::new(extra).small().color(ui.visuals().weak_text_color()));
                }
            });
        });
}

impl eframe::App for RecordsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_screenshot(&ctx);
        if self.loaded_at.elapsed() > Duration::from_secs(20) {
            self.reload();
        }
        ctx.request_repaint_after(Duration::from_secs(5));

        let today = Local::now().date_naive();

        egui::Panel::top("header")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 14)).fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Aufzeichnungen").size(24.0).strong());
                    ui.add_space(12.0);
                    let (txt, col) = match self.status {
                        Some(Status::Running) => ("●  Zeiterfassung läuft", GREEN),
                        Some(Status::Paused) => ("⏸  Zeiterfassung pausiert", ORANGE),
                        Some(Status::Break) => ("☕  Mittagspause", BROWN),
                        Some(Status::Closed) => ("✓  Feierabend", PURPLE),
                        None => ("○  Menüleisten-App nicht aktiv", ui.visuals().weak_text_color()),
                    };
                    ui.label(RichText::new(txt).color(col).strong());
                });
                ui.label(
                    RichText::new(store::data_file().display().to_string())
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(10.0);

                // Übersicht
                let (tw, tp, _) = self.sum_where(|d| d == today);
                let week_start = today - ChronoDuration::days(today.weekday().num_days_from_monday() as i64);
                let (ww, wp, wn) = self.sum_where(|d| d >= week_start && d <= today);
                let (mw, mp, mn) =
                    self.sum_where(|d| d.year() == today.year() && d.month() == today.month());
                let avg = if mn > 0 { mw / mn as i64 } else { 0 };
                ui.horizontal_wrapped(|ui| {
                    let begin = self
                        .records()
                        .iter()
                        .find(|(_, r)| r.date == today)
                        .map(|(d, r)| {
                            let manual = if d.start.is_some() { " (manuell)" } else { "" };
                            format!("Beginn {} Uhr{manual}", fmt_clock(r.first_start))
                        })
                        .unwrap_or_else(|| "noch kein Beginn".into());
                    card(ui, "HEUTE", tw, tp, &begin);
                    card(ui, "DIESE WOCHE", ww, wp, &format!("{wn} Arbeitstag(e)"));
                    card(ui, "DIESER MONAT", mw, mp, &format!("{mn} Arbeitstag(e)"));
                    card(ui, "Ø PRO TAG (MONAT)", avg, if mn > 0 { mp / mn as i64 } else { 0 }, "pro Arbeitstag");
                });
            });

        let mut goto: Option<NaiveDate> = None;
        egui::Panel::bottom("footer")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 10)).fill(ui.visuals().panel_fill))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("⟳  Neu laden").clicked() {
                        self.reload();
                    }
                    if ui.button("Markdown-Datei öffnen").clicked() {
                        let _ = std::process::Command::new("/usr/bin/open").arg(store::data_file()).spawn();
                    }
                    if ui.button("Im Finder zeigen").clicked() {
                        let _ = std::process::Command::new("/usr/bin/open")
                            .arg("-R")
                            .arg(store::data_file())
                            .spawn();
                    }
                    ui.separator();
                    ui.label("Tag nachtragen:");
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.goto_date)
                            .desired_width(84.0)
                            .hint_text("TT.MM.JJJJ"),
                    );
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Öffnen").clicked() || enter {
                        match store::parse_date(&self.goto_date) {
                            Some(d) if d <= today => goto = Some(d),
                            Some(_) => self.error = Some("Zukünftige Tage lassen sich nicht bearbeiten.".into()),
                            None => self.error = Some(format!("„{}“ ist kein Datum (TT.MM.JJJJ).", self.goto_date)),
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("Backups: letzte {} Tage", store::MAX_BACKUPS))
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });
            });
        if let Some(d) = goto {
            self.error = None;
            self.goto_date.clear();
            self.editor = None;
            self.select(d);
        }

        if self.editor.is_some() {
            let mut edit = None;
            egui::Panel::right("editor")
                .resizable(false)
                .exact_size(340.0)
                .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)).fill(ui.visuals().panel_fill))
                .show(ui, |ui| {
                    edit = self.editor_panel(ui);
                });
            if let Some(e) = edit {
                self.apply(e);
            }
        }

        let selected = self.editor.as_ref().map(|e| e.date);
        let mut clicked: Option<NaiveDate> = None;
        egui::CentralPanel::default_margins().show(ui, |ui| {
            if let Some(err) = &self.error {
                ui.colored_label(Color32::RED, err);
            }
            let records = self.records();
            if records.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Noch keine Aufzeichnungen").size(18.0).strong());
                    ui.label("Sobald die Menüleisten-App läuft und der Mac entsperrt ist, wird hier jeder Arbeitstag eingetragen.");
                });
                return;
            }
            ui.label(
                RichText::new("Tag anklicken, um Arbeitsbeginn, Pausen oder Feierabend zu korrigieren.  * = manuell")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                egui::Grid::new("records")
                    .striped(true)
                    .num_columns(5)
                    .spacing([24.0, 8.0])
                    .min_col_width(70.0)
                    .show(ui, |ui| {
                        for h in ["Datum", "Beginn", "Ende", "Arbeitszeit", "Pausenzeit"] {
                            ui.label(RichText::new(h).strong().color(ACCENT));
                        }
                        ui.end_row();
                        for (d, r) in records.iter().rev() {
                            let is_today = r.date == today;
                            let mut date_txt = RichText::new(store::fmt_date(r.date));
                            if is_today {
                                date_txt = date_txt.strong().color(ACCENT);
                            }
                            if ui.selectable_label(selected == Some(r.date), date_txt).clicked() {
                                clicked = Some(r.date);
                            }
                            let mark = |manual: bool| if manual { " *" } else { "" };
                            ui.label(format!("{}{}", fmt_clock(r.first_start), mark(d.start.is_some())));
                            let end = if is_today && d.end.is_none() {
                                match (self.status, d.open_break()) {
                                    (_, Some(_)) => "Pause".to_string(),
                                    (Some(Status::Running), _) => "läuft".to_string(),
                                    _ => fmt_clock(r.last_end),
                                }
                            } else {
                                format!("{}{}", fmt_clock(r.last_end), mark(d.end.is_some()))
                            };
                            ui.label(end);
                            ui.label(RichText::new(store::fmt_hms(r.work_secs)).monospace().strong());
                            let pause = format!("{}{}", store::fmt_hms(r.pause_secs()), mark(!d.breaks.is_empty()));
                            ui.label(RichText::new(pause).monospace());
                            ui.end_row();
                        }
                    });
            });
        });
        if let Some(d) = clicked {
            self.select(d);
        }
    }
}

pub fn run(screenshot: Option<PathBuf>) -> eframe::Result {
    let shot = screenshot.is_some();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Tagwerk – Aufzeichnungen")
        .with_inner_size([1080.0, 600.0])
        .with_min_inner_size([760.0, 400.0]);
    if shot {
        viewport = viewport.with_active(false);
        #[cfg(target_os = "windows")]
        {
            viewport = viewport.with_window_level(egui::WindowLevel::AlwaysOnBottom);
        }
    }
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(move |builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Regular);
            builder.with_activate_ignoring_other_apps(!shot);
        }));
    }
    eframe::run_native(
        "Tagwerk – Aufzeichnungen",
        options,
        Box::new(move |_cc| Ok(Box::new(RecordsApp::new(screenshot)))),
    )
}
