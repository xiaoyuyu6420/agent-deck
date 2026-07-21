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

## done TTL

完成的任务保留绿色 5 分钟（`DONE_TTL_MS = 5 * 60 * 1000`），超时降为 idle/off。避免久远的 done 占槽。

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

完整实现见 `packages/host/src/board/theme.ts`。
