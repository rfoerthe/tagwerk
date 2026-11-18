//! Mitteilungen über die Mitteilungszentrale von macOS (UserNotifications).
//!
//! Nur in der installierten App (`Tagwerk.app`) verfügbar: Ohne App-Bundle mit Bundle-Kennung
//! bricht `UNUserNotificationCenter` das Programm ab. Ob und wie Mitteilungen erscheinen,
//! legt man unter Systemeinstellungen › Mitteilungen › Tagwerk fest.

use crate::rules::LimitAlert;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationSound, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::cell::{Cell, RefCell};

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
