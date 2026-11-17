//! „Bei Anmeldung starten“: die installierte App als Anmeldeobjekt eintragen
//! (SMAppService, macOS 13+). Eingestellt wird im Fenster „Einstellungen“.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

fn service() -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"SMAppService")?;
    unsafe { msg_send![cls, mainAppService] }
}

/// Ist Tagwerk als Anmeldeobjekt eingetragen?
pub fn enabled() -> bool {
    service()
        .map(|svc| {
            let status: isize = unsafe { msg_send![&*svc, status] };
            status == 1 // SMAppServiceStatusEnabled
        })
        .unwrap_or(false)
}

/// Anmeldeobjekt ein- oder austragen. Gelingt nur in der installierten App.
pub fn set(enable: bool) -> Result<(), String> {
    let svc = service().ok_or("Anmeldeobjekte werden erst ab macOS 13 unterstützt.")?;
    let err: *mut *mut AnyObject = std::ptr::null_mut();
    let ok: bool = unsafe {
        if enable {
            msg_send![&*svc, registerAndReturnError: err]
        } else {
            msg_send![&*svc, unregisterAndReturnError: err]
        }
    };
    if ok {
        Ok(())
    } else {
        Err("Das Anmeldeobjekt konnte nicht geändert werden (nur in der installierten App möglich).".to_string())
    }
}
