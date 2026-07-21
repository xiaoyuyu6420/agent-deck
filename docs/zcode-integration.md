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

| ZCode 原始 | DeckStatus |
|---|---|
| `task_status='error'` | `error` |
| `task_status='completed'` | `done` |
| `task_status='running'` + 有 open `requested` | `waiting` |
| `task_status='running'` + 无 open `requested` | `working` |
| `task_status IS NULL` 或其他 | `idle` |

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
   ORDER BY tu.started_at DESC LIMIT 1) AS detail
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

## 动作层（V1.1+，未实现）

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

**V1.1 探测实验**：能否用独立 spawn 的 `zcode.cjs app-server` 通过 `session/load({ sessionId })` attach 到 Desktop 已有的 sessionId。如果可以，V1.1 实现 Accept/Reject/Stop；不行则动作层永久标 unsupported。

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
