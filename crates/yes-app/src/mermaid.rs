use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::component::{
    ActiveTheme as _, IconName,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::*;
use wry::WebViewBuilder;
use yes_core::Language;

use crate::i18n::tr;

mod native_clip;
use native_clip::NativeClip;

static NEXT_DIAGRAM_ID: AtomicUsize = AtomicUsize::new(1);

pub struct MermaidDiagram {
    webview: Entity<gpui_wry::WebView>,
    clip: std::rc::Rc<NativeClip>,
    language: Language,
    scale: f32,
    id: usize,
}

impl MermaidDiagram {
    pub fn hide(entity: &Entity<Self>, cx: &mut App) {
        entity.read(cx).clip.hide();
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
    let html = mermaid_html(source, dark, language)?;
    let raw = WebViewBuilder::new()
        .with_html(html)
        .with_transparent(true)
        .build_as_child(window)?;
    let webview = cx.new(|cx| gpui_wry::WebView::new(raw, window, cx));
    let clip = std::rc::Rc::new(NativeClip::new(webview.read(cx).raw()));
    Ok(cx.new(|_| MermaidDiagram {
        webview,
        clip,
        language,
        scale: 1.,
        id: NEXT_DIAGRAM_ID.fetch_add(1, Ordering::Relaxed),
    }))
}

impl Render for MermaidDiagram {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let clip = self.clip.clone();
        let handle = self.webview.read(cx).handle();
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
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                clip.update(handle.raw(), bounds, window.content_mask().bounds);
                            },
                        )
                        .size_full(),
                    ),
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

fn mermaid_html(source: &str, dark: bool, language: Language) -> anyhow::Result<String> {
    let mermaid_js = load_mermaid_js()?;
    // JSON quoting alone does not protect an inline HTML script: </script>
    // terminates it even inside a JavaScript string. Escape every '<'.
    let encoded_source = serde_json::to_string(source)?.replace('<', "\\u003c");
    let labels = serde_json::to_string(&[
        tr(language, "mermaid.renderError"),
        tr(language, "mermaid.retry"),
    ])?
    .replace('<', "\\u003c");
    let theme = if dark { "dark" } else { "default" };
    let foreground = if dark { "#e5e7eb" } else { "#172033" };
    let surface = if dark { "#15181d" } else { "#ffffff" };
    Ok(format!(
        r#"<!doctype html>
<html><head><meta charset="utf-8"><style>
html,body{{margin:0;width:100%;height:100%;overflow:hidden;background:{surface};color:{foreground};font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}}
#stage{{position:absolute;inset:0;display:grid;grid-template-columns:minmax(0,1fr);grid-template-rows:minmax(0,1fr);place-items:center;overflow:hidden;cursor:grab}}
#stage.dragging{{cursor:grabbing}}
#diagram{{transform-origin:center center;transition:transform .08s linear}}
#diagram svg{{display:block;max-width:none}}
#fallback{{display:none;box-sizing:border-box;height:100%;overflow:auto;padding:18px}}
#failure-heading{{display:flex;align-items:center;justify-content:space-between;gap:16px;font-size:13px}}
#retry{{flex:none;border:1px solid currentColor;border-radius:6px;padding:6px 12px;background:transparent;color:inherit;cursor:pointer}}
#retry:disabled{{opacity:.5;cursor:wait}}
#error,#source{{white-space:pre-wrap;overflow-wrap:anywhere;font:12px/1.5 ui-monospace,monospace}}
#error{{color:#ef4444}}
</style><script>{mermaid_js}</script></head><body>
<div id="stage"><div id="diagram"></div></div>
<section id="fallback" aria-live="polite"><div id="failure-heading"><span id="failure-message"></span><button id="retry" type="button"></button></div><pre id="error"></pre><pre id="source"></pre></section>
<script>
const source={encoded_source}; let scale=1, x=0, y=0, dragging=false, sx=0, sy=0;
let fitScale=1, autoFit=true, diagramWidth=0, diagramHeight=0;
const stage=document.getElementById('stage'), diagram=document.getElementById('diagram');
// Toolbar percentages are relative to the fitted view (100% = fit to viewport).
const apply=()=>diagram.style.transform=`translate(${{x}}px,${{y}}px) scale(${{fitScale*scale}})`;
function fitView(){{
    if(!autoFit||!diagramWidth||!diagramHeight||stage.clientWidth<=32||stage.clientHeight<=32)return;
    fitScale=Math.min((stage.clientWidth-32)/diagramWidth,(stage.clientHeight-32)/diagramHeight);
    scale=1;x=0;y=0;apply();
}}
const zoomBy=factor=>{{autoFit=false;scale=Math.min(5,Math.max(.1,scale*factor));apply()}};
const resetView=()=>{{autoFit=true;fitView()}};
new ResizeObserver(fitView).observe(stage);
stage.addEventListener('wheel',e=>{{if(!(e.metaKey||e.ctrlKey))return;e.preventDefault();zoomBy(e.deltaY<0?1.1:.9)}},{{passive:false}});
stage.addEventListener('mousedown',e=>{{dragging=true;sx=e.clientX-x;sy=e.clientY-y;stage.classList.add('dragging')}});
addEventListener('mousemove',e=>{{if(dragging){{autoFit=false;x=e.clientX-sx;y=e.clientY-sy;apply()}}}});
addEventListener('mouseup',()=>{{dragging=false;stage.classList.remove('dragging')}});
const labels={labels}, fallback=document.getElementById('fallback'), retry=document.getElementById('retry');
document.getElementById('failure-message').textContent=labels[0];
retry.textContent=labels[1];
document.getElementById('source').textContent=source;
async function renderDiagram(){{
    if(retry.disabled)return;
    retry.disabled=true;
    diagram.replaceChildren();
    stage.style.display='grid';stage.style.visibility='hidden';
    try{{
        mermaid.initialize({{startOnLoad:false,securityLevel:'strict',theme:'{theme}'}});
        const result=await mermaid.render('yes-sessions-mermaid',source,diagram);
        diagram.innerHTML=result.svg;
        const svg=diagram.querySelector('svg'), box=svg.viewBox.baseVal;
        // Mermaid emits percentage widths for some diagram types. Use its logical
        // drawing dimensions so fitting is independent of the initial WebView size.
        diagramWidth=box.width;diagramHeight=box.height;
        svg.style.width=`${{diagramWidth}}px`;svg.style.height=`${{diagramHeight}}px`;
        diagram.style.width=`${{diagramWidth}}px`;diagram.style.height=`${{diagramHeight}}px`;
        fallback.style.display='none';stage.style.display='grid';stage.style.visibility='visible';
        resetView();
    }}catch(error){{
        diagram.replaceChildren();
        stage.style.display='none';fallback.style.display='block';
        document.getElementById('error').textContent=String(error);
    }}finally{{retry.disabled=false}}
}}
retry.addEventListener('click',renderDiagram);
renderDiagram();
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
    use yes_core::Language;

    #[test]
    fn diagram_source_cannot_terminate_the_script_element() {
        let source = "graph TD\nA[\"</ScRiPt><script>window.injected=true</script><!--\"]";
        let html = mermaid_html(source, false, Language::En).unwrap();
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

    #[test]
    fn render_failure_preserves_source_and_offers_retry_in_both_languages() {
        for language in [Language::En, Language::Zh] {
            let html = mermaid_html(
                "not valid mermaid <img src=x onerror=alert(1)>",
                true,
                language,
            )
            .unwrap();
            assert!(html.contains("document.getElementById('source').textContent=source"));
            assert!(html.contains("document.getElementById('error').textContent=String(error)"));
            assert!(html.contains("retry.addEventListener('click',renderDiagram)"));
            assert!(html.contains("finally{retry.disabled=false}"));
            assert!(html.contains("fallback.style.display='none';stage.style.display='grid'"));
            assert!(html.contains("securityLevel:'strict'"));
            let labels = html
                .split("const labels=")
                .nth(1)
                .unwrap()
                .split(", fallback=")
                .next()
                .unwrap();
            let labels: Vec<String> = serde_json::from_str(labels).unwrap();
            assert_eq!(
                labels,
                [
                    crate::i18n::tr(language, "mermaid.renderError"),
                    crate::i18n::tr(language, "mermaid.retry")
                ]
            );
        }
    }
}
