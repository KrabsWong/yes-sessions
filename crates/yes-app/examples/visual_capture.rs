use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::{Root, Theme, ThemeMode};
use gpui_kit::*;
use yes_core::{
    AccentColor, SessionProvider, ThemePreference, model::SessionKind, providers::CodeBuddyProvider,
};
use yes_sessions::{app::YesSessions, app_assets::AppAssets};

const WINDOW_WIDTH: f32 = 1200.0;
const WINDOW_HEIGHT: f32 = 800.0;

fn main() -> Result<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .context("resolve workspace root")?;
    let output_dir = workspace.join("target/visual");
    let fixture_home = workspace.join("target/visual-fixture-home");
    prepare_fixture(&fixture_home)?;
    validate_fixture(&fixture_home)?;
    fs::create_dir_all(&output_dir).context("create visual output directory")?;

    // SAFETY: This single-threaded example sets HOME before GPUI or any worker is created.
    unsafe { env::set_var("HOME", &fixture_home) };

    let platform = gpui_kit::platform::current_platform(false);
    let mut cx = VisualTestAppContext::with_asset_source(platform, Arc::new(AppAssets));
    cx.update(|cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Light, None, cx);
    });

    let mut app_handle = None;
    let window = cx
        .open_offscreen_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |window, cx| {
            Theme::change(ThemeMode::Light, Some(window), cx);
            let app = cx.new(|cx| YesSessions::new(window, cx));
            app_handle = Some(app.clone());
            cx.new(|cx| Root::new(app, window, cx))
        })
        .context("open offscreen Yes Sessions window")?;
    let window = AnyWindowHandle::from(window);
    let app = app_handle.context("capture Yes Sessions entity")?;

    settle(&mut cx, window)?;
    save_capture(&mut cx, window, output_dir.join("main-light.png"))?;

    // Drag across Markdown text so selection contrast is verified against
    // actual glyphs instead of only checking the theme token.
    drag(
        &mut cx,
        window,
        point(px(420.0), px(390.0)),
        point(px(900.0), px(420.0)),
    )?;
    save_capture(&mut cx, window, output_dir.join("text-selection-light.png"))?;
    click(&mut cx, window, 1000.0, 260.0)?;

    cx.update_window(window, |_, window, cx| {
        YesSessions::configure_theme(ThemePreference::Dark, AccentColor::Blue, window, cx);
        window.refresh();
    })?;
    settle(&mut cx, window)?;
    save_capture(&mut cx, window, output_dir.join("main-dark.png"))?;

    cx.update_window(window, |_, window, cx| {
        YesSessions::configure_theme(ThemePreference::Light, AccentColor::Blue, window, cx);
        window.refresh();
    })?;
    settle(&mut cx, window)?;

    cx.simulate_mouse_move(
        window,
        point(px(42.0), px(129.0)),
        None,
        Modifiers::default(),
    );
    cx.advance_clock(std::time::Duration::from_millis(250));
    cx.run_until_parked();
    save_capture(&mut cx, window, output_dir.join("stats-tooltip-light.png"))?;
    cx.simulate_mouse_move(
        window,
        point(px(700.0), px(70.0)),
        None,
        Modifiers::default(),
    );
    settle(&mut cx, window)?;

    click(&mut cx, window, 170.0, 82.0)?;
    cx.advance_clock(std::time::Duration::from_millis(250));
    cx.run_until_parked();
    save_capture(&mut cx, window, output_dir.join("provider-menu-light.png"))?;
    click(&mut cx, window, 170.0, 159.0)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("provider-selected-claude-light.png"),
    )?;
    click(&mut cx, window, 170.0, 82.0)?;
    cx.advance_clock(std::time::Duration::from_millis(250));
    cx.run_until_parked();
    click(&mut cx, window, 170.0, 125.0)?;

    click(&mut cx, window, 290.0, 129.0)?;
    save_capture(&mut cx, window, output_dir.join("directory-mode-light.png"))?;
    click(&mut cx, window, 225.0, 129.0)?;

    // Capture each ordinary tool type while the conversation is at its
    // deterministic initial position. Reselecting the parent between captures
    // resets both its scroll position and expanded-message state.
    click(&mut cx, window, 800.0, 540.0)?;
    save_capture(&mut cx, window, output_dir.join("tool-expanded-light.png"))?;
    click(&mut cx, window, 160.0, 204.0)?;

    click(&mut cx, window, 800.0, 635.0)?;
    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(850.0), px(650.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-160.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("tool-bash-expanded-light.png"),
    )?;
    click(&mut cx, window, 160.0, 204.0)?;

    click(&mut cx, window, 800.0, 730.0)?;
    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(850.0), px(650.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-220.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("tool-grep-expanded-light.png"),
    )?;
    click(&mut cx, window, 160.0, 204.0)?;

    // Hovering expands the navigator; clicking its last preview selects a
    // non-current user turn and scrolls that turn into view.
    cx.simulate_mouse_move(
        window,
        point(px(1170.0), px(470.0)),
        None,
        Modifiers::default(),
    );
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("navigator-hover-light.png"),
    )?;
    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(1170.0), px(470.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-180.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("navigator-scroll-light.png"),
    )?;
    click(&mut cx, window, 1080.0, 493.0)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("navigator-click-light.png"),
    )?;
    click(&mut cx, window, 1080.0, 388.0)?;
    cx.simulate_mouse_move(
        window,
        point(px(700.0), px(70.0)),
        None,
        Modifiers::default(),
    );
    settle(&mut cx, window)?;

    // Expand the selected parent, then open its child from the session tree.
    // This proves that the hierarchy remains usable even when the parent title
    // consumes all available row width.
    click(&mut cx, window, 304.0, 204.0)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("parent-expanded-light.png"),
    )?;
    click(&mut cx, window, 160.0, 240.0)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("subagent-detail-light.png"),
    )?;
    click(&mut cx, window, 760.0, 117.0)?;

    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(850.0), px(650.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-620.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;
    save_capture(&mut cx, window, output_dir.join("subagent-card-light.png"))?;
    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(850.0), px(650.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-420.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("subagent-card-actions-light.png"),
    )?;
    cx.update_entity(&app, |app, cx| {
        app.toggle_inline_sub_agent("visual-child".to_owned(), 4, cx);
    });
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("subagent-inline-light.png"),
    )?;
    cx.simulate_event(
        window,
        ScrollWheelEvent {
            position: point(px(850.0), px(420.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(1040.0))),
            ..Default::default()
        },
    );
    settle(&mut cx, window)?;

    // The settings button is the rightmost control in the fixed 1200 px header.
    cx.simulate_click(window, point(px(1150.0), px(26.0)), Modifiers::default());
    settle(&mut cx, window)?;
    save_capture(&mut cx, window, output_dir.join("settings-light.png"))?;

    cx.simulate_click(window, point(px(460.0), px(228.0)), Modifiers::default());
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("settings-experience-light.png"),
    )?;

    cx.simulate_click(window, point(px(560.0), px(205.0)), Modifiers::default());
    settle(&mut cx, window)?;
    save_capture(
        &mut cx,
        window,
        output_dir.join("settings-terminal-light.png"),
    )?;

    fs::remove_dir_all(&fixture_home).context("remove visual fixture home")?;
    println!("visual captures written to {}", output_dir.display());
    Ok(())
}

fn settle(cx: &mut VisualTestAppContext, window: AnyWindowHandle) -> Result<()> {
    cx.run_until_parked();
    cx.update_window(window, |_, window, _| window.refresh())?;
    cx.advance_clock(std::time::Duration::from_millis(34));
    cx.run_until_parked();
    Ok(())
}

fn click(cx: &mut VisualTestAppContext, window: AnyWindowHandle, x: f32, y: f32) -> Result<()> {
    cx.simulate_mouse_move(window, point(px(x), px(y)), None, Modifiers::default());
    cx.run_until_parked();
    cx.simulate_click(window, point(px(x), px(y)), Modifiers::default());
    settle(cx, window)
}

fn drag(
    cx: &mut VisualTestAppContext,
    window: AnyWindowHandle,
    from: Point<Pixels>,
    to: Point<Pixels>,
) -> Result<()> {
    cx.simulate_mouse_down(window, from, MouseButton::Left, Modifiers::default());
    settle(cx, window)?;
    cx.simulate_mouse_move(window, to, Some(MouseButton::Left), Modifiers::default());
    settle(cx, window)?;
    cx.simulate_mouse_up(window, to, MouseButton::Left, Modifiers::default());
    settle(cx, window)
}

fn save_capture(
    cx: &mut VisualTestAppContext,
    window: AnyWindowHandle,
    path: PathBuf,
) -> Result<()> {
    let image = cx.capture_screenshot(window)?;
    image
        .save(&path)
        .with_context(|| format!("save {}", path.display()))?;
    Ok(())
}

fn prepare_fixture(home: &Path) -> Result<()> {
    if home.exists() {
        fs::remove_dir_all(home).context("clear previous visual fixture")?;
    }

    let projects = home
        .join(".codebuddy/projects")
        .join("Users-visual-yes-sessions");
    let active_sessions = home.join(".codebuddy/sessions");
    let settings_dir = home.join("Library/Application Support/yes-sessions");
    fs::create_dir_all(&projects).context("create CodeBuddy fixture project")?;
    fs::create_dir_all(projects.join("visual-main/subagents"))
        .context("create CodeBuddy sub-agent fixture directory")?;
    fs::create_dir_all(&active_sessions).context("create CodeBuddy active sessions fixture")?;
    fs::create_dir_all(&settings_dir).context("create settings fixture")?;

    write_session(
        &projects.join("visual-main.jsonl"),
        "visual-main",
        &[
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:40:00Z","cwd":"/Users/visual/Projects/yes-sessions","sessionId":"visual-main","providerData":{"model":"claude-sonnet-4"},"content":[{"type":"input_text","text":"请分析当前桌面应用，并给出一个兼顾视觉还原、启动速度、工具调用信息密度和完整交互状态的原生重构方案，这是一条用于验证侧边栏省略号的超长会话标题。"}]}"##,
            r##"{"type":"reasoning","timestamp":"2026-08-15T09:40:02Z","rawContent":[{"type":"reasoning_text","text":"我会先核对窗口层级、会话列表密度和消息阅读区域，再确定迁移边界。"}]}"##,
            r####"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:40:06Z","content":[{"type":"output_text","text":"### 重构建议\n\n保留三栏信息层级，用 **GPUI Kit** 复刻标题栏、会话分组和消息卡片；对照 **`onContentClick`** 与 `onKuiklyContentClick` 的命名语义，Mermaid 单独交给 WKWebView。\n\n```rust\nTheme::change(ThemeMode::Light, Some(window), cx);\n```"}]}"####,
            r##"{"type":"function_call","name":"Skill","callId":"call-skill","timestamp":"2026-08-15T09:40:08Z","providerData":{"model":"claude-sonnet-4"},"arguments":{"skill":"native-desktop","path":"skills/native-desktop/SKILL.md"}}"##,
            r##"{"type":"function_call_result","name":"Skill","callId":"call-skill","timestamp":"2026-08-15T09:40:09Z","output":"Base directory for this skill: /Users/visual/.codebuddy/skills/native-desktop\n\nUse compact macOS desktop spacing and native interactions."}"##,
            r##"{"type":"function_call","name":"Bash","callId":"call-bash","timestamp":"2026-08-15T09:40:10Z","providerData":{"model":"claude-sonnet-4"},"arguments":{"command":"cargo check --workspace","description":"Check the Rust workspace"}}"##,
            r##"{"type":"function_call_result","name":"Bash","callId":"call-bash","timestamp":"2026-08-15T09:40:11Z","output":"Finished dev profile in 1.42s"}"##,
            r##"{"type":"function_call","name":"Grep","callId":"call-grep","timestamp":"2026-08-15T09:40:12Z","providerData":{"model":"claude-sonnet-4"},"arguments":{"pattern":"render_tool","path":"crates/yes-app/src"}}"##,
            r##"{"type":"function_call_result","name":"Grep","callId":"call-grep","timestamp":"2026-08-15T09:40:13Z","output":"crates/yes-app/src/conversation.rs:338:fn render_tool("}"##,
            r##"{"type":"function_call","name":"Agent","callId":"call-agent","timestamp":"2026-08-15T09:40:14Z","providerData":{"model":"claude-sonnet-4"},"arguments":{"description":"Audit the sidebar","prompt":"Compare the GPUI sidebar with the reference.","subagent_type":"Explore"}}"##,
            r##"{"type":"function_call_result","name":"Agent","callId":"call-agent","timestamp":"2026-08-15T09:40:15Z","status":"completed","providerData":{"toolResult":{"subAgent":{"sessionId":"visual-child"}}},"output":"I have thoroughly mapped the sidebar implementation.\n\n# Sidebar audit\n\nThe parent session row keeps its child count visible.\nThe child session is rendered inline beneath the parent.\nThe Bot icon and Explore badge match the reference.\nThe selected child keeps a purple left border.\nLong parent titles no longer displace the child count.\nThe disclosure button has a stable hit target.\nDate and directory grouping retain their state.\nThe child title uses a single-line ellipsis.\nThe session hierarchy remains keyboard accessible.\nThe child session can be opened directly.\nThe detail header links back to the parent.\nThe output card preserves line breaks.\nThe model badge remains visible.\nThe status badge reports completion.\nThe task label is visually separated.\nThe output label begins a second section.\nThe first twenty lines are shown by default.\nThe remaining output can be expanded.\nThe inline conversation is loaded on demand.\nThe full conversation action remains available.\nNo Electron runtime is required.\nNo general-purpose WebView is used.\nThe implementation is native GPUI.\nVisual regression coverage is enabled."}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:41:00Z","content":[{"type":"input_text","text":"很好，请把浅色模式作为第一张视觉基准图。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:41:05Z","content":[{"type":"output_text","text":"已准备 1200 × 800 的固定视口，并加入列表、推理、代码和工具调用等代表性内容。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:42:00Z","content":[{"type":"input_text","text":"再检查一次工具卡片展开后的输入和输出。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:42:05Z","content":[{"type":"output_text","text":"输入与输出已经使用独立区块展示。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:43:00Z","content":[{"type":"input_text","text":"快速导航悬停后也应该显示消息预览。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:43:05Z","content":[{"type":"output_text","text":"已加入悬停展开状态和点击定位行为。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:44:00Z","content":[{"type":"input_text","text":"导航条目超过八条时应该在浮层内部滚动。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:44:05Z","content":[{"type":"output_text","text":"浮层内部滚动不会带动外层会话。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:45:00Z","content":[{"type":"input_text","text":"第一条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:45:05Z","content":[{"type":"output_text","text":"已记录第一条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:46:00Z","content":[{"type":"input_text","text":"第二条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:46:05Z","content":[{"type":"output_text","text":"已记录第二条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:47:00Z","content":[{"type":"input_text","text":"第三条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:47:05Z","content":[{"type":"output_text","text":"已记录第三条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:48:00Z","content":[{"type":"input_text","text":"第四条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:48:05Z","content":[{"type":"output_text","text":"已记录第四条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:49:00Z","content":[{"type":"input_text","text":"第五条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:49:05Z","content":[{"type":"output_text","text":"已记录第五条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:50:00Z","content":[{"type":"input_text","text":"第六条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:50:05Z","content":[{"type":"output_text","text":"已记录第六条补充消息。"}]}"##,
            r##"{"type":"message","role":"user","timestamp":"2026-08-15T09:51:00Z","content":[{"type":"input_text","text":"第七条补充消息用于验证导航滚动位置。"}]}"##,
            r##"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:51:05Z","content":[{"type":"output_text","text":"已记录第七条补充消息。"}]}"##,
        ],
    )?;
    write_session(
        &projects.join("visual-main/subagents/visual-child.jsonl"),
        "visual-child",
        &[
            r#"{"type":"message","role":"user","timestamp":"2026-08-15T09:40:14Z","cwd":"/Users/visual/Projects/yes-sessions","sessionId":"visual-child","providerData":{"isSubAgent":true,"agent":"Explore","model":"claude-sonnet-4"},"content":[{"type":"input_text","text":"检查侧边栏会话树的缩进、图标和选中状态。"}]}"#,
            r####"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:40:15Z","providerData":{"isSubAgent":true,"agent":"Explore","model":"claude-sonnet-4"},"content":[{"type":"output_text","text":"### 审计计划\n\n我会检查树形缩进，并核对 **图标、选中态和工具卡片**。"}]}"####,
            r##"{"type":"function_call","name":"Grep","callId":"child-grep","timestamp":"2026-08-15T09:40:16Z","providerData":{"isSubAgent":true,"agent":"Explore","model":"claude-sonnet-4"},"arguments":{"pattern":"session-wrapper","path":"crates/yes-app/src/app.rs"}}"##,
            r##"{"type":"function_call_result","name":"Grep","callId":"child-grep","timestamp":"2026-08-15T09:40:17Z","output":"crates/yes-app/src/app.rs: session-wrapper applies depth-based left padding"}"##,
            r#"{"type":"message","role":"assistant","timestamp":"2026-08-15T09:40:18Z","providerData":{"isSubAgent":true,"agent":"Explore","model":"claude-sonnet-4"},"content":[{"type":"output_text","text":"已完成子 Agent 侧边栏审计，子项保持独立缩进，工具调用保留输入与输出。"}]}"#,
        ],
    )?;
    write_session(
        &projects.join("visual-settings.jsonl"),
        "visual-settings",
        &[
            r#"{"type":"message","role":"user","timestamp":"2026-08-14T15:20:00Z","cwd":"/Users/visual/Projects/yes-sessions","sessionId":"visual-settings","content":[{"type":"input_text","text":"统一设置页的间距、按钮状态与中英文文案。"}]}"#,
        ],
    )?;
    write_session(
        &projects.join("visual-provider.jsonl"),
        "visual-provider",
        &[
            r#"{"type":"message","role":"user","timestamp":"2026-08-13T11:10:00Z","cwd":"/Users/visual/Projects/provider-adapters","sessionId":"visual-provider","content":[{"type":"input_text","text":"检查四个 provider 的会话列表性能和字段一致性。"}]}"#,
        ],
    )?;

    write_active_session(
        &active_sessions.join("visual-main.json"),
        "visual-main",
        "请分析当前桌面应用，并给出一个兼顾视觉还原、启动速度、工具调用信息密度和完整交互状态的原生重构方案，这是一条用于验证侧边栏省略号的超长会话标题",
        1_786_786_860_000,
    )?;
    write_active_session(
        &active_sessions.join("visual-settings.json"),
        "visual-settings",
        "设置页面视觉规范",
        1_786_720_800_000,
    )?;
    write_active_session(
        &active_sessions.join("visual-provider.json"),
        "visual-provider",
        "Provider 数据兼容性",
        1_786_619_400_000,
    )?;

    fs::write(
        settings_dir.join("settings.json"),
        r#"{
  "language": "zh",
  "theme": "light",
  "accentColor": "blue",
  "defaultApp": "codebuddy",
  "collapseBashBlocks": true,
  "enableTitleMarquee": false,
  "showThinkingContent": true,
  "chatLayout": "left",
  "sidebarCollapsed": false,
  "preferredTerminal": "auto"
}"#,
    )
    .context("write settings fixture")?;

    Ok(())
}

fn validate_fixture(home: &Path) -> Result<()> {
    let provider = CodeBuddyProvider::with_root(home.join(".codebuddy"));
    let sessions = provider
        .sessions()
        .context("read visual fixture sessions")?;
    ensure!(
        sessions.iter().any(|session| session.id == "visual-main"),
        "visual fixture is missing its parent session"
    );
    ensure!(
        sessions.iter().any(|session| {
            session.id == "visual-child"
                && session.kind == SessionKind::Subagent
                && session.parent_session_id.as_deref() == Some("visual-main")
        }),
        "visual fixture is missing its child session relationship"
    );
    Ok(())
}

fn write_session(path: &Path, session_id: &str, lines: &[&str]) -> Result<()> {
    let mut source = lines.join("\n");
    source.push('\n');
    fs::write(path, source)
        .with_context(|| format!("write {session_id} fixture at {}", path.display()))
}

fn write_active_session(path: &Path, session_id: &str, title: &str, updated_at: i64) -> Result<()> {
    let value = serde_json::json!({
        "sessionId": session_id,
        "updatedAt": updated_at,
        "meta": { "currentTopic": title },
    });
    fs::write(path, serde_json::to_vec_pretty(&value)?)
        .with_context(|| format!("write active session fixture at {}", path.display()))
}
