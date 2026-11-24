# Changelog

Alle wesentlichen Änderungen an diesem Projekt werden hier festgehalten. Das Format
folgt [Keep a Changelog](https://keepachangelog.com/de/1.1.0/), die Versionsnummern
folgen der [Semantischen Versionierung](https://semver.org/lang/de/).

## [Unveröffentlicht]

### Geändert
- Menü: Ein verfügbares Update steht ganz oben jetzt als farbiges Banner (wie in Barforge) –
  in Akzentfarbe, bei einer Beta orange, mit „Update verfügbar“, der Version und einer Kapsel
  „Aktualisieren“. Bisher war es eine unauffällige Zeile „Auf Tagwerk … aktualisieren“, die
  zwischen den übrigen Einträgen kaum auffiel.

## [1.9.1] - 2026-11-24

### Zusammenfassung
Fehlerbehebung: Wer über Mitternacht gearbeitet hat, kann die Zeit nach 00:00 Uhr jetzt in der
Wochenansicht entfallen lassen – den Arbeitsbeginn auf das erste Entsperren im
Arbeitszeitrahmen legen, dann zählt sie auch für Ruhezeit und Höchstarbeitszeit nicht mehr.
Ohne Angabe beginnt der Tag mit dem frühesten Entsperren, der Dialog nennt die Uhrzeit.

### Behoben
- Wochenansicht: Wer über Mitternacht gearbeitet hat (oder vor bzw. nach dem
  Arbeitszeitrahmen entsperrt war), kann den Arbeitsbeginn jetzt auf das erste Entsperren im
  Rahmen legen, damit die Zeit davor entfällt. Bisher galt dieser Wert als „automatisch“ und
  „Übernehmen“ bzw. Ziehen dorthin änderte nichts. Ohne Angabe beginnt der Tag mit dem
  frühesten Entsperren – über Mitternacht gearbeitet: um 00:00 Uhr; der Dialog „Arbeitsbeginn
  ändern“ nennt die Uhrzeit, ebenso „Zurücksetzen“ und „Erstes Entsperren verwenden“ im Menü.
  Für den Feierabend gilt dasselbe am Ende des Tages.

## [1.9.0] - 2026-11-24

### Zusammenfassung
Tagwerk hält sich jetzt selbst aktuell: Es sucht in den Releases auf GitHub nach neuen
Versionen, meldet sie ganz oben im Menü und installiert sie auf Klick. Vorher prüft es
Prüfsumme und Signatur; die laufende Erfassung bleibt erhalten. Einstellen lässt sich das im
neuen Reiter Einstellungen › Updates, auch ganz abschalten. Die Suche überträgt nichts aus
den Aufzeichnungen. Auf 1.9.0 selbst wird noch von Hand aktualisiert; ab dann geht es aus
der App.

### Hinzugefügt
- Updates aus der App: Tagwerk sucht kurz nach dem Start, stündlich und beim Öffnen des Menüs
  (höchstens alle 15 Minuten) in den GitHub-Releases nach einer neueren Version. Gibt es eine,
  steht ganz oben im Menü „Auf Tagwerk … aktualisieren“, und eine Mitteilung meldet sie einmal.
  Ein Klick zeigt die Neuigkeiten (aller übersprungenen Versionen), lädt das DMG, prüft Größe,
  SHA-256 und Signatur (dieselbe Developer ID und Bundle-Kennung, Gatekeeper), tauscht die App
  aus und startet neu – die laufende Erfassung wird vorher gesichert. Scheitert ein Schritt,
  bleibt die bisherige App. Liegt sie in einem nicht beschreibbaren Ordner, fragt macOS nach
  einem Administrator-Kennwort.
- Einstellungen › Updates (neuer Reiter): automatisch suchen, Beta-Updates erhalten,
  Neuigkeiten vor bzw. nach dem Update anzeigen, Stand der Suche mit „Jetzt suchen“ und
  „Laden und installieren“. Die Schalter liegen in den Benutzereinstellungen von macOS;
  „Standardwerte“ lässt sie unverändert.
- `TAGWERK_SIMULATE_UPDATE=<Version>` täuscht ein verfügbares Update vor (für
  `--settings-screenshot` mit `TAGWERK_SETTINGS_TAB=4` und `--menu-screenshot`).

## [1.8.0] - 2026-11-24

### Zusammenfassung
Die Datenbank lässt sich jetzt verschlüsseln – und jederzeit wieder entschlüsseln. Der
Schlüssel liegt im Schlüsselbund von macOS, Tagwerk startet also weiter ohne Passwort. Den
Wiederherstellungsschlüssel unbedingt aufbewahren: Ohne ihn und ohne den Eintrag im
Schlüsselbund sind die Aufzeichnungen verloren. **Eine verschlüsselte Datenbank können
ältere Versionen nicht öffnen** – vor einem Wechsel zurück die Verschlüsselung ausschalten.

### Hinzugefügt
- Datenbank verschlüsseln (Einstellungen › Allgemein › „Datenbank und Backups
  verschlüsseln“): `tagwerk.db` und alle Backups werden mit SQLCipher (AES-256)
  verschlüsselt. Der Schlüssel liegt im Schlüsselbund, Tagwerk startet weiter ohne
  Passwort. Vor dem Einschalten zeigt Tagwerk einen Wiederherstellungsschlüssel, später
  auch über „Wiederherstellungsschlüssel …“. Fehlt der Schlüssel im Schlüsselbund, fragt
  Tagwerk beim Start nach ihm. Das Ausschalten schreibt alles wieder unverschlüsselt und
  entfernt den Schlüssel. Zum Umstellen startet Tagwerk neu.
- Den Wiederherstellungsschlüssel legt „In die Zwischenablage kopieren“ direkt unter dem
  Schlüssel in die Zwischenablage, ohne den Dialog zu schließen.
- `--settings-screenshot`: `TAGWERK_SETTINGS_SHEET=encrypt` nimmt die Rückfrage „Datenbank
  verschlüsseln?“ auf.

### Geändert
- Tagwerk nutzt SQLCipher statt SQLite. Unverschlüsselte Datenbanken öffnet es wie
  bisher; ältere Versionen können eine verschlüsselte Datenbank nicht öffnen.
- `testdata/testdb.sh --install` bricht ab, wenn die zentrale Datenbank verschlüsselt ist,
  statt an `sqlite3` zu scheitern.

## [1.7.0] - 2026-11-21

### Zusammenfassung
Eine Prüfung der Rechen- und Bedienlogik hat 28 Unstimmigkeiten ergeben; sie sind behoben.
Die wichtigsten: Nach dem Feierabend geht keine Zeit mehr verloren, der laufende Tag zieht
Wochensaldo und Zeitkonto nicht mehr morgens ins Minus, und die Hinweise zu Ruhezeit,
Pausen und Höchstarbeitszeit sehen auch die Arbeit außerhalb des Arbeitszeitrahmens.
**Salden können sich dadurch ändern** – siehe „Geändert“.

### Geändert
- Saldo des laufenden Tages: Heute zählt das Soll nur, soweit es schon erarbeitet ist. Der
  Tag steht bei +0:00, bis das Soll erreicht ist, und danach im Plus; Wochensaldo und
  Zeitkonto fallen nicht mehr mit der ersten Arbeitssekunde um das Tagessoll. Was fehlt,
  zählt mit dem Feierabend oder ab dem nächsten Tag. Zeitausgleich zählt weiter sofort.
- Feierabend: Danach zeichnet Tagwerk weiter auf, zählt aber nichts mehr. „Feierabend
  aufheben“ bringt damit die Zeit dazwischen zurück – bisher war sie eine Pause. Der Dialog
  sagt jetzt „Danach zählt bis morgen nichts mehr“.
- Tagessoll in ganzen Minuten: Geht die Wochenarbeitszeit nicht auf volle Minuten auf
  (38,5 Std. an vier Tagen), bekommen die Arbeitstage der Reihe nach 9:37, 9:38, 9:37 und
  9:38 Std. statt je 9:37:30. Ein halber Abwesenheitstag hat das halbe Soll, auf ganze
  Minuten abgerundet. Tagessalden summieren sich damit ohne Rest zur Woche; bei solchen
  Wochenarbeitszeiten ändern sich frühere Tagessalden um bis zu eine Minute.
- Hinweise zu Pausen, Ruhezeit und Höchstarbeitszeit rechnen zusätzlich mit der Zeit
  außerhalb des Arbeitszeitrahmens. Ergibt sich ein Verstoß erst so, steht „mit der Zeit
  außerhalb des Arbeitszeitrahmens“ dabei. Abschnitte unter einer Minute, die Zeit nach dem
  Feierabend und nicht gezählte Zeit an freien Tagen bleiben außen vor.
- Arbeit an einem abgewählten Arbeitstag bekommt denselben Hinweis wie Arbeit am
  Wochenende: „Arbeit an einem freien Tag – zählt nicht im Saldo“ bzw. „… zählt ohne Soll
  als Mehrarbeit“.
- Vergessenes „Zurück am Mac“: Wählt es bis Mitternacht niemand, endet die Arbeit ohne Mac
  mit dem Arbeitszeitrahmen statt um Mitternacht, und beim nächsten Entsperren nennt eine
  Mitteilung den Eintrag, damit sich das Ende richtigstellen lässt.
- CSV: Ein Tag ohne Saldo (Arbeit an einem freien Tag, die nicht zählt) hat im Feld „Saldo“
  nichts mehr statt „+0:00“.
- Import: Vor dem Import sichert Tagwerk den Stand eigens
  (`backups/tagwerk-JJJJ-MM-TT-vor-import-HHMMSS.db`). Tage, die das Fenster nicht zuließe,
  werden übersprungen und genannt: Zeiten in der Zukunft, ein Arbeitsbeginn, der nicht vor
  dem Feierabend liegt, eine ganztägige Abwesenheit neben anderen Einträgen.
- Inspektor: Der Abschnitt „Automatisch erfasst“ lässt sich auf- und zuklappen und ist
  zunächst zugeklappt. Zugeklappt steht dort eine Zeile mit Anzahl und Spanne der Zeiten
  („5 Zeiten, 07:28 – 18:01“), das Dreieck vor der Überschrift zeigt sie einzeln. Tagwerk
  merkt sich die Wahl für alle Tage.

### Behoben
- Feierabend kurz nach Mitternacht sperrte den neuen Tag, bis ihn jemand aufhob. Vor
  Beginn des Arbeitszeitrahmens bietet das Menü keinen Feierabend mehr an.
- Ein Arbeitsbeginn nach dem Feierabend (oder ein Feierabend vor dem Arbeitsbeginn) ließ
  den Tag auf 0:00 h fallen und kürzte Arbeit ohne Mac endgültig. Menü, Inspektor und
  Wochenansicht lehnen das jetzt ab; im Menü sagt eine Mitteilung, warum.
- „Feierabend → Jetzt“ während „Arbeit unterbrechen“ machte die Unterbrechung zu
  Arbeitszeit. Der Tag endet jetzt mit der Unterbrechung.
- Inaktivität: Blieb der Mac über Nacht ohne Sperre, lief die Erfassung am Morgen nicht von
  selbst weiter.
- „Erfasste Zeit zählen“ bzw. „Heute erfassen“ abschalten konnte die Arbeitszeit erhöhen:
  Waren Arbeitsbeginn und Feierabend von Hand gesetzt, zählte die ganze Spanne dazwischen.
  Ohne den Haken gelten beide nicht mehr.
- Rückwirkend geänderte Arbeitstage: Ein Tag, der als freier Tag „nicht gezählt“ markiert
  war und nachträglich Arbeitstag wurde, bekam das volle Soll bei null gezählter Arbeit;
  ein Tag nur mit der Markierung fiel ganz aus dem Soll. An Arbeitstagen gilt die
  Markierung nicht mehr.
- Markdown-Export: Der Saldo rechnete sekundengenau und wich von Fenster, CSV und PDF ab;
  die Summenzeile passte nicht zu den Zeilen.
- PDF: Die Fußzeile nannte „nur Tage mit Arbeitszeit“, obwohl das Soll auch Arbeitstage
  ohne Eintrag enthält.
- „Heute erfassen“ abwählen während „Ohne Mac weiterarbeiten“ oder einer Unterbrechung
  blendete „Zurück am Mac“ bzw. „Weiter arbeiten“ aus, obwohl beides weiterlief.
- Während „Ohne Mac weiterarbeiten“ wurde nicht regelmäßig gespeichert.
- Wochenansicht: Der Dialog „Arbeitsbeginn ändern“ bzw. „Feierabend ändern“ hält jetzt
  dieselben Grenzen ein wie das Ziehen (Arbeitszeitrahmen, nicht in der Zukunft, mindestens
  5 Minuten zum anderen Ende) und nennt den Grund einer Ablehnung; „Übernehmen“ ohne
  Änderung macht den erfassten Wert nicht mehr zu einem manuellen.
- Wochenansicht: Der Dialog „Arbeit ohne Mac eintragen“ lehnte den eigenen Vorschlag ab,
  wenn er kürzer war als die Mindestdauer. Der Vorschlag und eine genau gefüllte Pause
  gelten jetzt immer; kurz vor „jetzt“ beginnt der Vorschlag früher.
- Wochenansicht: Beim Ziehen neben abgesetzter Arbeit ohne Mac zeigte das Schild „−15 Min.“,
  obwohl die Arbeitszeit um 1:45 h stieg. Das Schild nennt jetzt die tatsächliche Änderung
  („· Arbeitszeit +1:45 h“).
- Wochenansicht: Der Magnet am erfassten Wert fing so weit, dass die Uhrzeiten daneben
  nicht erreichbar waren, auch mit ⌥ nicht.
- Wochenansicht: An einem freien Tag mit Abwesenheit ließ sich eine andere eintragen; jetzt
  lässt sie sich dort nur entfernen.
- Inspektor: Zeiten in der Zukunft ließen sich eintragen, wenn ein künftiger Tag gewählt
  war. An künftigen Tagen geht jetzt nur eine Abwesenheit, heute nichts nach der jetzigen
  Uhrzeit; der Inspektor sagt es über dem betroffenen Feld.
- Widerrufen (⌘Z) stellte den ganzen Korrekturstand des Tages wieder her und überschrieb
  damit, was inzwischen im Menü geändert wurde (Feierabend, „Ohne Mac weiterarbeiten“,
  „Heute erfassen“). Es nimmt jetzt nur zurück, was der Schritt selbst geändert hat, meldet,
  was deshalb geblieben ist, und lässt einen Tag unverändert, der sonst unzulässig würde.
- Inspektor: Ein halber Abwesenheitstag für heute ging nur über „ganztags“ – mit Löschen
  der erfassten Zeit. „Ganzer Tag/Halber Tag“ lässt sich jetzt vorab wählen, und die
  Rückfrage bietet „Halben Tag eintragen“ an.
- Inspektor: An freien Tagen ließ sich eine Abwesenheit eintragen; ein ganztägiger
  „Feiertag“ am Samstag sperrte dann Tag und Erfassung. Wie in der Wochenansicht geht das
  nicht mehr; eine vorhandene lässt sich entfernen.
- „Tag bearbeiten“: Ein Eintrag weit vor dem ersten erfassten Tag ließ alle Arbeitstage
  dazwischen als fehlend zählen – das Zeitkonto fiel um Hunderte Stunden. Vor dem ersten
  solchen Eintrag fragt jetzt ein Dialog nach und nennt die Folge.
- Export-Dialog: Die Vorschau zählte Urlaubstage als Arbeitstage und summierte anders als
  die Datei. Sie nimmt ihre Zahlen jetzt aus derselben Übersicht wie CSV und PDF.
- „Arbeitstage“ bedeutete je Stelle etwas anderes. Überall sind es jetzt die Tage mit
  Arbeitszeit; die Wochenzeile zählte bisher jeden Tag mit Eintrag.
- Inspektor: Der Haken „Erfasste Zeit zählen“ erklärt, dass die Zeit als Arbeitszeit zählt,
  und sagt, ob sie an diesem Tag auch in Saldo und Zeitkonto eingeht.
- Einstellungen: Der Hinweis im Blatt „Arbeitszeit ändern“ wertete beim Verschieben eines
  Stands nur das neue Datum aus. Er nennt jetzt die Tage, für die wirklich andere Regeln
  gelten, und warnt, sobald vergangene darunter sind.
- Einstellungen: „Standardwerte“ fragt nach und nennt, was zurückgesetzt wird und was
  bleibt.
- Einstellungen: Die Erklärung zur Nachfrage nach langer Sperre nennt die Dauer, die heute
  gilt (30 Min., bei längerer Mindestpause diese), und dass nur Sperren im
  Arbeitszeitrahmen desselben Tages gemeint sind.
- Einstellungen: Als Ende des Arbeitszeitrahmens lässt sich 24:00 einstellen; Beginn und
  Ende sind dafür Textfelder.
- Einstellungen: Der Schalter für Arbeit an freien Tagen heißt „Arbeit an freien Tagen in
  Saldo und Zeitkonto einrechnen“ (Spalte „Freie Tage“: „im Saldo“) – „zählen“ meint in der
  App jetzt nur noch „als Arbeitszeit zählen“. Das Tagessoll steht in „Std.“ und bei
  ungeraden Wochenarbeitszeiten als Spanne („9:37 bis 9:38 Std.“).

## [1.6.0] - 2026-11-21

### Zusammenfassung
Die Arbeitszeitregeln gelten jetzt ab einem Datum: Wer in Teilzeit geht, andere Arbeitstage
wählt oder den Rahmen ändert, rechnet frühere Tage nicht mehr neu. An freien Tagen zeichnet
Tagwerk weiter auf, zählt aber erst nach „Heute erfassen“, und Arbeit am Wochenende oder an
abgewählten Arbeitstagen geht standardmäßig nicht mehr in Saldo und Zeitkonto ein – **das
Zeitkonto kann sich dadurch rückwirkend ändern**; je Stand der Arbeitszeitregeln lässt sie
sich wieder als Mehrarbeit zählen. Neu ist außerdem die Abwesenheit Sonderurlaub.

### Hinzugefügt
- Arbeitszeitregeln mit Gültigkeitsdatum: Regelarbeitszeit, Arbeitstage,
  Arbeitszeitrahmen und Mindestpause gelten je Stand ab einem Datum. Eine Änderung rechnet
  damit frühere Tage nicht mehr neu – Soll, Saldo, Zeitkonto, Pausen, Hinweise,
  Wochenansicht und Export nehmen für jeden Tag den Stand, der an ihm galt. Der Reiter
  „Arbeitszeit“ zeigt alle Stände als Liste (heute gültig fett, geplante grau) mit „Neu …“,
  „Bearbeiten …“ und „Löschen“; das Blatt „Arbeitszeit ändern“ hat „Gültig ab“ (heute
  vorbelegt) und sagt, bis wann der Stand gilt und ob er rückwirkend ändert. Auch Stände in
  der Zukunft sind möglich. Export und PDF nennen bei einem Wechsel im Zeitraum die Regeln
  je Stand. Die Fußzeile im Fenster „Aufzeichnungen“ nennt die Regeln des gewählten Tages
  (in „Tage“ wie in „Woche“), ohne Auswahl die von heute. Die bisherigen Einstellungen
  werden zum ersten Stand, der seit Beginn gilt.
  Ältere Versionen lesen weiter die Regeln, die beim letzten Speichern galten.
- `--settings-screenshot`: `TAGWERK_SETTINGS_SHEET=new|first|<Zeile>` nimmt das Blatt
  „Arbeitszeit ändern“ auf.
- Freie Tage: Am Wochenende und an abgewählten Arbeitstagen zeichnet Tagwerk weiter auf,
  zählt die automatisch erfasste Zeit aber erst nach „Heute erfassen“ im Menü – rückwirkend
  ab dem ersten Entsperren. Bis dahin heißt der Status „Freier Tag – Erfassung zählt
  nicht“, und die Wochenansicht zeigt die Zeit orange als „nicht gezählt“; dort lässt sie
  sich per Rechtsklick („Erfasste Zeit zählen“) auch nachträglich zählen. Solange nichts
  zählt, bietet das Menü nur „Heute erfassen“ an; Arbeitsbeginn, Arbeit unterbrechen, Ohne
  Mac weiterarbeiten, Pause als Arbeitszeit zählen und Feierabend erscheinen erst danach.
  Im Inspektor steht dafür der Abschnitt „Erfassung“ mit dem Haken „Erfasste Zeit zählen“;
  ohne ihn lassen sich dort Arbeitsbeginn, Pausen und Feierabend nicht festlegen (nur
  entfernen), oben steht „Freier Tag – die erfasste Zeit zählt nicht“, und die Tabelle
  zeigt weder Beginn noch Ende. Die Wochenansicht zeigt dann auch keine Kanten für
  Arbeitsbeginn und Feierabend. Arbeit ohne Mac lässt sich überall eintragen und zählt
  immer. „Alle Einträge löschen …“ an einem solchen Tag nimmt auch den Haken bei
  „Heute erfassen“ weg und lässt die Erfassung für heute aus, bis er wieder gesetzt ist.
  Neue Einstellung „Freie Tage“ unter „Erfassung“ (standardmäßig ein); die Entscheidung
  gilt je Tag, vergangene Tage bleiben unverändert.
- `--menu-screenshot`: `TAGWERK_NOW=JJJJ-MM-TT HH:MM` legt die Uhrzeit fest, damit die
  Bilder nicht von Tag und Uhrzeit der Aufnahme abhängen.
- Arbeit an freien Tagen im Saldo: Neuer Schalter „Arbeit an freien Tagen als Mehrarbeit
  zählen“ im Blatt „Arbeitszeit ändern“ – Teil jedes Stands der Arbeitszeitregeln, damit
  er ab einem Datum gilt; die Liste der Stände zeigt ihn in der Spalte „Freie Tage“.
  Standardmäßig ist er aus: Arbeit am Wochenende und an abgewählten Arbeitstagen wird
  angezeigt, wirkt sich aber nicht auf Saldo und Zeitkonto aus. Solche Tage haben in der
  Tabelle, im Inspektor, im Markdown-Export und im PDF keinen Saldo („–“), der Hinweis
  lautet „Arbeit am Wochenende – zählt nicht im Saldo“. Die Datenbank bekommt dafür die
  Spalte `free_days_count` in `work_rules`; ältere Versionen speichern ohne sie, dann gilt
  der Standard.
- Abwesenheit Sonderurlaub (Kürzel SU): senkt das Soll wie Urlaub, gilt aber immer
  ganztags – im Inspektor, in der Wochenansicht und im Export. Ältere Versionen kennen sie
  nicht und rechnen den Tag mit vollem Soll.
- Testdaten (`testdata/testdb.sh`): alle sechs Monate ein Tag Sonderurlaub (3, 9, 15 …
  Monate vor dem laufenden) und am Samstag der Woche davor 4 bis 6 Std. Arbeit, als freier
  Tag mit „Heute erfassen“.

### Geändert
- Arbeit an freien Tagen zählt standardmäßig nicht mehr als Mehrarbeit – auch rückwirkend:
  Die bisherigen Einstellungen werden zum ersten Stand der Arbeitszeitregeln, ohne den
  Haken. Wochensaldo und Zeitkonto ändern sich damit um die Arbeit an Wochenenden und
  abgewählten Arbeitstagen. Wer sie weiter zählen will, setzt den Haken „Arbeit an freien
  Tagen als Mehrarbeit zählen“ im jeweiligen Stand.
- Die Mindestpause steht nicht mehr im Reiter „Erfassung“, sondern bei den
  Arbeitszeitregeln. „Standardwerte“ lässt frühere Stände der Arbeitszeitregeln stehen und
  setzt sie ab heute zurück. Die Bilder der Einstellungen im README sind neu, dazu kommt
  eines vom Blatt „Arbeitszeit ändern“.
- Die Bilder des Menüs im README sind neu; dazu kommt eines vom freien Tag mit „Heute
  erfassen“. Der Text zum Menü nennt jetzt auch das Zeitkonto und den grauen Status.
- Das geöffnete DMG zeigt ein gestaltetes Fenster: Titel „Tagwerk“, App-Symbol und Ordner
  „Programme“ mit einem Pfeil dazwischen und dem Hinweis, die App hineinzuziehen.

## [1.5.0] - 2026-11-20

### Zusammenfassung
Die Wochenansicht wird zum Werkzeug: Arbeitsbeginn und Feierabend lassen sich mit der Maus
verschieben, Abwesenheiten per Rechtsklick eintragen, und der Tag in Bearbeitung ist
markiert. Ein Tag lässt sich mit „Alle Einträge löschen …“ ganz leeren. Saldo und
Zeitkonto zählen jetzt auch vergangene Arbeitstage ohne Eintrag – **das Zeitkonto kann
dadurch sinken**; fehlende Urlaubs-, Feier- und Krankheitstage zeigt die Liste zum
Nachtragen an.

### Hinzugefügt
- Wochenansicht: Arbeitsbeginn und Feierabend lassen sich mit der Maus verschieben. An der
  Oberkante des ersten bzw. der Unterkante des letzten Abschnitts erscheint ein Griff;
  beim Ziehen zeigt ein Schild die neue Uhrzeit und die Abweichung, der Tageskopf rechnet
  mit, und Arbeit ohne Mac, die entfiele, ist gestrichelt. Die Zeit rastet auf 5 Minuten
  ein (mit ⌥ auf die Minute) und am erfassten Wert, der den manuellen Wert wieder
  aufhebt. Esc bricht ab, ⌘Z nimmt die Änderung zurück. Arbeitsbeginn und Feierabend
  tragen eine Linie in Textfarbe, manuell festgelegte kräftiger und mit einem Punkt. Ohne Maus
  geht dasselbe über das Kontextmenü (ändern mit Uhrzeit-Feld, zurücksetzen) und für
  VoiceOver über Aktionen am Abschnitt.
- Inspektor: „Alle Einträge löschen …“ ganz unten leert einen Tag – automatisch erfasste
  Zeiten wie manuelle Einträge (Arbeitsbeginn, Feierabend, Pausen, Arbeit ohne Mac,
  Abwesenheit). Ein Bestätigungsdialog nennt, was wegfällt; „Abbrechen“ ist die
  Standardtaste. ⌘Z stellt den Tag samt erfasster Zeiten wieder her. Wird der heutige Tag
  gelöscht, zählt die Menüleisten-App eine laufende Erfassung bzw. laufende Arbeit ohne Mac
  ab dem Löschen weiter, statt die gelöschte Zeit beim nächsten Speichern zurückzuschreiben.
  Der Befehl erscheint nur bei Einträgen außer der Abwesenheit; eine Abwesenheit allein
  entfernt man mit „Keine“.
- Wochenansicht: Abwesenheit per Rechtsklick (Untermenü „Abwesenheit“: Urlaub, Krank,
  Feiertag – je ganz oder halb –, Zeitausgleich oder keine), an Arbeitstagen und auch im
  Voraus. Sie liegt als türkises, gestricheltes Band über dem Arbeitszeitrahmen des Tages
  und lässt sich mit ⌘Z widerrufen. Ebenfalls per Rechtsklick: „Alle Einträge löschen …“
  (mit derselben Rückfrage wie im Inspektor).
- Wochenansicht: Der Tag in Bearbeitung (in „Tage“ ausgewählt) ist markiert – Datum auf
  einer Pille in Magenta, gestrichelter Rahmen in Magenta um die Spalte. Ein Klick oder Rechtsklick in eine
  Tagesspalte wählt den Tag, auch für den Inspektor in „Tage“.
- „Aufzeichnungen“: Ein Doppelklick auf eine Zeile der Liste oder ein Rechtsklick („In der
  Wochenansicht zeigen“) springt in die Woche des Tags bzw. der Wochenzeile.
- „Aufzeichnungen“: Vergangene Arbeitstage ganz ohne Eintrag erscheinen in der Liste (mit
  Fragezeichen, 0:00 h und ihrem Soll als Minus) und lassen sich auswählen, um Zeiten oder
  eine Abwesenheit nachzutragen. Auch Export (Markdown, CSV, PDF) führt sie auf, und die
  Wochenansicht zeigt im Tageskopf ihr fehlendes Soll in Rot (z. B. „−7:48 h“).

### Geändert
- Eine ganztägige Abwesenheit (Urlaub, Krank, Feiertag, Zeitausgleich) lässt sich nur noch
  an einem Tag ohne andere Einträge eintragen; erfasste Zeiten und Korrekturen löscht man
  vorher selbst mit „Alle Einträge löschen …“. Im Inspektor und im Rechtsklick-Menü sind
  ganze Tage sonst gesperrt (mit Hinweis); ein halber Tag geht weiter neben Arbeitszeit.
  Für heute zählen nur die Einträge von Hand: Die automatisch erfasste Zeit entfällt nach
  einer Rückfrage mit der Abwesenheit, die Erfassung endet für heute; ⌘Z holt beides zurück.
  Umgekehrt sperrt eine ganztägige Abwesenheit den Tag: Arbeitsbeginn, Feierabend, Pausen
  und Arbeit ohne Mac lassen sich im Inspektor und in der Wochenansicht erst wieder
  eintragen oder ändern, wenn die Abwesenheit auf „Keine“ steht. Auch die Menüleisten-App
  erfasst an so einem Tag nichts (auch nicht über Mitternacht hinein) und zeigt „Heute
  abwesend: …“ statt Arbeitsbeginn, Unterbrechen, Ohne Mac und Feierabend.
- Saldo und Zeitkonto zählen das Soll jedes Arbeitstags bis gestern – auch ohne Eintrag, ab
  dem ersten erfassten Tag. Bisher drückten nicht erfasste Arbeitstage den Saldo nicht, und
  ein vergessener Urlaubstag fiel nicht auf. Heute zählt das Soll wie bisher erst mit
  Arbeitszeit oder Abwesenheit, künftige Tage (eine geplante Abwesenheit) noch gar nicht.
  **Das Zeitkonto kann dadurch sinken:** Nicht eingetragene Urlaubs-, Feier- und
  Krankheitstage zeigt die Liste jetzt als „fehlend“ – dort als Abwesenheit nachtragen.
- Ein manueller Feierabend zählt ab dem letzten Sperren davor, ein manueller Arbeitsbeginn
  bis zum ersten Entsperren danach – auch wenn der Mac nach dem Feierabend bzw. vor dem
  Beginn noch entsperrt war. Bisher galt das letzte Sperren bzw. erste Entsperren des
  ganzen Tages: Wer abends kurz an den Mac ging und den Feierabend dann zurückverlegte,
  verlor die Zeit zwischen dem Sperren und dem Feierabend, und die Wochenansicht zeigte den
  Feierabend nicht an. Das gilt auch für Beginn oder Feierabend in der Mittagslücke; die
  Zeit vor dem Beginn bzw. nach dem Feierabend entfällt wie bisher.
- Testdaten (`testdata/testdb.sh`): Der Zeitraum umfasst 18 volle Kalendermonate plus den
  laufenden Monat bis gestern (bisher sechs Wochen plus die laufende Woche). Die
  eingebauten Regelverstöße – zu kurze Ruhezeit, Arbeit vor 7 und nach 22 Uhr, zu wenig
  Pause – wiederholen sich alle sechs Wochen, von der laufenden Woche aus rückwärts
  gezählt. Dazu kommen Abwesenheiten, damit kein Werktag ohne Eintrag bleibt: die
  bundesweiten Feiertage, 30 Urlaubstage im Jahr (Karwoche, Brückentag nach Himmelfahrt,
  Sommer- und Herbsturlaub, zwischen den Jahren, Heiligabend und Silvester halb, der Rest
  einzeln) und 6 Tage Zeitausgleich (Brückentage, sonst Freitage). Die Wochen gleichen das
  Zeitkonto aus, sodass es um null pendelt.
- Die Bilder des Fensters „Aufzeichnungen“ und der Wochenansicht im README sind neu und
  zeigen Abwesenheit, die Linien für Arbeitsbeginn und Feierabend und den Tag in
  Bearbeitung.

## [1.4.0] - 2026-11-19

### Zusammenfassung
Tagwerk meldet sich jetzt über die Mitteilungszentrale von macOS, bevor die
Höchstarbeitszeit erreicht ist: bei 9:45 Std. und bei 10 Std. Arbeit, jeweils einmal am
Tag. Die Einstellungen zeigen, ob Mitteilungen erlaubt sind, und führen bei Bedarf direkt
in die Systemeinstellungen.

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
- Neuer Hinweis bei mehr als 6 Std. Arbeit am Stück ohne Pause (§ 4 ArbZG), z. B.
  „7:00 Std. ohne Pause – erlaubt sind höchstens 6 Std. am Stück“. Bisher wurde nur die
  Summe der Pausen geprüft: Wer sieben Stunden durcharbeitete und danach 30 Min. Pause
  machte, bekam keinen Hinweis. Unterbrechungen unter 15 Min. gelten dabei nicht als Pause.
  Fehlt die Pause ganz, bleibt es beim bisherigen Hinweis auf die vorgeschriebene Pause.
- Fehler der Datenbank bleiben nicht mehr unbemerkt: Schlägt das Speichern oder das
  Einlesen in der Menüleisten-App fehl, steht das rot unter dem Status im Menü und im
  Tooltip des Symbols, bis es wieder gelingt; beim ersten Mal kommt zusätzlich eine
  Mitteilung. Bisher stand der Fehler nur in der Standardfehlerausgabe. Ein gescheitertes
  Einlesen wird jetzt beim nächsten Takt wiederholt.
- Abwesenheiten: Im Inspektor lässt sich für einen Tag Urlaub, Krank, Feiertag oder
  Zeitausgleich eintragen, als ganzer oder halber Tag. Urlaub, Krankheit und Feiertag
  senken das Soll des Tages (halber Tag: auf die Hälfte) – ein halber Urlaubstag ergibt
  kein Minus mehr. Zeitausgleich lässt das Soll stehen, auch ohne Arbeitszeit, sodass das
  Zeitkonto um den freien Tag sinkt. Die Abwesenheit steht in der Tabelle als Kürzel hinter
  dem Datum (U, K, F, ZA), im Tageskopf der Wochenansicht, im Export (Markdown, CSV, PDF)
  und lässt sich widerrufen. Gespeichert wird in einer eigenen Tabelle ohne neue
  Schema-Version; ältere Versionen öffnen die Datenbank weiter.
- Zeitkonto: der fortlaufende Saldo aller erfassten Tage, im Menü der Menüleiste („Zeitkonto:
  +1:39 h“) und als fünfte Kennzahl im Fenster „Aufzeichnungen“. In den Einstellungen lässt
  es sich auf einen Beginn („Zählt erst ab“) und einen Übertrag aus der Zeit davor (z. B.
  `+12:30`) einstellen.
- Arbeitstage einstellbar: Mo bis Fr als einzelne Schalter, das Wochensoll verteilt sich
  gleichmäßig auf die gewählten Tage (z. B. „4-Tage-Woche · 8:00 h pro Arbeitstag“). An
  abgewählten Tagen gibt es kein Soll, Arbeit dort zählt als Mehrarbeit; die Wochenansicht
  schattiert sie wie das Wochenende. Bisher war das Soll fest Wochenstunden / 5 an Mo–Fr.
- Inaktivitätserkennung (einschaltbar, Standard 10 Min.): Bleibt der entsperrte Mac so
  lange ohne Eingabe, endet die Erfassung rückwirkend mit der letzten Eingabe – wie bei
  einer Sperre, also erst ab der Mindestpause eine Pause – und läuft mit der nächsten
  Eingabe weiter. Solange eine App den Bildschirm wach hält (Videokonferenz, Präsentation),
  zählt die Zeit weiter. Der Status im Menü zeigt „Keine Eingabe seit … – pausiert“.
- Nachfrage nach langer Sperre (einschaltbar): Nach mindestens 30 Min. Sperre fragt beim
  Entsperren eine Mitteilung mit Knöpfen, ob die Zeit Arbeit ohne Mac war (Meeting,
  Telefonat, Networking, Sonstige Arbeit) oder schon Feierabend – dann endet der Tag mit
  dem Sperren. Ohne Antwort bleibt es eine Pause. Nur in der installierten App.
- Erinnerung an die Pause über die Mitteilungszentrale: bei 5:45 Std. Arbeit am Stück
  „Noch 15 Min. bis 6 Std. ohne Pause“, bei 6 Std. „6 Std. ohne Pause gearbeitet“. Nach
  einer Pause von mindestens 15 Min. beginnt die Zählung neu.
- Export als CSV (Tabelle für Excel oder Numbers) und als PDF (Übersicht zum Drucken mit
  Wochensummen und Hinweisen) – im Sichern-Dialog über „Format“ und auf der Kommandozeile
  mit `--format md|csv|pdf` bzw. über die Endung von `-o`.
- Import im Fenster „Aufzeichnungen“: Ablage › „Importieren …“ (⇧⌘I) liest einen
  Markdown-Export ein. Vorher nennt ein Dialog Zeitraum und Zahl der Tage, die ersetzt
  werden.
- Widerrufen und Wiederholen (⌘Z, ⇧⌘Z) im Fenster „Aufzeichnungen“: Jede Korrektur aus dem
  Inspektor und der Wochenansicht lässt sich zurücknehmen, bis zu 50 Schritte, solange das
  Fenster offen ist. Das Menü „Bearbeiten“ nennt den Schritt („Widerrufen: Pause
  entfernen“), danach wird der betroffene Tag gezeigt. Mehrere Schritte im Uhrzeit-Feld
  zählen als eine Eingabe. Wiederhergestellt werden nur die Korrekturen des Tages –
  inzwischen erfasste Zeit bleibt. In Dialogen und im Fenster „Einstellungen“ gilt ⌘Z wie
  bisher dem Textfeld.
- Filter für die Tabelle der Tage (Symbolleiste und Menü „Darstellung“): Zeitraum (dieser
  oder letzter Monat, dieses oder letztes Jahr), „Nur Tage mit Hinweisen“ und „Nur
  korrigierte Tage“, beliebig kombinierbar. Bei aktivem Filter ist das Symbol gefüllt, und
  die Fußzeile zeigt „Filter: 5 von 33 Tagen“. Kennzahlen und Wochensummen zählen weiter
  alle Tage. Wird ein ausgeblendeter Tag geöffnet, hebt das den Filter auf.
- Wochenansicht: Ein Doppelklick auf einen Tageskopf oder Abschnitt öffnet den Tag in der
  Ansicht „Tage“ mit dem Inspektor. Ein Rechtsklick zeigt ein Kontextmenü mit allem, was an
  der Stelle geht (Tag bearbeiten, Uhrzeit kopieren, Arbeit ohne Mac eintragen oder
  löschen). Die Klicks mit der linken Taste bleiben, wie sie waren.
- Die Wochenansicht ist für VoiceOver zugänglich: eine Gruppe mit der Woche und ihren
  Summen, darin je Tag ein Element mit Arbeitszeit, Pausen und Hinweisen sowie je Abschnitt,
  Pause und Arbeit ohne Mac eines mit den Uhrzeiten. Bisher war das selbst gezeichnete
  Raster für Bedienungshilfen leer.
- Einstellung „Arbeitszeit von heute zeigen“: Die Arbeitszeit steht dann rechts neben dem
  Symbol in der Menüleiste (z. B. `6:12`), mit gleich breiten Ziffern. Standard: aus.
- Einstellung „Kurzbefehle“: ⌃⌥⌘P („Arbeit unterbrechen“ bzw. „Weiter arbeiten“) und ⌃⌥⌘A
  („Aufzeichnungen“) wirken dann in jeder App, auch bei geschlossenem Menü. Standard: aus.
  Die Kurzbefehle brauchen keine Freigabe unter „Bedienungshilfen“; ist eine
  Tastenkombination schon vergeben, meldet Tagwerk das.
- Die Wochenansicht nutzt den Platz: Das Raster zeigt nur die Stunden, in denen in der
  Woche gearbeitet wurde, mit einer Stunde Luft davor und danach (mindestens neun Stunden,
  in der laufenden Woche bis zur aktuellen Uhrzeit), und freie Tage ohne Eintrag als
  schmale Spalten. Bisher standen immer der ganze Arbeitszeitrahmen (07–22 Uhr) und volle
  Wochenendspalten da. Der Knopf mit dem Doppelpfeil neben „Heute“ schaltet auf den ganzen
  Rahmen um; die Wahl bleibt gespeichert.
- Info-Knöpfe (ⓘ) im Inspektor: Was Abwesenheit, Arbeitsbeginn, Pausen, Arbeit ohne Mac und
  Feierabend bewirken, steht als Tooltip am Knopf neben der Überschrift und erscheint auf
  Klick als kleines Fenster.
- `--menu-screenshot <png>`: öffnet das Menü der Menüleiste und speichert es als Bild; den
  Zustand setzt `TAGWERK_MENU_STATE=break|away|closed`. Der Reiter für
  `--settings-screenshot` lässt sich mit `TAGWERK_SETTINGS_TAB=0…3` wählen.

### Geändert
- Salden sind überall farbig, grün im Plus und rot im Minus – nicht mehr nur im Menü: in
  der Spalte „Saldo“ der Tabelle, in den Wochensummen, in den Kennzahlen (Woche, Monat,
  Zeitkonto) und im Kopf der Wochenansicht. In der ausgewählten Zeile bleibt die Schrift
  weiß.
- Der Inspektor ist kürzer: Unter dem Datum stehen Arbeitszeit, Pausen und Saldo als drei
  Zahlen nebeneinander statt in einer Zeile, die mitten in „Saldo +1:32 h“ umbrach. Die
  erklärenden Absätze zu Pausen, Arbeit ohne Mac und automatisch Erfasstem sind in die
  Info-Knöpfe gewandert; zur Abwesenheit steht nur noch dann ein Satz, wenn eine
  eingetragen ist.
- Einstellungen › Erfassung: „Arbeit ohne Mac: mindestens 15 Minuten“ sagt jetzt selbst,
  dass es um die Mindestdauer geht; der Text darunter ist kürzer.
- Die Mindestdauer für Arbeit ohne Mac hängt nicht mehr an der Mindestpause. Bisher durfte
  sie nie darunter liegen und stieg mit ihr – ein Telefonat von zehn Minuten in einer
  langen Mittagspause oder vor dem Arbeitsbeginn ließ sich so nicht eintragen, obwohl es
  mit der Frage, ab wann eine Sperre eine Pause ist, nichts zu tun hat. Jetzt sind 1 bis
  240 Minuten möglich, der Standard bleibt 15; die Pfeile gehen in Fünferschritten. Für
  gespeicherte Werte und Tage ändert sich nichts.
- Die Bilder im README sind neu und zeigen den aktuellen Stand: das Menü mit Zeitkonto,
  die Ansicht „Woche“ im Fenster „Aufzeichnungen“ und alle vier Reiter der Einstellungen,
  die jetzt auch einzeln beschrieben sind.
- `--screenshot`, `--week-screenshot` und `--settings-screenshot` holen das Bild jetzt vom
  Fensterserver, statt die Ansicht selbst zu zeichnen. Bisher fehlte, was macOS erst beim
  Zusammensetzen ergänzt: Das gewählte Segment von „Tage | Woche“ und die Reiterleiste der
  Einstellungen blieben leer. Bei gesperrtem Bildschirm gibt es kein solches Bild; dann
  wird wie bisher gezeichnet.
- Das Fenster „Einstellungen“ hat vier Reiter – Allgemein, Arbeitszeit, Erfassung und
  Mitteilungen – statt einer langen Liste.
- Die Mitteilungen zur Höchstarbeitszeit und zur Pause lassen sich in den Einstellungen
  einzeln abschalten; die Hinweise im Menü und im Fenster bleiben davon unberührt.
- Das Fenster „Aufzeichnungen“ ist mindestens 940 Punkte breit (bisher 780), damit die fünf
  Kennzahlen nebeneinander passen.
- „Pause als Arbeitszeit zählen“ in der Menüleiste fragt den Grund ab: Jede Pause hat ein
  Untermenü mit Meeting, Telefonat, Networking und Sonstiges – wie „Ohne Mac
  weiterarbeiten“ und die Dialoge im Fenster „Aufzeichnungen“. Bisher wurde die Pause ohne
  Grund eingetragen.
- Hinweise zu einer abgelehnten Eingabe im Inspektor (z. B. „Eine Pause dauert mindestens
  15 Min.“) stehen jetzt direkt über den Feldern der Pause bzw. der Arbeit ohne Mac statt
  ganz am Ende des Inspektors, wo sie meist außerhalb des sichtbaren Bereichs lagen. Ein
  Fehler beim Speichern erscheint als Dialog am Fenster.
- Der Dialog „Arbeit ohne Mac eintragen“ in der Wochenansicht zeigt den Hinweis zu einer
  abgelehnten Eingabe ebenfalls rot direkt über den Feldern; der erklärende Text darüber
  bleibt stehen. Bisher ersetzte der Hinweis den erklärenden Text in normaler Schrift.
- Arbeitsbeginn und Feierabend: Für Arbeit ohne Mac gilt jetzt die letzte Angabe, wie schon
  bei Pausen. Wer den Arbeitsbeginn später oder den Feierabend früher festlegt, kürzt Arbeit
  ohne Mac davor bzw. danach – ein Termin um 8:00 steht einem Beginn um 9:20 nicht mehr im
  Weg. Der Inspektor weist darauf hin (⌘Z nimmt es zurück), aus dem Menü kommt eine
  Mitteilung mit dem entfallenen Termin. Wird die Arbeit ohne Mac erst nach Beginn bzw.
  Feierabend eingetragen, zählt sie wie bisher auch davor bzw. danach. Bisher gewann immer
  die Arbeit ohne Mac, egal in welcher Reihenfolge.
- Hinweise auf Regelverstöße und Fehlermeldungen im Menü der Menüleiste brechen um, statt
  das Menü zu verbreitern: Sie sind höchstens so breit wie die breiteste Zeile mit Status
  und Kennzahlen (mindestens 280 Punkte), Folgezeilen stehen eingerückt unter dem Text.
- Die Tabelle der Tage zeigt Warnschild und Stift nebeneinander, wenn ein Tag sowohl
  Hinweise hat als auch manuell korrigiert ist; bisher verdrängte das Warnschild den
  Stift. Beide Symbole haben feste Plätze und einen eigenen Tooltip. Dafür ist die
  Symbolspalte breiter; gespeicherte Spaltenbreiten der Tabelle werden einmalig
  zurückgesetzt.
- Texte zum Wochenende richtiggestellt: Der Arbeitszeitrahmen gilt an jedem Tag, und
  Arbeit am Wochenende zählt ohne Soll als Mehrarbeit. Die Einstellungen, der Hinweis
  („Arbeit am Wochenende – zählt ohne Soll als Mehrarbeit“) und die Fußzeile im Fenster
  „Aufzeichnungen“ behaupteten bisher, außerhalb von Mo–Fr werde nichts gezählt. An der
  Berechnung ändert sich nichts.

### Geändert
- Testdaten (`testdata/testdb.sh`): An rund 10 % der Tage wird mehr als 6 Std. ohne Pause
  gearbeitet (6:10 bis 8:45 Std. am Stück, höchstens kurze Sperren unter 15 Min.) – Tagwerk
  meldet dort „Ab 6 Std. Arbeit sind 30 Min. Pause vorgeschrieben“. Die Tage sind über den
  Zeitraum verteilt; das Skript listet sie in seiner Übersicht auf.

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

[Unveröffentlicht]: https://github.com/rfoerthe/tagwerk/compare/v1.9.1...HEAD
[1.9.1]: https://github.com/rfoerthe/tagwerk/compare/v1.9.0...v1.9.1
[1.9.0]: https://github.com/rfoerthe/tagwerk/compare/v1.8.0...v1.9.0
[1.8.0]: https://github.com/rfoerthe/tagwerk/compare/v1.7.0...v1.8.0
[1.7.0]: https://github.com/rfoerthe/tagwerk/compare/v1.6.0...v1.7.0
[1.6.0]: https://github.com/rfoerthe/tagwerk/compare/v1.5.0...v1.6.0
[1.5.0]: https://github.com/rfoerthe/tagwerk/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/rfoerthe/tagwerk/compare/v1.3.0...v1.4.0
[1.3.0]: https://github.com/rfoerthe/tagwerk/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/rfoerthe/tagwerk/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/rfoerthe/tagwerk/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/rfoerthe/tagwerk/releases/tag/v1.0.0
