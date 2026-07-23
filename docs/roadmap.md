# Roadmap / Ideas Backlog

> 这是一份**设想清单，不是承诺**。原则：**做要克制（比 Codex 强一点就够），想都记下（不丢失点子）**。
>
> 分级只表达"值不值得近期做"，不代表排期。任何一项动手前都要重新评估工作量与定位。

## 分级约定

| 标记 | 含义 |
|---|---|
| 🟢 | **比 Codex 强一点**：近期值得做，工作量可控，差异化卖点 |
| 🟡 | **中期**：有人手或社区有人接才做 |
| 🔵 | **远期 / 脑洞**：只存档，不承诺，避免丢失 |
| ⛔ | **不做 V1**：`product.md` 已排除，此处仅存档理由 |
| ✅ | **已完成**：已落地，此处仅存档 |

判断 🟢 的三条硬指标：① 拉开与 Codex Micro 的差距；② 工作量小（多数是配置化 / 挪代码，不是新模块）；③ 能写进 README 宣发。

---

## A. Client（同一套协议的不同终端）

协议已经是 client-agnostic（见 `protocol.md`），软件 app 是第一个 client，硬件只是其中一种。下面都是"讲同一套 `leds`/`action` 的终端"。

| Client | 标记 | 说明 |
|---|---|---|
| **桌面软件 app（virtual keyboard + liquid-glass）** | ✅ | **已作为首个 client 落地**：虚拟键盘 UI、catalog/bind/pin、跳转打开会话（Codex deep link 直达 thread / ZCode 到 workspace）、liquid-glass 渲染。是产品主形态（见 ADR 0001）。下一步在此基础上接裁决动作。 |
| tmux / wezterm status line | 🟢 | 把槽位渲染进 `status-right`，零硬件、终端党命中率高，社区冷启动关键 |
| 菜单栏（macOS `NSStatusItem`） | 🟡 | 格 menu bar icon，点开下拉即 board，"穷人版 deck" |
| 手机 / PWA（连 `ws://127.0.0.1:8787`） | 🟡 | 离开电脑也能收 waiting 推送；iOS 快捷指令 / Android Tasker 都行 |
| 物理硬件状态牌 | 🟡 | V2 PCB/固件未实施（见 `bom.md`/`pcb.md`），现为可选 client，非主语 |
| Stream Deck 官方插件 | 🔵 | 直接吃现有 Stream Deck 用户群 |
| 环境灯（Hue / LIFX / 米家） | 🔵 | waiting 时整屋变橙，把"牌"升级成"氛围" |
| Apple Watch glance | 🔵 | 瞄手腕 |

> 软件操作台已落地，tmux status 现定位为"终端党专用轻量 client"而非社区冷启动唯一路径（软件 app 已承担该角色）。

## B. Backend Adapter（生态护城河）

`zcode` / `codex` 两个 backend 已独立并接入（见 `architecture.md`）。Adapter 越多，协议越值钱。

| Adapter | 标记 | 说明 |
|---|---|---|
| **ZCode（sqlite observer）** | ✅ | 已落地：双库只读 + ATTACH，poll/catalog/poll_pinned 全通 |
| **Codex（app-server observer）** | ✅ | 已落地：`app-server --listen stdio://` 子进程，poll/catalog/poll_pinned 全通（codex-cli 0.145.0-alpha.27 验证）；**会话级跳转也已落地**（`codex://threads/<threadId>` deep link，点按键直达 ChatGPT.app 目标 thread，见 codex-integration.md） |
| CI/CD（GitHub Actions webhook） | 🟢 | queued/running/success/failure 直接映射 board——**把定位从"AI agent 状态牌"放宽到"任何长跑任务状态牌"，使用场景放大一个量级，且不违背现有设计** |
| 本地长任务（make / cargo / 训练 / 下载） | 🟢 | 同上，CLI wrapper 即可接 |
| Aider / Cline / Continue | 🟡 | 开源好接，是 AI coding adapter 的下一站 |
| Claude Code | 🔵 | 要扒 `~/.claude`，`product.md` 暂排 |
| 自家 agent SDK | 🟡 | 给个 `adapter.md` + stdin/stdout 最小规范，让人接自己的脚本 |
| 系统/运维事件（备份、磁盘、证书） | 🔵 | 非核心，远期 |

> 🟢 两条是定位上的一次质变机会：状态牌不一定只服务 AI，任何"不想盯又不能错过"的长任务都算。

## C. Theme / 多模态反馈

`theme` 是纯函数 `paint(...) → {rgb, br, fx}`（见 `status-model.md`），是和 Codex Micro 拉开差距的核心。**liquid-glass 渲染已在软件 app 落地**（发光 LED 渐变），配置化是下一步。

| 项 | 标记 | 说明 |
|---|---|---|
| liquid-glass 渲染（软件 app） | ✅ | 已落地：毛玻璃 + LED radial-gradient 发光，见 `apps/desktop/src/style.css` |
| Theme 配置化（`~/.agent-deck/themes/*.toml`） | 🟢 | 现在 `CODEX_THEME` 硬编码在 `crates/board/src/theme.rs`，挪到配置文件 = 即时差异化卖点，工作量小 |
| 风险规则可配置（`~/.agent-deck/rules.toml`） | 🟢 | `status-model.md` 的风险推断表挪到配置，用户自定义哪些命令算 high |
| 协议加 `audio` / `haptic` / `notification` 消息类型 | 🟡 | 把 theme 从"灯"抽象成"多模态反馈通道"，是后面所有非灯 client 的地基，**尽早定进 `protocol`，晚改代价大** |
| 声音主题（每状态/风险配音色） | 🟡 | 依赖上一条 |
| 自定义 risk→反馈映射 | 🟡 | 全交给 theme |

> 🟢 两条（theme + 风险规则配置化）是"代码挪到配置"的典型低投入高产出，建议绑在一起做。

## D. 风险模型

风险 + urgency 渐变是现有真正的创新（Codex Micro 没有）。

| 项 | 标记 | 说明 |
|---|---|---|
| 风险规则可配置 | 🟢 | 见 C 节，同一件事的两个面 |
| 风险源扩展（外发网络、覆盖受保护文件、API 花费、sudo） | 🟡 | 规则配置化之后再扩源 |
| 风险→动作联动（high 自动 `Freeze`） | 🟡 | 让 deck 从"通知器"变"护栏" |
| 学习型风险（按历史 accept/reject 拟合） | 🔵 | 亮点，但工作量大 |

## E. Action（裁决工具箱）

`accept/reject/stop/freeze/set_mode/send` 协议已留好（见 `protocol.md`），`Pin`/`Focus` 已实现，其余 V1 返回 unsupported。**完整规格见 [action-spec.md](./action-spec.md)。**

| 项 | 标记 | 说明 |
|---|---|---|
| **Pin / Focus / Bind / Catalog** | ✅ | 已落地：手动钉会话到槽、持久化、catalog 选择器、跳转打开会话 |
| **Phase 0：ZCode `session/load` attach 探测** | 🟢 | 决定 ZCode 侧裁决能否实现，一天能出，**阻塞 Phase 2**。见 action-spec.md §5.1 |
| **Phase 1：Codex 裁决实现（Accept/Reject/Stop）** | 🟢 | 通道已就绪（app-server stdio），实现成本低。见 action-spec.md §4 |
| Phase 2：ZCode 裁决（视 Phase 0） | 🟡 | 阻塞于 Phase 0 探测结果 |
| Phase 3：FreezeAll/Unfreeze/SetMode + 全局热键 | 🟡 | Board 本地动作 + 全局热键（`⌘⇧A` accept 等），不切窗裁决 |
| 宏动作（接受+跑测试、拒绝+让 agent 解释） | ⛔ | product.md 已排：不做通用宏，只做有限裁决 Action |
| 双人审批流（high risk push 到队友手机） | 🔵 | 团队共用 agent 场景 |
| 自然语言语音扩展（"接受所有低风险"） | ⛔ | product.md 已排到 V4 |

## F. 硬件变体

开源硬件（KiCad + JLCPCB 友好，最小 9 元件，见 `bom.md` / `pcb.md`），license MIT + CERN-OHL-S。**V2 未实施，硬件现为可选 client（ADR 0001）。**

| 变体 | 标记 | 说明 |
|---|---|---|
| 无线化 BLE（PCB 已预留焊盘） | 🟡 | 桌面零线材 |
| 主控换 ESP32-S3（带 WiFi，走 WebSocket） | 🟡 | 省 USB 线 |
| 矩阵灯板（无键，纯 LED）/ OLED 屏款 | 🔵 | 形态变体 |
| 旋转编码器 / 触摸滑条 / 脚踏板 | 🔵 | 输入扩展，脚踏板可脚踩 Accept |
| 分体 / 群组（一 backend 一块灯） | 🔵 | "agent 机柜" |
| 主控换 CH32V003（¥3 极致便宜版） | 🔵 | 批量送朋友的极简款 |

---

## 近期优先级建议

按"投入产出比 × 拉开差距 × 壮大社区"三维度，🟢 里值得先动的（软件操作台已落地后的新排序）：

1. **裁决动作 Phase 0 + Phase 1**（E）—— **兑现"不离开主键盘就能裁决"的产品核心承诺**，这是操作台相对纯观察牌的最大分化点。Phase 0（ZCode 探测）一天出结论，Phase 1（Codex 裁决）通道已就绪。见 [action-spec.md](./action-spec.md)。
2. **theme + 风险规则配置化**（C + D）—— 代码挪配置，工作量小，差异化卖点可宣发，与裁决并行不冲突。
3. **CI/CD + 本地长任务 adapter**（B）—— 定位从"AI 状态牌"放宽到"长任务状态牌"，使用场景放大一个量级。
4. **tmux status client**（A）—— 终端党专用轻量 client，社区冷启动（软件 app 已分担该角色，优先级后移）。

`protocol` 加 `audio`/`haptic`/`notification` 消息类型（C）建议尽早定草案，哪怕不实现——它是后续多模态 client 的地基，晚改代价大。
