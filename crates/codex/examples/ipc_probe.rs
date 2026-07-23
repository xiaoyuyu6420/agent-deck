//! E2E 探针：起 CodexObserver，验证 app-server RPC + ipc.sock 连通性与降级。
//!
//! 运行：`cargo run -p agent-deck-codex --example ipc_probe`
//!
//! ⚠️ 重要发现（2026-07-23 实测）：ipc 总线上的 `thread-stream-state-changed`
//! **不是** turn 状态（working/waiting）广播，而是对话**内容**增量同步
//!（change.type=patches/snapshot），且只推给已注册的 stream follower。
//! 作为 clientType:"extension" 连入收不到它。turn 状态（active/idle）的
//! IPC 传递路径尚未完全确认（见 docs/codex-integration.md 实时状态节）。
//! 因此本探针验证的是：① app-server RPC（thread/list）连通；② ipc.sock
//! 握手 + following 广播连通；③ GUI 无 active turn 时正确降级返回空。
//! 完整的 working/waiting 实时覆盖验证待 ipc 接入重新设计后补。
use agent_deck_codex::{CodexObserver, CodexObserverOptions};
use agent_deck_protocol::DeckStatus;
use std::time::{Duration, Instant};
use agent_deck_codex::{CodexObserver, CodexObserverOptions};
use agent_deck_protocol::DeckStatus;
use std::time::{Duration, Instant};

fn main() {
    println!("=== codex backend e2e 探针（持续 60 秒）===\n");

    let mut obs = CodexObserver::new(CodexObserverOptions::default());
    match obs.open() {
        Ok(()) => println!("✅ open() 成功（app-server 子进程 + ipc watcher 已启动）"),
        Err(e) => {
            println!("❌ open() 失败: {e}");
            return;
        }
    }
    // ipc watcher 在 open() 时后台启动并连 ipc.sock；给 3 秒握手。
    std::thread::sleep(Duration::from_secs(3));
    // 注意：observer 内部持有 IpcStateWatcher，但未暴露 is_connected()。
    // 通过一个间接信号判断 ipc 是否可能连上：ipc.sock 文件是否存在 + GUI 进程是否在跑。
    let sock_ok = std::path::Path::new(
        &std::env::var("HOME").unwrap_or_default(),
    ).join(".codex/ipc/ipc.sock").exists();
    let gui_running = std::process::Command::new("pgrep")
        .args(["-f", "ChatGPT.app/Contents/MacOS/ChatGPT"])
        .status().map(|s| s.success()).unwrap_or(false);
    println!(
        "ipc watcher 后台运行中 | ipc.sock 存在={sock_ok} | GUI 在跑={gui_running}"
    );
    if !sock_ok || !gui_running {
        println!("⚠️ ipc.sock 或 GUI 不在 → 不会有实时状态，纯静态降级。");
    }
    println!();

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut saw_active = false;
    let mut round = 0u32;
    println!(">>> 现在请在 ChatGPT GUI 给一个 codex 会话发消息触发 turn <<<\n");

    while Instant::now() < deadline {
        round += 1;
        match obs.poll_once() {
            Ok(snaps) => {
                let active: Vec<_> = snaps
                    .iter()
                    .filter(|s| matches!(s.status, DeckStatus::Working | DeckStatus::Waiting | DeckStatus::Error))
                    .collect();
                let ts = Instant::now();
                if active.is_empty() {
                    // 只在偶数轮打印 idle，减少噪音
                    if round % 5 == 1 {
                        println!("[{ts:?}] round {round}: {} snaps, 全 Idle", snaps.len());
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
        println!("✅✅✅ 成功捕获到非 Idle 状态（ipc 实时覆盖生效）！");
    } else {
        println!("⚠️ 60 秒内未捕获到非 Idle 状态。");
        println!("   可能原因：① GUI 在此期间没有 active turn；");
        println!("            ② ipc watcher 未连上（GUI 没开）。");
    }

    // catalog 作为通道连通性兜底证明
    match obs.catalog_once() {
        Ok(snaps) => println!("\n（catalog 通道正常：{} 个历史会话）", snaps.len()),
        Err(e) => println!("\n（catalog 失败: {e}）"),
    }
}
