# 0001 — 软件 Agent 操作台为产品主轴，硬件退为可选 client

Status: accepted (2026-07-23)

## 决策

Agent Deck 的产品主轴从最初的"被动观察的物理状态牌"转为 **macOS 软件 Agent 操作台**：桌面软件 app 是一等公民 client，硬件状态牌降为可选 client 之一。产品主语不再以"物理牌"开头。

## 背景

`docs/product.md` 原定位是"物理状态牌，被动观察 ZCode/Codex 会话"，并明列红线"❌ 不是新会话入口 / ❌ 不做通用宏 / 观察会话不开新会话"。但随后三个 feat 已让代码超出该定位：

- `open_slot_session`（commit `b087cc5`）：从 deck 一键跳进已有 ZCode/Codex 会话（ZCode 走 second-instance 绕过信任弹窗，Codex 走 osascript）
- `pin` + `catalog` + `bind picker`（`df99266` / `b087cc5`）：把历史会话手动钉到固定槽并持久化
- 虚拟键盘 UI + liquid-glass 渲染（`5cdbbf2` / `b087cc5`）：把"物理牌"软件化

这些能力组合起来已构成"可跳转、可绑定、可派发动作的软件操作台"，而非"被动观察牌"。同时 `docs/roadmap.md` 写下的近期优先级（tmux client / theme 配置化 / CI-CD adapter）与实际开发方向几乎不重叠。定位文档与代码出现结构性分歧。

## Considered Options

1. **认操作台为新主轴（采纳）** — 承认代码已领先，把文档回写匹配实际方向。硬件不废，但不再是主语。
2. 拉回被动观察牌 — 删除/弱化 open_session/bind/虚拟键盘，回归 roadmap 原优先级。代价：丢弃已落地的差异化能力。
3. 先不定边界，悬置 — 风险：文档持续漂移，新人困惑。

选 1。理由：操作台方向是产品真正的差异化（相对 Codex Micro 纯灯效），且代码已沉淀；回拉成本高于文档回写成本。

## Consequences

- `docs/product.md` 主语、取舍表、"不做"清单需重写匹配（已在本批次完成）。
- `docs/roadmap.md` 近期优先级重排：裁决动作 spec/探测提前，tmux/CI-CD adapter 后移（已在本批次完成）。
- 红线"不开新会话"保留并细化：**只允许跳转聚焦已有会话，绝不 spawn 新会话**——这把"不是新会话入口"从绝对否定改为边界明确的有限放行。
- 红线"不做通用宏"重新定义：**裁决类 Action（Accept/Reject/Stop 等）合法，任意用户自定义宏仍不做**。见 `docs/action-spec.md`。
- 裁决动作（原 roadmap E 节 "V1 unsupported"）升级为近期项，但本批次仅产出 spec，不实现代码。
- 硬件 V2（PCB/固件）保持"未实施"，不因此废弃；只是不再作为产品主语出现在定位文档里。
