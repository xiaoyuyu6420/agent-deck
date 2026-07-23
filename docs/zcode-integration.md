# ZCode 接入说明

## 已验证的事实（本机 2026-07-21）

### 数据源

| 文件 | 用途 | 关键表 |
|---|---|---|
| `~/.zcode/v2/tasks-index.sqlite` | 任务状态 | `tasks`（task_status, task_id, workspace_path, updated_at） |
| `~/.zcode/cli/db/db.sqlite` | 工具调用记录 | `tool_usage`（session_id, approval_status, status, started_at, completed_at, tool_name, side_effect_scope） |

### `tasks.task_status` 实际取值

```
running    2
completed  122
error      8
NULL       3   ← 注意，未启动
```

### `tool_usage.approval_status` 实际取值

```
none       30989
requested  1     ← 这就是"等确认"
```

### 状态映射

⚠️ **关键：`tasks.task_status` 不是实时状态。** ZCode 在一轮结束时写 `completed`，但用户继续对话时**不会**把它改回 `running`。所以一个正在跑的会话（UI 左栏转圈）在 `tasks` 表里仍记为 `completed`。唯一实时写入的是 `tool_usage` 表（每次工具调用都立即落盘 started/completed 时间戳）。因此状态判定必须结合 `tool_usage` 的"是否有未完成的 running 记录"（即下表的 `active`）。

| ZCode 原始 | DeckStatus |
|---|---|
| `task_status='error'` | `error` |
| `task_status='completed'` + `active=true`（有未完成的 tool）| `working`（会话其实还在跑，对应 UI 转圈）|
| `task_status='completed'` + `active=false` | `done`（真完成了）|
| `task_status='running'` + 有 open `requested` | `waiting` |
| `task_status='running'` + 无 open `requested` | `working` |
| `task_status IS NULL` 或其他 | `idle` |

`active` 定义：该 task_id 在 `tool_usage` 里存在 `status='running' AND completed_at IS NULL` 的记录。

## 关键 SQL（防陈旧）

跨库 join，30 分钟时间窗，强制 running task：

```sql
ATTACH '/Users/USER/.zcode/cli/db/db.sqlite' AS cli;

SELECT
  t.task_id,
  t.title,
  t.task_status,
  t.workspace_path,
  t.updated_at,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.approval_status = 'requested'
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
      AND tu.started_at > (strftime('%s','now') - 1800) * 1000
  ) THEN 1 ELSE 0 END AS waiting,
  (SELECT tu.tool_name || ': ' || COALESCE(tu.side_effect_scope, '')
   FROM cli.tool_usage tu
   WHERE tu.session_id = t.task_id AND tu.approval_status = 'requested'
     AND tu.status = 'running' AND tu.completed_at IS NULL
   ORDER BY tu.started_at DESC LIMIT 1) AS detail,
  CASE WHEN EXISTS(
    SELECT 1 FROM cli.tool_usage tu
    WHERE tu.session_id = t.task_id
      AND tu.status = 'running'
      AND tu.completed_at IS NULL
  ) THEN 1 ELSE 0 END AS active
FROM tasks t
WHERE t.task_status IN ('running', 'completed', 'error')
  AND t.deleted = 0
  AND t.archived = 0
ORDER BY t.updated_at DESC
LIMIT 20;
```

### 防陈旧的三个必须条件

1. `task_status='running'`（已完成的 task 上残留 requested 不算 waiting）
2. `tu.status='running'`（tool 自身也在跑）
3. `tu.completed_at IS NULL`
4. `tu.started_at > now - 30min`（30 分钟前的也忽略，避免 db 写回延迟污染）

### `active`（实时性）的说明

`active` 加了 5 分钟时间窗（`ACTIVE_WINDOW_SECS`）：`status='running' AND completed_at IS NULL AND started_at > now - 5min`。这个窗口是**必须的**——ZCode 3.4.2 不总是回写 `completed_at`，会留下永久的僵尸 `running` 记录（实测有 age 27 分钟以上的僵尸），不加窗口会把已结束的会话永久卡在 Working。5 分钟能滤掉所有僵尸，又不会漏判真实运行的工具（正常 tool 调用都在 5 分钟内完成）。

> 注：ZCode 桌面 UI 的转圈状态来自 host 进程内存的 runtime projection，**不对外暴露**（ACP `session/list` 恒 idle，`session/resume`+`read` 因进程隔离也只读到静态副本）。`tool_usage` 是唯一实时落盘且可读的数据源，`active` 是基于它的最佳近似。

## 自指防护

Agent Deck host 自己也是 ZCode session，会出现在 tasks 表里。配置：

```json
{
  "excludeWorkspaces": ["/Users/.../codex 键盘"],
  "excludeTaskIds": ["sess_xxx"]
}
```

## 触发机制

监听 `-wal` 文件 mtime，节流 500ms 重查。兜底 500ms 轮询。

```ts
// ZCode 写 DB → -wal 文件 mtime 变 → 触发 pollOnce()
```

## 只读模式

打开 DB 必须用 `readonly: true`：

```ts
new Database(path, { readonly: true, fileMustExist: false })
```

绝不写回官方 DB。

## 动作层（Phase 0 探测，未实现）

> 完整规格见 [action-spec.md](./action-spec.md) §5。本节是 ZCode 侧的事实依据。

ZCode 支持 ACP（Agent Client Protocol）+ 自定义扩展。完整方法集（从 app.asar 抽取）：

```
session/list  session/send  session/stop  session/setMode  session/setModel
session/setThoughtLevel  session/subscribe  session/cancelBackgroundTask
interaction/requestPermission  interaction/requestUserInput
task/response   ← 批准/拒绝的入口
```

入口二进制：`/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs`

```bash
node zcode.cjs app-server    # ACP stdio server
node zcode.cjs --help        # 0.15.2
```

**Phase 0 探测实验（未开始）**：能否用独立 spawn 的 `zcode.cjs app-server` 通过 `session/load({ sessionId })` attach 到 Desktop 已有的 sessionId。

- **成功**（能 attach + 发 `task/response` 后 Desktop 侧确实收到裁决 + Desktop 不被干扰）→ Phase 2 实现 Accept/Reject/Stop（`task/response` 批准/拒绝、`session/stop` 中断）
- **失败**（任一不满足）→ ZCode 侧裁决永久标 unsupported，ZCode Backend 只保留观察 + 跳转（`open_slot_session`）

这是可接受的降级——ZCode 用户仍可点 deck 跳进 Desktop 窗口手动裁决。探测步骤与判据见 action-spec.md §5.1。

### --mode 取值（与 Plan/Act/Review 对齐）

`zcode.cjs` 支持 `--mode build|edit|plan|yolo`：

| Deck PolicyMode | zcode --mode |
|---|---|
| `plan` | `plan` |
| `act` | `build` |
| `review` | `edit` |

（`yolo` 对应 host 不暴露的"全自动"模式）

## 风险

| 风险 | 对策 |
|---|---|
| ZCode sqlite schema 变 | 启动时 `.schema tasks` 自检，字段缺失降级 |
| `approval_status` 陈旧 | SQL 强制 running + 时间窗 |
| 自指 | config 排除 |
| Desktop IPC 私有 | V1 不依赖，纯 sqlite 观察 |
| host 自己被 ZCode 任务观察 | excludeWorkspaces |
