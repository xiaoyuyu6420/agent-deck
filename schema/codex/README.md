# Codex app-server 协议 schema

由 `codex app-server generate-json-schema` 生成，钉版本防协议漂移。

## 生成方式

```bash
# codex 二进制位于 ChatGPT.app 内（off-PATH）
CODEX=/Applications/ChatGPT.app/Contents/Resources/codex
"$CODEX" app-server generate-json-schema --out ./schema/codex
```

生成后只保留**顶层**文件（逐方法单文件 + 两个聚合 schema），移除了冗余的 `v1/`、`v2/` 逐方法子目录——顶层聚合 `codex_app_server_protocol.v2.schemas.json` 已含全部定义。

## 当前版本

- **codex-cli**: `0.145.0-alpha.27`
- **生成日期**: 2026-07-23
- **client method 总数**: 87（见 `ClientRequest.json` 的 `oneOf`）

## 关键文件

| 文件 | 内容 |
|---|---|
| `ClientRequest.json` | 客户端→服务端的所有请求方法（87 个 `oneOf` 变体，含 `thread/list`、`thread/resume`、`thread/read`、`turn/interrupt`、`serverRequest/resolved` 等） |
| `ServerNotification.json` | 服务端推送的通知（`thread/status/changed`、`turn/started` 等） |
| `ServerRequest.json` | 服务端→客户端的请求（审批 `serverRequest/*`，Accept/Reject 通过 `serverRequest/resolved` 响应） |
| `ClientNotification.json` | 客户端→服务端的通知 |
| `codex_app_server_protocol.schemas.json` / `.v2.schemas.json` | 全量聚合 schema（含所有 `$definitions`） |
| `*.json`（其余） | 各方法/类型的独立 schema，便于人读单方法 |

## 与实现的关系

- **观察通道**（已落地）：`thread/list`，见 `crates/codex/src/observer.rs`。
- **会话跳转**（已落地）：不走 RPC，走 `codex://threads/<threadId>` deep link，见 `docs/codex-integration.md`。`thread/resume`、`thread/read` 的 schema 在此作为协议依据备查。
- **裁决动作**（Phase 1，未实现）：`serverRequest/resolved`（Accept/Reject）、`turn/interrupt`（Stop）。实现时按本 schema 校验消息，规格见 `docs/action-spec.md` §4。
