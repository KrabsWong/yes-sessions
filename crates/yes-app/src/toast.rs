use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{App, Window};
use yes_core::Language;

struct CopySuccess;

pub(crate) fn copy_success(language: Language, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::success(crate::i18n::tr(language, "common.copySuccess")).id::<CopySuccess>(),
        cx,
    );
}
