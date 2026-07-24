# UI 端到端测试（Accessibility API + WKWebView）

驱动**真实 macOS WKWebView** 的端到端测试，覆盖 `用户操作 → IPC → Rust 后端 → board 状态 → UI 重绘` 整条链路。

> 定位：本地 macOS 回归。**不进 CI**（CI 跑在 ubuntu，无 WebView；AX API 也是 macOS 专有）。

## 为什么不用 WebDriver / Playwright

- **tauri-driver v2 不支持 macOS**：源码平台门控 `#[cfg(any(target_os="linux", windows))]`，macOS 直接报 `not supported on this platform`。Tauri v2 官方文档亦注明「macOS provides no desktop WebDriver client」。
- **safaridriver 只能驱动独立 Safari**，驱动不了嵌入 Tauri app 的 WKWebView。
- **Playwright WebKit** 驱动的是它自带的 WebKit，不是 app 内的 WKWebView，测不到真实 IPC + 窗口行为。
- **Accessibility API 是 macOS 原生路径**：WKWebView 会把 DOM 树暴露为 AX 树，每个元素带 `AXDOMIdentifier`（= DOM id）和 `AXDOMClassList`（= class），可用 CSS 风格选择器定位；`AXPress` 触发真实点击。这是 macOS 上驱动嵌入 webview 的可行方案。

## 技术栈

| 层 | 选择 | 理由 |
|---|---|---|
| 驱动协议 | macOS Accessibility API | 唯一能驱动 app 内 WKWebView 的原生路径 |
| 驱动器 | `ax_driver.swift`（本 crate 自带） | AX API 在 ApplicationServices C framework，swift 是无依赖的惯用桥 |
| 测试客户端 | Rust（同 crate） | 全栈一致，与 fixture 惯例统一 |
| 通信 | JSON-RPC over stdin/stdout | Rust 起 swift 子进程，逐行收发 JSON |
| 运行环境 | 本地 macOS | WKWebView 是发布引擎 |

## 前置依赖

```bash
# 1. 构建 release app（生成本 e2e 默认查找的 .app 产物）
pnpm build:desktop
# 产物：target/release/bundle/macos/Agent Deck.app

# 2. Xcode/swift 工具链（swift 命令可用）—— macOS 通常自带
swift --version

# 3. 辅助功能授权（一次性）
#    系统设置 → 隐私与安全性 → 辅助功能 → 允许运行 e2e 的终端/iTerm
```

## 运行

```bash
pnpm test:ui
# 或：cargo run -p agent-deck-desktop-e2e
```

`test:ui` 会自动：
1. 创建隔离的临时 SQLite fixture（仿 zcode schema）
2. 设置 `AGENT_DECK_TASKS_DB` / `AGENT_DECK_TOOL_DB` 指向它
3. 用 `open -n -a <app>` 启动 app（携带隔离 env）
4. spawn `ax_driver.swift`，Rust 通过 JSON-RPC 驱动 AX 树

## 数据隔离

测试**绝不触碰真实 `~/.zcode`**。`apps/desktop/src-tauri/src/lib.rs` 的 `default_config()` 优先读 `AGENT_DECK_TASKS_DB` / `AGENT_DECK_TOOL_DB`，未设时才 fallback 到 `~/.zcode/v2/...`。

## 覆盖的冒烟用例

| # | 用例 | 验证的链路 |
|---|---|---|
| 1 | app 启动，键盘视图渲染 ≥1 个 key slot | webview 启动 + 首次 paint + board 初始化 |
| 2 | 设置按钮打开面板，auto-fill 控件可见 | 视图切换 + DOM 重建 |
| 3 | 翻转 auto-fill，AXValue 状态翻转 | UI 点击 → change handler → 重绘 |
| 4 | 点击 key，应用不崩 | 交互路径不崩 |

> 用例 3 只断言 **UI 状态**翻转。后端持久化（`set_auto_fill`）由命令层测试 `crates/host-core/tests/e2e_desktop_service.rs` 单独覆盖，UI e2e 不重复测后端。

## 选择器约定

AX 树把 DOM 映射为：`<button>`→`AXButton`、`<input type=checkbox>`→`AXCheckBox`、`<div>`→`AXGroup`、文本→`AXStaticText`。`ax_driver.swift` 的 `matches()` 支持类 CSS 写法：`button#btn-settings`、`input#auto-fill`、`[aria-label=key-0]`、`[aria-label=key-*]`（前缀通配）。

> **WKWebView 的 AX 限制**：普通 `<div>` 的 `class` 不进 AX 树（只有 button/input 的 id 进 `AXDOMIdentifier`）。所以键盘的 key 元素加了 `role="button"` + `aria-label="key-<i>"`（`apps/desktop/src/main.ts`）——这让它变成可点击的 AXButton，且 aria-label 经 `AXDescription` 暴露成稳定定位锚。这同时改善了真实无障碍体验，对用户零负面影响。

## 环境变量

| 变量 | 作用 | 默认 |
|---|---|---|
| `AGENT_DECK_APP` | 指向自定义 `.app` 路径 | `target/release/bundle/macos/Agent Deck.app` |
| `AGENT_DECK_TASKS_DB` | 覆盖 tasks 数据库路径（e2e 用） | `~/.zcode/v2/tasks-index.sqlite` |
| `AGENT_DECK_TOOL_DB` | 覆盖 tool 数据库路径（e2e 用） | `~/.zcode/cli/db/db.sqlite` |

## 与现有测试体系的关系

| 测试 | 跑在哪 | 覆盖 |
|---|---|---|
| `pnpm test:rust` | CI（ubuntu） | Rust 单元 + 命令层 e2e，不含 desktop crate、不含本 e2e |
| `pnpm test:e2e` | CI（ubuntu） | host-core 的 IPC 命令层 e2e（fixture sqlite） |
| `pnpm test:ui`（本文） | **本地 macOS** | 真实 WKWebView + UI + IPC + 后端 |
| `cargo test -- --ignored` | 本地 | 连真实 `~/.zcode` 的真机回归 |

## 已知限制

- **仅 macOS**：依赖 Accessibility API。Windows/Linux 需另选方案（WebView2 automation / WebKitGTK）。
- **DOM 重建**：前端用 `innerHTML` 每帧重绘，AX 句柄会失效。driver 每次操作都重新 `freshWebArea()` 拿最新快照，不缓存元素引用。
- **AX 授权时序**：app 刚启动时 AX 树可能未就绪；harness 启动后 sleep 4s 并用 `wait` 轮询。
- **窗口特性**：app 是 `alwaysOnTop` + 透明无边框，不影响 AX 定位（AX 不依赖视觉）。

## 故障排查

- **`failed to spawn swift`** → 装工具链：`xcode-select --install`
- **`timeout waiting for`** → AX 未授权，或 app 未启动。确认辅助功能已授权当前终端；确认 `pnpm build:desktop` 产物存在。
- **`no webview`** → app 启动失败。单独 `open` app 看能否正常显示。
- **AX 树只到 AXWindow** → app 刚启动 AX 未就绪，重试；或 webview 内容未加载完。
