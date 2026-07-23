//! E2E 探针：验证 CodexObserver::dispatch_once(Stop) 对真实 codex 的通道。
//!
//! 运行：`cargo run -p agent-deck-codex --example dispatch_probe`
//!
//! 验证点：
//! 1. open() 成功（app-server 子进程）
//! 2. poll_once() 拿到 thread 列表
//! 3. dispatch_once(Stop) 对第一个 thread 走通 resume→interrupt
//!    - 若该 thread 无 active turn → 返回 ok:stop:...（已停止，无需中断）
//!    - 若 resume 失败 → 返回 error
//!
//! 不需要 GUI 在跑（app-server 能 resume 任意 notLoaded thread）。

use agent_deck_codex::{CodexObserver, CodexObserverOptions};
use agent_deck_protocol::Action;

fn main() {
    println!("=== Codex dispatch(Stop) 通道探针 ===\n");

    let mut obs = CodexObserver::new(CodexObserverOptions::default());
    match obs.open() {
        Ok(()) => println!("✅ open() 成功"),
        Err(e) => {
            println!("❌ open() 失败: {e}");
            return;
        }
    }

    match obs.poll_once() {
        Ok(snaps) => {
            println!("✅ poll_once() 拿到 {} 个 thread", snaps.len());
            if let Some(first) = snaps.first() {
                let id_short: String = first.session_id.chars().take(12).collect();
                println!("   首个: {id_short} ({:?})", first.status);
                let action = Action::Stop { i: Some(0) };
                println!("\n>>> dispatch_once(Stop) on slot 0 ...");
                match obs.dispatch_once(&action) {
                    Ok(status) => println!("✅ 返回: {status}"),
                    Err(e) => println!("❌ 错误: {e}"),
                }
            } else {
                println!("⚠️ 无 thread，无法测试 dispatch");
            }
        }
        Err(e) => println!("❌ poll_once() 失败: {e}"),
    }
}
