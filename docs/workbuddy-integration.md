# WorkBuddy 接入说明

## 定位

WorkBuddy 是 Agent Deck 的第三个 backend。WorkBuddy 是腾讯出品的一个**独立**桌面产品（`/Applications/WorkBuddy.app`，bundle id `com.workbuddy.workbuddy`，v5.2.6，Electron）——它**不是** CodeBuddy 桌面端（两者是不同产品）。WorkBuddy 内部**捆绑了 `codebuddy` CLI**（位于 `WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy`）作为它的任务执行引擎：每个 task 对应一个 `session-id`，WorkBuddy 用 `codebuddy --serve --session-id <id> --port <n>` 的方式管理多个并发 task。

> 历史误记纠正（2026-07-23）：早期文档把 WorkBuddy 写成"腾讯 CodeBuddy 桌面端"，这是不准确的。WorkBuddy 与 CodeBuddy 是两个不同的产品；WorkBuddy 只是内置了 codebuddy CLI 引擎。

**当前接入范围（2026-07-23）**：只读观察（列表/状态/bind/pin）+ **会话级跳转**（`workbuddy://chat/<sessionId>` deep link）。操控（Accept/Reject/Stop）未实现。

**e2e 实测（2026-07-23，WorkBuddy.app 运行中）**：观察通道完全可用。探针（`crates/workbuddy/examples/probe.rs`）读到 89 个会话文件，poll_once 返回 20 个（含 1 个 Working），catalog_once 返回 89 个（含 9 个 Working），状态映射（Working/Done）正确。

## 已验证的事实（本机 2026-07-23）

### 数据源

| 路径 | 用途 |
|---|---|
| `~/.workbuddy/projects/<workspace-dir>/<session-id>.jsonl` | 每个 task 一个文件，追加式事件流（状态/工具/AI 标题） |
| `~/.workbuddy/workbuddy.db` | 会话元数据：`sessions.custom_title`（用户改名）、`sessions.title`、`cwd`、`is_playground`、`is_background_automation`、`deleted_at`；自动化名在 `automations.name` + `cwds` |

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

WorkBuddy 不像 ZCode 有专门的 `task_status` 列。状态从事件流**推断**（见 `crates/workbuddy/src/mapper.rs` 的 `infer_status`）。优先级从高到低：

| 条件 | DeckStatus | 时间窗 |
|---|---|---|
| pending `function_call` 且 `requires_approval=true` | Waiting | 30 min（`WAITING_WINDOW_MS`） |
| pending `function_call`（无 matching result，未 completed） | Working | 5 min（`ACTIVE_WINDOW_MS`） |
| 最后一条 assistant `status=incomplete` **且** jsonl 文件仍在写入 | Working | 30 s（`STREAMING_WINDOW_MS` + file mtime） |
| 最近有 user `message`、其后还没有更新的 assistant，**且** 文件仍在写入 | Working | 30 s |
| 最后一条 assistant `status=completed` | Done（绿，已完成） | 衰减见 host open-aware TTL |
| 最后一条 assistant 是 stranded `incomplete`（用户 Stop 后文件已冻结） | Done（按中断点当完成） | 衰减见 host open-aware TTL |
| 以上均不满足 | Idle（开着但没在跑 / 从未完成） | — |

要点：

- `pending` = `function_call` 没有对应的 `function_call_result`（按 `callId` 配对），且 `status != "completed"`。
- **时间窗是必须的**：WorkBuddy 会在 jsonl 里留下僵尸 `function_call` 和 stranded `incomplete` 消息；不加窗会把历史会话永久卡在 Working。
- **只看最后一条 assistant 消息**的 `incomplete`——更早的 incomplete 残留不参与判定。
- 「user 刚发、assistant 还没开口」覆盖思考阶段（此时可能只有 `file-history-snapshot` / `reasoning`，还没有 tool）。
- **Done/Idle 语义（2026-07-23 修正，衰减 open-aware）**：mapper 对已完成/打断回合报告 Done。Done → Idle **不在 mapper 里按 5 分钟自动掉**，而由 host-core 按「是否在 Agent Deck 点开」衰减：
  - **未点开**：保持 Done，最长 `doneTtlUnopenedMs`（默认 12h）后强制 Idle
  - **点开后**：从点键时刻起 `doneTtlAfterOpenMs`（默认 5min）后 Idle
  - 配置见 `~/.agent-deck/settings.json` / 设置页
- **Stop / 打断（2026-07-23）**：用户主动打断后 WorkBuddy 不会把 `incomplete` 改成 `completed`，只是停止往 jsonl 追加。软 Working 信号因此要求 **jsonl file mtime 在 30s 内**；文件冻结后 incomplete 降为 Done，不再卡在 `run`。

### 标题

标题规则（2026-07-23 修正）：

- **用户改名的真正落点是 sqlite，不是 jsonl。**  
  `~/.workbuddy/workbuddy.db` → `sessions.custom_title`（用户改名）/ `sessions.title`（原始/AI 标题）。  
  实测：改「打招呼」→「ai」后 jsonl 仍只有旧 `ai-title=打招呼`，DB 才是 `custom_title=ai`。
- jsonl 仍可读：`custom-title.customTitle`、`ai-title.aiTitle`（作 fallback）。
- 最终优先级：**DB `custom_title` > DB `title` > jsonl customTitle > jsonl aiTitle**。
- 每轮 poll 会重读 DB，改名无需重启即可同步。
- `automation-*` 目录下的 headless 自动化任务通常**没有 AI 标题**，但 DB `custom_title` 常有（如自动化名）。

### 项目（workspace）展示 / bind 分组

WorkBuddy 三类 workspace（bind 第 2 步按分组展示，不再扁平文件夹列表）：

| 类型 | 识别信号 | bind 列表标签 |
|---|---|---|
| **任务** | `sessions.is_playground=1`，或 `~/WorkBuddy/<YYYY-MM-DD-HH-MM-SS>` 时间戳目录 | **会话标题**（`custom_title` / `title`），不是时间戳文件夹名 |
| **项目** | 其余真实路径（如 `~/Desktop/.../modjing`、`~/WorkBuddy/某命名文件夹`） | 文件夹名 `modjing` |
| **自动化** | `sessions.is_background_automation=1`，或 leaf `automation-*` | **`automations.name`**（按 `cwds` 反查），不是 auto 文件夹名 |

实现：

- 分类与标签：`crates/workbuddy/src/db_meta.rs` → `classify_workspace` / `load_automation_names`
- observer 每轮 poll 写入 `SessionSnapshot.project_category` + `project_label`
- 前端 bind 第 2 步按 任务 / 项目 / 自动化 分节（`apps/desktop/src/main.ts`）
- 已删除：`sessions.deleted_at IS NOT NULL` 的会话从 catalog/board 隐藏（jsonl 可能仍残留）
- 归档：WorkBuddy 会话侧目前可靠字段只有 `deleted_at`；自动化表另有 `deleted_at`

任务型会话在 WorkBuddy UI 左侧「任务」里不显示文件夹，但本地仍会落盘到 `~/WorkBuddy/<时间戳>/`。bind picker 直接显示会话标题，不必去翻原始路径。

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

## 会话级跳转（已落地）

点按键 → WorkBuddy.app 聚焦并定位到目标 task。

### 机制：`workbuddy://chat/<sessionId>` deep link

实现见 `apps/desktop/src-tauri/src/lib.rs` 的 `open_workbuddy_session`。

**逆向验证（2026-07-23，WorkBuddy.app v5.2.6）：**

1. **scheme 注册**：`Info.plist` `CFBundleURLSchemes = ["workbuddy"]`，LaunchServices 认领 `workbuddy:`。
2. **路由映射**（renderer `ROUTE_PREFIX_TO_DEEPLINK_HOST`）：
   - 应用内路径 `/task/<sessionId>` ↔ deeplink `workbuddy://chat/<sessionId>`
   - 其余如 `home` / `my-files` / `experts` 等同理，task 跳转只用 `chat`。
3. **`<sessionId>` 语义** = jsonl 文件名（去 `.jsonl`）= `--session-id` 参数 = 事件里的 `sessionId` 字段。`SessionSnapshot.session_id` 已是这个值。
4. **冷启动**：WorkBuddy 主进程有 early-open-url capture + renderer 队列，比 ChatGPT 更抗冷启动吞 URL；实现仍做了与 codex 同构的 warm-up（`open -a WorkBuddy` → 等主进程 → 再发 deep link），双保险。
5. **无进程风险**：URL dispatch 交给已运行实例，不 spawn 第二个 Electron 进程。

### 备选路径（未采用）

| 路径 | 为何不用 |
|---|---|
| `codebuddy --resume <session_id>` | 启 CLI/ACP 会话，不驱动 WorkBuddy 桌面窗口 |
| `http://127.0.0.1:<task_port>` | 端口需扫 `ps`，且 REST 要认证 |
| 仅 `open -a WorkBuddy` | 只能到首页，不能到具体 task |

## 未实现的能力

| 能力 | 现状 | 计划路径 |
|---|---|---|
| 观察（列表/状态/bind/pin） | ✅ 已实现（jsonl 扫描） | — |
| 跳转（点击键打开 task） | ✅ `workbuddy://chat/<sessionId>` | — |
| 操控（Accept/Reject/Stop/Send） | ❌ 未实现 | spawn `codebuddy --acp` 走 ACP JSON-RPC（与 Codex app-server 同族），trait 加 `dispatch` 方法（见 `docs/action-spec.md §3`） |

操控走 `codebuddy` CLI，**绕开 REST 认证问题**。

## 代码位置

| 文件 | 职责 |
|---|---|
| `crates/protocol/src/lib.rs` | `BackendId::Workbuddy` 枚举变体 |
| `crates/workbuddy/src/mapper.rs` | 纯函数：jsonl 事件 → `SessionSignals` → `SessionSnapshot`，含状态推断 + 17 个单测 |
| `crates/workbuddy/src/observer.rs` | `JsonlObserver`：扫 `~/.workbuddy/projects`，open/poll/catalog/poll_pinned 四方法，含 7 个单测 |
| `crates/workbuddy/tests/real_sessions.rs` | `#[ignore]` 真实集成测试（`cargo test --ignored`） |
| `crates/host-core/src/lib.rs` | `impl BackendObserver for JsonlObserver`、`HostConfig` 的 `enable_workbuddy`/`workbuddy_projects_dir` 字段、`HostCore::new` 注册 |
| `apps/desktop/src/main.ts` | `BackendId` 类型、`BACKEND_LABEL`、bind picker 数组 |
| `apps/desktop/src-tauri/src/lib.rs` | `open_workbuddy_session`：`workbuddy://chat/<sessionId>` deep link 跳转 |

## 降级契约

`JsonlObserver` 严格遵守与 ZCode/Codex 相同的隔离契约（见 `crates/zcode/src/observer.rs:86-97`）：`~/.workbuddy/projects` 不存在时 `open()` 返回 `Ok(())`、observer 保持空，**绝不上抛错误**。一个不可用的 backend 永不拖垮其他 backend。
