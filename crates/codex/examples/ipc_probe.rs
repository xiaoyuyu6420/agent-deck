//! E2E 探针：验证 app-server RPC + ipc.sock follower 注册 + snapshot 接收。
//!
//! 运行：`cargo run -p agent-deck-codex --example ipc_probe`
//!
//! 协议定稿（2026-07-23）：连上后 watcher 会自动对
//! `thread-stream-following-changed` 回 announce `following=true`，owner 随即
//! 推 snapshot。本探针验证：
//! 1. app-server RPC（thread/list）连通
//! 2. ipc 握手 + follower announce 后能收到状态（is_connected / status_of）
//! 3. GUI 有 active turn 时 poll 能看到 Working/Waiting
//!
//! 若 GUI 未开：降级为空，不崩溃。

use agent_deck_codex::{CodexObserver, CodexObserverOptions};
use agent_deck_protocol::DeckStatus;
use std::time::{Duration, Instant};

fn main() {
    println!("=== codex backend e2e 探针（follower 注册 + 持续 60 秒）===\n");

    let mut obs = CodexObserver::new(CodexObserverOptions::default());
    match obs.open() {
        Ok(()) => println!("✅ open() 成功（app-server 子进程 + ipc watcher 已启动）"),
        Err(e) => {
            println!("❌ open() 失败: {e}");
            return;
        }
    }

    // watcher 后台连 ipc + announce；给几秒握手与 snapshot。
    std::thread::sleep(Duration::from_secs(4));

    let sock_ok = std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
        .join(".codex/ipc/ipc.sock")
        .exists();
    // 用 Stdio::null 吞掉 pgrep 默认打印的 PID，避免污染探针输出。
    let gui_running = std::process::Command::new("pgrep")
        .args(["-f", "ChatGPT.app/Contents/MacOS/ChatGPT"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    println!("ipc.sock 存在={sock_ok} | GUI 在跑={gui_running}");
    if !sock_ok || !gui_running {
        println!("⚠️ GUI/socket 不可用 → 仅验证 RPC 降级，不会有实时状态。");
    }
    println!();

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut saw_active = false;
    let mut saw_any_snap = false;
    let mut round = 0u32;
    println!(">>> 在 ChatGPT GUI 给会话发消息可触发 Working；idle snapshot 也会被 poll 映射 <<<\n");

    while Instant::now() < deadline {
        round += 1;
        match obs.poll_once() {
            Ok(snaps) => {
                if !snaps.is_empty() {
                    saw_any_snap = true;
                }
                let active: Vec<_> = snaps
                    .iter()
                    .filter(|s| {
                        matches!(
                            s.status,
                            DeckStatus::Working | DeckStatus::Waiting | DeckStatus::Error
                        )
                    })
                    .collect();
                let ts = Instant::now();
                if active.is_empty() {
                    if round % 5 == 1 {
                        println!(
                            "[{ts:?}] round {round}: {} snaps（无 Working/Waiting）",
                            snaps.len()
                        );
                    }
                } else {
                    saw_active = true;
                    println!("[{ts:?}] round {round}: ✅ {} 个非 Idle！", active.len());
                    for s in &active {
                        let title: String = s.title.chars().take(40).collect();
                        println!(
                            "    {:?} id={} status={:?} title={}",
                            s.backend,
                            &s.session_id[..s.session_id.len().min(24)],
                            s.status,
                            title
                        );
                    }
                }
            }
            Err(e) => println!("round {round}: ❌ poll_once 失败: {e}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    println!("\n=== 探针结束（{round} 轮）===");
    if saw_active {
        println!("✅ 捕获到 Working/Waiting（ipc follower 实时覆盖生效）");
    } else if saw_any_snap {
        println!("ℹ️ 有会话快照但无 Working/Waiting（GUI 可能无 active turn，属正常）");
    } else {
        println!("⚠️ 60 秒内 poll 无非 Idle 会话。");
        println!("   可能：① GUI 无 active turn；② GUI 没开；③ announce 未完成。");
    }

    match obs.catalog_once() {
        Ok(snaps) => println!("\n（catalog 通道正常：{} 个历史会话）", snaps.len()),
        Err(e) => println!("\n（catalog 失败: {e}）"),
    }
}
