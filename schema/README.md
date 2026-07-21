# Schema

外部协议 schema 钉版本目录。Schema 进 git 防协议漂移。

## codex/

```bash
codex app-server generate-json-schema --out ./schema/codex
```

Phase 1 时执行，结果 commit 进来。CodexAdapter 实现时按这里的 schema 校验消息。

## zcode/

ZCode 用 ACP（Agent Client Protocol）+ 自定义扩展。
schema 来源：`/Applications/ZCode.app/Contents/Resources/app.asar` 解包后的 `node_modules/@agentclientprotocol/sdk/schema/schema.json`。

V1.1 动作层实施时一并抽取。
