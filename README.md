# Tagwerk

Tagwerk ist eine kleine Menüleisten-App für macOS, die deine Arbeitszeit automatisch
erfasst: gezählt wird die Zeit, in der der Mac entsperrt ist. Arbeitsbeginn,
Mittagspause und Feierabend lassen sich zusätzlich von Hand festlegen oder korrigieren.

## Funktionen

- **Automatische Erfassung** – Sperren, Entsperren, Ruhezustand und Benutzerwechsel
  werden erkannt; jede entsperrte Phase wird als Periode gespeichert.
- **Menüleiste** – zeigt den Status (läuft, pausiert, Mittagspause, Feierabend) sowie
  die Arbeitszeit, die Pausen und den Beginn von heute.
- **Manuelle Korrekturen** – Arbeitsbeginn, Mittagspause und Feierabend jeweils
  „jetzt“, „vor N Minuten“ oder zu einer frei gewählten Uhrzeit. Korrekturen liegen als
  eigene Schicht über den erfassten Perioden und lassen sich jederzeit zurücknehmen.
- **Pausenvorschlag** – eine längere Lücke um die Mittagszeit (11:00–14:30 Uhr,
  mindestens 20 Minuten) kann mit einem Klick als Mittagspause eingetragen werden.
- **Hinweis zum Arbeitszeitgesetz** – warnt, wenn die Pausen nach § 4 ArbZG nicht
  reichen (30 Minuten ab 6 Stunden, 45 Minuten ab 9 Stunden Arbeit; gezählt werden
  Pausen ab 15 Minuten).
- **Fenster „Aufzeichnungen“** – Übersicht aller Tage, in der sich einzelne Tage
  nachträglich bearbeiten lassen.
- **Export als Markdown** – für einen frei wählbaren Zeitraum (Standard: die letzten
  vier Wochen) über das Fenster, das Menü oder die Kommandozeile.
- **Bei Anmeldung starten** – per Menüpunkt als Anmeldeobjekt eintragen
  (nur in der installierten App).

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
```

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

Sie enthält die automatisch erfassten Perioden (Mac entsperrt) und getrennt davon die
manuellen Korrekturen (Arbeitsbeginn, Pausen, Feierabend). Bearbeitet werden die Daten
im Fenster „Aufzeichnungen“; Änderungen dort übernimmt die laufende App automatisch.

Vor der ersten Änderung eines Tages wird die Datenbank unter
`backups/tagwerk-JJJJ-MM-TT.db` gesichert; die letzten 15 Backups bleiben erhalten.

### Export

Über „Exportieren …“ im Fenster „Aufzeichnungen“ (oder „Als Markdown exportieren …“ in
der Menüleiste) wird ein Zeitraum als Markdown-Datei gespeichert. Vorbelegt sind die
letzten vier Wochen (28 Tage bis heute); „Dieser Monat“ und „Letzter Monat“ lassen sich
mit einem Klick wählen. Die Datei enthält

1. eine **Übersichtstabelle** mit Beginn, Ende, Arbeits- und Pausenzeit je Tag und einer
   Summenzeile,
2. **`## Korrekturen`** – die manuellen Entscheidungen,
3. **`## Erfasste Perioden`** – die automatisch erfassten Zeiten.

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

## Lizenz

Tagwerk steht unter der MIT-Lizenz – siehe [LICENSE](LICENSE).
