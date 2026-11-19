//! Mitteilungen über die Mitteilungszentrale von macOS (UserNotifications).
//!
//! Nur in der installierten App (`Tagwerk.app`) verfügbar: Ohne App-Bundle mit Bundle-Kennung
//! bricht `UNUserNotificationCenter` das Programm ab. Ob und wie Mitteilungen erscheinen,
//! legt man unter Systemeinstellungen › Mitteilungen › Tagwerk fest; den Stand zeigt das
//! Fenster „Einstellungen“ (siehe [`permission`]).

use crate::day::Activity;
use crate::rules::LimitAlert;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSBundle, NSError, NSOperationQueue, NSSet, NSString, NSURL};
use objc2_user_notifications::{
    UNAlertStyle, UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse, UNNotificationSetting,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
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
                "Tagwerk erinnert über die Mitteilungszentrale an Pause und Höchstarbeitszeit, fragt \
                 nach langer Sperre nach und meldet Fehler beim Speichern."
            }
            Permission::Hidden => {
                "Erlaubt, aber Mitteilungen erscheinen weder als Banner noch in der Mitteilungszentrale. \
                 Damit Tagwerk erinnern und nachfragen kann, unter Systemeinstellungen › Mitteilungen › \
                 Tagwerk einen Stil wählen."
            }
            Permission::NotDetermined | Permission::Denied => {
                "Damit Tagwerk an Pause und Höchstarbeitszeit erinnern und nach langer Sperre nachfragen \
                 kann, unter Systemeinstellungen › Mitteilungen › Tagwerk „Mitteilungen erlauben“ einschalten."
            }
            Permission::Unavailable => {
                "Erinnerungen und die Nachfrage nach langer Sperre gibt es nur in der installierten App."
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

        /// Ein Knopf der Nachfrage nach langer Sperre wurde gewählt. Die Antwort geht im
        /// Hauptthread an die Menüleisten-App; ein Klick auf die Mitteilung selbst tut nichts.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            if let Some(answer) = LockAnswer::from_action(&response.actionIdentifier().to_string()) {
                let block = RcBlock::new(move || {
                    if let Some(f) = ON_LOCK_ANSWER.with(Cell::get) {
                        f(answer);
                    }
                });
                unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
            }
            completion_handler.call(());
        }
    }
);

/// Antwort auf die Nachfrage, was eine lange Sperre war. Ohne Antwort bleibt sie eine Pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockAnswer {
    /// Arbeit ohne Mac mit diesem Grund.
    Work(Activity),
    /// Feierabend war schon mit dem Sperren.
    CloseDay,
}

impl LockAnswer {
    /// Die Knöpfe der Nachfrage, in dieser Reihenfolge.
    pub const ALL: [LockAnswer; 5] = [
        LockAnswer::Work(Activity::Meeting),
        LockAnswer::Work(Activity::Call),
        LockAnswer::Work(Activity::Networking),
        LockAnswer::Work(Activity::Other),
        LockAnswer::CloseDay,
    ];

    /// Kennung des Knopfs in der Mitteilung.
    fn action(self) -> String {
        match self {
            LockAnswer::Work(a) => format!("work-{}", a.key()),
            LockAnswer::CloseDay => "close-day".to_string(),
        }
    }

    fn from_action(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.action() == id)
    }

    /// Beschriftung des Knopfs.
    pub fn title(self) -> &'static str {
        match self {
            LockAnswer::Work(Activity::Other) => "Sonstige Arbeit",
            LockAnswer::Work(a) => a.label(),
            LockAnswer::CloseDay => "Feierabend",
        }
    }
}

/// Kategorie der Nachfrage nach langer Sperre (mit den Knöpfen aus [`LockAnswer::ALL`]).
const LOCK_CATEGORY: &str = "lock-ask";

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    /// Zuletzt abgefragter Stand (für das Fenster „Einstellungen“ beim Öffnen).
    static LAST: Cell<Option<Permission>> = const { Cell::new(None) };
    /// Der Delegate wird von der Mitteilungszentrale nur schwach gehalten.
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    /// Wer die Antwort auf die Nachfrage nach langer Sperre bekommt (die Menüleisten-App).
    static ON_LOCK_ANSWER: Cell<Option<fn(LockAnswer)>> = const { Cell::new(None) };
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

    // Knöpfe der Nachfrage nach langer Sperre.
    let actions: Vec<Retained<UNNotificationAction>> = LockAnswer::ALL
        .iter()
        .map(|a| {
            UNNotificationAction::actionWithIdentifier_title_options(
                &NSString::from_str(&a.action()),
                &NSString::from_str(a.title()),
                UNNotificationActionOptions::empty(),
            )
        })
        .collect();
    let category = UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
        &NSString::from_str(LOCK_CATEGORY),
        &NSArray::from_retained_slice(&actions),
        &NSArray::new(),
        UNNotificationCategoryOptions::empty(),
    );
    center.setNotificationCategories(&NSSet::from_retained_slice(&[category]));

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
    post_text(alert.id(), &alert.title(), &alert.body());
}

/// Mitteilung sofort zeigen. Eine neue ersetzt eine ältere mit derselben Kennung `id`.
/// Ohne App-Bundle passiert nichts.
pub fn post_text(id: &str, title: &str, body: &str) {
    post_with(id, title, body, None);
}

/// Wer die Antworten auf [`ask_lock`] bekommt (im Hauptthread).
pub fn on_lock_answer(f: fn(LockAnswer)) {
    ON_LOCK_ANSWER.with(|c| c.set(Some(f)));
}

/// Nachfrage nach langer Sperre: Mitteilung mit den Knöpfen aus [`LockAnswer::ALL`]. Wer sie
/// übergeht, lässt die Zeit eine Pause sein. Gibt zurück, ob Mitteilungen verfügbar sind.
pub fn ask_lock(title: &str, body: &str) -> bool {
    post_with(LOCK_CATEGORY, title, body, Some(LOCK_CATEGORY));
    ENABLED.with(Cell::get)
}

fn post_with(id: &str, title: &str, body: &str, category: Option<&str>) {
    if !ENABLED.with(Cell::get) {
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    if let Some(category) = category {
        content.setCategoryIdentifier(&NSString::from_str(category));
    }
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(id),
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
