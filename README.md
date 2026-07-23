# Agent Deck

> macOS 软件 Agent 操作台。观察 ZCode / Codex 会话状态，关键时刻不离开主键盘就能跳转、绑定、裁决。

## 定位

**不是**新会话入口（只跳转聚焦已有会话）、**不是**通用宏工具箱。**是**一个软件操作台：嵌入现有工作流，长任务跑着的时候你不盯屏，状态牌提醒你，需要时一键裁决。硬件状态牌是其可选 client 之一。详见 [docs/product.md](./docs/product.md)。

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
  codex/       CodexObserver（app-server JSON-RPC）
  host-core/   编排 + DesktopService + E2E
apps/
  desktop/     Tauri 2 悬浮窗 + 系统托盘
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

详见 [docs/status-model.md](./docs/status-model.md) 与 [docs/rust-rewrite.md](./docs/rust-rewrite.md)。术语定义见 [CONTEXT.md](./CONTEXT.md)。

## Roadmap

设想与分级见 [docs/roadmap.md](./docs/roadmap.md)。

原则：**做要克制（比 Codex 强一点就够），想都记下**。近期 🟢：裁决动作（Phase 0 探测 + Phase 1 Codex 实现，见 [docs/action-spec.md](./docs/action-spec.md)）、theme/风险规则配置化、CI/CD 与本地长任务 adapter。

## License

MIT (软件) / CERN-OHL-S (硬件，后续)
