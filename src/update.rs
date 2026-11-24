//! Updates: erkennt neuere Versionen in den GitHub-Releases von Tagwerk, lädt das DMG und
//! installiert es über die laufende App – ohne fremdes Framework, nach dem Vorbild von Barforge.
//!
//! Ablauf:
//! - **Suchen**: Die Menüleisten-App fragt kurz nach dem Start und danach stündlich die Liste
//!   der Releases ab (`api.github.com`, über `/usr/bin/curl`), außerdem beim Öffnen des Menüs,
//!   wenn die letzte Suche länger als 15 Minuten her ist. Beta-Versionen (Vorabversionen oder
//!   Versionen wie `1.9.0-beta.1`) zählen nur mit „Beta-Updates erhalten“. Gefunden wird die
//!   höchste Version über der laufenden; die Neuigkeiten umfassen alle übersprungenen Versionen.
//!   Eine neue Version meldet eine Mitteilung – einmal je Version.
//! - **Laden**: das DMG des Releases (nur von `github.com/<Repo>/releases/download/…`, nur
//!   HTTPS), höchstens so groß wie im Release angegeben; passt die SHA-256 aus der
//!   `.sha256`-Datei des Releases nicht, wird nichts installiert.
//! - **Installieren**: Ein Skript außerhalb der App wartet, bis Tagwerk beendet ist, prüft die
//!   Signatur des DMGs und der App darin (dieselbe Developer ID, dieselbe Bundle-Kennung,
//!   Gatekeeper), prüft die Version, tauscht das App-Bundle aus und startet Tagwerk neu. Scheitert
//!   ein Schritt, bleibt die alte App und startet wieder. Ist der Ordner der App nicht
//!   beschreibbar, läuft das Skript nach Eingabe eines Administrator-Kennworts.
//!   Das Ergebnis schreibt das Skript in den Datenordner (`.update-result`); der nächste Start
//!   meldet es.
//!
//! Installieren kann nur die Menüleisten-App ([`allow_install`]) aus `Tagwerk.app` mit einer
//! Developer-ID-Signatur. Ohne das (z. B. `cargo run`) wird nur gesucht, und das Update
//! lässt sich auf GitHub laden.
//!
//! Die Einstellungen (automatisch suchen, Beta-Updates, Neuigkeiten vor bzw. nach dem Update)
//! liegen in den Benutzereinstellungen von macOS, nicht in der Datenbank: Sie betreffen die
//! App, nicht die Aufzeichnungen.
//!
//! Zum Testen: `TAGWERK_SIMULATE_UPDATE=<Version>` täuscht ein verfügbares Update vor (ohne
//! Netz; installiert wird nichts).

use crate::store;
use chrono::{DateTime, Local};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};

/// Repository der Releases.
pub const REPO: &str = "rfoerthe/tagwerk";
/// Laufende Version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Changelog dieser Version – für die Neuigkeiten nach einem Update.
const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Obergrenze für das DMG (es hat wenige MB); stoppt nur eine Antwort ohne Ende.
const DOWNLOAD_CEILING: u64 = 200 * 1024 * 1024;

// ---------- Versionen ----------

/// Bestandteil einer Vorabkennung (`beta`, `1`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Text(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self, other) {
            (Ident::Num(a), Ident::Num(b)) => a.cmp(b),
            (Ident::Text(a), Ident::Text(b)) => a.cmp(b),
            // Zahlen stehen vor Text (Semantic Versioning 2.0.0, Abschnitt 11).
            (Ident::Num(_), Ident::Text(_)) => Less,
            (Ident::Text(_), Ident::Num(_)) => Greater,
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Version nach Semantic Versioning: `1.9.0`, `v1.9.0-beta.1` (Build-Angaben nach `+` zählen
/// nicht).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<Ident>,
}

impl Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (s, None),
        };
        let nums: Vec<u64> = core.split('.').map(|p| p.parse().ok()).collect::<Option<_>>()?;
        if nums.is_empty() || nums.len() > 3 {
            return None;
        }
        let mut c = [0; 3];
        c[..nums.len()].copy_from_slice(&nums);
        let pre = match pre {
            None => Vec::new(),
            Some(p) => {
                let ids: Vec<Ident> = p
                    .split('.')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse().map(Ident::Num).unwrap_or_else(|_| Ident::Text(s.to_string())))
                    .collect();
                if ids.is_empty() {
                    return None;
                }
                ids
            }
        };
        Some(Self { core: c, pre })
    }

    /// Vorabversion (Beta, RC …)?
    pub fn is_pre(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Equal,
            // 1.9.0-beta.1 < 1.9.0
            (true, false) => Greater,
            (false, true) => Less,
            (false, false) => self.pre.cmp(&other.pre),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Ist `version` (ohne „v“) eine Beta-Version?
pub fn is_beta(version: &str) -> bool {
    Version::parse(version).is_some_and(|v| v.is_pre())
}

// ---------- Releases ----------

/// Datei eines Releases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub url: String,
    /// Größe laut GitHub (0: unbekannt).
    pub size: u64,
}

/// Ein Release, wie GitHub es meldet (ohne Entwürfe).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Version ohne „v“, z. B. `1.9.0`.
    pub name: String,
    version: Version,
    /// Auf GitHub als Vorabversion markiert.
    pub prerelease: bool,
    /// Seite des Releases.
    pub page: String,
    pub dmg: Option<Asset>,
    pub sha: Option<Asset>,
    /// Neuigkeiten (Markdown).
    pub notes: String,
}

/// Nur Dateien dieses Repositorys über HTTPS – `curl` lädt sonst auch `file://`.
fn trusted_download(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

/// Liest die Antwort von `GET /repos/<Repo>/releases`.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Die Antwort von GitHub ist unlesbar ({e})."))?;
    let list = value.as_array().ok_or("Die Antwort von GitHub enthält keine Releases.")?;
    let mut releases = Vec::new();
    for r in list {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let Some(tag) = r["tag_name"].as_str() else { continue };
        let Some(version) = Version::parse(tag) else { continue };
        let name = tag.trim().trim_start_matches(['v', 'V']).to_string();
        let assets: Vec<(String, Asset)> = r["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|a| {
                        let file = a["name"].as_str()?;
                        let url = a["browser_download_url"].as_str().filter(|u| trusted_download(u))?;
                        Some((file.to_string(), Asset { url: url.to_string(), size: a["size"].as_u64().unwrap_or(0) }))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Am liebsten das DMG mit dem erwarteten Namen, sonst irgendeins.
        let expected = format!("Tagwerk-{name}.dmg");
        let dmg = assets
            .iter()
            .find(|(f, _)| *f == expected)
            .or_else(|| assets.iter().find(|(f, _)| f.ends_with(".dmg")));
        let sha = dmg.and_then(|(f, _)| assets.iter().find(|(s, _)| *s == format!("{f}.sha256")));
        releases.push(Release {
            name,
            version,
            prerelease: r["prerelease"].as_bool() == Some(true),
            page: r["html_url"].as_str().unwrap_or_default().to_string(),
            dmg: dmg.map(|(_, a)| a.clone()),
            sha: sha.map(|(_, a)| a.clone()),
            notes: r["body"].as_str().unwrap_or_default().to_string(),
        });
    }
    Ok(releases)
}

/// Ein Update, das sich installieren lässt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Version ohne „v“.
    pub version: String,
    pub beta: bool,
    pub page: String,
    pub dmg: Asset,
    pub sha: Option<Asset>,
    /// Neuigkeiten aller Versionen seit der laufenden (Version, Markdown), die neueste zuerst.
    pub notes: Vec<(String, String)>,
}

/// Das beste Update für `current`: die höchste neuere Version mit DMG im gewählten Kanal.
pub fn select(releases: &[Release], current: &str, betas: bool) -> Option<Offer> {
    let current = Version::parse(current);
    let mut eligible: Vec<&Release> = releases
        .iter()
        .filter(|r| current.as_ref().is_none_or(|c| r.version > *c))
        .filter(|r| betas || !(r.prerelease || r.version.is_pre()))
        .collect();
    eligible.sort_by(|a, b| b.version.cmp(&a.version));
    let best = *eligible.iter().find(|r| r.dmg.is_some())?;
    let notes = eligible
        .iter()
        .filter(|r| r.version <= best.version)
        .map(|r| (r.name.clone(), r.notes.clone()))
        .collect();
    Some(Offer {
        version: best.name.clone(),
        beta: best.prerelease || best.version.is_pre(),
        page: best.page.clone(),
        dmg: best.dmg.clone()?,
        sha: best.sha.clone(),
        notes,
    })
}

// ---------- Neuigkeiten ----------

/// Ein Absatz der Neuigkeiten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Bullet(String),
    Para(String),
}

/// Markdown der Release-Notizen bzw. des Changelogs in Absätze zerlegen: Überschriften,
/// Aufzählungen (mit eingerückten Folgezeilen) und Absätze (umbrochene Zeilen werden
/// zusammengefügt). Inline-Auszeichnung fällt weg. Der Abschnitt „Installation“, den
/// `github-release.sh` anhängt, gehört nicht zu den Neuigkeiten.
pub fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut cur: Option<Block> = None;
    let flush = |cur: &mut Option<Block>, out: &mut Vec<Block>| {
        if let Some(b) = cur.take() {
            out.push(b);
        }
    };
    for line in md.lines() {
        let trimmed = line.trim();
        if let Some(h) = trimmed.strip_prefix('#') {
            flush(&mut cur, &mut out);
            let h = h.trim_start_matches('#').trim();
            if h.eq_ignore_ascii_case("Installation") {
                break;
            }
            if !h.is_empty() {
                out.push(Block::Heading(inline(h)));
            }
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut cur, &mut out);
            continue;
        }
        // Verweise am Ende des Changelogs: [1.8.0]: https://…
        if trimmed.starts_with('[') && trimmed.contains("]: ") {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
            flush(&mut cur, &mut out);
            cur = Some(Block::Bullet(inline(item)));
            continue;
        }
        let text = inline(trimmed);
        match &mut cur {
            Some(Block::Bullet(b)) if line.starts_with(' ') || line.starts_with('\t') => {
                b.push(' ');
                b.push_str(&text);
            }
            Some(Block::Para(p)) => {
                p.push(' ');
                p.push_str(&text);
            }
            _ => {
                flush(&mut cur, &mut out);
                cur = Some(Block::Para(text));
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// `**fett**`, `` `Code` `` und `[Text](URL)` als schlichter Text.
fn inline(s: &str) -> String {
    let s = s.replace("**", "").replace('`', "");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match after.find("](").and_then(|close| after[close + 2..].find(')').map(|end| (close, close + 2 + end))) {
            Some((close, end)) => {
                out.push_str(&rest[..open]);
                out.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Abschnitte des Changelogs `text` für die Versionen über `after` bis einschließlich `upto`,
/// die neueste zuerst.
pub fn changelog_notes(text: &str, after: Option<&Version>, upto: &Version) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## [") {
            if let Some(s) = current.take() {
                sections.push(s);
            }
            let name = rest.split(']').next().unwrap_or_default().to_string();
            current = Some((name, String::new()));
        } else if line.starts_with('[') && line.contains("]: ") {
            // Verweise am Ende
            if let Some(s) = current.take() {
                sections.push(s);
            }
        } else if let Some((_, body)) = &mut current {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(s) = current.take() {
        sections.push(s);
    }
    sections
        .into_iter()
        .filter(|(name, _)| {
            Version::parse(name).is_some_and(|v| v <= *upto && after.is_none_or(|a| v > *a))
        })
        .collect()
}

// ---------- Herunterladen ----------

/// Wie viele Bytes der Download schreiben darf: die Größe laut Release, wenn sie vernünftig ist.
fn byte_limit(expected: u64) -> u64 {
    if expected > 0 && expected <= DOWNLOAD_CEILING {
        expected
    } else {
        DOWNLOAD_CEILING
    }
}

/// Taugt der geladene Inhalt als DMG des Releases? (Die Signatur prüft das Skript.)
fn download_ok(received: u64, expected: u64) -> bool {
    received > 0 && received <= DOWNLOAD_CEILING && (expected == 0 || received == expected)
}

/// SHA-256 aus einer `.sha256`-Datei (`<hex>  <Datei>`).
fn parse_sha256(text: &str) -> Option<String> {
    let hex = text.split_whitespace().next()?.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

fn curl() -> std::process::Command {
    let mut c = std::process::Command::new("/usr/bin/curl");
    c.args(["--silent", "--show-error", "--location", "--proto", "=https", "--proto-redir", "=https"])
        .args(["--user-agent", &format!("Tagwerk/{CURRENT}")])
        .stdin(std::process::Stdio::null());
    c
}

/// Text von `url` laden.
fn http_get(url: &str, accept: &str) -> Result<String, String> {
    let out = curl()
        .args(["--max-time", "30", "--header", &format!("Accept: {accept}"), "--write-out", "\n%{http_code}", url])
        .output()
        .map_err(|e| format!("curl ließ sich nicht starten ({e})."))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim().trim_start_matches("curl: ").trim();
        return Err(if err.is_empty() { "Keine Verbindung zu GitHub.".to_string() } else { err.to_string() });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    let code: u16 = code.trim().parse().unwrap_or(0);
    match code {
        200..=299 => Ok(body.to_string()),
        403 | 429 => Err(format!("GitHub begrenzt gerade die Anfragen (HTTP {code}). Später erneut versuchen.")),
        _ => Err(format!("GitHub antwortet mit HTTP {code}.")),
    }
}

/// Die Releases abfragen. GitHub liefert ab und zu eine leere Liste für ein Repository mit
/// Releases und Sekunden später die volle – dann noch zweimal fragen.
fn fetch_releases() -> Result<Vec<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=30");
    for attempt in 0..3 {
        let releases = parse_releases(&http_get(&url, "application/vnd.github+json")?)?;
        if !releases.is_empty() || attempt == 2 {
            return Ok(releases);
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    Ok(Vec::new())
}

/// Lädt das DMG nach `file` und prüft Größe und SHA-256.
fn download(offer: &Offer, file: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(file);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let limit = byte_limit(offer.dmg.size);
    let out = curl()
        .args(["--fail", "--max-time", "900", "--max-filesize", &limit.to_string(), "--output"])
        .arg(file)
        .arg(&offer.dmg.url)
        .output()
        .map_err(|e| format!("curl ließ sich nicht starten ({e})."))?;
    let fail = |msg: String| {
        let _ = std::fs::remove_file(file);
        Err(msg)
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return fail(format!("Laden fehlgeschlagen: {}", err.trim().trim_start_matches("curl: ").trim()));
    }
    let received = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    if !download_ok(received, offer.dmg.size) {
        return fail(format!("Das geladene Update ist unvollständig ({received} von {} Bytes).", offer.dmg.size));
    }
    if let Some(sha) = &offer.sha {
        let expected = parse_sha256(&http_get(&sha.url, "application/octet-stream")?);
        let actual = std::process::Command::new("/usr/bin/shasum")
            .args(["-a", "256"])
            .arg(file)
            .output()
            .ok()
            .and_then(|o| parse_sha256(&String::from_utf8_lossy(&o.stdout)));
        if expected.is_none() || expected != actual {
            return fail("Die Prüfsumme des geladenen Updates stimmt nicht.".to_string());
        }
    }
    Ok(())
}

// ---------- Installieren ----------

/// Wer das Update signiert haben muss: Bundle-Kennung und Team-ID der laufenden App.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signer {
    bundle_id: String,
    team_id: String,
}

impl Signer {
    /// Nur schlichte Zeichen – beides landet in den Signaturanforderungen des Skripts.
    fn new(bundle_id: &str, team_id: &str) -> Option<Self> {
        let id_ok = !bundle_id.is_empty()
            && bundle_id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && bundle_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        let team_ok = team_id.len() == 10 && team_id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        (id_ok && team_ok).then(|| Self { bundle_id: bundle_id.to_string(), team_id: team_id.to_string() })
    }

    /// Aus der Ausgabe von `codesign -dv --verbose=2`.
    fn from_codesign(output: &str) -> Option<Self> {
        let field = |key: &str| output.lines().find_map(|l| l.strip_prefix(key)).map(str::trim);
        Self::new(field("Identifier=")?, field("TeamIdentifier=")?)
    }

    /// Das DMG: von diesem Team mit einer Developer ID signiert.
    fn dmg_requirement(&self) -> String {
        format!("anchor apple generic and certificate leaf[subject.OU] = \"{}\"", self.team_id)
    }

    /// Die App im DMG: dieselbe App vom selben Team.
    fn app_requirement(&self) -> String {
        format!("identifier \"{}\" and {}", self.bundle_id, self.dmg_requirement())
    }
}

/// Das App-Bundle, aus dem Tagwerk läuft (`…/Tagwerk.app`), sonst `None` (z. B. `cargo run`).
fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if !exe.to_string_lossy().contains(".app/Contents/MacOS/") {
        return None;
    }
    exe.ancestors().nth(3).map(Path::to_path_buf)
}

/// Signatur der laufenden App (einmal abgefragt).
fn signer() -> Option<Signer> {
    static SIGNER: std::sync::OnceLock<Option<Signer>> = std::sync::OnceLock::new();
    SIGNER
        .get_or_init(|| {
            let bundle = app_bundle()?;
            let out = std::process::Command::new("/usr/bin/codesign")
                .args(["-dv", "--verbose=2"])
                .arg(&bundle)
                .output()
                .ok()?;
            // codesign schreibt die Angaben nach stderr.
            Signer::from_codesign(&String::from_utf8_lossy(&out.stderr))
        })
        .clone()
}

/// Wo die App liegt – entscheidet, ob und wie sie sich ersetzen lässt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Location {
    /// Ordner beschreibbar: Das Skript läuft als dieser Benutzer.
    Writable,
    /// Ordner nicht beschreibbar: Das Skript braucht ein Administrator-Kennwort.
    NeedsAdmin,
    /// Schreibgeschützt (im DMG, App Translocation): Ersetzen geht gar nicht.
    Immutable,
}

fn location(bundle: &Path) -> Location {
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Location::Immutable;
    }
    let Some(dir) = bundle.parent() else { return Location::Immutable };
    let probe = dir.join(format!(".tagwerk-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Location::Writable
        }
        // EROFS: schreibgeschütztes Volume
        Err(e) if e.raw_os_error() == Some(30) => Location::Immutable,
        Err(_) => Location::NeedsAdmin,
    }
}

/// Ergebnisdatei des Skripts im Datenordner.
fn result_file() -> PathBuf {
    store::data_dir().join(".update-result")
}

/// Ein Fehlercode des Skripts (`fail-…`) oder `None` für „fertig“.
fn failure_code(marker: &str) -> Option<String> {
    let m = marker.trim();
    m.starts_with("fail").then(|| m.to_string())
}

/// Ist das Skript beim Kopieren oder Tauschen gescheitert, fehlte vermutlich das Schreibrecht:
/// Dann gleich mit Administrator-Kennwort versuchen.
fn force_admin(last_failure: Option<&str>) -> bool {
    matches!(last_failure, Some("fail-copy" | "fail-swap"))
}

/// Für `sh` in einfache Anführungszeichen setzen.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Als Zeichenkette für AppleScript.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))
}

/// Das Installationsskript. Argumente: $1 App, $2 DMG, $3 PID (darauf warten), $4
/// Ergebnisdatei, $5 Benutzer-ID (für den Neustart, wenn das Skript als root läuft), $6 die
/// erwartete Version.
///
/// Vor jedem Schritt, der scheitern kann, steht sein Code in `<Ergebnis>.progress`; erst am
/// Ende wird daraus die Ergebnisdatei – mit `ok` oder dem Code des gescheiterten Schritts.
fn installer_script(signer: &Signer) -> String {
    r##"#!/bin/sh
APP="$1"; DMG="$2"; PID="$3"; RESULT="$4"; ASUSER="$5"; EXPECTED_VERSION="$6"
SCRIPT="$0"
running_as_root() { [ "$(/usr/bin/id -u)" = "0" ]; }
note() {
    if running_as_root; then
        [ -n "$ASUSER" ] || return 1
        # Der Datenordner gehört dem Benutzer: erst die Rechte abgeben, dann schreiben.
        /usr/bin/sudo -n -u "#$ASUSER" /bin/sh -c '/bin/echo "$1" > "$2.progress"' marker "$1" "$RESULT" 2>/dev/null
        return
    fi
    /bin/echo "$1" > "$RESULT.progress" 2>/dev/null
}
finalize() {
    if running_as_root; then
        [ -n "$ASUSER" ] || return 1
        /usr/bin/sudo -n -u "#$ASUSER" /bin/mv -f "$RESULT.progress" "$RESULT" 2>/dev/null
        return
    fi
    /bin/mv -f "$RESULT.progress" "$RESULT" 2>/dev/null
}
cleanup_script() { case "$SCRIPT" in /*.sh) /bin/rm -f "$SCRIPT";; esac; }
relaunch() {
    if [ -n "$ASUSER" ] && running_as_root; then
        /bin/launchctl asuser "$ASUSER" /usr/bin/open "$1" && return
    fi
    /usr/bin/open "$1"
}
give_up() {
    /bin/rm -f "$DMG"
    finalize
    relaunch "$APP"
    cleanup_script
    exit 1
}
while kill -0 "$PID" 2>/dev/null; do sleep 0.3; done
note fail-dmg-verify
/usr/bin/codesign -v --strict -R='@DMG_REQ@' "$DMG" 2>/dev/null || give_up
note fail-tempdir
MNT="$(/usr/bin/mktemp -d)" || give_up
note fail-mount
if ! /usr/bin/hdiutil attach "$DMG" -nobrowse -readonly -quiet -mountpoint "$MNT"; then
    /bin/rmdir "$MNT" 2>/dev/null
    give_up
fi
SRC="$MNT/Tagwerk.app"
LAUNCH="$APP"
if [ ! -d "$SRC" ]; then
    note fail-no-app-in-dmg
else
    DIR="$(/usr/bin/dirname "$APP")"
    # Erst vollständig kopieren, dann tauschen: Scheitert das Kopieren, bleibt die alte App.
    # Der Name ist versteckt, damit Spotlight die Kopie nicht als App aufnimmt.
    STAGE="$DIR/.Tagwerk.app.update-new"
    /bin/rm -rf "$STAGE"
    note fail-copy
    if /usr/bin/ditto "$SRC" "$STAGE"; then
        # Alle erweiterten Attribute weg (Quarantäne, FinderInfo aus dem DMG).
        /usr/bin/xattr -cr "$STAGE" 2>/dev/null
        note fail-version
        VERSION="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$STAGE/Contents/Info.plist" 2>/dev/null)"
        if [ "$VERSION" = "$EXPECTED_VERSION" ]; then
            # Mit abgeschaltetem Gatekeeper lehnt spctl alles ab; dann bleibt codesign die Prüfung.
            GATEKEEPER_OK=0
            if /usr/sbin/spctl --status 2>/dev/null | /usr/bin/grep -q disabled; then
                GATEKEEPER_OK=1
            elif /usr/sbin/spctl -a -t exec "$STAGE" >/dev/null 2>&1; then
                GATEKEEPER_OK=1
            fi
            note fail-verify
            if /usr/bin/codesign -v --deep --strict -R='@APP_REQ@' "$STAGE" 2>/dev/null \
                && [ "$GATEKEEPER_OK" = 1 ]; then
                note fail-swap
                BACKUP="$APP.update-old.$PID"
                /bin/rm -rf "$APP".update-old.* 2>/dev/null
                if /bin/mv "$APP" "$BACKUP" && /bin/mv "$STAGE" "$APP"; then
                    note ok
                    # Als root installiert: Die App gehört wieder dem Benutzer, sonst scheitert
                    # das nächste Update ohne Kennwort.
                    if running_as_root && [ -n "$ASUSER" ]; then
                        /usr/sbin/chown -R "$ASUSER" "$APP" 2>/dev/null
                    fi
                    /bin/rm -rf "$BACKUP"
                else
                    [ -d "$BACKUP" ] && [ ! -d "$APP" ] && /bin/mv "$BACKUP" "$APP"
                fi
            fi
        fi
    fi
    /bin/rm -rf "$STAGE"
fi
/usr/bin/hdiutil detach "$MNT" -quiet 2>/dev/null || /usr/bin/hdiutil detach "$MNT" -force -quiet 2>/dev/null
/bin/rmdir "$MNT" 2>/dev/null
/bin/rm -f "$DMG"
finalize
relaunch "$LAUNCH"
cleanup_script
"##
    .replace("@DMG_REQ@", &signer.dmg_requirement())
    .replace("@APP_REQ@", &signer.app_requirement())
}

/// Befehl für `do shell script … with administrator privileges`: das Skript inline (keine
/// Datei, die jemand vor dem Start als root ändern könnte), in einer eigenen Sitzung, damit es
/// das Ende von Tagwerk überlebt. macOS hat kein `setsid(1)`; perl kann `setsid(2)` aufrufen.
fn elevated_command(script: &str, args: &[String]) -> String {
    let args: Vec<String> = args.iter().map(|a| sh_quote(a)).collect();
    let argv = format!("/bin/sh -c {} tagwerk-installer {}", sh_quote(script), args.join(" "));
    let setsid = "/usr/bin/perl -e 'use POSIX (); POSIX::setsid(); $SIG{HUP} = \"IGNORE\"; exec @ARGV; exit 127'";
    format!(
        "set -- {argv}; {{ if [ -x /usr/bin/perl ]; then exec {setsid} \"$@\"; \
         else exec /usr/bin/nohup \"$@\"; fi; }} >/dev/null 2>&1 &"
    )
}

mod app {
    //! Zustand, Zeitgeber, Dialoge – alles im Hauptthread.

    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSAlertThirdButtonReturn, NSApplication, NSAutoresizingMaskOptions,
        NSBorderType, NSColor, NSFont, NSFontAttributeName, NSFontWeightSemibold, NSForegroundColorAttributeName,
        NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScrollView, NSTextView, NSWorkspace,
    };
    use objc2_foundation::{
        NSArray, NSAttributedString, NSDictionary, NSMutableAttributedString, NSPoint, NSRect, NSRunLoop,
        NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSURL, NSUserDefaults,
    };
    use std::cell::RefCell;
    use std::ptr::NonNull;

    fn ns(s: &str) -> Retained<NSString> {
        NSString::from_str(s)
    }

    /// Stand der Updates.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub enum State {
        /// Noch nicht gesucht.
        #[default]
        Idle,
        Checking,
        UpToDate,
        Available(Offer),
        /// Anteil, sobald die Größe bekannt ist.
        Downloading(Option<f64>),
        Installing,
        Failed(String),
    }

    enum Job {
        Check { rx: Receiver<Result<Vec<Release>, String>>, manual: bool },
        Download { rx: Receiver<Result<(), String>>, file: PathBuf, offer: Offer },
        Admin { rx: Receiver<Result<bool, String>>, file: PathBuf, offer: Offer },
    }

    #[derive(Default)]
    struct Updater {
        state: State,
        last_checked: Option<DateTime<Local>>,
        listeners: Vec<fn()>,
        job: Option<Job>,
        poll: Option<Retained<NSTimer>>,
        timers: Vec<Retained<NSTimer>>,
        install_here: bool,
    }

    thread_local! {
        static UPDATER: RefCell<Updater> = RefCell::new(Updater::default());
    }

    fn with<R>(f: impl FnOnce(&mut Updater) -> R) -> R {
        UPDATER.with(|u| f(&mut u.borrow_mut()))
    }

    // ----- Einstellungen (Benutzereinstellungen von macOS) -----

    const KEY_AUTO: &str = "UpdateAutoCheck";
    const KEY_BETA: &str = "UpdateIncludeBeta";
    const KEY_NOTES_BEFORE: &str = "UpdateNotesBefore";
    const KEY_NOTES_AFTER: &str = "UpdateNotesAfter";
    const KEY_NOTIFIED: &str = "UpdateNotifiedVersion";
    const KEY_FROM: &str = "UpdateFromVersion";
    const KEY_SHOWN: &str = "UpdateNotesShown";
    const KEY_FAILURE: &str = "UpdateLastFailure";

    fn defaults() -> Retained<NSUserDefaults> {
        NSUserDefaults::standardUserDefaults()
    }

    fn get_bool(key: &str, default: bool) -> bool {
        let d = defaults();
        if d.objectForKey(&ns(key)).is_some() {
            d.boolForKey(&ns(key))
        } else {
            default
        }
    }

    fn set_bool(key: &str, value: bool) {
        defaults().setBool_forKey(value, &ns(key));
    }

    fn get_string(key: &str) -> Option<String> {
        defaults().stringForKey(&ns(key)).map(|s| s.to_string())
    }

    fn set_string(key: &str, value: Option<&str>) {
        let d = defaults();
        match value {
            Some(v) => unsafe { d.setObject_forKey(Some(&ns(v)), &ns(key)) },
            None => d.removeObjectForKey(&ns(key)),
        }
    }

    /// Automatisch nach Updates suchen (Standard: ja).
    pub fn auto_check() -> bool {
        get_bool(KEY_AUTO, true)
    }

    pub fn set_auto_check(on: bool) {
        set_bool(KEY_AUTO, on);
        if on {
            check_if_stale();
        }
    }

    /// Beta-Versionen anbieten (Standard: ja, wenn gerade eine Beta läuft).
    pub fn include_beta() -> bool {
        get_bool(KEY_BETA, is_beta(CURRENT))
    }

    /// Der Kanal ändert, was als Update gilt: gleich neu suchen.
    pub fn set_include_beta(on: bool) {
        set_bool(KEY_BETA, on);
        if !matches!(state(), State::Idle) {
            check(true);
        }
    }

    /// Neuigkeiten vor dem Update zeigen (mit „Jetzt aktualisieren“ / „Später“).
    pub fn notes_before() -> bool {
        get_bool(KEY_NOTES_BEFORE, true)
    }

    pub fn set_notes_before(on: bool) {
        set_bool(KEY_NOTES_BEFORE, on);
    }

    /// Neuigkeiten nach dem Update zeigen – falls sie nicht schon davor zu sehen waren.
    pub fn notes_after() -> bool {
        get_bool(KEY_NOTES_AFTER, true)
    }

    pub fn set_notes_after(on: bool) {
        set_bool(KEY_NOTES_AFTER, on);
    }

    // ----- Stand -----

    pub fn state() -> State {
        with(|u| u.state.clone())
    }

    pub fn last_checked() -> Option<DateTime<Local>> {
        with(|u| u.last_checked)
    }

    /// Läuft gerade eine Suche (auch eine im Hintergrund, die ein Angebot stehen lässt)?
    pub fn busy() -> bool {
        with(|u| u.job.is_some()) || matches!(state(), State::Installing)
    }

    /// `f` wird nach jeder Änderung des Stands aufgerufen (im Hauptthread).
    pub fn subscribe(f: fn()) {
        with(|u| u.listeners.push(f));
    }

    fn emit() {
        let listeners = with(|u| u.listeners.clone());
        for f in listeners {
            f();
        }
    }

    fn set_state(state: State) {
        let changed = with(|u| {
            let changed = u.state != state;
            u.state = state;
            changed
        });
        if changed {
            emit();
        }
    }

    /// Die Menüleisten-App installiert; das eigenständige Fenster „Aufzeichnungen“ nicht (die
    /// Menüleisten-App liefe sonst mit der alten Version weiter).
    pub fn allow_install() {
        with(|u| u.install_here = true);
    }

    fn simulated() -> Option<String> {
        std::env::var("TAGWERK_SIMULATE_UPDATE").ok().filter(|v| Version::parse(v).is_some())
    }

    /// Warum sich ein Update hier nicht installieren lässt (`None`: es geht).
    pub fn install_blocker() -> Option<&'static str> {
        if simulated().is_some() {
            return None;
        }
        if app_bundle().is_none() {
            return Some("Installieren kann nur die App „Tagwerk.app“ – das Update lässt sich auf GitHub laden.");
        }
        if !with(|u| u.install_here) {
            return Some("Installieren lässt sich das Update in der Menüleisten-App von Tagwerk.");
        }
        if signer().is_none() {
            return Some(
                "Diese Tagwerk-App ist nicht mit einer Developer ID signiert – das Update lässt sich auf GitHub laden.",
            );
        }
        None
    }

    // ----- Zeitgeber -----

    fn after(seconds: f64, repeats: bool, f: impl Fn() + 'static) -> Retained<NSTimer> {
        let block = block2::RcBlock::new(move |_t: NonNull<NSTimer>| f());
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(seconds, repeats, &block) };
        // Auch während das Menü offen ist (der Fortschritt läuft dort sichtbar mit).
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
        timer
    }

    fn start_polling() {
        if with(|u| u.poll.is_some()) {
            return;
        }
        let timer = after(0.25, true, poll);
        with(|u| u.poll = Some(timer));
    }

    /// Start der Menüleisten-App: Ergebnis eines Updates melden, dann kurz nach dem Start und
    /// stündlich suchen (wenn eingeschaltet).
    pub fn start() {
        if let Some(version) = simulated() {
            set_state(State::Available(simulated_offer(&version)));
            return;
        }
        consume_install_result();
        let first = after(6.0, false, || {
            if auto_check() {
                check(false);
            }
        });
        let hourly = after(60.0 * 60.0, true, || {
            if auto_check() {
                check(false);
            }
        });
        with(|u| u.timers.extend([first, hourly]));
    }

    fn simulated_offer(version: &str) -> Offer {
        let current = Version::parse(CURRENT).expect("Version aus Cargo.toml");
        Offer {
            version: version.to_string(),
            beta: is_beta(version),
            page: format!("https://github.com/{REPO}/releases"),
            dmg: Asset { url: String::new(), size: 0 },
            sha: None,
            notes: changelog_notes(CHANGELOG, None, &current).into_iter().take(1).collect(),
        }
    }

    // ----- Suchen -----

    /// Nach Updates suchen. `manual`: aus den Einstellungen angestoßen (zeigt „Wird gesucht …“
    /// und meldet keine Mitteilung).
    pub fn check(manual: bool) {
        if busy() || matches!(state(), State::Downloading(_)) {
            return;
        }
        if let Some(version) = simulated() {
            with(|u| u.last_checked = Some(Local::now()));
            set_state(State::Available(simulated_offer(&version)));
            return;
        }
        // Eine Suche im Hintergrund lässt ein gefundenes Update stehen.
        if manual || !matches!(state(), State::Available(_)) {
            set_state(State::Checking);
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_releases());
        });
        with(|u| u.job = Some(Job::Check { rx, manual }));
        start_polling();
    }

    /// Suchen, wenn eingeschaltet und die letzte Suche länger als 15 Minuten her ist (beim
    /// Öffnen des Menüs).
    pub fn check_if_stale() {
        if !auto_check() || busy() {
            return;
        }
        if matches!(state(), State::Downloading(_) | State::Installing) {
            return;
        }
        let fresh = last_checked().is_some_and(|t| (Local::now() - t).num_minutes() < 15);
        if !fresh {
            check(false);
        }
    }

    fn finish_check(result: Result<Vec<Release>, String>, manual: bool) {
        with(|u| u.last_checked = Some(Local::now()));
        match result {
            Err(e) => {
                // Offline o. Ä.: Ein schon gefundenes Update bleibt bei einer Suche im Hintergrund.
                if manual || !matches!(state(), State::Available(_)) {
                    with(|u| u.state = State::Failed(e));
                }
                emit();
            }
            Ok(releases) => match select(&releases, CURRENT, include_beta()) {
                Some(offer) => {
                    if !manual && get_string(KEY_NOTIFIED).as_deref() != Some(&offer.version) {
                        set_string(KEY_NOTIFIED, Some(&offer.version));
                        crate::notify::post_text(
                            "update-available",
                            "Tagwerk-Update verfügbar",
                            &format!(
                                "Version {} lässt sich im Menü von Tagwerk installieren.",
                                offer.version
                            ),
                        );
                    }
                    with(|u| u.state = State::Available(offer));
                    emit();
                }
                None => {
                    with(|u| u.state = State::UpToDate);
                    emit();
                }
            },
        }
    }

    // ----- Zeitgeber für Hintergrundarbeit -----

    fn poll() {
        let Some(job) = with(|u| u.job.take()) else {
            stop_polling();
            return;
        };
        match job {
            Job::Check { rx, manual } => match rx.try_recv() {
                Ok(result) => {
                    stop_polling();
                    finish_check(result, manual);
                }
                Err(TryRecvError::Empty) => with(|u| u.job = Some(Job::Check { rx, manual })),
                Err(TryRecvError::Disconnected) => {
                    stop_polling();
                    finish_check(Err("Die Suche wurde abgebrochen.".to_string()), manual);
                }
            },
            Job::Download { rx, file, offer } => match rx.try_recv() {
                Ok(Ok(())) => {
                    stop_polling();
                    launch_installer(file, offer);
                }
                Ok(Err(e)) => {
                    stop_polling();
                    download_failed(&offer, &e);
                }
                Err(TryRecvError::Empty) => {
                    let total = offer.dmg.size;
                    if total > 0 {
                        let received = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
                        let fraction = (received as f64 / total as f64).min(1.0);
                        let step = |f: Option<f64>| f.map(|f| (f * 100.0) as i64);
                        if let State::Downloading(current) = state() {
                            if step(current) != step(Some(fraction)) {
                                set_state(State::Downloading(Some(fraction)));
                            }
                        }
                    }
                    with(|u| u.job = Some(Job::Download { rx, file, offer }));
                }
                Err(TryRecvError::Disconnected) => {
                    stop_polling();
                    download_failed(&offer, "Das Laden wurde abgebrochen.");
                }
            },
            Job::Admin { rx, file, offer } => match rx.try_recv() {
                Ok(Ok(true)) => {
                    stop_polling();
                    quit_for_install();
                }
                Ok(Ok(false)) => {
                    // Kennwortabfrage abgebrochen: Das Angebot bleibt.
                    stop_polling();
                    let _ = std::fs::remove_file(&file);
                    set_state(State::Available(offer));
                }
                Ok(Err(e)) => {
                    stop_polling();
                    let _ = std::fs::remove_file(&file);
                    set_state(State::Failed(e));
                }
                Err(TryRecvError::Empty) => with(|u| u.job = Some(Job::Admin { rx, file, offer })),
                Err(TryRecvError::Disconnected) => {
                    stop_polling();
                    set_state(State::Available(offer));
                }
            },
        }
    }

    fn stop_polling() {
        if let Some(t) = with(|u| u.poll.take()) {
            t.invalidate();
        }
    }

    /// Laden gescheitert: Das Angebot bleibt (zum erneuten Versuch), der Grund kommt als
    /// Mitteilung und Dialog.
    fn download_failed(offer: &Offer, reason: &str) {
        set_state(State::Available(offer.clone()));
        alert("Update nicht geladen", reason, None);
    }

    // ----- Installieren -----

    /// „Aktualisieren“ (Menü oder Einstellungen): Neuigkeiten zeigen (wenn eingeschaltet), das
    /// DMG laden und das Installationsskript starten. Tagwerk beendet sich danach und startet
    /// neu.
    pub fn install() {
        let State::Available(offer) = state() else { return };
        if let Some(reason) = install_blocker() {
            if alert("Update auf GitHub laden", reason, Some("Auf GitHub öffnen")) {
                open_release_page();
            }
            return;
        }
        let shown = notes_before();
        if shown && !confirm(&offer) {
            return;
        }
        if simulated().is_some() {
            alert("Simuliertes Update", "TAGWERK_SIMULATE_UPDATE ist gesetzt – es wird nichts geladen.", None);
            return;
        }
        let Some(bundle) = app_bundle() else { return };
        if location(&bundle) == Location::Immutable {
            alert(
                "Tagwerk in den Ordner „Programme“ bewegen",
                "Tagwerk läuft an einem Ort, der sich nicht aktualisieren lässt, etwa im Disk-Image. \
                 Tagwerk in den Ordner „Programme“ ziehen, von dort öffnen und es erneut versuchen.",
                None,
            );
            return;
        }
        set_bool(KEY_SHOWN, shown);
        let file = std::env::temp_dir().join(format!("tagwerk-update-{}", std::process::id())).join(
            format!("Tagwerk-{}.dmg", offer.version),
        );
        set_state(State::Downloading(None));
        let (tx, rx) = mpsc::channel();
        let (o, f) = (offer.clone(), file.clone());
        std::thread::spawn(move || {
            let _ = tx.send(download(&o, &f));
        });
        with(|u| u.job = Some(Job::Download { rx, file, offer }));
        start_polling();
    }

    /// Das Skript starten – als dieser Benutzer oder mit Administrator-Kennwort.
    fn launch_installer(dmg: PathBuf, offer: Offer) {
        let fail = |e: String| {
            let _ = std::fs::remove_file(&dmg);
            set_state(State::Failed(e));
        };
        let (Some(bundle), Some(signer)) = (app_bundle(), signer()) else {
            return fail("Die Signatur dieser App ließ sich nicht lesen.".to_string());
        };
        let result = result_file();
        let _ = std::fs::create_dir_all(store::data_dir());
        let _ = std::fs::remove_file(&result);
        let _ = std::fs::remove_file(result.with_extension("progress"));
        let args = vec![
            bundle.to_string_lossy().into_owned(),
            dmg.to_string_lossy().into_owned(),
            std::process::id().to_string(),
            result.to_string_lossy().into_owned(),
            unsafe { getuid() }.to_string(),
            offer.version.clone(),
        ];
        set_string(KEY_FROM, Some(CURRENT));
        set_state(State::Installing);
        let script = installer_script(&signer);
        let last_failure = get_string(KEY_FAILURE);
        if location(&bundle) == Location::Writable && !force_admin(last_failure.as_deref()) {
            let path = std::env::temp_dir().join(format!("tagwerk-update-{}.sh", std::process::id()));
            if let Err(e) = std::fs::write(&path, script) {
                return fail(e.to_string());
            }
            if let Err(e) = spawn_detached(&path, &args) {
                let _ = std::fs::remove_file(&path);
                return fail(format!("Das Installationsskript ließ sich nicht starten ({e})."));
            }
            quit_for_install();
        } else {
            let command = elevated_command(&script, &args);
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(run_as_admin(&command));
            });
            with(|u| u.job = Some(Job::Admin { rx, file: dmg, offer }));
            start_polling();
        }
    }

    unsafe extern "C" {
        fn getuid() -> u32;
        fn setsid() -> i32;
    }

    /// Startet das Skript in einer eigenen Sitzung: Es muss das Ende von Tagwerk überleben,
    /// auch wenn launchd die Prozesse der App abräumt.
    fn spawn_detached(script: &Path, args: &[String]) -> std::io::Result<()> {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg(script)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Nur setsid(2) zwischen fork und exec – das ist dort erlaubt.
        unsafe {
            cmd.pre_exec(|| {
                setsid();
                Ok(())
            });
        }
        cmd.spawn().map(|_| ())
    }

    /// `do shell script … with administrator privileges`. `Ok(false)`: abgebrochen.
    fn run_as_admin(command: &str) -> Result<bool, String> {
        let source = format!(
            "do shell script {} with prompt {} with administrator privileges",
            applescript_string(command),
            applescript_string("Tagwerk möchte ein Update installieren.")
        );
        let out = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &source])
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(true);
        }
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("-128") {
            return Ok(false);
        }
        Err(format!("Installation mit Administratorrechten fehlgeschlagen: {}", err.trim()))
    }

    /// Beenden, damit das Skript die App ersetzen kann (die Menüleisten-App sichert dabei den
    /// Stand, siehe `NSApplicationWillTerminateNotification`).
    fn quit_for_install() {
        let t = after(0.4, false, || {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        });
        with(|u| u.timers.push(t));
    }

    /// Nach dem Start: Ergebnis eines Updates melden.
    fn consume_install_result() {
        let result = result_file();
        // Ein Rest von `.progress` heißt: Das Skript läuft noch oder ist mittendrin gestorben.
        let _ = std::fs::remove_file(result.with_extension("progress"));
        let Ok(marker) = std::fs::read_to_string(&result) else { return };
        let _ = std::fs::remove_file(&result);
        let from = get_string(KEY_FROM);
        let shown = get_bool(KEY_SHOWN, false);
        set_string(KEY_FROM, None);
        defaults().removeObjectForKey(&ns(KEY_SHOWN));
        match failure_code(&marker) {
            None => {
                set_string(KEY_FAILURE, None);
                if from.as_deref() == Some(CURRENT) {
                    // Die alte App ist wieder gestartet, ohne dass etwas fehlschlug: nichts zu melden.
                    return;
                }
                let current = Version::parse(CURRENT).expect("Version aus Cargo.toml");
                let notes = changelog_notes(CHANGELOG, from.as_deref().and_then(Version::parse).as_ref(), &current);
                if notes_after() && !shown && !notes.is_empty() {
                    let t = after(1.0, false, move || show_notes_after(&notes));
                    with(|u| u.timers.push(t));
                } else {
                    crate::notify::post_text(
                        "update-installed",
                        "Tagwerk aktualisiert",
                        &format!("Tagwerk {CURRENT} ist installiert."),
                    );
                }
            }
            Some(code) => {
                crate::notify::post_text(
                    "update-failed",
                    "Tagwerk-Update nicht installiert",
                    &format!(
                        "Das Update wurde geladen, ließ sich aber nicht anwenden ({code}). Die neueste Version \
                         lässt sich auf GitHub laden."
                    ),
                );
                set_string(KEY_FAILURE, Some(&code));
            }
        }
    }

    // ----- Dialoge -----

    /// Öffnet die Seite des angebotenen Releases (oder die Liste der Releases).
    pub fn open_release_page() {
        let page = match state() {
            State::Available(o) if !o.page.is_empty() => o.page,
            _ => format!("https://github.com/{REPO}/releases"),
        };
        if let Some(url) = NSURL::URLWithString(&ns(&page)) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    fn activate(mtm: MainThreadMarker) {
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }

    /// Einfacher Hinweis; mit `action` ein zweiter Knopf. `true`: `action` gewählt.
    fn alert(title: &str, text: &str, action: Option<&str>) -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return false };
        let a = NSAlert::new(mtm);
        a.setMessageText(&ns(title));
        a.setInformativeText(&ns(text));
        if let Some(action) = action {
            a.addButtonWithTitle(&ns(action));
            a.addButtonWithTitle(&ns("Abbrechen"));
        } else {
            a.addButtonWithTitle(&ns("OK"));
        }
        activate(mtm);
        a.runModal() == NSAlertFirstButtonReturn && action.is_some()
    }

    /// Neuigkeiten vor dem Update: „Jetzt aktualisieren“ oder „Später“.
    fn confirm(offer: &Offer) -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return false };
        let a = NSAlert::new(mtm);
        let beta = if offer.beta { " (Beta)" } else { "" };
        a.setMessageText(&ns(&format!("Tagwerk {}{beta} ist verfügbar", offer.version)));
        a.setInformativeText(&ns(&format!(
            "Installiert ist {CURRENT}. Tagwerk lädt das Update, prüft die Signatur und startet neu; \
             die laufende Erfassung wird vorher gesichert."
        )));
        if !offer.notes.iter().all(|(_, n)| n.trim().is_empty()) {
            a.setAccessoryView(Some(&notes_view(mtm, &offer.notes)));
        }
        a.addButtonWithTitle(&ns("Jetzt aktualisieren"));
        a.addButtonWithTitle(&ns("Später"));
        let more = !offer.page.is_empty();
        if more {
            a.addButtonWithTitle(&ns("Auf GitHub ansehen"));
        }
        activate(mtm);
        let answer = a.runModal();
        if more && answer == NSAlertThirdButtonReturn {
            open_release_page();
        }
        answer == NSAlertFirstButtonReturn
    }

    /// Neuigkeiten nach einem Update.
    fn show_notes_after(notes: &[(String, String)]) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let a = NSAlert::new(mtm);
        a.setMessageText(&ns(&format!("Tagwerk wurde auf {CURRENT} aktualisiert")));
        a.setInformativeText(&ns("Das ist neu:"));
        a.setAccessoryView(Some(&notes_view(mtm, notes)));
        a.addButtonWithTitle(&ns("OK"));
        activate(mtm);
        a.runModal();
    }

    /// Neuigkeiten als Text in einer Rolle.
    fn notes_view(mtm: MainThreadMarker, notes: &[(String, String)]) -> Retained<NSScrollView> {
        let text = NSMutableAttributedString::new();
        let size = NSFont::systemFontSize();
        let append = |s: &str, font: Retained<NSFont>, indent: bool, space_before: f64| {
            let style = NSMutableParagraphStyle::new();
            style.setParagraphSpacing(4.0);
            style.setParagraphSpacingBefore(space_before);
            if indent {
                style.setHeadIndent(14.0);
                style.setDefaultTabInterval(14.0);
                style.setTabStops(Some(&NSArray::new()));
            }
            let keys = unsafe { [NSFontAttributeName, NSParagraphStyleAttributeName, NSForegroundColorAttributeName] };
            let values: [Retained<AnyObject>; 3] = [
                Retained::into_super(Retained::into_super(font)),
                Retained::into_super(Retained::into_super(Retained::into_super(style))),
                Retained::into_super(Retained::into_super(NSColor::labelColor())),
            ];
            let attrs = NSDictionary::from_retained_objects(&keys, &values);
            // Schrift, Absatzstil und Farbe sind gültige Werte für diese Schlüssel.
            let part = unsafe { NSAttributedString::new_with_attributes(&ns(&format!("{s}\n")), &attrs) };
            text.appendAttributedString(&part);
        };
        let regular = || NSFont::systemFontOfSize(size - 1.0);
        let bold = |s: f64| NSFont::systemFontOfSize_weight(s, unsafe { NSFontWeightSemibold });
        for (i, (version, md)) in notes.iter().enumerate() {
            if notes.len() > 1 {
                append(&format!("Version {version}"), bold(size + 1.0), false, if i == 0 { 0.0 } else { 12.0 });
            }
            for block in blocks(md) {
                match block {
                    Block::Heading(h) => append(&h, bold(size - 1.0), false, 6.0),
                    Block::Bullet(b) => append(&format!("•\t{b}"), regular(), true, 0.0),
                    Block::Para(p) => append(&p, regular(), false, 0.0),
                }
            }
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(460.0, 260.0));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(NSBorderType::BezelBorder);
        let content = scroll.contentSize();
        let tv = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), content),
        );
        tv.setEditable(false);
        tv.setSelectable(true);
        tv.setVerticallyResizable(true);
        tv.setHorizontallyResizable(false);
        tv.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        tv.setMinSize(NSSize::new(0.0, content.height));
        tv.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        tv.setTextContainerInset(NSSize::new(6.0, 8.0));
        if let Some(container) = unsafe { tv.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        if let Some(storage) = unsafe { tv.textStorage() } {
            storage.setAttributedString(&text);
        }
        scroll.setDocumentView(Some(&tv));
        scroll
    }

    /// „Zuletzt gesucht: 24.11.26, 15:51“
    pub fn last_checked_text() -> Option<String> {
        last_checked().map(|t| format!("Zuletzt gesucht: {}", t.format("%d.%m.%y, %H:%M")))
    }

    /// Kurzer Stand für die Einstellungen: (SF-Symbol, Farbe, Text).
    pub fn status(state: &State) -> Option<(&'static str, Retained<NSColor>, String)> {
        Some(match state {
            State::Idle => return None,
            State::Checking => ("arrow.triangle.2.circlepath", NSColor::secondaryLabelColor(), "Wird gesucht …".into()),
            State::UpToDate => ("checkmark.circle.fill", NSColor::systemGreenColor(), "Du verwendest die neueste Version.".into()),
            State::Available(o) => (
                "arrow.down.circle.fill",
                if o.beta { NSColor::systemOrangeColor() } else { NSColor::controlAccentColor() },
                format!("Update verfügbar: {}{}", o.version, if o.beta { " (Beta)" } else { "" }),
            ),
            State::Downloading(Some(f)) => {
                ("arrow.down.circle", NSColor::secondaryLabelColor(), format!("Update wird geladen … {} %", (f * 100.0) as i64))
            }
            State::Downloading(None) => ("arrow.down.circle", NSColor::secondaryLabelColor(), "Update wird geladen …".into()),
            State::Installing => ("gearshape.2.fill", NSColor::secondaryLabelColor(), "Wird installiert und neu gestartet …".into()),
            State::Failed(e) => ("exclamationmark.triangle.fill", NSColor::systemOrangeColor(), format!("Suche fehlgeschlagen: {e}")),
        })
    }
}

pub use app::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_follow_semver() {
        assert!(v("1.9.0") > v("1.8.0"));
        assert!(v("1.10.0") > v("1.9.9"));
        assert!(v("v2.0") > v("1.99.99"));
        assert!(v("1.9.0-beta.1") < v("1.9.0"));
        assert!(v("1.9.0-beta.2") > v("1.9.0-beta.1"));
        assert!(v("1.9.0-beta.10") > v("1.9.0-beta.9"));
        assert!(v("1.9.0-rc.1") > v("1.9.0-beta.5"));
        assert!(v("1.9.0-beta") < v("1.9.0-beta.1"));
        assert!(v("1.9.0-1") < v("1.9.0-alpha"));
        assert_eq!(v("1.9.0+build.5"), v("1.9.0"));
        assert!(v("1.9.0-beta.1").is_pre());
        assert!(!v("1.9.0").is_pre());
        for bad in ["", "x", "1.2.3.4", "1..2", "1.2.3-", "v"] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    const SAMPLE: &str = r#"[
      {"tag_name":"v1.10.0-beta.1","draft":false,"prerelease":true,"html_url":"https://github.com/rfoerthe/tagwerk/releases/tag/v1.10.0-beta.1","body":"Beta",
       "assets":[{"name":"Tagwerk-1.10.0-beta.1.dmg","size":100,"browser_download_url":"https://github.com/rfoerthe/tagwerk/releases/download/v1.10.0-beta.1/Tagwerk-1.10.0-beta.1.dmg"}]},
      {"tag_name":"v2.0.0","draft":true,"prerelease":false,"assets":[]},
      {"tag_name":"v1.9.1","draft":false,"prerelease":false,"html_url":"p191","body":"Fix",
       "assets":[{"name":"Tagwerk-1.9.1.dmg","size":200,"browser_download_url":"https://github.com/rfoerthe/tagwerk/releases/download/v1.9.1/Tagwerk-1.9.1.dmg"},
                 {"name":"Tagwerk-1.9.1.dmg.sha256","size":80,"browser_download_url":"https://github.com/rfoerthe/tagwerk/releases/download/v1.9.1/Tagwerk-1.9.1.dmg.sha256"}]},
      {"tag_name":"v1.9.0","draft":false,"prerelease":false,"html_url":"p190","body":"Neu",
       "assets":[{"name":"Tagwerk-1.9.0.dmg","size":300,"browser_download_url":"file:///etc/passwd"}]},
      {"tag_name":"v1.8.0","draft":false,"prerelease":false,"body":"Alt",
       "assets":[{"name":"Tagwerk-1.8.0.dmg","size":300,"browser_download_url":"https://github.com/rfoerthe/tagwerk/releases/download/v1.8.0/Tagwerk-1.8.0.dmg"}]}
    ]"#;

    #[test]
    fn releases_are_parsed_without_drafts_and_foreign_urls() {
        let r = parse_releases(SAMPLE).unwrap();
        assert_eq!(r.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["1.10.0-beta.1", "1.9.1", "1.9.0", "1.8.0"]);
        assert_eq!(r[1].sha.as_ref().map(|a| a.size), Some(80));
        assert!(r[2].dmg.is_none(), "nur Downloads aus dem Repository");
        assert!(parse_releases("{}").is_err());
        assert!(parse_releases("kaputt").is_err());
    }

    #[test]
    fn select_picks_the_highest_in_the_channel_with_all_notes() {
        let r = parse_releases(SAMPLE).unwrap();
        let stable = select(&r, "1.8.0", false).unwrap();
        assert_eq!(stable.version, "1.9.1");
        assert!(!stable.beta);
        // 1.9.0 hat kein brauchbares DMG, seine Neuigkeiten gehören trotzdem dazu.
        assert_eq!(stable.notes, vec![("1.9.1".into(), "Fix".into()), ("1.9.0".into(), "Neu".into())]);
        let beta = select(&r, "1.8.0", true).unwrap();
        assert_eq!(beta.version, "1.10.0-beta.1");
        assert!(beta.beta);
        assert_eq!(beta.notes.len(), 3);
        assert!(select(&r, "1.9.1", false).is_none());
        assert!(select(&r, "1.10.0", true).is_none());
        // Eine Beta-Version bekommt die fertige Version auch ohne Beta-Kanal.
        assert_eq!(select(&r, "1.9.1-beta.3", false).unwrap().version, "1.9.1");
    }

    #[test]
    fn notes_become_blocks() {
        let md = "### Zusammenfassung\nDie Datenbank lässt sich\njetzt **verschlüsseln**.\n\n### Hinzugefügt\n- Erstens mit `code`\n  und Folgezeile\n- Zweitens [Link](https://x.y)\n\n### Installation\n\nSigniert …";
        assert_eq!(
            blocks(md),
            vec![
                Block::Heading("Zusammenfassung".into()),
                Block::Para("Die Datenbank lässt sich jetzt verschlüsseln.".into()),
                Block::Heading("Hinzugefügt".into()),
                Block::Bullet("Erstens mit code und Folgezeile".into()),
                Block::Bullet("Zweitens Link".into()),
            ]
        );
        assert_eq!(inline("a [b] c [d](e"), "a [b] c [d](e");
    }

    #[test]
    fn changelog_sections_between_versions() {
        let text = "# Changelog\n\n## [Unveröffentlicht]\n\n## [1.9.0] - 2026-12-01\n### Neu\n- A\n\n## [1.8.0] - 2026-11-24\n- B\n\n## [1.7.0] - 2026-11-21\n- C\n\n[1.9.0]: https://x\n";
        let notes = changelog_notes(text, Some(&v("1.7.0")), &v("1.9.0"));
        assert_eq!(notes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["1.9.0", "1.8.0"]);
        assert_eq!(notes[1].1.trim(), "- B");
        assert_eq!(changelog_notes(text, None, &v("1.7.0")).len(), 1);
        // Der eingebettete Changelog enthält die laufende Version.
        assert_eq!(changelog_notes(CHANGELOG, None, &v(CURRENT))[0].0, CURRENT);
    }

    #[test]
    fn download_checks() {
        assert_eq!(byte_limit(0), DOWNLOAD_CEILING);
        assert_eq!(byte_limit(1234), 1234);
        assert_eq!(byte_limit(DOWNLOAD_CEILING + 1), DOWNLOAD_CEILING);
        assert!(download_ok(10, 10));
        assert!(download_ok(10, 0));
        assert!(!download_ok(9, 10));
        assert!(!download_ok(0, 0));
        let hex = "a".repeat(64);
        assert_eq!(parse_sha256(&format!("{hex}  Tagwerk-1.9.0.dmg\n")), Some(hex));
        assert_eq!(parse_sha256("zz  x"), None);
    }

    #[test]
    fn signer_from_codesign_output() {
        let out = "Executable=/Applications/Tagwerk.app/Contents/MacOS/Tagwerk\nIdentifier=de.foerther.tagwerk\nFormat=app bundle\nTeamIdentifier=ABCDE12345\n";
        let s = Signer::from_codesign(out).unwrap();
        assert_eq!(s.app_requirement(), "identifier \"de.foerther.tagwerk\" and anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\"");
        assert!(Signer::from_codesign("Identifier=de.foerther.tagwerk\nTeamIdentifier=not set\n").is_none());
        assert!(Signer::new("de.x\"; rm", "ABCDE12345").is_none());
    }

    #[test]
    fn failure_codes() {
        assert_eq!(failure_code("ok\n"), None);
        assert_eq!(failure_code("fail-swap\n").as_deref(), Some("fail-swap"));
        assert!(force_admin(Some("fail-copy")));
        assert!(!force_admin(Some("fail-verify")));
        assert!(!force_admin(None));
    }

    #[test]
    fn quoting_survives_the_shell() {
        let args = vec!["/Applications/Tag werk.app".to_string(), "it's".to_string()];
        let script = "#!/bin/sh\nprintf '%s|' \"$@\"\n";
        let argv = format!("/bin/sh -c {} x {}", sh_quote(script), args.iter().map(|a| sh_quote(a)).collect::<Vec<_>>().join(" "));
        let out = std::process::Command::new("/bin/sh").args(["-c", &argv]).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/Applications/Tag werk.app|it's|");
        // Der Befehl mit Administratorrechten startet im Hintergrund und kehrt sofort zurück.
        let cmd = elevated_command("exit 0", &args);
        assert!(cmd.ends_with('&'));
        assert!(cmd.contains("POSIX::setsid()"));
    }

    #[test]
    fn applescript_strings_round_trip() {
        let s = "a \"b\" \\ c\nd 'e' $f";
        let out = std::process::Command::new("/usr/bin/osascript")
            .args(["-e", &format!("return {}", applescript_string(s))])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n'), s);
    }

    /// Gegen die echten Releases: suchen, laden, Prüfsumme und Signatur des DMGs prüfen.
    /// `cargo test -- --ignored update::tests::real_release`
    #[test]
    #[ignore = "braucht Netz"]
    fn real_release_downloads_and_verifies() {
        let releases = fetch_releases().unwrap();
        let offer = select(&releases, "1.7.0", false).expect("ein Release über 1.7.0");
        assert!(offer.sha.is_some(), "Release ohne .sha256");
        let file = std::env::temp_dir().join(format!("tagwerk-update-test-{}", std::process::id())).join("t.dmg");
        download(&offer, &file).unwrap();
        // Das DMG trägt die Developer ID, mit der auch die App signiert ist.
        let out = std::process::Command::new("/usr/bin/codesign").args(["-dv", "--verbose=2"]).arg(&file).output().unwrap();
        let info = String::from_utf8_lossy(&out.stderr);
        let team = info.lines().find_map(|l| l.strip_prefix("TeamIdentifier=")).unwrap().trim();
        let signer = Signer::new("de.foerther.tagwerk", team).unwrap();
        let ok = std::process::Command::new("/usr/bin/codesign")
            .args(["-v", "--strict", &format!("-R={}", signer.dmg_requirement())])
            .arg(&file)
            .status()
            .unwrap();
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
        assert!(ok.success());
    }

    #[test]
    fn installer_script_is_valid_sh() {
        let signer = Signer::new("de.foerther.tagwerk", "ABCDE12345").unwrap();
        let script = installer_script(&signer);
        assert!(script.contains("certificate leaf[subject.OU] = \"ABCDE12345\""));
        assert!(!script.contains("@DMG_REQ@") && !script.contains("@APP_REQ@"));
        let out = std::process::Command::new("/bin/sh").args(["-n", "-c", &script]).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}
