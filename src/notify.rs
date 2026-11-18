//! Mitteilungen über die Mitteilungszentrale von macOS (UserNotifications).
//!
//! Nur in der installierten App (`Tagwerk.app`) verfügbar: Ohne App-Bundle mit Bundle-Kennung
//! bricht `UNUserNotificationCenter` das Programm ab. Ob und wie Mitteilungen erscheinen,
//! legt man unter Systemeinstellungen › Mitteilungen › Tagwerk fest; den Stand zeigt das
//! Fenster „Einstellungen“ (siehe [`permission`]).

use crate::rules::LimitAlert;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSError, NSOperationQueue, NSString, NSURL};
use objc2_user_notifications::{
    UNAlertStyle, UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationSetting, UNNotificationSettings,
    UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

/// Darf Tagwerk Mitteilungen zeigen?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Ohne App-Bundle (z. B. `cargo run`) gibt es keine Mitteilungen.
    Unavailable,
    /// macOS hat noch nicht gefragt bzw. die Frage ist noch offen.
    NotDetermined,
    /// In den Systemeinstellungen ausgeschaltet.
    Denied,
    /// Erlaubt, aber weder als Banner noch in der Mitteilungszentrale (Stil „Ohne“).
    Hidden,
    Allowed,
}

impl Permission {
    /// Unter Systemeinstellungen › Mitteilungen zu ändern (nicht erlaubt oder ausgeblendet)?
    pub fn needs_settings(self) -> bool {
        matches!(self, Permission::NotDetermined | Permission::Denied | Permission::Hidden)
    }

    /// Kurzer Stand für das Fenster „Einstellungen“.
    pub fn text(self) -> &'static str {
        match self {
            Permission::Unavailable => "Nur in der installierten App",
            Permission::NotDetermined => "Noch nicht erlaubt",
            Permission::Denied => "Nicht erlaubt",
            Permission::Hidden => "Ausgeblendet",
            Permission::Allowed => "Erlaubt",
        }
    }

    /// Erklärung darunter.
    pub fn note(self) -> &'static str {
        match self {
            Permission::Allowed => {
                "Bei 9:45 und 10 Std. Arbeit erinnert Tagwerk über die Mitteilungszentrale an die \
                 Höchstarbeitszeit."
            }
            Permission::Hidden => {
                "Erlaubt, aber Mitteilungen erscheinen weder als Banner noch in der Mitteilungszentrale. \
                 Damit Tagwerk bei 9:45 und 10 Std. Arbeit an die Höchstarbeitszeit erinnern kann, \
                 unter Systemeinstellungen › Mitteilungen › Tagwerk einen Stil wählen."
            }
            Permission::NotDetermined | Permission::Denied => {
                "Damit Tagwerk bei 9:45 und 10 Std. Arbeit an die Höchstarbeitszeit erinnern kann, \
                 unter Systemeinstellungen › Mitteilungen › Tagwerk „Mitteilungen erlauben“ einschalten."
            }
            Permission::Unavailable => {
                "Erinnerungen an die Höchstarbeitszeit (bei 9:45 und 10 Std. Arbeit) gibt es nur in \
                 der installierten App."
            }
        }
    }

    fn from_settings(s: &UNNotificationSettings) -> Self {
        let status = s.authorizationStatus();
        if status == UNAuthorizationStatus::NotDetermined {
            Permission::NotDetermined
        } else if status == UNAuthorizationStatus::Denied {
            Permission::Denied
        } else if s.alertStyle() == UNAlertStyle::None
            && s.notificationCenterSetting() != UNNotificationSetting::Enabled
        {
            Permission::Hidden
        } else {
            Permission::Allowed
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TagwerkNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Mitteilungen auch zeigen, während Tagwerk die aktive App ist (z. B. bei offenem Fenster).
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion_handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }
    }
);

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    /// Zuletzt abgefragter Stand (für das Fenster „Einstellungen“ beim Öffnen).
    static LAST: Cell<Option<Permission>> = const { Cell::new(None) };
    /// Der Delegate wird von der Mitteilungszentrale nur schwach gehalten.
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
}

/// Beim Start der App: Delegate setzen und (beim ersten Mal) um Erlaubnis fragen.
/// `bundled`: läuft als `Tagwerk.app` – sonst bleiben Mitteilungen aus.
pub fn init(mtm: MainThreadMarker, bundled: bool) {
    if !bundled {
        return;
    }
    let delegate: Retained<Delegate> = unsafe { msg_send![super(Delegate::alloc(mtm).set_ivars(())), init] };
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    ENABLED.with(|e| e.set(true));

    let done = RcBlock::new(|granted: Bool, error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            eprintln!("Tagwerk: Mitteilungen nicht verfügbar: {}", error.localizedDescription());
        } else if !granted.as_bool() {
            eprintln!("Tagwerk: Mitteilungen sind in den Systemeinstellungen ausgeschaltet.");
        }
    });
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &done,
    );
    permission(|_| {});
}

/// Zuletzt bekannter Stand (`None`: noch nicht abgefragt).
pub fn last_permission() -> Option<Permission> {
    if ENABLED.with(Cell::get) {
        LAST.with(Cell::get)
    } else {
        Some(Permission::Unavailable)
    }
}

/// Fragt ab, ob Mitteilungen erlaubt sind. `done` läuft danach im Hauptthread – immer erst
/// später aus der Ereignisschleife, nie schon während des Aufrufs.
pub fn permission(done: fn(Permission)) {
    // `done` (fn-Zeiger) und `p` (Copy) dürfen in einen anderen Thread.
    let on_main = move |p: Permission| {
        let block = RcBlock::new(move || {
            LAST.with(|l| l.set(Some(p)));
            done(p);
        });
        unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
    };
    if !ENABLED.with(Cell::get) {
        on_main(Permission::Unavailable);
        return;
    }
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        on_main(Permission::from_settings(unsafe { settings.as_ref() }));
    });
    UNUserNotificationCenter::currentNotificationCenter().getNotificationSettingsWithCompletionHandler(&block);
}

/// Systemeinstellungen › Mitteilungen öffnen, wenn möglich gleich bei Tagwerk.
pub fn open_settings() {
    let mut url = "x-apple.systempreferences:com.apple.Notifications-Settings.extension".to_string();
    if let Some(id) = NSBundle::mainBundle().bundleIdentifier() {
        url.push_str(&format!("?id={id}"));
    }
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Mitteilung zur Höchstarbeitszeit sofort zeigen.
pub fn post(alert: LimitAlert) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&alert.title()));
    content.setBody(&NSString::from_str(&alert.body()));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(alert.id()),
        &content,
        None,
    );
    let done = RcBlock::new(|error: *mut NSError| {
        if let Some(error) = unsafe { error.as_ref() } {
            eprintln!("Tagwerk: Mitteilung konnte nicht gezeigt werden: {}", error.localizedDescription());
        }
    });
    UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, Some(&done));
}
