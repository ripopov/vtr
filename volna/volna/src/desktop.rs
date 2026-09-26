//! Native shell identity and icon assets belong to the GPUI frontend.

pub const APP_ID: &str = "io.github.ripopov.volna";

/// X11 uses pixels; Wayland resolves APP_ID through the installed desktop entry.
#[cfg(target_os = "linux")]
pub fn window_icon() -> std::sync::Arc<image::RgbaImage> {
    std::sync::Arc::new(
        image::load_from_memory(include_bytes!("../assets/app-icon/volna-256.png"))
            .expect("bundled Volna icon must be a valid PNG")
            .into_rgba8(),
    )
}

/// Also give unbundled `cargo run` processes their own Dock icon on macOS.
/// Finder uses the ICNS resource declared in the application's Info.plist.
#[cfg(target_os = "macos")]
pub fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let mtm = MainThreadMarker::new().expect("application startup runs on the main thread");
    let data = NSData::with_bytes(include_bytes!("../assets/app-icon/volna.icns"));
    let icon = NSImage::initWithData(NSImage::alloc(), &data)
        .expect("bundled Volna icon must be a valid ICNS");
    // SAFETY: AppKit is called on the main thread with a non-null, valid image.
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&icon)) };
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn desktop_icns_decodes_with_appkit_without_a_window() {
    use objc2::AnyThread;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSData;

    let data = NSData::with_bytes(include_bytes!("../assets/app-icon/volna.icns"));
    let icon = NSImage::initWithData(NSImage::alloc(), &data).expect("AppKit ICNS decoder");
    assert!(icon.isValid());
    assert!(icon.size().width > 0.0);
    assert_eq!(icon.size().width, icon.size().height);
}
