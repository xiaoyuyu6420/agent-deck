# Codex 接入说明

> **实现现状（2026-07-23）**：
> - **观察通道**已落地——`CodexObserver` 走 `codex app-server --listen stdio://` 子进程 + `initialize` + `thread/list` 轮询（见 `crates/codex/src/rpc.rs`，对 codex-cli 0.145.0-alpha.27 验证），poll/catalog/poll_pinned 全通。
> - **会话级跳转**已落地——点按键触发 `codex://threads/<threadId>` deep link，聚焦/启动 ChatGPT.app 并定位到该 thread（见下文 [会话级跳转](#会话级跳转已落地)）。这与 ZCode 只能跳到 workspace 形成本质区别。
> - 下文"裁决动作（Phase 1）"是后续，规格见 [action-spec.md](./action-spec.md)。

## 已验证的事实（本机 2026-07-21）

```bash
$ codex --version
$ codex app-server --help
$ codex remote-control --help
```

存在两条官方控制通道：

### 1. `codex app-server`（推荐用于 host）

实验性 app-server，提供完整 RPC。

```bash
codex app-server daemon start       # 起 daemon
codex app-server daemon version     # 查版本
codex app-server --listen stdio://  # stdio 模式
codex app-server --listen unix://   # unix socket
codex app-server --listen ws://IP:PORT

# 协议 schema 生成（钉版本用）
codex app-server generate-json-schema --out ./schema/codex
codex app-server generate-ts --out ./src/generated
```

`~/.codex/ipc/ipc.sock` 已存在（daemon 自动起）。

### 2. `codex remote-control`（专门为外部控制设计）

```bash
codex remote-control start   # 起 daemon（带远程控制）
codex remote-control stop
codex remote-control --json  # JSON 输出
```

**这个可能比裸 app-server 更稳，专为外部控制设计**。Phase 1 优先评估。

## 协议（ACP + Codex 扩展）

Codex 用类 ACP 协议。核心方法（从官方 schema）：

### 状态观察

```
thread/list                    → 当前所有 thread（分页，data + nextCursor）
thread/loaded/list             → 已加载（在内存中）的 thread
thread/read {threadId}         → 单查一个 thread 的完整信息（含 path/cwd/status/preview）
thread/status/changed          → 订阅状态变化事件
```

> 协议全集由 `codex app-server generate-json-schema` 生成（v2 共 87 个 client method），已落库到 `schema/codex/`。

ThreadStatus：
```
notLoaded | idle | systemError | active(activeFlags[])
activeFlags: waitingOnApproval | waitingOnUserInput
```

### 主事件订阅

```
thread/status/changed   → { threadId, status }
turn/started            → 回合开始
turn/completed          → 回合结束
ServerRequest approvals → 审批请求
```

### 动作

```
serverRequest/resolved   → Accept/Reject 审批
turn/interrupt           → Stop
thread/start             → 新建
thread/resume            → 按 threadId 重新加入（rejoin）一个 thread
```

`thread/resume` 的官方语义（来自 `ThreadResumeParams` schema 描述）：三种方式 by threadId / by history / by path；**"If thread_id identifies a running thread, app-server rejoins that thread"**——是真正的 rejoin，不是从磁盘 hydrate 一个无关的 headless 副本。这一点与 ZCode 的 ACP `session/resume`（只能 hydrate 无关副本、驱动不了桌面窗口）形成根本区别。实测（2026-07-23）对真实 threadId 调用成功，`thread/loaded/list` 随后确认该 thread 已加载。

### 配置

```
settings / model_reasoning_effort   → reasoning level（旋钮用）
```

## 状态映射

```
active + waitingOnApproval | waitingOnUserInput → waiting
active + 无 flag                                  → working
idle + 最近 completed                             → done
systemError / failed                              → error
idle                                              → idle
```

## 观察通道实现（已落地）

实际采用的方案（与早期设想的"remote-control / ipc.sock"不同）：

- `JsonRpcClient::spawn`（`crates/codex/src/rpc.rs:97`）：起 `codex app-server --listen stdio://` 子进程
- `initialize` + `notifications/initialized` 握手（rpc.rs:114）。`InitializeParams` 只要求 `clientInfo{name,version}`（官方 schema 验证；早期代码多传的 `protocolVersion` 已移除）
- `thread/list` 轮询拉取（observer.rs:134），非事件订阅
- `poll_once`（近 20 活跃）/ `catalog_once`（长窗口 200/90 天）/ `poll_pinned_once`（按 id 直查）三档

> 早期设想"探测 `remote-control` → fallback `app-server daemon` → 连 `ipc.sock`"**未采用**——stdio 子进程方案更简单且已验证，不需要 daemon/socket 管理。

## 会话级跳转（已落地）

点按键 → ChatGPT.app 聚焦并定位到目标会话。这是 codex 相对 zcode 的核心优势：zcode 只能跳到 workspace（项目目录），codex 能精确到 thread。

### 机制：`codex://threads/<threadId>` deep link

实现见 `apps/desktop/src-tauri/src/lib.rs` 的 `open_codex_session`：dispatch `codex://threads/{session_id}` 给系统 `open(1)`。

**逆向验证（2026-07-23，codex-cli 0.145.0-alpha.27 / ChatGPT.app）：**

1. **scheme 注册**：ChatGPT.app 通过 LaunchServices 认领 `codex:` scheme（`lsregister -dump` 确认 `claimed schemes: codex:`，bundle = ChatGPT）。
2. **deep link 模板**：解包 `ChatGPT.app/Contents/Resources/app.asar` 后，渲染层在 5 处构造 `codex://threads/${id}`——"Open in app" 菜单项（Chrome 扩展 thread → 桌面 App）、`copyAppLink` 动作等。
3. **`<id>` 的语义 = `threadId` = `thread.id`（rollout UUID）**，**不是** rollout 的 `session_id`。后者是 git/worktree 会话标识（`codex_turn_diff_event` 里 `thread_id` 与 `session_id` 是两个不同字段）。两者在多数 thread 里数值相等属巧合，语义正确字段是 `thread.id`。因此 `crates/codex/src/mapper.rs` 的 `map_thread` 用 `t.id` 作为 `SessionSnapshot.session_id`。
4. **无进程风险**：URL dispatch 派发给已运行的 ChatGPT.app 实例，不 spawn 第二个进程，没有单实例锁冲突（对照 zcode 方案 (B) 的死路）。

### 备选通道：`thread/resume` RPC（未用于跳转，留作未来）

deep link 已能驱动桌面窗口跳转，无需 RPC。但 `thread/resume {threadId}` 是官方正式 `ClientRequest` method，可在独立 app-server 连接上 rejoin 任意 thread（实测成功）。潜在用途：在键盘端做 in-app 控制（如直接发消息、stop）而不切窗口。当前 observer 的长连接是同步阻塞轮询，不适合复用做 resume；若启用，应像实测那样用独立的临时连接（spawn → initialize → resume → drop）。

### `thread/read` 单查（潜在增强）

`thread/read {threadId, includeTurns}` 可单查任意 thread 的完整信息（实测返回 status/cwd/path/preview）。当前 `poll_pinned_once` 仍走 `thread/list` 客户端过滤（app-server 无按 id 直查的便捷 RPC）；未来若需精确刷新某个 pinned thread，可改用 `thread/read`。

### 实机验证结论（2026-07-23 实测）

对两个不同 thread 实测了冷启动与热启动两种场景：

| 场景 | 结果 |
|---|---|
| **热启动**（GUI 已运行） | ✅ **精确跳转到目标 thread**。对 thread A（modjing / "首页前端"）和 thread B（智能驾驶 / "review 项目文案"）分别发 deep link，均跳到对应历史会话，可重复、按 id 区分。 |
| **冷启动**（GUI 未运行） | ⚠️ **只到项目起始页**。deep link 启动了 App 并定位到正确 *项目*，但 URL 在早期启动流程被吞掉，未导航到具体 thread（显示 new-thread landing page）。 |

**结论**：deep link 的精确跳转依赖 GUI 已就绪（URL handler 已注册）。

**已实现的修复**（`open_codex_session`）：检测 ChatGPT.app 是否在跑——若没跑，先 `open -a ChatGPT` 拉起、轮询等待主进程就绪（`pgrep ChatGPT.app/Contents/MacOS/ChatGPT`，最长 ~10s）+ 500ms 渲染缓冲，再发 deep link。这样冷启动也能精确跳转。`chatgpt_app_running()` 用主进程路径区分 GUI 与 `codex app-server` 子进程（后者在 `Contents/Resources` 下）。

## 裁决动作（Phase 1，未实现）

观察通道已就绪，裁决动作在**同一个 `JsonRpcClient`** 上发对应 method 即可：

| Action | method | 参数 |
|---|---|---|
| Accept | `serverRequest/resolved` | `{ requestId, status: "accepted" }` |
| Reject | `serverRequest/resolved` | `{ requestId, status: "rejected" }` |
| Stop / StopAll | `turn/interrupt` | `{ threadId }` |

待解设计点：`thread/list` 当前不带未决 `requestId`，实现 Accept/Reject 需让 observer 在 poll 时携带最近 ServerRequest 的 requestId。详见 [action-spec.md](./action-spec.md) §4。

## 风险

| 风险 | 对策 |
|---|---|
| Codex 协议漂移 | generate-json-schema 钉版本到 schema/codex/ |
| app-server 实验性 | fallback `remote-control`，两家都试 |
| codex 未启动 | adapter 优雅降级，不报错只 log |

## 实时状态（ipc.sock）：e2e 发现与现状

> 本节由 2026-07-23 的端到端实测修正，记录一个重要的协议认知更新。

### 背景

独立 spawn 的 app-server 看不到 GUI 的 live thread 状态（进程内存隔离，`thread/list` 全 `notLoaded`）。为补全 working/waiting，`crates/codex/src/ipc.rs` 的 `IpcStateWatcher` 连 GUI 的 `~/.codex/ipc/ipc.sock`（IpcRouter，4 字节小端长度前缀 + JSON），订阅广播。

### e2e 实测发现（协议认知修正）

探针 `crates/codex/examples/ipc_probe.rs` + python 裸 ipc 监听双通道实测，揭示一个**关键的协议理解错误**：

| 广播 method | 最初理解 | 实测真相（逆向 app.asar 确认） |
|---|---|---|
| `thread-stream-state-changed` | turn 状态（working/waiting）广播，payload `{status, threadId}` | ❌ **是对话内容增量同步**（`change.type=patches/snapshot`），且只推给已注册的 stream **follower**（`targetClientIds=getFollowerClientIds`）。作为 `clientType:"extension"` 连入**收不到**它 |
| `thread-stream-following-changed` | — | ✅ 能收到，payload `{conversationId, following:bool}`，表示 GUI 当前聚焦哪个 thread |

实测：60 秒持续监听，GUI 有操作时**零条** `thread-stream-state-changed`，仅收到 `following-changed`。

### 当前状态

- ✅ **ipc.sock 握手连通**（`initialize` → clientId）。
- ✅ **app-server RPC 通**（`thread/list` 返回全集，catalog 25 个会话）。
- ✅ **降级正常**（GUI 无 active turn 时 poll 返回空，不崩溃）。
- ⚠️ **working/waiting 实时覆盖未生效**：`parse_broadcast` 解析的 `{status, threadId}` 结构与真实 payload（`{conversationId, change:{...}}`）不匹配，且 extension client 非 follower 收不到内容流。

### 待重新设计的路径

要拿到真正的 turn 状态，需重新评估 ipc 接入方式（按可能性排序）：
1. **注册成 stream follower**：发 follower 注册请求，接收 `thread-stream-state-changed` 的 snapshot，从中解析 `conversationState` 的 active/resumeState 字段。
2. **轮询 `thread/stream/status` 类请求**（若 IPC 总线有对应 request method）。
3. **退回 app-server 的 `thread/status/changed` JSON-RPC 通知**：但那是 GUI app-server 的内部通道（stdio），外部 app-server 进程收不到。

`IpcStateWatcher` 的帧编解码（length-prefixed）+ 握手 + 后台线程架构**已验证可用**，仅需修正订阅/解析逻辑——这是后续任务，不在当前范围。当前 codex 会话在 deck 上显示的状态是静态 `notLoaded`（与 ipc 接入前一致，不退化）。
