//! Globale Kurzbefehle der Menüleisten-App: Sie wirken in jeder App, nicht nur bei geöffnetem
//! Menü.
//!
//! Registriert wird über `RegisterEventHotKey` (Carbon). Anders als ein Ereignis-Monitor
//! braucht das keine Freigabe unter Systemeinstellungen › Datenschutz › Bedienungshilfen, und
//! Tagwerk sieht nur diese Tastenkombinationen, keine anderen Eingaben. Die Kurzbefehle sind
//! fest und in den Einstellungen abschaltbar; die Tasten sind über ihre Lage bestimmt (P und
//! A liegen auf deutscher und amerikanischer Tastatur gleich).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

/// Was ein globaler Kurzbefehl auslöst.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// „Arbeit unterbrechen“ bzw. „Weiter arbeiten“ / „Zurück am Mac“.
    TogglePause,
    /// Fenster „Aufzeichnungen“ öffnen.
    ShowRecords,
}

impl Action {
    pub const ALL: [Action; 2] = [Action::TogglePause, Action::ShowRecords];

    fn id(self) -> u32 {
        match self {
            Action::TogglePause => 1,
            Action::ShowRecords => 2,
        }
    }

    fn from_id(id: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Virtueller Tastencode (`kVK_ANSI_P`, `kVK_ANSI_A`).
    fn key_code(self) -> u32 {
        match self {
            Action::TogglePause => 0x23,
            Action::ShowRecords => 0x00,
        }
    }

    /// Tastenkombination zum Anzeigen.
    pub fn shortcut(self) -> &'static str {
        match self {
            Action::TogglePause => "⌃⌥⌘P",
            Action::ShowRecords => "⌃⌥⌘A",
        }
    }

    /// Was der Kurzbefehl tut (für die Einstellungen).
    pub fn label(self) -> &'static str {
        match self {
            Action::TogglePause => "Arbeit unterbrechen bzw. weiter arbeiten",
            Action::ShowRecords => "Aufzeichnungen öffnen",
        }
    }
}

/// `⌃⌥⌘P: Arbeit unterbrechen bzw. weiter arbeiten · ⌃⌥⌘A: Aufzeichnungen öffnen`
pub fn summary() -> String {
    Action::ALL.iter().map(|a| format!("{}: {}", a.shortcut(), a.label())).collect::<Vec<_>>().join("  ·  ")
}

// ---------- Carbon ----------

type OsStatus = i32;
type Ref = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

/// `'keyb'`
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
/// `'----'`
const PARAM_DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
/// `'hkid'`
const TYPE_EVENT_HOT_KEY_ID: u32 = u32::from_be_bytes(*b"hkid");
/// Kennung der Kurzbefehle von Tagwerk (`'TGWK'`).
const SIGNATURE: u32 = u32::from_be_bytes(*b"TGWK");
/// ⌃⌥⌘ (`controlKey | optionKey | cmdKey`).
const MODIFIERS: u32 = (1 << 12) | (1 << 11) | (1 << 8);

type Handler = extern "C" fn(call: Ref, event: Ref, user_data: *mut c_void) -> OsStatus;

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn GetApplicationEventTarget() -> Ref;
    fn InstallEventHandler(
        target: Ref,
        handler: Handler,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut Ref,
    ) -> OsStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyId,
        target: Ref,
        options: u32,
        out_ref: *mut Ref,
    ) -> OsStatus;
    fn UnregisterEventHotKey(hot_key: Ref) -> OsStatus;
    fn GetEventParameter(
        event: Ref,
        name: u32,
        param_type: u32,
        actual_type: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> OsStatus;
}

thread_local! {
    /// Wer die Kurzbefehle ausführt (die Menüleisten-App).
    static RUN: Cell<Option<fn(Action)>> = const { Cell::new(None) };
    /// Registrierte Kurzbefehle (leer = ausgeschaltet).
    static REGISTERED: RefCell<Vec<Ref>> = const { RefCell::new(Vec::new()) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Wird von macOS im Hauptthread aufgerufen, wenn ein Kurzbefehl gedrückt wurde.
extern "C" fn pressed(_call: Ref, event: Ref, _user_data: *mut c_void) -> OsStatus {
    let mut id = EventHotKeyId { signature: 0, id: 0 };
    let status = unsafe {
        GetEventParameter(
            event,
            PARAM_DIRECT_OBJECT,
            TYPE_EVENT_HOT_KEY_ID,
            std::ptr::null_mut(),
            std::mem::size_of::<EventHotKeyId>(),
            std::ptr::null_mut(),
            (&mut id as *mut EventHotKeyId).cast(),
        )
    };
    if status == 0 && id.signature == SIGNATURE {
        if let (Some(action), Some(run)) = (Action::from_id(id.id), RUN.with(Cell::get)) {
            run(action);
        }
    }
    0
}

/// Sind die Kurzbefehle gerade registriert?
pub fn enabled() -> bool {
    REGISTERED.with(|r| !r.borrow().is_empty())
}

/// Kurzbefehle ein- oder ausschalten; `run` führt sie aus. Nur im Hauptthread. Lässt sich
/// einer nicht registrieren, bleibt keiner aktiv, und der Fehler nennt die Tastenkombination.
pub fn set_enabled(on: bool, run: fn(Action)) -> Result<(), String> {
    RUN.with(|r| r.set(Some(run)));
    if on == enabled() {
        return Ok(());
    }
    if !on {
        unregister_all();
        return Ok(());
    }
    let target = unsafe { GetApplicationEventTarget() };
    if !INSTALLED.with(Cell::get) {
        let spec = EventTypeSpec { class: EVENT_CLASS_KEYBOARD, kind: EVENT_HOT_KEY_PRESSED };
        let status =
            unsafe { InstallEventHandler(target, pressed, 1, &spec, std::ptr::null_mut(), std::ptr::null_mut()) };
        if status != 0 {
            return Err(format!("Die Kurzbefehle lassen sich nicht einrichten (Fehler {status})."));
        }
        INSTALLED.with(|i| i.set(true));
    }
    for action in Action::ALL {
        let mut hot_key: Ref = std::ptr::null_mut();
        let id = EventHotKeyId { signature: SIGNATURE, id: action.id() };
        let status = unsafe { RegisterEventHotKey(action.key_code(), MODIFIERS, id, target, 0, &mut hot_key) };
        if status != 0 || hot_key.is_null() {
            unregister_all();
            return Err(format!(
                "{} lässt sich nicht als Kurzbefehl registrieren (Fehler {status}) – vermutlich \
                 benutzt ihn schon eine andere App.",
                action.shortcut()
            ));
        }
        REGISTERED.with(|r| r.borrow_mut().push(hot_key));
    }
    Ok(())
}

fn unregister_all() {
    for hot_key in REGISTERED.with(|r| std::mem::take(&mut *r.borrow_mut())) {
        unsafe { UnregisterEventHotKey(hot_key) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_texts() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id(0), None);
        assert_eq!(
            summary(),
            "⌃⌥⌘P: Arbeit unterbrechen bzw. weiter arbeiten  ·  ⌃⌥⌘A: Aufzeichnungen öffnen"
        );
        // Vierbuchstaben-Codes wie in den Carbon-Headern.
        assert_eq!(EVENT_CLASS_KEYBOARD, 0x6B65_7962);
        assert_eq!(MODIFIERS, 0x1900);
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn CreateEvent(allocator: *const c_void, class: u32, kind: u32, when: f64, attributes: u32, out: *mut Ref) -> OsStatus;
        fn SetEventParameter(event: Ref, name: u32, param_type: u32, size: usize, data: *const c_void) -> OsStatus;
        fn SendEventToEventTarget(event: Ref, target: Ref) -> OsStatus;
        fn ReleaseEvent(event: Ref);
    }

    thread_local! {
        static SEEN: RefCell<Vec<Action>> = const { RefCell::new(Vec::new()) };
    }

    /// Registriert die Kurzbefehle wirklich (für kurze Zeit systemweit) und schickt der App
    /// das Ereignis, das macOS bei einem Tastendruck schickt – prüft also Registrierung,
    /// Ereignisbehandlung und das Auslesen der Kennung, nicht die Tastatur selbst:
    /// `cargo test hotkeys_reach_the_handler -- --ignored`
    #[test]
    #[ignore]
    fn hotkeys_reach_the_handler() {
        fn run(action: Action) {
            SEEN.with(|s| s.borrow_mut().push(action));
        }
        set_enabled(true, run).expect("Kurzbefehle registriert");
        assert!(enabled());
        let send = |signature: u32, id: u32| unsafe {
            let mut event: Ref = std::ptr::null_mut();
            assert_eq!(CreateEvent(std::ptr::null(), EVENT_CLASS_KEYBOARD, EVENT_HOT_KEY_PRESSED, 0.0, 0, &mut event), 0);
            let hot_key = EventHotKeyId { signature, id };
            let size = std::mem::size_of::<EventHotKeyId>();
            let data = (&hot_key as *const EventHotKeyId).cast();
            assert_eq!(SetEventParameter(event, PARAM_DIRECT_OBJECT, TYPE_EVENT_HOT_KEY_ID, size, data), 0);
            let status = SendEventToEventTarget(event, GetApplicationEventTarget());
            ReleaseEvent(event);
            status
        };
        assert_eq!(send(SIGNATURE, Action::ShowRecords.id()), 0);
        assert_eq!(send(SIGNATURE, Action::TogglePause.id()), 0);
        // Fremde Kennungen bleiben ohne Wirkung.
        send(u32::from_be_bytes(*b"XXXX"), Action::TogglePause.id());
        send(SIGNATURE, 99);
        assert_eq!(SEEN.with(|s| s.borrow().clone()), [Action::ShowRecords, Action::TogglePause]);
        set_enabled(false, run).unwrap();
        assert!(!enabled());
    }
}
