# 架构总览

> 本文档描述当前 Rust + Tauri 实现（legacy TypeScript host 已于 2026-07 删除，见 `rust-rewrite.md`）。

## 三层

```
┌─────────────────────────────────────────────────────────────┐
│  外设层                                                      │
│  - RP2040 固件（USB CDC JSON Lines）或软件键盘（Tauri webview）│
│  - 扫键、读编码器/摇杆、刷状态 RGB                            │
│  - 无业务逻辑，只渲染 LedFrame                               │
└────────────────────────┬────────────────────────────────────┘
                         │ board-updated 事件（200ms 轮询推送）
┌────────────────────────▼─────────────────────────────────────┐
│  agent-deck-desktop（Tauri，Rust 后端 + TS 前端）            │
│                                                              │
│  ┌── host-core ───────────────────────────────────────────┐ │
│  │  Observer 层（只读、被动，各 backend 独立 crate）        │ │
│  │  - zcode:     双 sqlite 只读跨库 join                   │ │
│  │  - codex:     app-server RPC(线程列表) + ipc.sock(实时状态)│ │
│  │  - workbuddy: jsonl 文件扫描                            │ │
│  │  → 统一 SessionSnapshot，实现 BackendObserver trait      │ │
│  ├──────────────────────────────────────────────────────────┤ │
│  │  Board 层（crates/board，纯函数 + 本地状态）             │ │
│  │  - SessionBoard: slot_count 槽位，pin survives recompute │ │
│  │  - slot_allocator: 按优先级抢位（Waiting>Working>Done）  │ │
│  │  - theme: 状态→RGB + urgency 渐变（纯函数）             │ │
│  │  → BoardState + LedFrame                                │ │
│  ├──────────────────────────────────────────────────────────┤ │
│  │  DesktopService: tick(200ms) 编排，dispatch_action(stub) │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                              │
│  Tauri 命令层: list_sessions / pin_slot / open_slot_session  │
│  前端(main.ts): 虚拟键盘 UI + bind picker + 设置面板          │
└─────────────────────────────────────────────────────────────┘
```

## Crate 依赖图

```
apps/desktop/src-tauri  (Tauri 二进制，前端 + 后端)
  ├── host-core         (编排：HostCore/Board/Service/Observer 注册)
  │     ├── board       (SessionBoard/slot_allocator/theme 纯逻辑)
  │     ├── protocol    (SessionSnapshot/DeckStatus/BackendId 唯一真相)
  │     ├── zcode       (SqliteObserver + mapper)
  │     ├── codex       (CodexObserver + rpc + ipc)
  │     └── workbuddy   (JsonlObserver + mapper)
  └── protocol
```

`protocol` 是三方唯一真相：所有 crate 都依赖它，它不依赖任何 crate。`BackendId` 枚举（`protocol/src/lib.rs`）是 backend 标识的唯一来源（小写序列化）。

## 各层职责

### Observer 层（各 backend crate，只读）
每个 backend 实现一个独立 crate，导出 `XxxObserver` + mapper。它们实现 host-core 的 `BackendObserver` trait（见下）。核心契约：
- **只读**：绝不写回 `~/.zcode` / `~/.codex` / `~/.workbuddy`。
- **降级契约**：数据源缺失时 `open()`/`poll()` 返回 `Ok(空)`，绝不上抛错误、绝不 panic。某 backend 挂了不影响其它 backend。
- **三档查询**：`poll`（活跃子集给 board）/ `list_catalog`（全集给 bind picker）/ `poll_pinned`（按 id 刷新钉住的会话）。

详见 [backend-adapters.md](./backend-adapters.md) 的适配规范。

### Board 层（`crates/board`，纯函数 + 本地状态）
- `SessionBoard`：维护所有 backend 的 SessionSnapshot 总表，按 `{backend}:{session_id}` 做 key 隔离。`replace_backend_sessions` 清某 backend 缓存时，pinned 的会话会被保护（survives recompute）。
- `slot_allocator`：纯函数抢位算法。优先级 `Waiting(5) > Error(4) > Working(3) > Done(2) > Idle(1) > Off(0)`，同优先级按 urgency → updated_at。
- `theme`：状态→RGB + urgency 渐变，纯函数，见 `status-model.md`。

### 编排层（`host-core`）
- `HostCore`：持有所有 observer + board，`tick_at(now)` 是核心循环——遍历 observer poll → board.replace → 对 pinned ids 调 poll_pinned 刷新 → recompute。
- `DesktopService`：在 HostCore 之上加持久化（pins.json）、focus、list_sessions 聚合、`dispatch_action`（当前全局 stub，返回 `unsupported:{op}`，裁决动作规格见 `action-spec.md`）。

### Tauri 层（`apps/desktop`）
- Rust 侧（`src-tauri/src/lib.rs`）：暴露 Tauri 命令（list_sessions/pin_slot/open_slot_session/set_focus），200ms 后台轮询线程调 `service.tick()` 推 `board-updated` 事件。
- 前端（`src/main.ts`）：虚拟键盘 UI、bind picker（backend→project→session 三步选择器）、设置面板，监听 `board-updated` 重绘。

## 数据流：某 backend 出现待审批 → 灯变色

```
1. codex GUI 里一个 thread 进入 waitingOnApproval
2. GUI app-server 通过 ipc.sock 广播 thread-stream-state-changed
3. codex/ipc.rs IpcStateWatcher 后台线程收到，更新 thread 状态表
4. host-core tick(200ms) → CodexObserver.poll_once()
5. poll 拉 thread/list(列表) + 查 ipc 状态表覆盖(实时状态)
   → SessionSnapshot { status: Waiting }
6. board.replace_backend_sessions(Codex, snaps)
7. board.recompute() → slot_allocator 抢位（Waiting 优先级最高）
8. → BoardState + LedFrame（Waiting 槽点亮对应颜色）
9. Tauri emit("board-updated") → 前端重绘
```

（zcode 的链路类似，但第 1-3 步是"ZCode 写 tool_usage 表 → observer 轮询 sqlite 读到"；workbuddy 是"jsonl 追加事件 → observer 轮询扫描"。）

## 数据流：用户点按键跳转

```
1. 前端点按键 → invoke('open_slot_session', { i })
2. Rust open_slot_session → set_focus + 取 slot 的 backend/session_id
3. 按 backend 分支：
   - codex: 检测 GUI 是否在跑 → 没跑先 open -a ChatGPT 等就绪
            → open "codex://threads/<thread.id>" → GUI 聚焦+定位会话
   - zcode: open "zcode://workspace/open?path=<项目>" → 落到项目(非会话)
   - workbuddy: 检测 GUI 是否在跑 → 没跑先 open -a WorkBuddy 等就绪
               → open "workbuddy://chat/<sessionId>" → GUI 聚焦+定位 task
```

## 关键不变量

- `crates/protocol/src/lib.rs` 是三方唯一真相（SessionSnapshot/DeckStatus/BackendId）。
- 各 backend 数据源**只读**，绝不写回官方 DB/文件/socket。
- 200ms 轮询是 host-core 唯一的驱动时钟（非事件驱动）；codex 的 ipc watcher 是唯一的事件驱动例外（后台线程收广播，但结果仍经 poll 取用）。
- backend 可独立失败（zcode 挂不影响 codex 灯）。
- 所有 handler 抛错必须捕获，不让进程崩。

## 后端接入

新增一个 backend 要触碰 protocol/board/host-core/desktop 四层，标识需在多处手工同步。完整 checklist 与模板见 [backend-adapters.md](./backend-adapters.md)。
