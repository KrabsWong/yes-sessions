//! Native attachment presentation. Remote resources open only on explicit user actions.
use crate::i18n::tr;
use base64::Engine as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, WindowExt as _,
    button::{Button, ButtonVariants as _},
    notification::Notification,
    tooltip::Tooltip,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use yes_core::{AttachmentSource, Language, SessionAttachment};

const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone)]
struct LoadedAttachment {
    available: bool,
    size: Option<u64>,
    image: Option<Arc<Image>>,
    fallback: bool,
}
struct AttachmentState {
    items: Vec<SessionAttachment>,
    loaded: Vec<Option<LoadedAttachment>>,
}

#[derive(IntoElement)]
pub(crate) struct Attachments {
    pub id: usize,
    pub items: Vec<SessionAttachment>,
    pub language: Language,
}

fn decoded_data(data: &str) -> anyhow::Result<Vec<u8>> {
    let (header, body) = data
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("Invalid embedded resource"))?;
    anyhow::ensure!(
        header.starts_with("data:") && header.ends_with(";base64"),
        "Unsupported embedded resource"
    );
    anyhow::ensure!(
        body.len() as u64 <= MAX_IMAGE_BYTES * 4 / 3 + 4,
        "Embedded resource too large"
    );
    let bytes = base64::engine::general_purpose::STANDARD.decode(body)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_IMAGE_BYTES,
        "Embedded resource too large"
    );
    Ok(bytes)
}

fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else if bytes.starts_with(b"\0\0\x01\0") {
        Some(ImageFormat::Ico)
    } else if std::str::from_utf8(&bytes[..bytes.len().min(1024)]).is_ok_and(|prefix| {
        prefix.trim_start().starts_with("<svg")
            || (prefix.trim_start().starts_with("<?xml") && prefix.contains("<svg"))
    }) {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

fn load_attachment(item: &SessionAttachment) -> LoadedAttachment {
    let (available, size, bytes) = match &item.source {
        AttachmentSource::RemoteUrl(_) => (true, None, None),
        AttachmentSource::DataUrl(data) => match decoded_data(data) {
            Ok(bytes) => (
                true,
                Some(bytes.len() as u64),
                item.is_image().then_some(bytes),
            ),
            Err(_) => (false, None, None),
        },
        AttachmentSource::LocalPath(path) => {
            match regular_file(path).and_then(|file| Ok((file.metadata()?, file))) {
                Ok((metadata, file)) if metadata.is_file() => {
                    let bytes = if item.is_image() && metadata.len() <= MAX_IMAGE_BYTES {
                        let mut bytes = Vec::new();
                        file.take(MAX_IMAGE_BYTES + 1)
                            .read_to_end(&mut bytes)
                            .ok()
                            .filter(|_| bytes.len() as u64 <= MAX_IMAGE_BYTES)
                            .map(|_| bytes)
                    } else {
                        None
                    };
                    (
                        bytes.is_some() || !item.is_image() || metadata.len() > MAX_IMAGE_BYTES,
                        Some(metadata.len()),
                        bytes,
                    )
                }
                _ => (false, None, None),
            }
        }
    };
    let (available, size, bytes, fallback) = if !available {
        match item
            .embedded_fallback
            .as_deref()
            .and_then(|data| decoded_data(data).ok())
        {
            Some(bytes) => (
                true,
                Some(bytes.len() as u64),
                item.is_image().then_some(bytes),
                true,
            ),
            None => (available, size, bytes, false),
        }
    } else {
        (available, size, bytes, false)
    };
    let image = bytes.and_then(|bytes| {
        image_format(&bytes).map(|format| Arc::new(Image::from_bytes(format, bytes)))
    });
    LoadedAttachment {
        available,
        size,
        image,
        fallback,
    }
}

fn regular_file(path: &Path) -> std::io::Result<File> {
    if !fs::metadata(path)?.is_file() {
        return Err(std::io::Error::other("Resource is not a regular file"));
    }
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("Resource is not a regular file"));
    }
    Ok(file)
}

fn safe_name(name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let name: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != ':')
        .take(180)
        .collect();
    if name.is_empty() || name == "." || name == ".." {
        "attachment".to_owned()
    } else {
        name
    }
}

fn size_label(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024. * 1024.))
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.)
    } else {
        format!("{size} B")
    }
}

fn export_attachment(item: &SessionAttachment, destination: &Path) -> anyhow::Result<()> {
    match &item.source {
        AttachmentSource::LocalPath(source) => {
            let mut input = regular_file(source)?;
            anyhow::ensure!(
                input.metadata()?.is_file(),
                "Resource is not a regular file"
            );
            let source_metadata = input.metadata()?;
            if fs::metadata(destination).is_ok_and(|target| {
                target.dev() == source_metadata.dev() && target.ino() == source_metadata.ino()
            }) {
                return Ok(());
            }
            let mut output = File::create(destination)?;
            std::io::copy(&mut input, &mut output)?;
        }
        AttachmentSource::DataUrl(data) => {
            fs::write(destination, decoded_data(data)?)?;
        }
        AttachmentSource::RemoteUrl(_) => {
            anyhow::bail!("Remote resources must be opened in the browser")
        }
    }
    Ok(())
}

fn save_as(item: SessionAttachment, language: Language, window: &mut Window, cx: &mut App) {
    let name = safe_name(&item.name);
    let initial = match &item.source {
        AttachmentSource::LocalPath(path) => path.parent().unwrap_or(Path::new("/")),
        _ => Path::new("/"),
    };
    let receiver = cx.prompt_for_new_path(initial, Some(&name));
    window
        .spawn(cx, async move |cx| {
            let result = match receiver.await {
                Ok(Ok(Some(path))) => {
                    cx.background_executor()
                        .spawn(async move { export_attachment(&item, &path) })
                        .await
                }
                Ok(Ok(None)) => return,
                _ => Err(anyhow::anyhow!("Unable to select destination")),
            };
            let _ = cx.update(|window, cx| {
                let notification = if result.is_ok() {
                    Notification::success(tr(language, "attachment.saved"))
                } else {
                    Notification::error(tr(language, "attachment.saveError"))
                };
                window.push_notification(notification, cx);
            });
        })
        .detach();
}

fn open_attachment(item: SessionAttachment, language: Language, window: &mut Window, cx: &mut App) {
    match &item.source {
        AttachmentSource::LocalPath(path) => cx.open_with_system(path),
        AttachmentSource::RemoteUrl(url) => cx.open_url(url),
        AttachmentSource::DataUrl(_) => {
            window
                .spawn(cx, async move |cx| {
                    let result: anyhow::Result<PathBuf> = cx
                        .background_executor()
                        .spawn(async move {
                            let AttachmentSource::DataUrl(data) = &item.source else {
                                unreachable!()
                            };
                            let bytes = decoded_data(data)?;
                            let unique = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)?
                                .as_nanos();
                            let directory = std::env::temp_dir()
                                .join(format!("yes-attachment-{}-{unique}", std::process::id()));
                            fs::DirBuilder::new().mode(0o700).create(&directory)?;
                            let path = directory.join(safe_name(&item.name));
                            let mut file = OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .mode(0o600)
                                .open(&path)?;
                            file.write_all(&bytes)?;
                            Ok(path)
                        })
                        .await;
                    let _ = cx.update(|window, cx| match result {
                        Ok(path) => cx.open_with_system(&path),
                        Err(_) => window.push_notification(
                            Notification::error(tr(language, "attachment.unavailable")),
                            cx,
                        ),
                    });
                })
                .detach();
        }
    }
}

fn actions(item: &SessionAttachment, language: Language) -> impl IntoElement {
    let open = item.clone();
    let save = item.clone();
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .child(
            Button::new("open")
                .outline()
                .label(tr(language, "attachment.open"))
                .text_size(px(12.))
                .on_click(move |_, window, cx| open_attachment(open.clone(), language, window, cx)),
        )
        .when(
            !matches!(item.source, AttachmentSource::RemoteUrl(_)),
            |this| {
                this.child(
                    Button::new("save")
                        .outline()
                        .label(tr(language, "attachment.saveAs"))
                        .text_size(px(12.))
                        .on_click(move |_, window, cx| save_as(save.clone(), language, window, cx)),
                )
            },
        )
        .when_some(
            match &item.source {
                AttachmentSource::LocalPath(path) => Some(path.clone()),
                _ => None,
            },
            |this, path| {
                this.child(
                    Button::new("reveal")
                        .outline()
                        .label(tr(language, "attachment.reveal"))
                        .text_size(px(12.))
                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                )
            },
        )
}

impl RenderOnce for Attachments {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let items = self.items.clone();
        let state = window.use_keyed_state(("attachments", self.id), cx, |_, _| AttachmentState {
            items: Vec::new(),
            loaded: vec![],
        });
        if state.read(cx).items != items {
            state.update(cx, |state, _| {
                state.items = items.clone();
                state.loaded = vec![None; items.len()];
            });
            let target = state.downgrade();
            cx.spawn(async move |cx| {
                for (index, item) in items.iter().enumerate() {
                    let item = item.clone();
                    let loaded = cx
                        .background_executor()
                        .spawn(async move { load_attachment(&item) })
                        .await;
                    let _ = target.update(cx, |state, cx| {
                        if state.items == items {
                            state.loaded[index] = Some(loaded);
                            cx.notify();
                        }
                    });
                }
            })
            .detach();
        }
        let loaded = state.read(cx).loaded.clone();
        let language = self.language;
        let mut list = div()
            .id(("attachment-list", self.id))
            .debug_selector(|| "attachment-list".into())
            .flex()
            .flex_wrap()
            .gap_2()
            .w_full()
            .min_w_0();
        for (index, mut item) in self.items.into_iter().enumerate() {
            let data = loaded.get(index).and_then(Option::as_ref);
            let full_path = match &item.source {
                AttachmentSource::LocalPath(path) => path.to_string_lossy().into_owned(),
                AttachmentSource::RemoteUrl(url) => url.clone(),
                AttachmentSource::DataUrl(_) => item.name.clone(),
            };
            if data.is_some_and(|data| data.fallback) {
                if let Some(data) = item.embedded_fallback.take() {
                    item.source = AttachmentSource::DataUrl(data);
                }
            }
            let available = data.is_some_and(|data| data.available);
            let subtitle = match data {
                None => tr(language, "attachment.loading").to_string(),
                Some(data) if !data.available => tr(language, "attachment.unavailable").to_string(),
                Some(data) => data
                    .size
                    .map(size_label)
                    .unwrap_or_else(|| tr(language, "attachment.remote").to_string()),
            };
            let mut card = div()
                .id(("attachment", index))
                .debug_selector(|| "attachment-card".into())
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(320.))
                .min_w_0()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .overflow_hidden();
            if let Some(image) = data.and_then(|data| data.image.clone()) {
                let preview_item = item.clone();
                let preview_image = image.clone();
                card = card.child(
                    div()
                        .id("thumbnail")
                        .debug_selector(|| "attachment-thumbnail".into())
                        .h(px(180.))
                        .flex_shrink_0()
                        .w_full()
                        .overflow_hidden()
                        .bg(cx.theme().muted)
                        .cursor_pointer()
                        .child(
                            img(image)
                                .debug_selector(|| "attachment-image".into())
                                .w_full()
                                .h(px(180.))
                                .max_h(px(180.))
                                .object_fit(ObjectFit::Contain)
                                .with_fallback(move || {
                                    div()
                                        .p_3()
                                        .child(tr(language, "attachment.imageError"))
                                        .into_any_element()
                                }),
                        )
                        .on_click(move |_, window, cx| {
                            open_preview(
                                preview_item.clone(),
                                preview_image.clone(),
                                language,
                                window,
                                cx,
                            )
                        }),
                );
            }
            let open = item.clone();
            card = card.child(
                div()
                    .debug_selector(|| "attachment-info".into())
                    .flex_shrink_0()
                    .w_full()
                    .when(data.is_some_and(|data| data.image.is_some()), |view| {
                        view.border_t_1().border_color(cx.theme().border)
                    })
                    .bg(cx.theme().background)
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .id("filename")
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .tooltip(move |window, cx| {
                                Tooltip::new(full_path.clone()).build(window, cx)
                            })
                            .when(available, |this| {
                                this.cursor_pointer().on_click(move |_, window, cx| {
                                    open_attachment(open.clone(), language, window, cx)
                                })
                            })
                            .child(Icon::new(IconName::FileText).size(px(16.)).flex_shrink_0())
                            .child(
                                div()
                                    .debug_selector(|| "attachment-name".into())
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .child(item.name.clone()),
                            )
                            .when_some(data.and_then(|data| data.size), |view, bytes| {
                                view.child(
                                    div()
                                        .debug_selector(|| "attachment-size".into())
                                        .flex_shrink_0()
                                        .text_size(px(12.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("({})", size_label(bytes))),
                                )
                            }),
                    )
                    .when(data.and_then(|data| data.size).is_none(), |view| {
                        view.child(
                            div()
                                .text_size(px(12.))
                                .text_color(cx.theme().muted_foreground)
                                .child(subtitle),
                        )
                    })
                    .when(available, |this| this.child(actions(&item, language))),
            );
            list = list.child(card);
        }
        list
    }
}

struct ImagePreview {
    item: SessionAttachment,
    image: Arc<Image>,
    language: Language,
    zoom: f32,
}
fn open_preview(
    item: SessionAttachment,
    image: Arc<Image>,
    language: Language,
    window: &mut Window,
    cx: &mut App,
) {
    let title = item.name.clone();
    let preview = cx.new(|_| ImagePreview {
        item,
        image,
        language,
        zoom: 1.,
    });
    window.open_dialog(cx, move |dialog, window, _| {
        dialog
            .title(div().min_w_0().pr_8().truncate().child(title.clone()))
            .width(preview_width(window))
            .margin_top(
                ((window.viewport_size().height - preview_height(window) - px(80.)) / 2.)
                    .max(px(24.)),
            )
            .overlay(true)
            .overlay_closable(true)
            .keyboard(true)
            .close_button(true)
            .child(preview.clone())
    });
}

fn preview_width(window: &Window) -> Pixels {
    (window.viewport_size().width * 0.8)
        .max(px(200.))
        .min(px(860.))
}

fn preview_height(window: &Window) -> Pixels {
    (window.viewport_size().height * 0.6)
        .max(px(120.))
        .min(px(480.))
}

impl Render for ImagePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language = self.language;
        let preview_height = preview_height(window);
        div()
            .debug_selector(|| "attachment-preview-modal".into())
            .flex()
            .flex_col()
            .w_full()
            .h(preview_height)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .p_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("zoom-out")
                                    .ghost()
                                    .label("−")
                                    .tooltip(tr(language, "attachment.zoomOut"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = (this.zoom / 1.25).max(0.25);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("fit")
                                    .ghost()
                                    .label(tr(language, "attachment.fit"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = 1.;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("zoom-in")
                                    .ghost()
                                    .label("+")
                                    .tooltip(tr(language, "attachment.zoomIn"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.zoom = (this.zoom * 1.25).min(8.);
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(actions(&self.item, language)),
            )
            .child(
                div()
                    .id("image-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .child(
                        img(self.image.clone())
                            .w((preview_width(window) - px(34.)) * self.zoom)
                            .h((preview_height - px(88.)).max(px(32.)) * self.zoom)
                            .object_fit(ObjectFit::Contain)
                            .with_fallback(move || {
                                div()
                                    .p_3()
                                    .child(tr(language, "attachment.imageError"))
                                    .into_any_element()
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{Attachments, decoded_data, export_attachment, load_attachment, safe_name};
    use gpui_kit::{
        Context, IntoElement, ParentElement, Render, Styled, TestAppContext, VisualTestContext,
        Window, div, px, size,
    };
    use std::{fs, path::PathBuf};
    use yes_core::{AttachmentSource, Language, SessionAttachment};
    #[test]
    fn filenames_and_embedded_resources_are_bounded() {
        assert_eq!(safe_name("../../some/file.txt"), "file.txt");
        assert_eq!(safe_name(".."), "attachment");
        assert_eq!(safe_name("a\0b.txt"), "ab.txt");
        assert_eq!(
            decoded_data("data:text/plain;base64,aGVsbG8=").unwrap(),
            b"hello"
        );
        assert!(decoded_data("data:text/plain,hello").is_err());
        assert!(decoded_data("data:image/png;base64,??").is_err());
        assert_eq!(
            super::image_format(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            Some(gpui_kit::ImageFormat::Svg)
        );
    }
    #[test]
    fn missing_resources_stay_unavailable_and_directories_are_not_files() {
        for path in [
            PathBuf::from("/nonexistent/yes-attachment.png"),
            std::env::temp_dir(),
        ] {
            let item = SessionAttachment {
                name: "image.png".into(),
                mime_type: None,
                embedded_fallback: None,
                source: AttachmentSource::LocalPath(path),
            };
            let loaded = load_attachment(&item);
            assert!(!loaded.available);
            assert!(loaded.image.is_none());
        }
    }
    #[test]
    fn remote_images_are_not_downloaded() {
        let item = SessionAttachment {
            name: "a.png".into(),
            mime_type: Some("image/png".into()),
            embedded_fallback: None,
            source: AttachmentSource::RemoteUrl("https://example.invalid/a.png".into()),
        };
        let loaded = load_attachment(&item);
        assert!(loaded.available);
        assert!(loaded.image.is_none());
    }
    #[test]
    fn export_preserves_original_when_destination_is_hardlinked() {
        let directory = std::env::temp_dir().join(format!(
            "yes-attachment-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let source = directory.join("original.txt");
        let alias = directory.join("alias.txt");
        let copy = directory.join("copy.txt");
        fs::write(&source, b"original content").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let item = SessionAttachment {
            name: "original.txt".into(),
            mime_type: None,
            embedded_fallback: None,
            source: AttachmentSource::LocalPath(source.clone()),
        };
        export_attachment(&item, &alias).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"original content");
        export_attachment(&item, &copy).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), b"original content");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleted_local_image_uses_embedded_fallback() {
        let item = SessionAttachment {
            name: "image.png".into(),
            mime_type: Some("image/png".into()),
            embedded_fallback: Some("data:image/png;base64,iVBORw0KGgo=".into()),
            source: AttachmentSource::LocalPath("/nonexistent/yes-attachment.png".into()),
        };
        let loaded = load_attachment(&item);
        assert!(loaded.available && loaded.fallback);
        assert!(loaded.image.is_some());
    }

    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w_full().p_3().child(Attachments {
                id: 0,
                language: Language::En,
                items: vec![SessionAttachment {
                    name: format!("{}.pdf", "very-long-attachment-filename-".repeat(20)),
                    mime_type: None,
                    embedded_fallback: None,
                    source: AttachmentSource::LocalPath("/nonexistent/document.pdf".into()),
                }],
            })
        }
    }

    struct TallImageView;
    impl Render for TallImageView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            use base64::Engine as _;
            let image = base64::engine::general_purpose::STANDARD.encode(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"320\" height=\"1200\"><rect width=\"320\" height=\"1200\" fill=\"red\"/></svg>",
            );
            div()
                .w_full()
                .p_3()
                .child(Attachments {
                    id: 0,
                    language: Language::En,
                    items: vec![SessionAttachment {
                        name: "a-long-portrait-screenshot-with-a-descriptive-filename.svg".into(),
                        mime_type: Some("image/svg+xml".into()),
                        embedded_fallback: None,
                        source: AttachmentSource::DataUrl(format!(
                            "data:image/svg+xml;base64,{image}"
                        )),
                    }],
                })
                .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
        }
    }

    #[gpui_kit::test]
    fn image_preview_is_a_dismissible_modal_in_the_same_window(cx: &mut TestAppContext) {
        use gpui_kit::AppContext as _;
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(900.), px(700.)), |window, cx| {
            let view = cx.new(|_| TallImageView);
            gpui_kit::component::Root::new(view, window, cx)
        });
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let window_count = visual.update(|_, cx| cx.windows().len());
        let thumbnail = visual.debug_bounds("attachment-thumbnail").unwrap();
        visual.simulate_click(thumbnail.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(visual.update(|_, cx| cx.windows().len()), window_count);
        assert!(visual.debug_bounds("dialog-layer").is_some());
        let preview = visual.debug_bounds("attachment-preview-modal").unwrap();
        assert!(preview.size.width > px(600.) && preview.bottom() < px(700.));
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(visual.debug_bounds("attachment-preview-modal").is_none());
        visual.simulate_click(thumbnail.center(), Default::default());
        visual.run_until_parked();
        visual.simulate_click(gpui_kit::point(px(8.), px(350.)), Default::default());
        visual.run_until_parked();
        assert!(visual.debug_bounds("attachment-preview-modal").is_none());
    }

    #[gpui_kit::test]
    fn portrait_image_stays_above_file_information(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        for width in [280., 800.] {
            let window = cx.open_window(size(px(width), px(700.)), |_, _| TallImageView);
            let mut visual = VisualTestContext::from_window(*window, cx);
            visual.run_until_parked();
            let thumbnail = visual.debug_bounds("attachment-thumbnail").unwrap();
            let image = visual.debug_bounds("attachment-image").unwrap();
            let info = visual.debug_bounds("attachment-info").unwrap();
            let card = visual.debug_bounds("attachment-card").unwrap();
            assert_eq!(thumbnail.size.height, px(180.));
            assert!(
                image.bottom() <= thumbnail.bottom(),
                "image must fit its reserved area: {image:?} {thumbnail:?}"
            );
            assert!(
                thumbnail.bottom() <= info.top(),
                "file information must be below the image"
            );
            assert!(info.bottom() <= card.bottom());
            assert!(card.right() <= px(width - 12.));
        }
    }
    #[gpui_kit::test]
    fn long_attachment_filename_fits_narrow_window(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.open_window(size(px(360.), px(600.)), |_, _| TestView);
        let mut visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        let card = visual.debug_bounds("attachment-card").unwrap();
        assert!(card.size.width <= px(336.));
        assert!(card.right() <= px(348.));
        assert!(card.size.height < px(140.));
    }
}
