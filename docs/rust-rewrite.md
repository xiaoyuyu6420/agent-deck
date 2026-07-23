# Rust / Tauri 重写说明

## 状态

主路径已切到 **Rust + Tauri 2**，旧 TypeScript 实现（`packages/`）已删除，Rust 为唯一实现。

## 结构

```
crates/
  protocol/    共享类型（DeckStatus / LedFrame / BoardState / Action）
  board/       theme + slotAllocator + SessionBoard
  zcode/       mapper + SqliteObserver（双 sqlite 只读 + 30min 防陈旧）
  codex/       CodexObserver（app-server JSON-RPC）
  host-core/   HostCore 编排 + DesktopService（Tauri IPC 业务层）+ E2E
apps/
  desktop/     Tauri 2 悬浮窗 + 系统托盘
```

## 命令

```bash
# 全部 Rust 单测 + E2E（默认 pnpm test）
pnpm test

# 仅 host-core 集成/E2E
pnpm test:e2e

# 桌面开发（托盘 + 悬浮窗，观察真实 ~/.zcode）
pnpm dev

# 打包
pnpm build:desktop
```

## E2E 覆盖清单

### e2e_zcode_flow（sqlite → board 全链路）— 7
1. working → 蓝 breathe
2. waiting(requested) → 暖色
3. working → waiting → accept(写回 db) → working → done
4. error → 红 solid
5. excludeWorkspaces 自指防护
6. 30min 陈旧 requested 不算 waiting
7. leds 帧 5 槽形状

### e2e_board_priority（抢位 / urgency / 生命周期）— 5
1. waiting > working > done 抢前槽
2. low-risk waiting：solid → 3min 后 blink_fast 偏红
3. >5 会话只占 5 槽
4. 任务删除后从 board 消失
5. focus 跨 tick 保持

### e2e_desktop_service（Tauri IPC 命令层）— 4
1. 空 DB → demo 回退
2. set_focus 更新 focused 槽
3. accept/reject/stop V1 返回 unsupported
4. 真实 sqlite 任务关闭 demo 并上色

**合计：16 个 host-core 集成测试 + 17 个 crate 单测 = 33 个 Rust 测试全绿。**

## 架构边界

| 层 | 技术 | 职责 |
|---|---|---|
| UI | TS + HTML/CSS (Tauri WebView) | 只展示与按钮 |
| 壳 | Tauri Rust | 托盘、窗口、轮询推送 |
| 业务 | host-core / board / zcode | 观察、抢位、灯效、动作 stub |

Accept/Reject 真动作、Codex、USB 硬件见后续 Phase。
