use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{App, Window};
use yes_core::Language;

struct CopySuccess;

pub(crate) fn copy_success(
    language: Language,
    target_key: &str,
    window: &mut Window,
    cx: &mut App,
) {
    window.push_notification(
        Notification::success(
            crate::i18n::tr(language, "common.copySuccess")
                .replace("{target}", crate::i18n::tr(language, target_key)),
        )
        .id::<CopySuccess>(),
        cx,
    );
}
