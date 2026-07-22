# 产品定位

## 一句话

> Agent Deck 是一块物理"状态牌"，被动观察 ZCode / Codex 会话状态，状态变化时提醒，关键时刻介入。

## 不是什么

- ❌ 不是新会话入口（你仍然在 ZCode Desktop / Codex 里跑任务）
- ❌ 不是新工作流
- ❌ 不是 IDE 替代品
- ❌ 不是 Stream Deck（不做通用宏）

## 是什么

```
你在 ZCode Desktop / Codex 里跑长任务
        ↓ 去干别的（不盯屏）
任务进 waiting / done / error
        ↓
deck 灯变绿/蓝/红  + 可选提示音/语音
        ↓ 你瞥一眼
按 Accept / Reject / Stop（或语音「批准」）→ 回到工作
```

## 核心体验

1. **扫视 0.5 秒能知道谁在等、厉不厉害**（颜色 + 闪速 + urgency 渐变）
2. **不离开主键盘就能裁决**（按一个键，或语音「批准」）
3. **同一套状态语义，ZCode / Codex 通用**

## 关键设计取舍

| 取舍 | 选择 | 理由 |
|---|---|---|
| 会话归属 | **观察 Desktop，不开新会话** | 嵌入现有工作流，不另起炉灶 |
| 后端 | **ZCode + Codex** | Claude CLI hook 不可异步阻塞，等 Claude Desktop |
| 平台 | **macOS 优先** | 主力 ZCode Desktop 在 Mac；Win 在 V3 加 |
| 槽位数 | **物理 5 / 软件默认 8** | 物理 5 是 100×100mm MX 间距上限（A1-A5）；软件槽位不受 PCB 限制，desktop 默认 8 |
| 协议 | **USB CDC JSON Lines** | 调试友好，固件好实现 |

## 不做（V1）

- 开新会话 / 独立 spawn
- 桥接 ZCode Desktop 私有 IPC
- Claude Code 适配
- 蓝牙/电池（PCB 预留焊盘 DNP）
- Windows
- 完整 12 键 + 摇杆宏（V1 只焊 9 元件最小集）
- 语音（V4）

## 长期愿景

做成开源的 "Agent 操作台协议"——硬件只是第一个 client。模拟盘、菜单栏、手机、Stream Deck 都是同协议的 client。ZCode/Codex/Claude/Future 各家都适配成同一状态机。

社区给 Cursor、Aider、自家 agent 写 adapter，就赢了。

Codex Micro 卖 $230 封闭灯效；Agent Deck 卖的是 agent 操作台的 USB/HID 开放标准。

具体设想与分级见 [roadmap.md](./roadmap.md)。原则：**做要克制（比 Codex 强一点就够），想都记下**。
