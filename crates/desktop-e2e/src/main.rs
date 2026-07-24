//! UI end-to-end runner for the Agent Deck Tauri app.
//!
//! Drives the real macOS app via the Accessibility API (through ax_driver.swift),
//! against isolated fixture data (never touches ~/.zcode). Each smoke case
//! asserts one slice of the user-action → IPC → backend → UI-repaint loop.
//!
//! Run: cargo run -p agent-deck-desktop-e2e
//! Prereqs: `pnpm build:desktop` (builds the .app), Xcode/swift toolchain,
//!          and Accessibility permission for the terminal running this
//!          (System Settings → Privacy & Security → Accessibility).

mod fixture;
mod harness;

use anyhow::Result;
use harness::E2e;

fn main() -> Result<()> {
    println!("=== Agent Deck UI e2e (Accessibility API + WKWebView) ===\n");

    let mut e2e = E2e::start()?;

    let outcome = run_cases(&mut e2e);
    // Drop tears the app + driver down regardless of outcome.
    let _ = outcome.is_ok();
    outcome?;

    println!("\n=== ALL SMOKE CASES PASSED ✅ ===");
    Ok(())
}

fn run_cases(e2e: &mut E2e) -> Result<()> {
    case_app_boots(e2e)?;
    case_open_settings(e2e)?;
    case_toggle_autofill(e2e)?;
    case_key_interaction(e2e)?;
    Ok(())
}

/// 1. App boots, keyboard view renders at least one key slot.
fn case_app_boots(e2e: &mut E2e) -> Result<()> {
    println!("[1] app boots, keyboard view renders...");
    e2e.wait("[aria-label=key-0]")?;
    let count = e2e.count("[aria-label=key-*]")?;
    assert!(count >= 1, "expected ≥1 key slot, got {count}");
    println!("    ✅ {count} 个 key slot 已渲染");
    Ok(())
}

/// 2. Settings button opens the settings panel (auto-fill checkbox appears).
fn case_open_settings(e2e: &mut E2e) -> Result<()> {
    println!("[2] open settings panel...");
    e2e.click("button#btn-settings")?;
    e2e.wait("input#auto-fill")?;
    println!("    ✅ 设置面板已打开，auto-fill 控件可见");
    Ok(())
}

/// 3. Toggle auto-fill in the UI and assert the checkbox state flips.
///
/// Note: this asserts the *UI* state changed (AXValue 0↔1). The backend
/// persistence of `set_auto_fill` is covered separately by the command-layer
/// test `crates/host-core/tests/e2e_desktop_service.rs`, so we don't re-prove
/// it here — UI e2e focuses on user-visible behavior.
fn case_toggle_autofill(e2e: &mut E2e) -> Result<()> {
    println!("[3] toggle auto-fill, verify UI state flips...");
    let (before, _) = e2e.value("input#auto-fill")?;
    e2e.click("input#auto-fill")?;
    // Give the change handler + repaint a beat.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let (after, _) = e2e.value("input#auto-fill")?;
    assert_ne!(
        before, after,
        "auto-fill AXValue did not change after click (before={before:?}, after={after:?})"
    );
    println!("    ✅ auto-fill {before:?} → {after:?}（UI 状态翻转确认）");
    Ok(())
}

/// 4. Keyboard key is clickable and doesn't crash the app. Go back to the
///    keyboard view first (case 3 left us in settings).
fn case_key_interaction(e2e: &mut E2e) -> Result<()> {
    println!("[4] key slot interaction (no crash)...");
    e2e.click("button#btn-back")?;
    e2e.wait("[aria-label=key-0]")?;
    e2e.click("[aria-label=key-0]")?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(e2e.alive()?, "app webview unreachable after key click");
    println!("    ✅ 点击 key 后应用仍存活");
    Ok(())
}
