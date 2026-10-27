//! Dock icon Quit, logout and AppleScript `quit` ask first, like Cmd+Q.
//!
//! Those send AppKit's `terminate:`. tao 0.37's app delegate answers no
//! `applicationShouldTerminate:`, so AppKit goes straight to
//! `applicationWillTerminate:` and `RunEvent::Exit`, which stops every
//! terminal without asking. [`install`] adds that method to the
//! class of whatever object is NSApp's delegate (not tao's class by name).
//! It answers "terminate now" when nothing is running or the user already
//! said Quit; otherwise it shows the Cmd+Q question and cancels, and the
//! question's Quit exits through `app.exit` (`[NSApp stop:]`, not
//! `terminate:`). A logout with work running is therefore cancelled by macOS.
//!
//! Every lookup is checked: with no delegate, a delegate that already
//! answers the selector (a newer tao), or a failed add, it logs once and
//! quitting stays as it was.

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{class, ffi, msg_send, sel};
use std::sync::OnceLock;
use tauri::AppHandle;

/// `NSApplicationTerminateReply`.
const NS_TERMINATE_CANCEL: usize = 0;
const NS_TERMINATE_NOW: usize = 1;

static APP: OnceLock<AppHandle> = OnceLock::new();

/// `extern "C"`, not `"C-unwind"`: a panic that reaches this boundary aborts
/// the process (in a release build `panic = "abort"` already does, and the
/// crash hook has written `logs/crash.log` first). It never unwinds into
/// AppKit, in any build.
type ShouldTerminate = extern "C" fn(&AnyObject, Sel, *mut AnyObject) -> usize;

/// Once, on the main thread, after tao made its event loop (Tauri `setup`).
pub fn install(app: &AppHandle) {
    if APP.set(app.clone()).is_err() {
        return;
    }
    // SAFETY: called on the main thread; every pointer is checked before use.
    if let Err(reason) = unsafe { add_should_terminate() } {
        eprintln!("DCTerminalv2: Dock and logout quit will not ask first: {reason}");
    }
}

unsafe fn add_should_terminate() -> Result<(), &'static str> {
    let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
    let ns_app = ns_app.as_ref().ok_or("no NSApplication")?;
    let delegate: *mut AnyObject = msg_send![ns_app, delegate];
    let delegate = delegate.as_ref().ok_or("NSApp has no delegate")?;
    let class: &AnyClass = delegate.class();
    let selector = sel!(applicationShouldTerminate:);
    if class.responds_to(selector) {
        return Err("the app delegate already answers applicationShouldTerminate:");
    }
    let handler: ShouldTerminate = should_terminate;
    // `Q@:@`: NSUInteger return, self, _cmd, sender.
    let added = ffi::class_addMethod(
        class as *const AnyClass as *mut AnyClass,
        selector,
        std::mem::transmute::<ShouldTerminate, Imp>(handler),
        c"Q@:@".as_ptr(),
    );
    if !added.as_bool() {
        return Err("class_addMethod failed");
    }
    Ok(())
}

extern "C" fn should_terminate(_: &AnyObject, _: Sel, _: *mut AnyObject) -> usize {
    let Some(app) = APP.get() else {
        return NS_TERMINATE_NOW;
    };
    // Nothing here catches a panic: under `panic = "abort"` that would catch
    // nothing, and in a debug build the `extern "C"` boundary aborts it.
    if crate::windows::should_terminate(app) {
        NS_TERMINATE_NOW
    } else {
        NS_TERMINATE_CANCEL
    }
}
