//! E2E 探针：验证 CodexObserver::dispatch_once(Stop) 对真实 codex 的通道。
//!
//! 运行：`cargo run -p agent-deck-codex --example dispatch_probe`
//!
//! 验证点与裁决（诚实区分"通道可达"与"端到端走通"）：
//! 1. open() 成功（app-server 子进程）
//! 2. poll_once() 拿到 thread 列表
//! 3. dispatch_once(Stop) 对第一个 thread 走 resume→interrupt
//!
//! 退出码语义（本探针只保证"通道不报错"，不保证 interrupt 真打断一个 turn）：
//! - exit 0：open/poll/dispatch 三段全部 Ok，**且** poll 拿到 ≥1 thread（dispatch
//!   路径确被执行过）。注意：observer 吞掉 `turn/interrupt` 的返回值
//!   （observer.rs stop_thread），所以本探针无法判定 interrupt 是否真打到
//!   一个 running turn —— "真机生效"仍属 action-spec §4.2 的开放点。
//! - exit 2：open/poll 通道健康，但 poll 返回 0 thread（无 GUI active turn 时
//!   app-server 内存隔离、notLoaded thread 被 recency 过滤）。此情形下
//!   dispatch 路径**未被执行**，不得解读为"dispatch 通道已验证"。
//! - exit 1：任一段返回 Err（open/poll/dispatch 失败）。
//!
//! 覆盖边界：本探针只测 `Action::Stop { i: Some(0) }`（slot=index 0 分支），
//! 不覆盖 `i: None` → 焦点 slot 分支（observer.target_thread_id 已处理）。
//! 不测 Accept/Reject（阻塞于 requestId 捕获，见 action-spec §4.2）。
//!
//! 不需要 GUI 在跑（app-server 能 resume 任意 notLoaded thread）。

use std::process::ExitCode;

use agent_deck_codex::{CodexObserver, CodexObserverOptions};
use agent_deck_protocol::Action;

fn main() -> ExitCode {
    println!("=== Codex dispatch(Stop) 通道探针 ===\n");

    let mut obs = CodexObserver::new(CodexObserverOptions::default());
    match obs.open() {
        Ok(()) => println!("✅ open() 成功"),
        Err(e) => {
            println!("❌ open() 失败: {e}");
            return ExitCode::from(1);
        }
    }

    let snaps = match obs.poll_once() {
        Ok(snaps) => {
            println!("✅ poll_once() 拿到 {} 个 thread", snaps.len());
            snaps
        }
        Err(e) => {
            println!("❌ poll_once() 失败: {e}");
            return ExitCode::from(1);
        }
    };

    let Some(first) = snaps.first() else {
        println!("⚠️ 0 thread → open/poll 通道健康，但 dispatch 路径未执行");
        println!("   如需端到端验证：在 ChatGPT GUI 给某会话发消息触发 active turn 后重跑。");
        return ExitCode::from(2);
    };

    let id_short: String = first.session_id.chars().take(12).collect();
    println!("   首个: {id_short} ({:?})", first.status);

    // 仅覆盖 slot=index 0 分支；None/焦点 slot 分支不在此探针覆盖范围。
    let action = Action::Stop { i: Some(0) };
    println!("\n>>> dispatch_once(Stop) on slot 0 ...");
    match obs.dispatch_once(&action) {
        Ok(status) => {
            println!("✅ 返回: {status}");
            println!("\nPASS：open/poll/dispatch 三段通道均 Ok 且 dispatch 路径已执行。");
            println!("（interrupt 是否真打断 turn 不可观测，仍属 spec §4.2 开放点）");
            ExitCode::from(0)
        }
        Err(e) => {
            println!("❌ dispatch_once 错误: {e}");
            ExitCode::from(1)
        }
    }
}
