use crate::id;
use cocoa::appkit::{NSAppearanceNameVibrantDark, NSAppearanceNameVibrantLight};
use gpui::WindowAppearance;
use objc::{msg_send, sel, sel_impl};
use objc2_foundation::NSString;

pub(crate) unsafe fn window_appearance_from_native(appearance: id) -> WindowAppearance {
    let name: id = msg_send![appearance, name];
    unsafe {
        if name == NSAppearanceNameVibrantLight {
            WindowAppearance::VibrantLight
        } else if name == NSAppearanceNameVibrantDark {
            WindowAppearance::VibrantDark
        } else if name == NSAppearanceNameAqua {
            WindowAppearance::Light
        } else if name == NSAppearanceNameDarkAqua {
            WindowAppearance::Dark
        } else {
            println!(
                "unknown appearance: {:?}",
                (&*name.cast::<NSString>()).to_string()
            );
            WindowAppearance::Light
        }
    }
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    pub static NSAppearanceNameAqua: id;
    pub static NSAppearanceNameDarkAqua: id;
}
