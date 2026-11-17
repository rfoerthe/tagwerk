# Changelog

Alle wesentlichen Änderungen an diesem Projekt werden hier festgehalten. Das Format
folgt [Keep a Changelog](https://keepachangelog.com/de/1.1.0/), die Versionsnummern
folgen der [Semantischen Versionierung](https://semver.org/lang/de/).

## [Unveröffentlicht]

## [1.1.0] - 2026-11-17

### Zusammenfassung
Das Fenster „Aufzeichnungen“ öffnet sich jetzt direkt in der laufenden App – schneller,
und Korrekturen erscheinen sofort in der Menüleiste. Neu ist das Fenster „Über
Tagwerk“; „Bei Anmeldung starten“ ist in die Einstellungen umgezogen.

### Hinzugefügt
- Fenster „Über Tagwerk“ mit Version, Lizenz und Link zum Projekt – im Menü der
  Menüleiste unter „Einstellungen …“ und im Menü „Tagwerk“ des Fensters
  „Aufzeichnungen“.

### Geändert
- „Bei Anmeldung starten“ ist kein Menüpunkt der Menüleiste mehr, sondern ein
  Ankreuzfeld im Fenster „Einstellungen“. „Standardwerte“ lässt es unverändert.
- Das Fenster „Aufzeichnungen“ öffnet sich in der laufenden App statt in einem zweiten
  Prozess. Es erscheint schneller, und Korrekturen gelten in der Menüleiste sofort.
  Solange es offen ist, hat Tagwerk ein Symbol im Dock; ⌘Q schließt weiterhin nur das
  Fenster.

## [1.0.0] - 2026-11-17

### Zusammenfassung
Die erste Version von Tagwerk: eine Menüleisten-App für macOS, die die Arbeitszeit
automatisch erfasst, solange der Mac entsperrt ist. Sie prüft Pausen, Ruhezeit und
Höchstarbeitszeit nach dem Arbeitszeitgesetz, rechnet Soll und Saldo aus und bietet
dazu das Fenster „Aufzeichnungen“, eine Wochenansicht, Einstellungen und einen Export
als Markdown. Alle Daten bleiben lokal auf dem Mac.

### Hinzugefügt
- Wochenansicht: Jeder Tageskopf zeigt neben der Arbeitszeit auch die Pausen des Tages
  (☕︎ 0:45 h), die Kopfzeile die Summe der Pausen der Woche.
- Automatische Erfassung über Sperren, Entsperren, Ruhezustand und Benutzerwechsel.
  Jede Sperre ab der Mindestpause (Standard 15 Minuten) gilt als Pause, kürzere
  Unterbrechungen zählen zur Arbeitszeit.
- Menüleiste mit Status, Arbeitszeit von heute samt Soll, Pausen, Beginn, Saldo der
  Woche und Hinweisen auf Regelverstöße. Der Status ist farbig: grün für „läuft“,
  orange für Pause, indigo für Feierabend.
- „Arbeit unterbrechen“ und „Weiter arbeiten“ (⌘P bei geöffnetem Menü) halten die
  Erfassung an, auch wenn der Mac entsperrt bleibt. Die Pause übersteht einen Neustart
  der App und endet beim Abmelden oder Neustarten des Macs.
- Arbeitsbeginn und Feierabend lassen sich „jetzt“, „vor N Minuten“ oder zu einer
  frei gewählten Uhrzeit festlegen. Korrekturen liegen als eigene Schicht über den
  erfassten Zeiten und lassen sich jederzeit zurücknehmen.
- Natives Fenster „Aufzeichnungen“ (⌘A) mit Kennzahlen für heute, Woche, Monat und
  Durchschnitt pro Arbeitstag, einer Tabelle aller Tage nach Kalenderwochen und einem
  Inspektor zum Korrigieren von Beginn, Pausen und Feierabend – auch für Tage ohne
  Aufzeichnung. Folgt dem Hell- und Dunkelmodus von macOS.
- Wochenansicht (⌘W) im Stil eines Kalenders: je Tag eine Spalte, jeder
  Arbeitsabschnitt ein Block. Blättern mit ‹ › oder den Pfeiltasten, „Heute“ (⌘T),
  Uhrzeiten per Klick in die Zwischenablage kopieren, Hinweise per Klick auf den
  Tageskopf. Arbeit außerhalb des Arbeitszeitrahmens erscheint orange als
  „nicht gezählt“.
- Einstellungen für Arbeitszeitrahmen (Standard Mo–Fr 07:00–22:00 Uhr),
  Regelwochenarbeitszeit (Standard 39 Stunden), Mindestpause und Symbolsatz der
  Menüleiste. Änderungen gelten sofort, auch für bereits erfasste Tage.
- Neun selbst gezeichnete Symbolsätze für die Menüleiste: Tagbogen, Schlossuhr,
  Tagesring, Bauarbeiter, Handwerkerin, Kaffeetasse, Hamsterrad, Roboter und
  Kaninchen – mit Vorschau aller Zustände in den Einstellungen.
- Regelprüfung nach Arbeitszeitgesetz: Pausen nach § 4 ArbZG, Ruhezeit von 11 Stunden
  nach § 5 ArbZG, Höchstarbeitszeit von 10 Stunden sowie Arbeit am Wochenende und
  außerhalb des Arbeitszeitrahmens. Verstöße erscheinen in Menüleiste, Tabelle,
  Inspektor, Wochenansicht und Export.
- Soll und Saldo je Tag, Woche und Monat. Der Saldo zählt nur Tage, an denen gearbeitet
  wurde – Urlaub, Feiertage und Krankheit drücken ihn nicht.
- Export eines frei wählbaren Zeitraums als Markdown (⌘E) mit Übersichtstabelle,
  Hinweisen, Korrekturen und erfassten Perioden. Ein Export lässt sich mit
  `Tagwerk --import <datei.md>` wieder einlesen.
- Kommandozeile: `--records`, `--export`, `--import` sowie `--screenshot`,
  `--week-screenshot` und `--settings-screenshot` für Bilder der Fenster.
- Lokale SQLite-Datenbank unter `~/Library/Application Support/Tagwerk/tagwerk.db` mit
  täglichem Backup vor der ersten Änderung eines Tages; die letzten 15 Backups bleiben
  erhalten.
- Einmalige Übernahme der Daten aus `tagwerk.md` früherer Entwicklungsstände und aus
  der Vorgängerversion „TrackMyWork“.
- „Bei Anmeldung starten“ trägt die installierte App als Anmeldeobjekt ein.
- `build.sh` baut die App für Apple Silicon, packt sie in ein DMG, signiert und
  notarisiert es und installiert sie auf Wunsch nach `/Applications`.
- `github-release.sh` veröffentlicht ein geprüftes DMG als Release auf GitHub.

[Unveröffentlicht]: https://github.com/rfoerthe/tagwerk/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/rfoerthe/tagwerk/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/rfoerthe/tagwerk/releases/tag/v1.0.0
