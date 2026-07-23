# Phase 0 手动验证

> ⚠️ **本文档已过时**（2026-07-23）：描述的 `pnpm verify` 命令基于已删除的 legacy TypeScript host 实现（`packages/host`），该命令已不存在。
>
> 当前的验证方式：
> - 自动化回归：`pnpm test:rust`（含 host-core 全部 E2E）
> - 手动观察：`pnpm dev:desktop` 启动 Tauri 桌面应用，观察真实 `~/.zcode` 状态
>
> 下文保留作历史参考，命令不再可用。

`pnpm verify` 起一份连真实 `~/.zcode` 的 host，把每次 board 状态变化彩色打印到终端，
用来肉眼对照 Phase 0 验收 Demo（方案 §12）的链路是否真的通。

它和 `pnpm dev` 的区别：

| 维度 | `pnpm dev` | `pnpm verify` |
|---|---|---|
| 数据源 | `~/.zcode`（真实） | `~/.zcode`（真实） |
| 端口 | 固定 8787 | 动态分配（不冲突） |
| 输出 | 起 ink simulator TUI | 直接彩色打印到 stdout |
| 自动防自指 | ❌（需手配 config） | ✅（自动 exclude 当前 cwd + repo 根） |
| 用途 | 日常使用 | 一次性验证 / 调试 |

## 使用

```bash
pnpm verify                          # 用真实 ~/.zcode
pnpm verify --port 9000              # 指定端口
pnpm verify --zcode-home /tmp/fake   # 指向其他路径
pnpm verify --raw                    # 额外打印 leds 帧 JSON
```

启动后会显示类似这样的输出：

```
agent-deck verify
  zcodeHome:        /Users/munich/.zcode
  gateway port:     49862
  excludeWorkspaces:
    - /Users/.../codex 键盘/packages/host
    - /Users/.../codex 键盘

── 8:26:03 PM ──  mode=act focus=A1
  A1  ████  waiting  zcode/sess_dd29b65  Kimi 风格 Token 动态涌出效果
         ↳ AskUserQuestion: userInteraction (focused)
  A2  ████  working  zcode/sess_b6f23ea  审查之前的 commit
  A3  — empty —
  A4  — empty —
  A5  — empty —
```

每个槽位一行，色块就是该槽当前的 RGB，肉眼直接判断颜色对不对。

## 验收清单（方案 §12 Phase 0）

`pnpm verify` 起着，对照下面跑一遍：

| 步骤 | 操作 | 预期 |
|---|---|---|
| 1 | 在 ZCode Desktop 跑一个新任务（不弹确认） | A 槽出现，色块蓝、fx `breathe` |
| 2 | 让任务执行一个需要确认的工具（Bash/Edit） | 同槽变橙，等 ~2min 后偏红、fx `blink_*` |
| 3 | 在 ZCode 点 Accept | 同槽变蓝（working） |
| 4 | 任务完成 | 同槽变绿（done），5min 后降为空 |
| 5 | 跑一个会失败的任务（例如让它 Bash 故意 exit 1） | 同槽变红、fx `solid` |
| 6 | 同时跑 6 个任务 | 只有前 5 个有槽位，第 6 个等位 |
| 7 | 把 verify 自己的开发会话放在 ZCode 里跑 | **不应**出现（被 exclude） |

每步对得上，Phase 0 就算通过。

## 颜色对照

| 状态 | RGB | 含义 |
|---|---|---|
| off / idle | 灰/暗白 | 空槽 / 已绑定但无任务 |
| working | `[48, 79, 254]` 蓝 | 在跑；>5min 偏紫 |
| waiting (低风险) | 浅橙渐深 | 等你确认；urgency 随时间拉满 |
| waiting (高风险，如 Bash) | 直接到中橙/红 | shell 类工具直接抬高 urgency 下限 |
| done | `[0, 255, 76]` 绿 | 完成 |
| error | `[255, 0, 51]` 红 | 出错 |

## 已知限制

- verify 不替代 e2e 测试（`pnpm test:e2e`）：它依赖人工触发 ZCode 状态，做不了回归。
- verify 自动 exclude 当前 cwd + repo 根；如果你在仓库外的目录跑了 ZCode 任务，
  需要在 `~/.agent-deck/config.json` 里手配 `excludeWorkspaces`。
- 当前未支持 Codex backend（Phase 1），verify 只观察 ZCode。
