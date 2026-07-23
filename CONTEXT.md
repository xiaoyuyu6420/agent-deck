# Agent Deck

Agent Deck 是一个 macOS 软件 Agent 操作台：观察多个 AI agent（ZCode / Codex）的会话状态，在关键时刻让用户不离开主键盘就能跳转、绑定、裁决。硬件状态牌是其可选 client 之一，不是产品主语。

> 本文件是**术语表（glossary）**，只定义概念"是什么"，不含实现细节。实现决策见 `docs/adr/`，规格见 `docs/` 下各文档。

## Language — 协议层

**Backend**：
Agent Deck 观察的外部 agent 产品。当前两个：`Zcode`、`Codex`。
_Avoid_: 后端、provider、agent（"agent"指会话的执行者，不是被观察的产品本身）

**Session**：
一个 Backend 上的一次任务实例（ZCode 的 task / Codex 的 thread）。协议一等类型 `SessionSnapshot`：backend、session_id、status、risk、detail、waiting_since、updated_at、workspace_path。
_Avoid_: 会话（口语可，正式用 Session）、task、thread（这些是各 Backend 的原始词，不跨 Backend 使用）

**DeckStatus**：
Session 的六态枚举，跨 Backend 统一：`Off / Idle / Working / Waiting / Done / Error`。有 `priority()` 全序用于抢位排序（Waiting > Error > Working > Done > Idle > Off）。
_Avoid_: state、状态值（口语可，正式用 DeckStatus）

**Risk**：
对某个 waiting Session 的副作用烈度推断，三档 `Low / Medium / High`，给 urgency 加 boost。推断规则见 `docs/status-model.md`。
_Avoid_: 危险等级、severity

**Urgency**：
一个 Session "多该被注意"的连续值，由 waiting_since 时长 + risk boost 合成，驱动 LED 闪烁渐变。全等待阈值 `URGENCY_FULL_WAIT_MS = 2min`。
_Avoid_: 紧急度（口语可，正式用 Urgency）

**Slot**：
Board 上一个固定位置，按 `i: usize` 编号。物理 5 个（A1-A5，PCB 上限），软件默认 8 个（`SLOT_COUNT`）。承载一个 Session 的当前快照 + LED 表现。
_Avoid_: 槽（口语可）、键、key

**Board**：
所有 Slot 的总表（`SessionBoard`），按 priority + urgency 抢位分配 Session 到 Slot，产出 `LedFrame`（灯光）与 `BoardState`（UI 绑定）。
_Avoid_: 面板、dashboard

**Observer**：
被动只读的数据采集层（`BackendObserver` trait），把某 Backend 的会话拉成 `Vec<SessionSnapshot>`。当前两实现：`SqliteObserver`（ZCode，读 sqlite）、`CodexObserver`（Codex，app-server JSON-RPC）。
_Avoid_: adapter（adapter 更偏"协议转换"，observer 强调"被动观察不写回"）、source

**LedFrame**：
Board 产出的灯光帧（`Vec<LedSlot>`，每槽 `rgb/br/fx`），供硬件 client 或虚拟键盘渲染。
_Avoid_: 灯效数据

## Language — 操作台交互层

**Pin**：
把某个 Session 手动钉到固定 Slot `i` 的底层动作（`Action::Pin`）。被 Pin 的 Session **survives recompute**（不参与自动抢位重排）、**Done TTL 不清**（不会像普通完成态那样 5 分钟后消失）、持久化到 `~/.agent-deck/pins.json`。`session_id = None` 表示解钉。
_Avoid_: 固定、锁定、锁定槽、lock

**Catalog**：
全量历史会话的数据源，区别于 Observer `poll()` 的"近 20 条活跃窗口"。给 Bind picker 用，ZCode 侧取全量 tasks（500）、Codex 侧取长窗口 threads（200/90 天）。
_Avoid_: 列表、历史记录、sessions

**Bind**：
用户交互流程：从 Catalog 选一个历史 Session，Pin 到某 Slot。**底层动作是 Pin，Catalog 是数据源，Bind 是 UI 流程**——三者是同一件事的三个层面，不是独立功能。
_Avoid_: 绑定（口语可，正式文档用 Bind）、关联

**Virtual keyboard**：
桌面软件 app 用屏幕上的虚拟按键模拟物理 deck 形态的软件 client。是当前主 client（硬件 V2 未实施）。
_Avoid_: 软键盘、模拟器

**Liquid-glass**：
桌面软件 app 的渲染层视觉风格（毛玻璃半透明 + LED 发光渐变）。纯 UI 概念，与协议层无关。
_Avoid_: 玻璃态、毛玻璃（口语可，正式用 Liquid-glass）

## Language — 动作层

**Action**：
用户对 Session/Board 发起的裁决或控制操作，协议枚举 10 种（`Focus / Accept / Reject / Stop / StopAll / FreezeAll / Unfreeze / SetMode / Send / Pin`）。见 `docs/action-spec.md`。
_Avoid_: 命令、command、宏（Action 是有限的裁决动作集合，不是通用宏）

**Dispatch**：
把一个 Action 交给对应 Backend 的 Observer 执行（trait 方法 `dispatch(action)`）。读写共用同一 Observer 连接，不单开 ActionRouter。见 `docs/action-spec.md`。
_Avoid_: 路由、转发
