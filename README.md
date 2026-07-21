# Agent Deck

> 跨工具的 AI agent 状态牌。被动观察 ZCode / Codex 会话状态，状态变化时提醒。

## 定位

**不是**会话入口、**不是**新工作流。**是**一块物理「状态牌」：嵌入现有工作流，长任务跑着的时候你不盯屏，灯告诉你状态。

```
你在 ZCode Desktop / Codex 里跑长任务 → 去干别的
任务进 waiting / done / error → deck 灯变色
按 Accept / Reject / Stop → 回到工作
```

## 当前主路径：Rust / Tauri

```bash
# 安装 Rust（若尚未安装）
# curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

pnpm install

# 跑全部 Rust 单测 + E2E
pnpm test:rust

# 只跑 E2E
pnpm test:rust:e2e

# 启动桌面应用（托盘 + 悬浮窗，观察真实 ~/.zcode）
pnpm dev:desktop

# 打包 macOS/Windows 安装包
pnpm build:desktop
```

## 仓库结构

```
crates/
  protocol/    共享类型（host / UI / 固件唯一真相）
  board/       theme + slotAllocator + SessionBoard
  zcode/       SqliteObserver + mapper
  host-core/   编排 + DesktopService + E2E
apps/
  desktop/     Tauri 2 悬浮窗 + 系统托盘
packages/      旧 TypeScript 实现（legacy oracle，勿作主入口）
docs/          产品与协议文档
```

## 状态模型

| 状态 | 颜色 | fx | 含义 |
|---|---|---|---|
| off | 黑 | solid | 未绑定 |
| idle | 白 dim | solid | 已绑定空闲 |
| working | 蓝 | breathe | 在跑 |
| waiting + low | 浅橙 | solid | 等你（不急） |
| waiting + high | 急红 | blink_fast | 等你（急） |
| done | 绿 | solid | 完成 |
| error | 红 | solid | 错误 |

详见 [docs/status-model.md](./docs/status-model.md) 与 [docs/rust-rewrite.md](./docs/rust-rewrite.md)。

## Legacy TypeScript

旧 `packages/host` + `packages/simulator` 仍可用作对照：

```bash
pnpm --filter @agent-deck/host test
pnpm --filter @agent-deck/host test:e2e
```

日常开发与发布以 **Rust / Tauri** 为准。

## License

MIT (软件) / CERN-OHL-S (硬件，后续)
