# 产品定位

## 一句话

> Agent Deck 是一个 macOS 软件 **Agent 操作台**：观察多个 AI agent（ZCode / Codex）的会话状态，关键时刻让你不离开主键盘就能跳转、绑定、裁决。硬件状态牌是其可选 client 之一。

> 定位演进：本项目最初定位为"被动观察的物理状态牌"，随桌面端交互能力（跳转/绑定/派发动作）落地，已演进为"软件操作台"。决策记录见 [ADR 0001](./adr/0001-software-console-as-primary-axis.md)。

## 不是什么

- ❌ 不是新会话入口（只跳转聚焦已有会话，**绝不 spawn 新会话**）
- ❌ 不是通用宏工具箱（只提供有限的裁决 Action：Accept/Reject/Stop 等，不接受用户自定义宏）
- ❌ 不是 IDE 替代品（你仍在 ZCode Desktop / Codex 里跑任务）
- ❌ 不是 Stream Deck（不做任意按键映射）

## 是什么

```
你在 ZCode Desktop / Codex 里跑长任务
        ↓ 去干别的（不盯屏）
任务进 waiting / done / error
        ↓ deck 灯变绿/蓝/红 + urgency 渐变
        ↓ 你瞥一眼（扫视 0.5 秒知道谁在等、厉不厉害）
┌─────────────────────────────────────────┐
│ 软件操作台能做的三件事：                    │
│  1. 跳转：点槽 → 聚焦/打开对应后端会话      │
│  2. 绑定：从 Catalog 选历史会话 Pin 到槽   │
│  3. 裁决：Accept/Reject/Stop 不切窗        │
└─────────────────────────────────────────┘
        ↓ 回到工作
```

## 核心体验

1. **扫视 0.5 秒能知道谁在等、厉不厉害**（颜色 + 闪速 + urgency 渐变）
2. **不离开主键盘就能裁决**（按一个键 Accept/Reject/Stop，或语音「批准」——语音 V4）
3. **同一套状态语义，ZCode / Codex 通用**（DeckStatus 六态跨 Backend 统一）
4. **软件操作台为默认形态，硬件为可选增强**（没硬件也能用，有硬件是锦上添花）

## 关键设计取舍

| 取舍 | 选择 | 理由 |
|---|---|---|
| 主形态 | **macOS 软件 app（一等公民）** | 零硬件门槛，所有交互能力（跳转/绑定/裁决）先在软件落地 |
| 会话归属 | **只跳转/聚焦已有会话，绝不 spawn 新会话** | 嵌入现有工作流，不另起炉灶 |
| 动作范围 | **有限裁决 Action，不做通用宏** | 兑现"不离开主键盘就能裁决"，但不退化成宏工具箱。见 [action-spec.md](./action-spec.md) |
| 后端 | **ZCode + Codex** | Claude CLI hook 不可异步阻塞，等 Claude Desktop |
| 平台 | **macOS 优先** | 主力 ZCode Desktop 在 Mac；Win 在 V3 加 |
| 槽位数 | **物理 5 / 软件默认 8** | 物理 5 是 100×100mm MX 间距上限（A1-A5）；软件槽位不受 PCB 限制，desktop 默认 8 |
| 协议 | **USB CDC JSON Lines（硬件）/ Tauri event（软件）** | 调试友好，同一套 BoardState/LedFrame 两端通用 |

## 不做（V1）

- **开新会话 / 独立 spawn**（红线：只跳转聚焦，不开新）
- **任意用户自定义宏**（红线：只做有限裁决 Action）
- 桥接 ZCode Desktop 私有 IPC
- Claude Code 适配
- 蓝牙/电池（PCB 预留焊盘 DNP）
- Windows
- 完整 12 键 + 摇杆宏（V1 只焊 9 元件最小集）
- 语音（V4）

## 长期愿景

做成开源的 **"Agent 操作台协议"**——软件 app 是第一个 client，硬件状态牌、菜单栏、手机、Stream Deck 都是同协议的 client。ZCode/Codex/Claude/Future 各家都适配成同一状态机。

社区给 Cursor、Aider、自家 agent 写 adapter，就赢了。

Codex Micro 卖 $230 封闭灯效；Agent Deck 卖的是 agent 操作台的开放标准（软件免费 + 硬件开源）。

具体设想与分级见 [roadmap.md](./roadmap.md)。原则：**做要克制（比 Codex 强一点就够），想都记下**。术语定义见根目录 [CONTEXT.md](../CONTEXT.md)。
