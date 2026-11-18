# Changelog

Alle wesentlichen Änderungen an diesem Projekt werden hier festgehalten. Das Format
folgt [Keep a Changelog](https://keepachangelog.com/de/1.1.0/), die Versionsnummern
folgen der [Semantischen Versionierung](https://semver.org/lang/de/).

## [Unveröffentlicht]

### Hinzugefügt
- Mitteilungen zur Höchstarbeitszeit über die Mitteilungszentrale von macOS: bei 9:45 Std.
  Arbeit „Noch 15 Min. bis 10 Std. Arbeitszeit“, bei 10 Std. „10 Std. Arbeitszeit
  erreicht“. Jede Schwelle wird einmal am Tag gemeldet, sobald sie während der Arbeit
  (auch ohne Mac) überschritten wird – nicht für nachträglich eingetragene Zeit und nicht
  noch einmal nach einem Neustart der App. Beim ersten Start fragt macOS nach der
  Erlaubnis; nur in der installierten App (`Tagwerk.app`).
- Die Einstellungen zeigen unter „Mitteilungen“, ob Tagwerk Mitteilungen senden darf
  (erlaubt, nicht erlaubt, ausgeblendet oder nur in der installierten App). Ist die
  Erlaubnis nicht erteilt, öffnet „Systemeinstellungen öffnen“ direkt die
  Mitteilungseinstellungen von Tagwerk. Der Stand wird jedes Mal abgefragt, wenn das
  Fenster aktiv wird – auch nach der Rückkehr aus den Systemeinstellungen.
- `--settings-screenshot`: Stand der Mitteilungen zur Ansicht über
  `TAGWERK_NOTIFICATIONS=allowed|denied|hidden|undetermined`.

## [1.3.0] - 2026-11-18

### Zusammenfassung
Die Wochenansicht ist jetzt Teil des Fensters „Aufzeichnungen“ und über den Umschalter
„Tage | Woche“ erreichbar. Arbeitszeit und Pausen zählen überall nach den angezeigten
Uhrzeiten, sodass sich Tage, Wochen und Salden ohne Rundungsreste aufsummieren. Die
Kennzahlen im Menü sind reine Anzeige, und Daten von „TrackMyWork“ werden nicht mehr
übernommen.

### Geändert
- Status, Kennzahlen (Heute gearbeitet, Pausen heute, Beginn heute, Saldo diese Woche)
  und Hinweise auf Regelverstöße im Menü sind reine Anzeigefelder: Sie lassen sich nicht
  mehr anklicken und werden beim Überfahren nicht hervorgehoben; Farben und Schrift
  bleiben gleich. Bisher öffnete ein Klick den heutigen Tag im Fenster „Aufzeichnungen“.
- Die Wochenansicht ist jetzt Teil des Fensters „Aufzeichnungen“ statt eines eigenen
  Fensters. Der Umschalter „Tage | Woche“ in der Symbolleiste wechselt zwischen der
  Tabelle mit Inspektor und der Woche als Kalender, ebenso das Menü „Darstellung“ mit
  „Tage“ (⌘1) und „Woche“ (⌘2); „Heute“ (⌘T) springt zur aktuellen Woche. Das Fenster
  öffnet in der zuletzt benutzten Ansicht.
- Ein Wechsel von „Tage“ zu „Woche“ zeigt die Woche des ausgewählten Tags.
- In der Ansicht „Woche“ ersetzt die Wochenleiste (‹ › Heute, Kalenderwoche, Hinweise,
  Wochensumme) die Übersichtskarten; die Fußzeile bleibt.
- Auch eigenständig (`Tagwerk --records`) gibt es die Wochenansicht; Arbeit ohne Mac
  wird dann direkt in der Datenbank eingetragen bzw. gelöscht.
- `--week-screenshot` speichert das Fenster „Aufzeichnungen“ in der Ansicht „Woche“.
- Ein Klick auf einen freien Bereich der Woche öffnet den Dialog „Arbeit ohne Mac
  eintragen“ sofort (bisher erst nach der Doppelklick-Zeit).

### Behoben
- „Pause als Arbeitszeit zählen“ in der Menüleiste zeigt die Dauer einer Pause jetzt wie
  die Wochenansicht nach den angezeigten Uhrzeiten (15:25 – 15:45 sind 20 Min., bisher
  je nach Sekunden 19 Min.).
- Auch die Pausen eines Tages zählen jetzt nach den angezeigten Uhrzeiten: Die Summe ist
  die Summe der einzelnen Pausen – im Tageskopf der Wochenansicht, in der Spalte
  „Pausen“, im Inspektor, in den Übersichtskarten und unter „Pausen heute“ in der
  Menüleiste. Bisher zeigte der Tageskopf z. B. 0:19 h neben einer Pause von 0:20 h.
  Der Export bleibt sekundengenau.
- Ebenso die Arbeitszeit: Sie ist die Summe der Arbeitsabschnitte nach ihren angezeigten
  Uhrzeiten. So ergibt Ende − Beginn − Pausen genau die Arbeitszeit, die Tage einer Woche
  summieren sich genau zur Wochenzeile, und die Salden der Tage zum Saldo der Woche.
  Bisher wichen sie wegen abgeschnittener Sekunden um 1–2 Min. ab (z. B. 08:18 – 18:05
  mit 0:59 h Pause als 8:47 h statt 8:48 h). Der Export bleibt sekundengenau.

### Entfernt
- Die Übernahme der Daten aus der Vorgängerversion „TrackMyWork“
  (`~/Library/Application Support/TrackMyWork`). Sie lief bei jeder neu angelegten
  Datenbank erneut, sodass alte Tage nach dem Löschen von `tagwerk.db` wieder
  auftauchten.
- Der Menüpunkt „Wochenansicht …“ (⌘W) in der Menüleiste; die Woche ist über
  „Aufzeichnungen …“ und den Umschalter „Tage | Woche“ erreichbar.
- Der Doppelklick auf einen Tag in der Wochenansicht (öffnete den Tag im Fenster
  „Aufzeichnungen“).

## [1.2.0] - 2026-11-18

### Zusammenfassung
Telefonate, Meetings und Termine bei gesperrtem Mac zählen jetzt als Arbeitszeit:
„Ohne Mac weiterarbeiten“ in der Menüleiste, nachträglich per Klick in der
Wochenansicht oder im Fenster „Aufzeichnungen“. Die Datenbank wird dafür auf Schema 3
erweitert – ältere Tagwerk-Versionen können sie danach nicht mehr öffnen.

### Hinzugefügt
- **Arbeit ohne Mac:** Telefonate, Meetings oder Termine bei gesperrtem Mac zählen jetzt
  als Arbeitszeit. Sie liegen als eigene Korrektur über den erfassten Perioden und lassen
  sich jederzeit wieder entfernen.
  - Menüleiste: „Ohne Mac weiterarbeiten“ (Meeting, Telefonat, Networking, Sonstiges) zählt die Zeit
    bis zum nächsten Entsperren bzw. „Zurück am Mac“ – auch wenn der Mac dazwischen
    gesperrt ist oder schläft; spätestens um Mitternacht oder mit dem Feierabend endet es.
  - Menüleiste: „Pause als Arbeitszeit zählen“ listet die Pausen von heute.
  - Inspektor im Fenster „Aufzeichnungen“: neuer Abschnitt „Arbeit ohne Mac“ mit Grund
    (Auswahlbox Meeting, Telefonat, Networking, Sonstiges).
  - Wochenansicht: Arbeit ohne Mac ist schraffiert und mit der Art beschriftet, wenn Platz
    ist. Ein Klick auf einen freien Bereich öffnet einen Dialog mit Beginn, Ende und
    Grund – in einer Pause ist die ganze Pause vorgeschlagen, vor dem Arbeitsbeginn und
    nach dem Feierabend eine halbe Stunde. Über einem freien Bereich zeigen Zeigehand und
    Rahmen, was ein Klick vorschlägt. Pausen zeigen ihre Dauer, wenn Platz ist.
  - Wochenansicht: Ein Klick auf die Schraffur löscht Arbeit ohne Mac wieder; dort gilt
    dann wieder das Erfasste (gesperrt: Pause, entsperrt: Arbeitszeit). Ist es die gerade
    laufende Arbeit ohne Mac, endet sie ohne Eintrag und die Erfassung läuft weiter.
  - Export und Import: Art „Arbeit ohne Mac“ mit neuer Spalte „Notiz“ unter „Korrekturen“.
  - Einstellungen: Mindestdauer für Arbeit ohne Mac (Standard 15 Minuten, nie kürzer als
    die Mindestpause). Kürzere Abschnitte lehnen der Dialog der Wochenansicht und der
    Inspektor ab; ein Vorschlag vor dem Arbeitsbeginn oder nach dem Feierabend ist
    mindestens so lang.

### Geändert
- Eine manuelle Pause und Arbeit ohne Mac überschneiden sich nie: Der zuletzt
  eingetragene Eintrag gilt, der andere wird gekürzt – auch wenn der neuere später wieder
  entfernt wird. Ebenso überschreibt neue Arbeit ohne Mac ältere in derselben Zeit;
  aufeinanderfolgende Einträge bleiben eigene Abschnitte.
- Ist die Lücke zwischen neuer Arbeit ohne Mac und der benachbarten Arbeitszeit – erfasst
  oder ebenfalls ohne Mac – kürzer als die Mindestpause, wird sie dem neuen Abschnitt
  dazugerechnet; ein Hinweis zeigt das an.
- Schneidet neue Arbeit ohne Mac eine manuelle Pause an und bliebe davon weniger als die
  Mindestpause übrig, gehört der Rest zum neuen Abschnitt (mit Hinweis) – es entstehen
  keine Pausen mehr, die sich so nicht eintragen ließen.
- Ein manueller Arbeitsbeginn zählt bis zum ersten Entsperren, ein manueller Feierabend
  ab dem letzten Sperren – auch wenn dazwischen Arbeit ohne Mac liegt. Bisher verkürzte
  sie den Tag, und es entstand eine Pause zwischen ihr und dem ersten Entsperren.
- Uhrzeitfelder übernehmen volle Minuten (ein Vorschlag behielt bisher unsichtbare
  Sekunden). Die Dauer einzelner Abschnitte und Pausen richtet sich nach den angezeigten
  Uhrzeiten: 17:00 – 17:30 sind 30 Min.
- Datenbankschema 3 (neue Tabelle `work_blocks`); vorhandene Datenbanken werden beim
  Start automatisch erweitert. Ältere Tagwerk-Versionen können eine Datenbank im
  Schema 3 nicht mehr öffnen.
- Testdaten (`testdata/testdb.sh`): Der Zeitraum endet immer gestern (sechs Wochen plus
  die laufende Woche), an rund 25 % der Tage gibt es Arbeit ohne Mac – auch Networking
  und zwei Abschnitte direkt hintereinander, nachgetragene auf volle Minuten –, und die
  Testdatenbank wird nach `target/testdata/` geschrieben statt ins Repository.

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

[Unveröffentlicht]: https://github.com/rfoerthe/tagwerk/compare/v1.3.0...HEAD
[1.3.0]: https://github.com/rfoerthe/tagwerk/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/rfoerthe/tagwerk/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/rfoerthe/tagwerk/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/rfoerthe/tagwerk/releases/tag/v1.0.0
