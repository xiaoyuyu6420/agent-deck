# USB / WebSocket 协议

`packages/protocol/src/index.ts` 是 host / simulator / 固件 三方共用的**唯一真相源**。

## 传输层

### Host ↔ Simulator（WebSocket）

- URL: `ws://127.0.0.1:8787`
- 编码: JSON 文本帧
- 重连: client 1.5s 自动重连

### Host ↔ Device（USB CDC，V2）

- USB CDC ACM 虚拟串口
-波特率: 115200（CDC 实际不限）
- 编码: JSON Lines（一行一个 JSON，UTF-8，换行 `\n`）

## 消息格式

### Server → Client（host → simulator/device）

#### `leds` — 推灯帧

每次 Board 状态变化时广播。

```json
{
  "type": "leds",
  "slots": [
    { "i": 0, "rgb": [255, 109, 0], "br": 255, "fx": "blink_fast" },
    { "i": 1, "rgb": [48, 79, 254], "br": 180, "fx": "breathe" },
    { "i": 2, "rgb": null, "br": 0, "fx": "solid" }
  ]
}
```

字段：
- `i`: 槽位索引 0..N-1
- `rgb`: `[r,g,b]` 0-255，或 `null` 关灯
- `br`: 亮度 0-255
- `fx`: `solid` | `breathe` | `blink_slow` | `blink_fast`

#### `board` — 推 board state（UI 文字用）

```json
{
  "type": "board",
  "slots": [
    {
      "i": 0,
      "backend": "zcode",
      "sessionId": "sess_xxx",
      "title": "实现登录",
      "status": "waiting",
      "detail": "Bash: git push",
      "focused": true
    }
  ],
  "focus": 0,
  "mode": "act"
}
```

#### `focus` — 改变焦点（可选，simulator 也可从 board 推断）

```json
{ "type": "focus", "i": 2 }
```

#### `event` — 通用事件

```json
{ "type": "event", "event": "error", "data": "invalid json" }
```

### Client → Host（simulator/device → host）

#### `action` — 触发动作

```json
{ "t": "action", "action": { "op": "accept" } }
{ "t": "action", "action": { "op": "focus", "i": 2 } }
{ "t": "action", "action": { "op": "stop_all" } }
{ "t": "action", "action": { "op": "set_mode", "mode": "plan" } }
{ "t": "action", "action": { "op": "send", "i": 1, "text": "继续" } }
```

Action op 枚举：
- `focus` — 切焦点槽
- `accept` / `reject` / `stop` — 当前焦点或指定槽
- `stop_all` — 停所有
- `freeze_all` / `unfreeze` — 急停总闸
- `set_mode` — plan / act / review
- `send` — 发文本到 session

#### `key` — 物理按键事件（设备用）

```json
{ "t": "key", "id": "a1", "edge": "down" }
{ "t": "key", "id": "accept", "edge": "down", "fn": false }
```

key id 命名：
- `a1`..`a5` — 状态键
- `accept` / `reject` / `stop` / `new` / `ptt` — 操作键
- `fn` / `mode` — 功能键

edge: `down`（按下）| `up`（松开）

#### `enc` — 旋钮

```json
{ "t": "enc", "delta": 1 }
{ "t": "enc", "delta": -1 }
```

#### `joy` — 摇杆

```json
{ "t": "joy", "dir": "up", "edge": "down" }
```

dir: `up` | `down` | `left` | `right` | `center`

#### `voice` — 语音

```json
{ "t": "voice", "op": "ptt", "edge": "down" }
```

## Host 行为契约

- 每次 Board 状态变化，host **同时**广播 `leds` + `board`
- 槽位索引必须连续 0..N-1
- 空槽也要发 `{i, rgb:null, br:0, fx:'solid'}`
- 不发差异帧，每次都是完整快照（小数据量 OK）

## Client 行为契约

- client 收到 `leds` 应立即应用灯效
- 重连后 host 会重新广播当前状态
- client 发的 action 异步，host 不一定回复（成功/失败通过 `event` 通知）
