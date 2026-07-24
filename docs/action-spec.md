# Action Spec — 裁决与控制动作

> 本文件定义 Agent Deck 的动作层（Action）：用户对 Session / Board 发起的裁决与控制操作的语义、实现路径与分期计划。
>
> **状态：规格阶段（2026-07-23）。本文件不含代码实现，仅规定"该做什么、怎么做"。实现见下文 Phase 划分。**
>
> **更新（2026-07-23）**：Codex 的**会话级跳转**（点按键 → ChatGPT.app 聚焦并定位到目标 thread）已实测落地，走 `codex://threads/<threadId>` deep link，见 [codex-integration.md](./codex-integration.md) 的"会话级跳转"节。注意：跳转≠裁决，下文 Phase 1 的 Accept/Reject/Stop 裁决动作仍未实现。

## 1. 动作层是什么、不是什么

Agent Deck 的动作层是**有限的裁决动作集合**，兑现产品承诺"不离开主键盘就能裁决"。它**不是通用宏系统**——用户不能自定义任意动作序列，只能从协议枚举的 10 种 Action 里选择。

- ✅ 合法：Accept / Reject 一个等待审批的工具调用、Stop 一个失控回合、Pin 一个会话到槽、切 Plan/Act 模式
- ❌ 不做：任意 shell 宏、自定义多步编排、"接受+跑测试+发消息"组合宏（见 roadmap E 节）

## 2. Action 枚举语义

协议类型定义在 `crates/protocol/src/lib.rs`，`#[serde(tag = "op")]`，共 10 种：

| Action | 输入 | 语义 | 当前状态 |
|---|---|---|---|
| `Focus { i }` | slot index | 把 Board 焦点设到 slot `i`（仅本地状态，不触达 Backend） | ✅ 已实现（`set_focus`，本地） |
| `Pin { i, session_id }` | slot + 可选 session | 把 session 钉到 slot `i`；`None` 解钉。survives recompute，Done TTL 不清 | ✅ 已实现（`pin_slot`，本地 + 持久化） |
| `Accept { i }` | 可选 slot | 批准 slot `i`（或焦点 slot）对应的 waiting Session 的当前审批请求 | ⏳ 阻塞于 requestId 捕获（见 §4.2） |
| `Reject { i }` | 可选 slot | 拒绝对应 Session 的当前审批请求 | ⏳ 阻塞于 requestId 捕获（见 §4.2） |
| `Stop { i }` | 可选 slot | 中断对应 Session 的当前回合 | 🟡 通道已接入（resume→interrupt），真机生效待验证 |
| `StopAll` | — | 中断所有 running Session 的当前回合 | 🟡 通道已接入（广播 Stop） |
| `FreezeAll` | — | 冻结所有 Slot（暂停自动抢位重排，保留当前绑定） | ⏳ unsupported |
| `Unfreeze` | — | 解除冻结，恢复自动抢位 | ⏳ unsupported |
| `SetMode { mode }` | PolicyMode(Plan/Act/Review) | 把某 Backend 的策略模式设为 mode | ⏳ unsupported |
| `Send { i, text }` | 可选 slot + 文本 | 向对应 Session 发送一条用户消息 | ⏳ unsupported |

> **注**：`Focus` 与 `Pin` 是纯本地动作（改 Board 状态/持久化），已实现且不经 Dispatch 通道。其余 8 种需触达 Backend，走下文 Dispatch 机制。

## 3. 架构：扩展 BackendObserver trait

读写共用同一 Observer 连接，**不引入独立的 ActionRouter**。给 `crates/host-core/src/lib.rs` 的 `BackendObserver` trait 增加一个方法：

```rust
pub trait BackendObserver: Send {
    fn id(&self) -> BackendId;
    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>>;
    fn list_catalog(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> { /* 默认 poll */ }
    fn poll_pinned(&mut self, ids: &[String]) -> anyhow::Result<Vec<SessionSnapshot>> { /* 默认过滤 poll */ }

    /// Dispatch a write Action to this backend. Returns Ok with a human-readable
    /// status string, or Err if the backend cannot perform it.
    ///
    /// Default: the backend cannot perform any write action (returns
    /// "unsupported:{op}"). Backends override to implement the actions they support.
    fn dispatch(&mut self, action: &Action) -> anyhow::Result<String> {
        let op = action.op_tag(); // "accept" / "reject" / ...
        Ok(format!("unsupported:{op}"))
    }
}
```

`HostCore::dispatch_action(action: &Action)`（当前 stub，见 `lib.rs:473`）改为：解析 `action` 要作用的目标 slot → 取该 slot 绑定的 `Backend` → 选对应 Observer → 调 `observer.dispatch(action)`。无 slot 归属或 Backend 不支持的，返回 `unsupported`。

**为什么不分 ActionRouter**：裁决动作的生命周期与观察共享同一连接（Codex 的 stdio 子进程、ZCode 的 ACP attach），单开 Router 要再管一套连接，徒增复杂度。读写共用对象符合 roadmap "代码挪一下而非新模块" 的克制原则。详见 [ADR 0001](./adr/0001-software-console-as-primary-axis.md)。

## 4. Codex 实现路径（裁决：Phase 1；跳转：已落地）

Codex 的动作通道**已就绪**：`CodexObserver` 持有 `JsonRpcClient`，连的是 `codex app-server --listen stdio://` 子进程（见 `crates/codex/src/rpc.rs:97`，已对 codex-cli 0.145.0-alpha.27 验证）。

**会话级跳转（已落地，非本节范围）**：点按键 → `codex://threads/<threadId>` deep link → ChatGPT.app 聚焦并定位。这是 codex 相对 zcode 的核心优势（zcode 只能到 workspace）。详见 [codex-integration.md](./codex-integration.md) 的"会话级跳转"节。

**裁决动作（本节，Phase 1 未实现）**：在同一个 client 上发对应 method。

### 4.1 方法映射

| Action | Codex JSON-RPC method | 参数 |
|---|---|---|
| `Accept` | `serverRequest/resolved` | `{ requestId, status: "accepted" }` |
| `Reject` | `serverRequest/resolved` | `{ requestId, status: "rejected" }` |
| `Stop` / `StopAll` | `turn/interrupt` | `{ threadId }` |

### 4.2 requestId / threadId 从哪来

当前 `thread/list` 返回的 `ThreadListResult` 只含状态，不含挂起的 `requestId`。实现 Accept/Reject 需要让 observer 在 poll 时顺带携带最近一个未决 `ServerRequest` 的 `requestId`，存进 `SessionSnapshot.detail` 或新增字段，供 dispatch 时取用。这是 Phase 1 的具体设计点。

### 4.3 不支持的 Codex 动作

`FreezeAll / Unfreeze / SetMode` 中：`SetMode`（reasoning effort）Codex 有 `settings / model_reasoning_effort` 可对接（见 codex-integration.md），Phase 2 再评估；`FreezeAll/Unfreeze` 是纯 Board 本地状态（不触达 Backend），应在 HostCore 层实现而非 Observer。

## 5. ZCode 实现路径（Phase 0 探测，未拍死）

ZCode 侧裁决**押在一个未验证假设上**：能否用独立 spawn 的 `zcode.cjs app-server` 通过 `session/load({ sessionId })` attach 到 ZCode Desktop 已有的 session。**探测结论驱动后续——不提前拍死实现方式。**

### 5.1 Phase 0 探测实验设计

**目标**：判断 `zcode.cjs app-server` 能否 attach 到 Desktop 持有的 session。

**步骤**：
1. `node /Applications/ZCode.app/Contents/Resources/glm/zcode.cjs app-server` 起一个独立 ACP stdio server
2. 用 ZCode Desktop 界面开一个会话，记下它的 sessionId（可从 `tasks-index.sqlite` 查到）
3. 向独立 server 发 `session/load({ sessionId })`
4. 观察该 session 是否出现"已被加载"冲突、是否双写 `tool_usage`、状态是否实时反映

**成功判据**（全满足才算成功）：
- ✅ `session/load` 不报错，能读到该 session 的实时状态
- ✅ 对该 session 发 `task/response`（批准/拒绝）后，**Desktop 侧的会话确实收到裁决**（不是空操作）
- ✅ Desktop 会话不被干扰/中断（无双 owner 冲突）

**失败后果**：若任一不满足，**ZCode 侧裁决永久标 unsupported**，ZCode Backend 只保留观察 + 跳转（`open_slot_session`）。这是可接受的降级——ZCode 用户仍可点 deck 跳进 Desktop 窗口手动裁决。

### 5.2 成功时的实现映射（探测通过后）

| Action | ZCode ACP method |
|---|---|
| `Accept` | `task/response`（批准） |
| `Reject` | `task/response`（拒绝） |
| `Stop` | `session/stop` |

入口：`/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs`，`node zcode.cjs app-server` 起 ACP stdio server。

## 6. Phase 划分

| Phase | 内容 | 依赖 | 状态 |
|---|---|---|---|
| **Phase 0** | ZCode `session/load` attach 探测实验 | 一个可 attach 的 ZCode 会话 | 未开始（仅设计） |
| **Phase 1** | Codex 裁决实现：`serverRequest/resolved` + `turn/interrupt`，observer 携带 requestId，trait 加 `dispatch` | 无（通道已就绪） | **Stop 已落地（resume→interrupt）；Accept/Reject 阻塞于 requestId 捕获** |
| **Phase 2** | ZCode 裁决实现（视 Phase 0 结果） | Phase 0 通过 | 阻塞于 Phase 0 |
| **Phase 3** | Board 本地动作 `FreezeAll/Unfreeze/SetMode`，全局热键（roadmap E） | Phase 1 | 未开始 |

**近期优先做 Phase 0（探测）和 Phase 1（Codex）**：Phase 0 一天能出且决定 ZCode 裁决生死；Phase 1 通道已就绪，实现成本低、收益明确。Phase 2 阻塞于 Phase 0 结论。

## 7. 安全与边界

- **绝不 spawn 新会话**：所有 Action 只作用于已存在的 Session，遵守 ADR 0001 的红线（只跳转/裁决，不开新）。`thread/start`（Codex）、新建会话类操作不在动作层范围。
- **只读优先**：Observer 的观察通道永远只读（ZCode sqlite `readonly: true`，永不写回官方 DB）。只有显式的裁决 Action 才走写通道。
- **降级不报错**：Backend 不支持的 Action 返回 `unsupported:{op}` 字符串，前端可提示，不 panic、不 crash。
