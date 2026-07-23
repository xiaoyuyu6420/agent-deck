# 状态模型与灯效

## Deck 状态枚举

与 Codex Micro 官方状态机对齐。

| 状态 | 含义 | 优先级（抢位） |
|---|---|---|
| `off` | 未绑定槽位 | 0 |
| `idle` | 已绑定但空闲 | 1 |
| `done` | 任务完成 | 2 |
| `working` | 在跑 | 3 |
| `error` | 错误 | 4 |
| `waiting` | 等用户确认/输入 | 5 |

抢位规则：`waiting > error > working > done(recent) > idle > off`

## 基础色表（CODEX_THEME）

| 状态 | HEX | RGB |
|---|---|---|
| off | `#000000` | — |
| idle | `#FFFFFF` | [255,255,255] |
| working | `#304FFE` | [48, 79, 254] |
| waiting | `#FF6D00` | [255, 109, 0] |
| done | `#00FF4C` | [0, 255, 76] |
| error | `#FF0033` | [255, 0, 51] |

## urgency 渐变（关键创新）

waiting 状态不是固定色，随**等待时长 + 风险**渐变。

### 公式

```
ageSec = (now - waitingSince) / 1000
timeUrgency = clamp01(ageSec / 120)             // 2 分钟拉满
u = max(timeUrgency, RISK_BOOST[risk])

// 风险抬升（让高危命令立即显得急）
RISK_BOOST = { low: 0, medium: 0.25, high: 0.5 }
```

### 视觉编码

| u 值范围 | RGB | 亮度 | fx |
|---|---|---|---|
| 0.0 - 0.33 | `#FFB074` (浅橙) | 80-140 | solid |
| 0.33 - 0.66 | 中橙 | 140-200 | blink_slow |
| 0.66 - 1.0 | `#FF2200` (急红) | 200-255 | blink_fast |

具体颜色 = `lerpHex('#FFB074', '#FF2200', u)`
具体亮度 = `lerp(80, 255, u)`

## 风险等级推断

从 ZCode tool_usage.detail 字段推断：

| 关键词 | risk |
|---|---|
| `shell` `Bash` `git push` `git reset` `rm ` `delete` `destroy` | **high** |
| `fileWrite` `fileEdit` `Edit` `Write` `file` | **medium** |
| `userInteraction` `AskUser` `read` `Grep` `Glob` | **low** |
| 其他 / null | **medium**（保守） |

## 灯效（fx）

| fx | 行为 | 用途 |
|---|---|---|
| `solid` | 常亮 | idle/done/error/waiting+低urgency |
| `breathe` | 呼吸（亮暗循环，2s 周期） | working |
| `blink_slow` | 慢闪（0.8s 周期） | waiting+中urgency |
| `blink_fast` | 快闪（0.3s 周期） | waiting+高urgency |

## working 长跑提示

working 超过 5 分钟偏紫：

```
longRun = clamp01(ageSec / 300)
rgb = lerpHex('#304FFE', '#7B1FA2', longRun)  // 蓝→紫
```

含义："还在啃，可能卡住了"。

## done TTL（open-aware）

Done → Idle **不再**是「完成起算 5 分钟一律衰减」。对 **WorkBuddy**，host 层按「是否在 Agent Deck 点开」衰减：

| 场景 | 行为 | 默认常量 / 配置 |
|---|---|---|
| Done 后**从未点键** | 保持 Done | 最长 `DONE_TTL_UNOPENED_MS = 12h`（`doneTtlUnopenedMs`）后强制 Idle |
| Done 后**点过键** | 从点键时刻起倒计时 | `DONE_TTL_MS = 5min`（`doneTtlAfterOpenMs`）后 Idle |
| ZCode / Codex | 本轮不改 | mapper 原语义 |

要点：

- 「点开」= Agent Deck **点击该键**（`open_slot_session`），不依赖后端 App 是否唤起成功。
- 短 TTL 从 **点开时刻**起算，不是完成时刻；完成很久后才点开 → 再亮短 TTL。
- 配置写入 `~/.agent-deck/settings.json`，设置页可改。
- WorkBuddy mapper 只报告「完成 = Done」；衰减在 host-core 的 `decay_done_status`。

## 色弱友好

不只用颜色区分：
- **颜色**：基础语义
- **fx**：紧急度第二通道（solid/slow/fast）
- **亮度**：紧急度第三通道

三通道叠加，单色弱也能识别。

## theme.ts 公式参考

```ts
paint({ status, risk, waitingSince, now }, palette = CODEX_THEME) → { rgb, br, fx }
```

完整实现见 `crates/board/src/theme.rs`。
