use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::component::{
    ActiveTheme as _, IconName,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::*;
use wry::WebViewBuilder;
use yes_core::Language;

use crate::i18n::tr;

static NEXT_DIAGRAM_ID: AtomicUsize = AtomicUsize::new(1);

pub struct MermaidDiagram {
    webview: Entity<gpui_wry::WebView>,
    language: Language,
    scale: f32,
    id: usize,
}

impl MermaidDiagram {
    pub fn hide(entity: &Entity<Self>, cx: &mut App) {
        let webview = entity.read(cx).webview.clone();
        webview.update(cx, |webview, _| webview.hide());
    }
}

pub fn create_mermaid_diagram(
    source: &str,
    dark: bool,
    language: Language,
    window: &mut Window,
    cx: &mut App,
) -> anyhow::Result<Entity<MermaidDiagram>> {
    let html = mermaid_html(source, dark)?;
    let raw = WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        .build_as_child(window)?;
    let webview = cx.new(|cx| gpui_wry::WebView::new(raw, window, cx));
    Ok(cx.new(|_| MermaidDiagram {
        webview,
        language,
        scale: 1.,
        id: NEXT_DIAGRAM_ID.fetch_add(1, Ordering::Relaxed),
    }))
}

impl Render for MermaidDiagram {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.webview.update(cx, |webview, _| webview.show());
        let zoom_out = self.webview.clone();
        let zoom_out_owner = cx.weak_entity();
        let reset = self.webview.clone();
        let reset_owner = cx.weak_entity();
        let zoom_in = self.webview.clone();
        let zoom_in_owner = cx.weak_entity();
        let id = self.id;
        div()
            .w_full()
            .h(px(568.))
            .rounded_md()
            .border_1()
            .border_color(cx.theme().list_active_border)
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(
                div()
                    .h(px(40.))
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(cx.theme().button)
                    .border_b_1()
                    .border_color(cx.theme().list_active_border)
                    .text_size(px(12.))
                    .text_color(cx.theme().primary)
                    .child(tr(self.language, "mermaid.diagram"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(("mermaid-zoom-out", id))
                                    .ghost()
                                    .compact()
                                    .size(px(28.))
                                    .icon(IconName::Minus)
                                    .tooltip(tr(self.language, "mermaid.zoomOut"))
                                    .on_click(move |_, _, cx| {
                                        zoom_out.update(cx, |webview, _| {
                                            let _ = webview.raw().evaluate_script("zoomBy(1/1.2)");
                                        });
                                        let _ = zoom_out_owner.update(cx, |this, cx| {
                                            this.scale = (this.scale / 1.2).max(0.1);
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .min_w(px(50.))
                                    .text_center()
                                    .child(format!("{:.0}%", self.scale * 100.)),
                            )
                            .child(
                                Button::new(("mermaid-zoom-in", id))
                                    .ghost()
                                    .compact()
                                    .size(px(28.))
                                    .icon(IconName::Plus)
                                    .tooltip(tr(self.language, "mermaid.zoomIn"))
                                    .on_click(move |_, _, cx| {
                                        zoom_in.update(cx, |webview, _| {
                                            let _ = webview.raw().evaluate_script("zoomBy(1.2)");
                                        });
                                        let _ = zoom_in_owner.update(cx, |this, cx| {
                                            this.scale = (this.scale * 1.2).min(5.);
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                Button::new(("mermaid-reset", id))
                                    .ghost()
                                    .compact()
                                    .icon(IconName::RotateCw)
                                    .label(tr(self.language, "mermaid.reset"))
                                    .tooltip(tr(self.language, "mermaid.resetZoom"))
                                    .on_click(move |_, _, cx| {
                                        reset.update(cx, |webview, _| {
                                            let _ = webview.raw().evaluate_script("resetView()");
                                        });
                                        let _ = reset_owner.update(cx, |this, cx| {
                                            this.scale = 1.;
                                            cx.notify();
                                        });
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(500.))
                    .bg(cx.theme().selection.opacity(0.3))
                    .child(self.webview.clone()),
            )
            .child(
                div()
                    .h(px(28.))
                    .px_3()
                    .py(px(6.))
                    .text_size(px(10.))
                    .text_color(cx.theme().primary)
                    .child(tr(self.language, "mermaid.zoomHint")),
            )
    }
}

fn mermaid_html(source: &str, dark: bool) -> anyhow::Result<String> {
    let mermaid_js = load_mermaid_js()?;
    // JSON quoting alone does not protect an inline HTML script: </script>
    // terminates it even inside a JavaScript string. Escape every '<'.
    let encoded_source = serde_json::to_string(source)?.replace('<', "\\u003c");
    let theme = if dark { "dark" } else { "default" };
    let foreground = if dark { "#e5e7eb" } else { "#172033" };
    let surface = if dark { "#15181d" } else { "#ffffff" };
    Ok(format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><style>
html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:{surface};color:{foreground};font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}}
#stage{{position:absolute;inset:0;display:grid;place-items:center;overflow:hidden;cursor:grab}}
#stage.dragging{{cursor:grabbing}}
#diagram{{transform-origin:center center;transition:transform .08s linear;padding:32px}}
#diagram svg{{display:block;max-width:none}}
#error{{display:none;white-space:pre-wrap;padding:18px;color:#ef4444;font:12px ui-monospace,monospace}}
</style><script>{mermaid_js}</script></head><body>
<div id="stage"><div id="diagram"></div></div><pre id="error"></pre>
<script>
const source={encoded_source}; let scale=1, x=0, y=0, dragging=false, sx=0, sy=0;
const stage=document.getElementById('stage'), diagram=document.getElementById('diagram');
const apply=()=>diagram.style.transform=`translate(${{x}}px,${{y}}px) scale(${{scale}})`;
const zoomBy=factor=>{{scale=Math.min(5,Math.max(.1,scale*factor));apply()}};
const resetView=()=>{{scale=1;x=0;y=0;apply()}};
stage.addEventListener('wheel',e=>{{if(!(e.metaKey||e.ctrlKey))return;e.preventDefault();zoomBy(e.deltaY<0?1.1:.9)}},{{passive:false}});
stage.addEventListener('mousedown',e=>{{dragging=true;sx=e.clientX-x;sy=e.clientY-y;stage.classList.add('dragging')}});
addEventListener('mousemove',e=>{{if(dragging){{x=e.clientX-sx;y=e.clientY-sy;apply()}}}});
addEventListener('mouseup',()=>{{dragging=false;stage.classList.remove('dragging')}});
mermaid.initialize({{startOnLoad:false,securityLevel:'strict',theme:'{theme}'}});
mermaid.render('yes-sessions-mermaid',source).then(r=>diagram.innerHTML=r.svg).catch(error=>{{stage.style.display='none';const out=document.getElementById('error');out.style.display='block';out.textContent=String(error)}});
</script></body></html>"#
    ))
}

fn load_mermaid_js() -> anyhow::Result<String> {
    let development_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/mermaid.min.js");
    let packaged_path = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .map(|path| path.join("Resources/mermaid.min.js"));
    let path = packaged_path
        .filter(|path| path.is_file())
        .unwrap_or(development_path);
    Ok(std::fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::mermaid_html;

    #[test]
    fn diagram_source_cannot_terminate_the_script_element() {
        let source = "graph TD\nA[\"</ScRiPt><script>window.injected=true</script><!--\"]";
        let html = mermaid_html(source, false).unwrap();
        let encoded = html
            .split("const source=")
            .nth(1)
            .unwrap()
            .split("; let scale=")
            .next()
            .unwrap();
        assert!(!encoded.contains('<'));
        assert_eq!(serde_json::from_str::<String>(encoded).unwrap(), source);
    }
}
