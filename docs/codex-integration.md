# Codex 接入说明

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
thread/list                    → 当前所有 thread
thread/loaded/list             → 已加载的
thread/status/changed          → 订阅状态变化事件
```

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
```

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

## Phase 1 实施步骤

1. `codex app-server generate-json-schema --out schema/codex`，schema 进 git
2. CodexAdapter：
   - 启动时探测：先试 `codex remote-control start`，失败 fallback `codex app-server daemon start`
   - 连 `~/.codex/ipc/ipc.sock` 或新起 stdio 子进程
   - subscribe `thread/status/changed` + `ServerRequest approvals`
   - 映射到 DeckStatus
3. 动作层：实现 accept (serverRequest/resolved)、stop (turn/interrupt)

## V1 现状

V1 仅实现 stub（返回空快照），真正接入在 Phase 1。原因：
- 优先把 ZCode 灯做对（你主力）
- Codex 协议复杂度高于 ZCode sqlite
- 避免首版被协议调试拖慢

## 风险

| 风险 | 对策 |
|---|---|
| Codex 协议漂移 | generate-json-schema 钉版本到 schema/codex/ |
| app-server 实验性 | fallback `remote-control`，两家都试 |
| codex 未启动 | adapter 优雅降级，不报错只 log |
