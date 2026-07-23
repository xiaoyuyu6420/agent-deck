# 架构总览

## 三层

```
┌─────────────────────────────────────────────────────────────┐
│  外设层（RP2040 / simulator）                                 │
│  - 扫键、读编码器/摇杆                                        │
│  - 刷 5 颗状态 RGB                                            │
│  - USB CDC JSON Lines 或 WebSocket                            │
│  - 无业务逻辑                                                 │
└────────────────────────┬────────────────────────────────────┘
                         │ USB / WS
┌────────────────────────▼────────────────────────────────────┐
│  agent-deck-host（Mac 上 pnpm dev / launchd 常驻）           │
│  ┌────────────────────────────────────────────────────────┐ │
│  │  Observer 层（只读、被动）                              │ │
│  │  - ZCodeObserver: 双 sqlite + 文件 mtime                │ │
│  │  - CodexObserver: app-server thread/status/changed      │ │
│  │  - WorkBuddyObserver: ~/.workbuddy jsonl 事件流扫描     │ │
│  │  → 统一 SessionSnapshot 事件                            │ │
│  ├────────────────────────────────────────────────────────┤ │
│  │  Board 层                                              │ │
│  │  - SessionBoard: 5 槽位，按优先级抢位                   │ │
│  │  - slotAllocator: 纯函数抢位算法                        │ │
│  │  - theme: 状态→RGB + urgency 渐变（纯函数）             │ │
│  │  → 出 LedFrame + BoardState                            │ │
│  ├────────────────────────────────────────────────────────┤ │
│  │  Action 层（仅用户触发时介入）                          │ │
│  │  - ActionRouter: accept/reject/stop → 调 adapter        │ │
│  │  - SimulatorBridge: WS 消息 ↔ Action                    │ │
│  ├────────────────────────────────────────────────────────┤ │
│  │  Adapter 层（隔离各家协议）                             │ │
│  │  - ZcodeAdapter (sqlite 观察 + V1.1 ACP 动作)           │ │
│  │  - CodexAdapter (app-server 观察+动作)                  │ │
│  ├────────────────────────────────────────────────────────┤ │
│  │  Gateway: WS + HTTP 127.0.0.1:8787                      │ │
│  └────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

## 模块依赖图

```
main.ts
  ├── config.ts (load)
  ├── bus.ts (singleton)
  ├── board/SessionBoard.ts
  │     ├── board/theme.ts (paint 纯函数)
  │     ├── board/slotAllocator.ts (allocateSlots 纯函数)
  │     └── backends/types.ts (AgentBackend 接口)
  ├── backends/zcode/ZcodeAdapter.ts
  │     └── backends/zcode/SqliteObserver.ts
  │           └── backends/zcode/mapper.ts (mapZcodeRow 纯函数)
  ├── backends/codex/CodexAdapter.ts (V1 stub)
  ├── actions/ActionRouter.ts
  ├── gateway/server.ts (WS + HTTP)
  └── device/SimulatorBridge.ts
```

## 纯函数 vs 副作用

**纯函数（必须单测）**：
- `theme.paint(input): output` — 状态→RGB
- `slotAllocator.allocateSlots(sessions, opts): slots` — 抢位
- `zcode/mapper.mapZcodeRow(row): snapshot` — 数据映射
- `zcode/mapper.inferRisk(detail): risk` — 风险推断

**副作用层（不测，靠端到端验证）**：
- `SqliteObserver` — DB 读写、文件监听
- `Gateway` — 网络
- `SimulatorBridge` — 消息分发
- `SessionBoard` — 状态总表（半纯半副作用）

## 数据流：ZCode 待确认 → 灯变色

```
1. ZCode Desktop 写 tool_usage (approval_status='requested')
2. -wal 文件 mtime 变化
3. ZcodeSqliteObserver.fs.watch 触发
4. pollOnce() 执行 SQL
5. mapZcodeRow(row) → SessionSnapshot { status: 'waiting', risk: 'high', detail: 'Bash: ...' }
6. ZcodeAdapter 收到快照，广播给 subscribers
7. SessionBoard 收到，存入 sessions Map
8. SessionBoard.recompute()
9.   - 算 urgency
10.  - allocateSlots(scoredSessions) → 5 槽位
11.  - 对每槽 paint(snapshot) → rgb/br/fx
12.  - 组装 LedFrame + BoardState
13.  - 广播给 onLedFrame / onBoardState 订阅者
14. SimulatorBridge 收到，转给 Gateway.broadcast
15. WS 推给所有 client（simulator TUI / 真硬件）
16. simulator 收到 leds 帧 → React setState → 重渲染圆点颜色
```

整条链路从 ZCode 写 DB 到灯变色：< 500ms（节流 + WS 延迟）。

## 数据流：用户按 Accept

```
1. simulator: useInput 收到 'a' 键
2. wsClient.send({t:'action', action:{op:'accept'}})
3. host gateway 收到 WS 消息
4. SimulatorBridge.handleClientMessage(msg)
5. ActionRouter.dispatch({op:'accept'})
6.   - 查 board.focus 槽的 sessionId/backend
7.   - backend.accept(sessionId)
8. ZcodeAdapter.accept (V1.1)：spawn zcode.cjs app-server → task/response
9. ZCode Desktop 收到响应，approval 关闭
10. tool_usage.approval_status 更新
11. → 触发新一轮 SqliteObserver → status 变 working
12. → 灯变蓝
```

V1 backend.accept 未实现，第 7 步返回 unsupported，ActionRouter emit `action.failed`。用户在 simulator 看到无效果但进程不崩。

## 关键不变量

- `crates/protocol/src/lib.rs` 是三方唯一真相
- DB **只读**，绝不写回 ~/.zcode / ~/.codex
- Gateway 只绑 127.0.0.1，绝不暴露公网
- backend 可独立失败（zcode 挂不影响 codex 灯）
- 所有 handler 抛错必须捕获，不让进程崩
