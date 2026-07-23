//! E2E 探针：起 JsonlObserver，验证能读 WorkBuddy 的会话列表 + 状态映射。
//!
//! 运行：`cargo run -p agent-deck-workbuddy --example probe`
//!
//! 前提：WorkBuddy.app 在本机用过（产生 `~/.workbuddy/projects/*.jsonl`）。
use agent_deck_protocol::DeckStatus;
use agent_deck_workbuddy::{JsonlObserver, JsonlObserverOptions};
use std::path::PathBuf;

fn main() {
    println!("=== WorkBuddy backend e2e 探针 ===\n");

    let projects_dir = agent_deck_protocol::home_dir().join(".workbuddy/projects");
    println!("数据源: {}", projects_dir.display());
    println!(
        "目录存在: {}",
        if projects_dir.is_dir() { "✅" } else { "❌" } 
    );
    // 统计 jsonl 文件数（佐证数据源非空）
    let file_count = std::fs::read_dir(&projects_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .flat_map(|e| std::fs::read_dir(e.path()).ok().into_iter().flatten())
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
                .count()
        })
        .unwrap_or(0);
    println!("jsonl 会话文件数: {file_count}\n");

    let mut obs = JsonlObserver::new(JsonlObserverOptions::default());
    match obs.open() {
        Ok(()) => println!("✅ open() 成功"),
        Err(e) => {
            println!("❌ open() 失败: {e}");
            return;
        }
    }

    // poll_once：给 board 用的活跃子集
    println!("\n=== poll_once 结果（board 用，过滤纯 idle）===");
    match obs.poll_once() {
        Ok(snaps) => {
            println!("返回 {} 个 SessionSnapshot", snaps.len());
            print_snaps(&snaps);
            summarize(&snaps);
        }
        Err(e) => println!("❌ poll_once 失败: {e}"),
    }

    // catalog_once：给 bind picker 用的全集
    println!("\n=== catalog_once 结果（bind picker 用，含历史）===");
    match obs.catalog_once() {
        Ok(snaps) => {
            println!("返回 {} 个 SessionSnapshot", snaps.len());
            print_snaps(&snaps);
            summarize(&snaps);
        }
        Err(e) => println!("❌ catalog_once 失败: {e}"),
    }

    println!("\n=== 探针结束 ===");
}

fn print_snaps(snaps: &[agent_deck_protocol::SessionSnapshot]) {
    use agent_deck_protocol::{BackendId, ProjectCategory};
    for s in snaps.iter().take(20) {
        let title: String = s.title.chars().take(42).collect();
        let ws = s
            .workspace_path
            .as_ref()
            .and_then(|p| p.rsplit('/').next().map(String::from))
            .unwrap_or_else(|| "?".into());
        let waiting = if matches!(s.status, DeckStatus::Waiting) {
            format!(" waiting_since={:?}", s.waiting_since)
        } else {
            String::new()
        };
        let cat = match s.project_category {
            Some(ProjectCategory::Task) => "任务",
            Some(ProjectCategory::Automation) => "自动化",
            Some(ProjectCategory::Project) => "项目",
            None => "-",
        };
        let label = s.project_label.as_deref().unwrap_or("");
        println!(
            "  [{:?}] cat={:<4} label={:<24} ws={:<20} id={} status={:?}{} title={}",
            s.backend,
            cat,
            label,
            ws,
            &s.session_id[..s.session_id.len().min(12)],
            s.status,
            waiting,
            title
        );
        let _ = BackendId::Workbuddy; // 确认导出存在
    }
}

fn summarize(snaps: &[agent_deck_protocol::SessionSnapshot]) {
    use agent_deck_protocol::ProjectCategory;
    let mut counts = std::collections::HashMap::new();
    let mut cats = std::collections::HashMap::new();
    for s in snaps {
        *counts.entry(s.status).or_insert(0u32) += 1;
        let key = match s.project_category {
            Some(ProjectCategory::Task) => "任务",
            Some(ProjectCategory::Automation) => "自动化",
            Some(ProjectCategory::Project) => "项目",
            None => "未分类",
        };
        *cats.entry(key).or_insert(0u32) += 1;
    }
    println!("状态分布: {:?}", counts);
    println!("分类分布: {:?}", cats);
    let has_live = snaps
        .iter()
        .any(|s| matches!(s.status, DeckStatus::Working | DeckStatus::Waiting | DeckStatus::Error));
    if has_live {
        println!("✅ 检测到活跃会话（Working/Waiting/Error）");
    } else {
        println!("（无活跃会话——可能 WorkBuddy 当前没有在跑的任务，全是历史 idle）");
    }
}
