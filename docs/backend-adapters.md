# Backend 适配规范

> 新增一个 Agent backend（如 Claude Code、Cursor、Aider 等）时，按本规范实施。
> 本规范从 zcode / codex / workbuddy 三个现有 backend 的真实接入中提炼。
> 三个 backend 的逐项对比见各自的 `docs/<name>-integration.md`。

## 核心理念

每个 backend 是一个**独立 crate**，实现 `BackendObserver` trait，把目标 Agent 的会话状态**只读地**映射成统一的 `SessionSnapshot`。host-core 聚合所有 backend，board 分配槽位。各 backend 独立失败、互不影响。

**两条铁律：**
1. **只读**——绝不写回目标 Agent 的 DB / 文件 / socket。我们只观察。
2. **降级契约**——数据源缺失（Agent 没装、没运行、文件不存在）时，`open()`/`poll()` 返回 `Ok(vec![])`，绝不上抛错误、绝不 panic。某 backend 挂了不能拖垮整个 deck。

## 新增 backend 完整 Checklist

以新增 backend `foo` 为例，必须按顺序完成以下改动。**A-E 是代码，F 是文档**。

### A. 新建 crate（参照最新的 `crates/workbuddy/`）

```
crates/foo/
  Cargo.toml      # 依赖 agent-deck-protocol + thiserror；按数据源加 serde_json/rusqlite
  src/lib.rs      # 导出 FooObserver + FooObserverOptions + mapper 函数
  src/mapper.rs   # 纯函数：原始数据 → SessionSnapshot（必须无 IO，便于单测）
  src/observer.rs # open/poll_once/catalog_once/poll_pinned_once + 降级逻辑
```

mapper 必须是**纯函数**（无 IO、无全局状态），单测覆盖所有状态分支。observer 负责 IO 和降级。

### B. protocol 层——标识（唯一真相）

- `crates/protocol/src/lib.rs` 的 `BackendId` enum 加变体 `Foo`。
  - `#[serde(rename_all = "lowercase")]`，所以变体名小写 = 序列化名 = board key 前缀 = 前端类型字面量。
  - ⚠️ 这个小写名是后续多处手工同步的源头，命名要稳定（定下来就别改）。

### C. board 层——键前缀（⚠️ 两处必须一致）

`crates/board/src/session_board.rs` 有**两个** match 要加分支，且前缀字符串必须与 protocol 序列化名一致：

1. `key()` 方法（构造 `{prefix}:{session_id}` 隔离键）
2. `replace_backend_sessions()` 的 prefix match（按前缀清理某 backend 缓存）

> ⚠️ **这是最易漏的重复点**。两处前缀不一致会导致：清 backend 缓存时清错前缀、或 key 冲突。新增后务必 grep 确认。

### D. host-core 层——配置 + 注册 + trait impl

1. `Cargo.toml`（workspace 根）：members 加 `"crates/foo"`；`[workspace.dependencies]` 加 `agent-deck-foo = { path = "crates/foo" }`。
2. `crates/host-core/Cargo.toml`：加 `agent-deck-foo = { workspace = true }`。
3. `crates/host-core/src/lib.rs`：
   - `HostConfig` 加 `enable_foo: bool` + `foo_*_dir/path: Option<PathBuf>`（数据源路径覆盖）。
   - `Default for HostConfig` 给新字段默认值（`enable_foo: true`，路径 `None` 走自动推断）。
   - `HostCore::new` 加注册块：`if config.enable_foo { observers.push(Box::new(...)); }`（参照现有 codex/workbuddy 的写法，注意 try open + 失败不 panic）。
   - 加 `impl BackendObserver for FooObserver`：`id()` 返回 `BackendId::Foo`，三个查询方法委托给 observer。

### E. desktop 层——默认配置 + 跳转 + 前端

1. `apps/desktop/src-tauri/src/lib.rs`：
   - `default_config()` 加 `enable_foo: true` + 路径字段。
   - `open_slot_session` 的 `match backend` 加 `BackendId::Foo => open_foo_session(...)`。
   - 新增 `open_foo_session`（跳转；若暂未实现，返回明确错误如 `Err("Foo 跳转暂未实现")`，不要静默）。
2. `apps/desktop/src/main.ts`：
   - `BackendId` 类型加字面量（小写，与 protocol 一致）。
   - `BACKEND_LABEL` 加 `foo: 'Foo'`（显示名）。
   - bind picker 的 `backends: BackendId[]` 数组加 `'foo'`。

### F. 文档——`docs/foo-integration.md`

按下方"文档模板"写。

## 数据源选型指引

目标 Agent 的数据来源决定了 backend 的复杂度。三种已知模式（按复杂度递增）：

| 模式 | 代表 | 适用场景 | 复杂度 | 实时性 |
|---|---|---|---|---|
| **文件扫描（轮询）** | workbuddy (jsonl) | Agent 把会话事件追加写文件 | 低 | 轮询延迟(200ms) |
| **DB 只读查询** | zcode (sqlite) | Agent 用本地 DB 存会话状态 | 中 | 取决于 DB 落盘延迟 |
| **子进程 RPC + 事件通道** | codex (app-server + ipc.sock) | Agent 有 CLI daemon / 桌面 app，需拿 live 内存状态 | 高 | 事件驱动(准实时) |

**关键决策点**：能否拿到**实时 working/waiting 状态**？
- 文件/DB 模式：靠轮询 + 时间窗推断（workbuddy 用 5 分钟窗区分 idle/done；zcode 靠 tool_usage 的实时落盘）。
- 如果目标 Agent 有"独立进程看不到 GUI 内存 live 状态"的隔离问题（如 codex），需要**第二条事件通道**（codex 连 ipc.sock 订阅广播）。这是最复杂的情况，优先评估能否避免。

> 探测数据源时，先确认：① Agent 把状态写到哪（DB/文件/socket/云）？② 写入是否实时？③ 外部进程能否读到？④ 需要认证吗（workbuddy REST API 要密码，所以退回 jsonl）？

## 状态映射规范

所有 backend 最终产出 `SessionSnapshot { status: DeckStatus }`，`DeckStatus` 六态（`protocol/src/lib.rs`）：

| DeckStatus | 含义 | 优先级 |
|---|---|---|
| `Waiting` | 待用户审批/输入 | 5（最高） |
| `Error` | 出错 | 4 |
| `Working` | 正在执行 | 3 |
| `Done` | 刚完成 | 2 |
| `Idle` | 空闲/历史 | 1 |
| `Off` | 无会话 | 0 |

映射要点：
- **waiting_since**：当且仅当 `status == Waiting` 时设为 `Some(updated_at)`，否则 `None`。
- **session_id**：用 Agent 的**会话唯一标识**（codex 是 thread.id/rollout UUID，workbuddy 是 session-id，zcode 是 task_id）。这个 id 也是跳转 deep link 的参数，必须选对。
- **特殊状态修正**：若 Agent 的持久化状态非实时（如 zcode 的 task_status 完成后不改回 running、codex 的 notLoaded），需要在 mapper 或 observer 层做修正覆盖。在 integration doc 里明确记录修正逻辑。
- 不是所有状态都能映射：codex 无显式 Done，workbuddy 无 Error——缺的状态在 doc 里说明。

## BackendObserver trait（当前形态）

```rust
// crates/host-core/src/lib.rs
pub trait BackendObserver: Send {
    fn id(&self) -> BackendId;
    fn poll(&mut self) -> anyhow::Result<Vec<SessionSnapshot>>;
    fn list_catalog(&mut self) -> anyhow::Result<Vec<SessionSnapshot>> { /* 默认回退 poll */ }
    fn poll_pinned(&mut self, ids: &[String]) -> anyhow::Result<Vec<SessionSnapshot>> { /* 默认过滤 poll */ }
}
```

- **poll**：给 board 轮询用，返回活跃子集（过滤掉纯 idle/notLoaded，避免占满槽位）。
- **list_catalog**：给 bind picker 用，返回全集（含历史 idle，按 recency 窗截断）。
- **poll_pinned**：刷新用户手动钉住的会话（即使滑出 poll 窗口也保持可见）。
- **dispatch（裁决）当前不在 trait 里**，是全局 stub。未来若接入裁决（Accept/Reject/Stop），需决策是加进本 trait 还是另立（见 `action-spec.md`）。

> 若目标 Agent 的数据源不支持按 id 单查（如 codex app-server 无 per-id RPC、workbuddy 无索引），`poll_pinned` 就拉全集再客户端过滤——在 doc 里记录这个降级。

## 跳转（deep link）规范

跳转入口统一在 `apps/desktop/src-tauri/src/lib.rs` 的 `open_slot_session`，按 backend 分支。实现 `open_foo_session` 时：

- **优先用 URL scheme deep link**（`foo://...`），走系统 `open(1)`，不 spawn 第二个 app 进程（吸取 zcode 方案 (B) crash 教训）。
- **冷启动处理**：若 deep link 在 App 未运行时被吞（如 codex），先 `open -a <App>` 拉起、轮询等主进程就绪，再发 deep link。
- **粒度**：尽量做到 session 级（codex `codex://threads/<id>`）。做不到就降级到 workspace/project 级（zcode 只能到项目目录），在 doc 明确记录粒度限制。
- **暂未实现**：返回明确错误（`Err("Foo 跳转暂未实现")`），不要静默失败。

## 文档模板（`docs/foo-integration.md`）

参照 `docs/workbuddy-integration.md`（结构最完整）。推荐章节：

1. **定位与接入范围**——这个 backend 是什么 Agent、当前实现到哪一步（观察/跳转/裁决）。
2. **数据源**——路径、读取方式（文件/DB/RPC/socket）、是否启动外部进程、是否需要认证。表格列出关键路径。
3. **状态映射**——原始状态 → DeckStatus 的映射表，含特殊修正逻辑。
4. **实时性来源**——working/waiting 状态怎么拿到、延迟多少。
5. **降级契约**——数据源缺失时的行为（与其它 backend 一致：返回空，不报错）。
6. **跳转能力**——粒度（session/workspace）、机制（deep link）、冷启动处理、或"未实现"。
7. **裁决动作现状**——是否实现 Accept/Reject/Stop，对应 Agent 的什么 API；或标"未实现/Phase N"。
8. **代码位置**——表格列出 crate 内各文件 + 跨层接入点的 file:line。
9. **风险与对策**——协议漂移、进程隔离、私有 API 等。

## 三 backend 速查表

| 维度 | zcode | codex | workbuddy |
|---|---|---|---|
| 数据源 | 双 sqlite 只读 join | app-server RPC + ipc.sock 双通道 | jsonl 文件扫描 |
| 实时状态来源 | tool_usage 实时落盘 | ipc.sock 广播覆盖 notLoaded | recency 窗推断 |
| 跳转粒度 | workspace 级 | **session 级** | 未实现 |
| 裁决动作 | 未实现(Phase 0) | 未实现(Phase 1) | 未实现 |
| 启动外部进程 | 否 | 是(app-server 子进程) | 否 |
| error 态 | 有 | 有(systemError) | 无 |
| done 态 | 有 | 隐含(idle+recency) | 有 |
| risk 推断 | 有 | 无 | 有 |

## 已知技术债

- **BackendId 标识在 6 处手工同步**（protocol enum + board 2 处 + 前端 3 处），新增 backend 时易漏。未来可考虑用宏或 codegen 收敛，但目前手工对齐 + grep 校验可接受。
- **裁决动作（dispatch）未进 trait**，全局 stub。接入第一个裁决 backend 时需做 trait 设计决策。
- **codex ipc.sock 是私有协议**（v11），随 app 更新可能变。解析需防御性 coding，失败降级。
