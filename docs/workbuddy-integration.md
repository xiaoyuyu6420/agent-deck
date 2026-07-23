# WorkBuddy 接入说明

## 定位

WorkBuddy 是 Agent Deck 的第三个 backend。WorkBuddy 是腾讯出品的一个**独立**桌面产品（`/Applications/WorkBuddy.app`，bundle id `com.workbuddy.workbuddy`，v5.2.6，Electron）——它**不是** CodeBuddy 桌面端（两者是不同产品）。WorkBuddy 内部**捆绑了 `codebuddy` CLI**（位于 `WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy`）作为它的任务执行引擎：每个 task 对应一个 `session-id`，WorkBuddy 用 `codebuddy --serve --session-id <id> --port <n>` 的方式管理多个并发 task。

> 历史误记纠正（2026-07-23）：早期文档把 WorkBuddy 写成"腾讯 CodeBuddy 桌面端"，这是不准确的。WorkBuddy 与 CodeBuddy 是两个不同的产品；WorkBuddy 只是内置了 codebuddy CLI 引擎。

**当前接入范围（观察阶段，2026-07-23）**：只读观察——把 WorkBuddy 的 task 列表渲染到键盘上，支持 bind/pin。**跳转和操控未实现**（见下文"未实现的能力"）。

**e2e 实测（2026-07-23，WorkBuddy.app 运行中）**：观察通道完全可用。探针（`crates/workbuddy/examples/probe.rs`）读到 89 个会话文件，poll_once 返回 20 个（含 1 个 Working），catalog_once 返回 89 个（含 9 个 Working），状态映射（Working/Done）正确。

## 已验证的事实（本机 2026-07-23）

### 数据源

| 路径 | 用途 |
|---|---|
| `~/.workbuddy/projects/<workspace-dir>/<session-id>.jsonl` | 每个 task 一个文件，追加式事件流 |

文件名（去 `.jsonl`）即 `session-id`，与 `--serve --session-id` 进程参数、jsonl 内每行的 `sessionId` 字段三者一致。

### jsonl 事件结构

每行一个 JSON 对象，`type` 字段区分事件类型。实测高频类型：

| type | 含义 | 用到的字段 |
|---|---|---|
| `message` | 一轮对话消息 | `role`(user/assistant) |
| `function_call` | 工具调用 | `callId`, `name`(工具名), `status`, `arguments`(JSON 串，内含 `requires_approval`) |
| `function_call_result` | 工具调用返回 | `callId`（与 function_call 配对） |
| `reasoning` | 推理过程 | — |
| `ai-title` | 模型生成的标题 | `aiTitle` |
| `custom-title` | 用户设置的标题 | `title` |
| `file-history-snapshot` | 文件快照 | — |

每行都带 `timestamp`（毫秒 epoch）和 `cwd`（workspace 路径）。

### 状态映射

WorkBuddy 不像 ZCode 有专门的 `task_status` 列。状态从事件流**推断**（见 `crates/workbuddy/src/mapper.rs` 的 `infer_status`）：

| 条件 | DeckStatus |
|---|---|
| 有 pending 的 function_call 且 `requires_approval=true` | Waiting |
| 有 pending 的 function_call（无 matching result，未 completed） | Working |
| 无 pending 调用，且最后事件在 5 分钟内 | Idle（对话还活着，agent 在思考/读） |
| 无 pending 调用，且超过 5 分钟无活动 | Done |

`pending` = `function_call` 没有对应的 `function_call_result`（按 `callId` 配对），且 `status != "completed"`。

### 标题

`custom-title`（用户设置）优先于 `ai-title`（模型生成）。`automation-*` 目录下的 headless 自动化任务通常**没有标题**（显示 `(untitled)`），这是正常现象。

## 实测规模

```
poll  = 20 个 session（最近活跃，max_sessions=20）
catalog = 89 个 session（全量历史，max=500）
3/3 交互式 task 有正确中文标题
```

## 为什么不用 REST API

WorkBuddy 暴露了完整的本地 REST API（`http://127.0.0.1:<port>/api/v1/*`，端口实测为 58467 主 / 52282-52284 各 task），端点包括 `/api/v1/sessions`、`/api/v1/stats/session`、`/api/v1/daemon/*` 等。**但 REST API 走不通，原因：**

- `/api/v1/*` 需要认证（`POST /api/v1/auth/login {password}` 登录拿 session token）
- 这个 **password 不是进程命令行里的 `--token`**（那是组件间内部通信用的，REST 不接受，实测返回 `AUTH_REQUIRED`）
- password **不在 `settings.json`、不在进程参数、不以明文落盘**（connectors 凭据是 AES-256-GCM 加密的，且属于另一回事）
- 源码逻辑 `r.token || password`：登录成功后若服务端没返回 token 就用密码本身当 token，说明这是**需要人工设置的访问密码**

因此 observer 无法自动获取 REST 认证，**退回 jsonl 文件扫描方案**（零认证、零 token、纯只读）。这与 ZCode observer 读 sqlite 的模式一致。

## 未实现的能力

| 能力 | 现状 | 计划路径 |
|---|---|---|
| 观察（列表/状态/bind/pin） | ✅ 已实现（jsonl 扫描） | — |
| 跳转（点击键打开 task） | ❌ `open_slot_session` 返回"暂未实现" | `codebuddy --resume <session_id>`，或打开 `http://127.0.0.1:<task_port>`（端口从 `ps` 的 `--serve --session-id <id> --port <n>` 解析） |
| 操控（Accept/Reject/Stop/Send） | ❌ 未实现 | spawn `codebuddy --acp` 走 ACP JSON-RPC（与 Codex app-server 同族），trait 加 `dispatch` 方法（见 `docs/action-spec.md §3`） |

跳转和操控都走 `codebuddy` CLI，**绕开 REST 认证问题**。

## 代码位置

| 文件 | 职责 |
|---|---|
| `crates/protocol/src/lib.rs` | `BackendId::Workbuddy` 枚举变体 |
| `crates/workbuddy/src/mapper.rs` | 纯函数：jsonl 事件 → `SessionSignals` → `SessionSnapshot`，含状态推断 + 17 个单测 |
| `crates/workbuddy/src/observer.rs` | `JsonlObserver`：扫 `~/.workbuddy/projects`，open/poll/catalog/poll_pinned 四方法，含 7 个单测 |
| `crates/workbuddy/tests/real_sessions.rs` | `#[ignore]` 真实集成测试（`cargo test --ignored`） |
| `crates/host-core/src/lib.rs` | `impl BackendObserver for JsonlObserver`、`HostConfig` 的 `enable_workbuddy`/`workbuddy_projects_dir` 字段、`HostCore::new` 注册 |
| `apps/desktop/src/main.ts` | `BackendId` 类型、`BACKEND_LABEL`、bind picker 数组 |

## 降级契约

`JsonlObserver` 严格遵守与 ZCode/Codex 相同的隔离契约（见 `crates/zcode/src/observer.rs:86-97`）：`~/.workbuddy/projects` 不存在时 `open()` 返回 `Ok(())`、observer 保持空，**绝不上抛错误**。一个不可用的 backend 永不拖垮其他 backend。
