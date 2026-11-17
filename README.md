# Tagwerk

Tagwerk ist eine kleine Menüleisten-App für macOS, die deine Arbeitszeit automatisch
erfasst: gezählt wird die Zeit, in der der Mac entsperrt ist. Sperrst du den Mac für
mindestens 15 Minuten, ist das eine Pause; kürzere Unterbrechungen zählen weiter zur
Arbeitszeit. Über „Pausieren“ in der Menüleiste lässt sich eine Pause auch ohne Sperren
einlegen. Arbeitsbeginn und Feierabend lassen sich zusätzlich von Hand festlegen oder
korrigieren.

## Funktionen

- **Automatische Erfassung** – Sperren, Entsperren, Ruhezustand und Benutzerwechsel
  werden erkannt; jede entsperrte Phase wird als Periode gespeichert.
- **Pausen ohne Zutun** – jede Sperre ab der eingestellten Mindestdauer (Standard
  15 Minuten) ist eine Pause, egal ob Mittagspause oder Kaffeepause. Kürzere Sperren
  zählen zum laufenden Arbeitsblock.
- **Pausieren / Weiter arbeiten** – der Menüpunkt (⌘P bei geöffnetem Menü) hält die
  Erfassung an, auch wenn der Mac entsperrt bleibt, und setzt sie fort. Wird der Mac
  während der Pause gesperrt oder geht in den Ruhezustand, läuft die Erfassung nach dem
  Entsperren bzw. Aufwachen von selbst wieder. Auch hier gilt die Mindestdauer: Eine
  kürzere Pause zählt als Arbeitszeit. Beim Beenden der App (z. B. für ein Update) bleibt
  die Pause bestehen, beim Abmelden oder Neustarten des Macs endet sie.
- **Arbeitszeitrahmen** – gezählt wird nur Montag bis Freitag im eingestellten Zeitraum
  (Standard 07:00–22:00 Uhr). Außerhalb wird weder Arbeitszeit noch Pause gezählt.
- **Soll und Saldo** – aus der Regelwochenarbeitszeit (Standard 39 Stunden, also
  7:48 Stunden pro Arbeitstag) ergibt sich der Saldo je Tag, Woche und Monat.
- **Menüleiste** – zeigt den Status (läuft, Pause, pausiert, außerhalb des Arbeitszeitrahmens,
  Feierabend), die Arbeitszeit von heute samt Soll, die Pausen, den Beginn, den Saldo
  der Woche und Hinweise auf Regelverstöße.
- **Manuelle Korrekturen** – Arbeitsbeginn und Feierabend jeweils „jetzt“, „vor N
  Minuten“ oder zu einer frei gewählten Uhrzeit; im Fenster „Aufzeichnungen“ zusätzlich
  Pausen. Korrekturen liegen als eigene Schicht über den erfassten Perioden und lassen
  sich jederzeit zurücknehmen.
- **Hinweise bei Regelverstößen** – siehe [Regeln](#regeln).
- **Einstellungen** – Arbeitszeitrahmen, Regelwochenarbeitszeit und Mindestpause
  (Menüleiste „Einstellungen …“ oder ⌘, im Fenster „Aufzeichnungen“). Änderungen gelten
  sofort, auch für bereits erfasste Tage.
- **Fenster „Aufzeichnungen“** – natives macOS-Fenster mit Kennzahlen (heute, Woche,
  Monat, Durchschnitt), einer Tabelle aller Tage mit Saldo und Wochensummen und einem
  Inspektor, in dem sich Arbeitsbeginn, Pausen und Feierabend eines Tages korrigieren
  lassen – auch für Tage ohne Aufzeichnung. Hell- und Dunkelmodus folgen dem System.
- **Export als Markdown** – für einen frei wählbaren Zeitraum (Standard: die letzten
  vier Wochen) im Fenster „Aufzeichnungen“ oder über die Kommandozeile.
- **Bei Anmeldung starten** – per Menüpunkt als Anmeldeobjekt eintragen
  (nur in der installierten App).

## Regeln

Tagwerk berücksichtigt die folgenden Regeln aus Arbeitszeitgesetz und
Betriebsvereinbarung. Verstöße werden in der Menüleiste, in der Tabelle (Warnsymbol),
im Inspektor und im Export angezeigt – die Arbeitszeit selbst wird dadurch nicht
verändert.

| Regel                | Inhalt                                                                  |
|----------------------|-------------------------------------------------------------------------|
| Arbeitszeitrahmen    | Mo–Fr im eingestellten Zeitraum; Zeit außerhalb wird nicht gezählt, Arbeit am Wochenende gemeldet |
| Sollarbeitszeit      | Regelwochenarbeitszeit / 5 je Arbeitstag (39 Std. → 7:48 Std.)          |
| Pausen (§ 4 ArbZG)   | mehr als 6 Std. Arbeit: 30 Min., mehr als 9 Std.: 45 Min.; es zählen Pausen ab 15 Min. |
| Ruhezeit (§ 5 ArbZG) | mindestens 11 Std. zwischen dem Ende eines Arbeitstags und dem nächsten Beginn |
| Höchstarbeitszeit    | mehr als 10 Std. Arbeit an einem Tag                                    |
| Erfassung            | Beginn und Ende durch Entsperren/Sperren oder Pausieren/Weiter arbeiten; keine Einträge in der Zukunft |

Der Saldo vergleicht die Arbeitszeit mit dem Soll – aber nur für Tage, an denen
gearbeitet wurde. Urlaub, Feiertage oder Krankheit drücken den Saldo also nicht; Arbeit
am Wochenende zählt vollständig als Mehrarbeit.

## Voraussetzungen

- macOS 13 (Ventura) oder neuer, Apple Silicon (arm64)
- Zum Bauen: [Rust](https://rustup.rs) und die Xcode-Befehlszeilenwerkzeuge

## Bauen

Zum Ausprobieren während der Entwicklung:

```bash
cargo run
```

Ein fertiges App-Bundle samt DMG erzeugt `build.sh`:

```bash
./build.sh                 # sauber bauen, signieren und notarisieren
./build.sh --no-notarize   # nur bauen und signieren
./build.sh --no-sign       # nur bauen (ad-hoc signiert, nur für diesen Mac)
./build.sh --install       # zusätzlich nach /Applications installieren
```

`--install` lässt sich mit den anderen Optionen kombinieren (z. B.
`./build.sh --no-notarize --install`). Läuft Tagwerk bereits aus `/Applications`, wird
es vorher regulär beendet – die laufende Erfassung wird dabei gespeichert – und nach
dem Kopieren wieder gestartet.

Das Ergebnis liegt in `target/dist/Tagwerk-<Version>.dmg`. Das Skript führt vorher
`cargo clean` aus; ältere DMGs in `target/dist` werden dabei gelöscht.

Einstellungen über Umgebungsvariablen:

| Variable         | Bedeutung                                   | Standard                       |
|------------------|---------------------------------------------|--------------------------------|
| `SIGN_IDENTITY`  | Zertifikat „Developer ID Application: …“    | erste gefundene Developer-ID   |
| `NOTARY_PROFILE` | Schlüsselbund-Profil für `notarytool`       | `general-notary`               |
| `BUNDLE_ID`      | Bundle-Kennung                              | `com.tagwerk.app`              |

Das Notary-Profil wird einmalig angelegt mit:

```bash
xcrun notarytool store-credentials general-notary --apple-id <apple-id> --team-id <team-id>
```

## Aufruf

```text
Tagwerk                       Menüleisten-App (Standard)
Tagwerk --records             nur das Fenster „Aufzeichnungen“ öffnen
Tagwerk --export [--from TT.MM.JJJJ] [--to TT.MM.JJJJ] [-o datei.md]
                              Aufzeichnungen als Markdown exportieren
                              (Standard: die letzten 28 Tage, Ausgabe auf stdout)
Tagwerk --import <datei.md>   Tage aus einem Markdown-Export übernehmen
Tagwerk --screenshot <png>    Fenster „Aufzeichnungen“ als Bild speichern und beenden
Tagwerk --render-icon <png>   App-Symbol zeichnen (wird von build.sh genutzt)
```

Es läuft immer nur eine Instanz der Menüleisten-App.

## Daten

Alle Daten liegen in einer lokalen SQLite-Datenbank:

```text
~/Library/Application Support/Tagwerk/tagwerk.db
```

Sie enthält die automatisch erfassten Perioden (Mac entsperrt), getrennt davon die
manuellen Korrekturen (Arbeitsbeginn, Pausen, Feierabend) sowie die Einstellungen. Die
Perioden werden ungekürzt gespeichert; Mindestpause und Arbeitszeitrahmen wirken erst
bei der Berechnung. Bearbeitet werden die Daten im Fenster „Aufzeichnungen“; Änderungen
dort übernimmt die laufende App automatisch.

Vor der ersten Änderung eines Tages wird die Datenbank unter
`backups/tagwerk-JJJJ-MM-TT.db` gesichert; die letzten 15 Backups bleiben erhalten.

### Export

Über „Exportieren …“ im Fenster „Aufzeichnungen“ (Symbolleiste oder Menü „Ablage“, ⌘E)
wird ein Zeitraum als Markdown-Datei gespeichert. Vorbelegt sind die
letzten vier Wochen (28 Tage bis heute); „Dieser Monat“ und „Letzter Monat“ lassen sich
mit einem Klick wählen. Die Datei enthält

1. eine **Übersichtstabelle** mit Beginn, Ende, Arbeits- und Pausenzeit sowie Saldo je
   Tag und einer Summenzeile,
2. **`## Hinweise`** – Verstöße gegen die [Regeln](#regeln),
3. **`## Korrekturen`** – die manuellen Entscheidungen,
4. **`## Erfasste Perioden`** – die automatisch erfassten Zeiten.

Damit lässt sich ein Export mit `Tagwerk --import <datei.md>` wieder einlesen; die
enthaltenen Tage ersetzen dabei die vorhandenen Einträge dieser Tage.

### Ältere Versionen

Die Markdown-Datei `tagwerk.md` früherer Versionen wird beim ersten Start einmalig in
die Datenbank übernommen und danach in `tagwerk.md.importiert` umbenannt. Daten der
Vorgängerversion „TrackMyWork“ werden ebenfalls übernommen; der alte Ordner bleibt
unverändert.

Zum Testen lässt sich der Datenordner mit der Umgebungsvariablen `TAGWERK_DIR`
umlenken:

```bash
TAGWERK_DIR=/tmp/tagwerk-test cargo run
```

## Tests

```bash
cargo test
```

### Testdaten

`testdata/testdb.sh` erzeugt eine Testdatenbank mit sechs Wochen Arbeitszeiten
(KW 41–46/2026 plus Montag der KW 47, Wochensummen um 39 Stunden ± 10 %) und einigen
Regelverstößen: zu kurze Ruhezeit, Arbeit vor 7 und nach 22 Uhr, zu wenig Pause.

```bash
testdata/testdb.sh             # testdata/tagwerk.db neu bauen
testdata/testdb.sh --install   # zusätzlich als zentrale Datenbank einsetzen
testdata/testdb.sh --restore   # die echten Daten zurückholen
```

Mit `--install` wird die zentrale Datenbank vorher nach
`~/Library/Application Support/Tagwerk/vor-testdaten/` gesichert – außer sie enthält
schon Testdaten (die Testdatenbank ist dafür gekennzeichnet). `--restore` holt die
neueste Sicherung mit echten Daten zurück. Laufende Tagwerk-Instanzen werden dafür beendet und – wenn sie
als App liefen – danach neu gestartet. Ohne Austausch lässt sich die Testdatenbank auch
direkt öffnen:

```bash
TAGWERK_DIR=testdata cargo run -- --records
```

## Lizenz

Tagwerk steht unter der MIT-Lizenz – siehe [LICENSE](LICENSE).
