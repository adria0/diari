//! macOS-only bits.

use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

/// Sets the Dock icon. macOS has no per-window icons, so Slint's
/// `Window.icon` is ignored there, and a binary run outside an `.app`
/// bundle gets the generic one. Call on the main thread once the event
/// loop runs (winit creates NSApp).
pub fn set_dock_icon() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(include_bytes!("../icons/diari-256.png"));
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        log::warn!("could not decode the Dock icon");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    unsafe { app.setApplicationIconImage(Some(&image)) };
}
