//! Codex 桌面 GUI 的实时 thread 状态观察器（via `~/.codex/ipc/ipc.sock`）。
//!
//! ## 为什么需要这个模块
//!
//! 独立 spawn 的 `codex app-server` 子进程看不到 GUI 里正在跑的会话的实时
//! 状态：thread 的 live 状态（active/working/waiting）是 app-server **进程
//! 私有内存**，GUI 用的是它自己的 app-server 进程，两者内存隔离。所以
//! `CodexObserver` 的 `thread/list` 全是 `notLoaded`。
//!
//! 解决办法：连 GUI 的 IpcRouter socket，订阅实时广播。
//!
//! ## 协议（逆向自 ChatGPT.app app.asar，2026-07-23，codex-cli 0.145.0-alpha.27）
//!
//! - Unix domain socket，同 uid 无鉴权（socket 文件 `0600` + 属主校验）。
//! - 帧格式：**4 字节小端无符号长度前缀 + UTF-8 JSON payload**（单帧上限 256MiB）。
//! - 握手：发 `{type:"request", method:"initialize", params:{clientType:"extension"}}`
//!   → 收 `{type:"response", resultType:"success", result:{clientId}}`。
//! - 广播自动推送（无需 subscribe）：连上后 desktop client 会向所有其它 client
//!   广播 `thread-stream-state-changed`（version 11）等事件。
//!
//! ## 关注的广播
//!
//! `thread-stream-state-changed` 的 params 转发自 app-server 的
//! `thread/status/changed`，结构 = `{status: ThreadStatus, threadId}`。这是
//! working/waiting 状态的唯一实时来源。其余广播忽略。
//!
//! ## 降级行为
//!
//! 整个 watcher 是 best-effort：连不上（GUI 没开）→ `status_of` 恒返回 `None`
//! → observer 回退到原 `notLoaded` 状态（即现状，不退化）。任何协议/解析失败
//! 都不 panic、不阻塞 observer 主流程。

use crate::mapper::ThreadStatus;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 单帧 payload 上限（256 MiB），与 GUI 的 `_9` 帧解析器一致。
const MAX_FRAME: u32 = 256 * 1024 * 1024;
/// 连接/重连失败后的退避间隔。
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

/// 单个 thread 的最新实时状态快照。
#[derive(Debug, Clone)]
pub struct IpcState {
    pub thread_id: String,
    pub status: ThreadStatus,
    /// 收到该状态时的 Unix 时间戳（秒）。诊断用（可观察 watcher 活性）。
    #[allow(dead_code)]
    pub updated_at: u64,
}

/// 后台线程连 IpcRouter、持续接收广播、维护 thread 状态表。
///
/// 通过 `Arc` 共享给 `CodexObserver`，poll 时查询覆盖 SessionSnapshot 状态。
pub struct IpcStateWatcher {
    state: Arc<Mutex<HashMap<String, IpcState>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl IpcStateWatcher {
    /// 启动后台 watcher。立即返回，连接/解析都在后台线程。
    ///
    /// 失败（如 socket 路径不存在）不报错——后台线程会持续重试。调用方拿到
    /// 的 watcher 在连上之前 `status_of` 恒返回 `None`。
    pub fn spawn(socket_path: PathBuf) -> Self {
        let state: Arc<Mutex<HashMap<String, IpcState>>> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));

        // 后台线程持有一份 state/stop 的 Arc clone，主线程保留另一份。
        let state_for_thread = Arc::clone(&state);
        let stop_for_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("codex-ipc-watcher".into())
            .spawn(move || run_loop(socket_path, state_for_thread, stop_for_thread))
            .ok();

        Self {
            state,
            stop,
            join: handle,
        }
    }

    /// 查询某个 thread 的最新实时状态。`None` 表示没有该 thread 的实时数据
    /// （watcher 未连上、或该 thread 没发过状态变化广播）。
    pub fn status_of(&self, thread_id: &str) -> Option<ThreadStatus> {
        self.state
            .lock()
            .ok()
            .and_then(|m| m.get(thread_id).map(|s| s.status.clone()))
    }

    /// 当前是否已连上 IpcRouter（有任意状态记录即视为连上过）。诊断用。
    pub fn is_connected(&self) -> bool {
        self.state
            .lock()
            .map(|m| !m.is_empty())
            .unwrap_or(false)
    }
}

impl Drop for IpcStateWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // join 时后台线程可能正阻塞在 socket read；线程在下一次重连退避
        // 或 read 返回（socket 关闭）时检查 stop 标志退出。不强制等待，避免
        // 拖住主线程——线程是 daemon 行为，进程退出时自然回收。
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// 后台线程主循环：连 → 握手 → 读帧，断开则退避重连，直到 `stop` 置位。
fn run_loop(socket_path: PathBuf, state: Arc<Mutex<HashMap<String, IpcState>>>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match connect_and_run(&socket_path, &state, &stop) {
            Ok(()) => {
                // 正常退出（stop 置位或对端关闭）。若不是 stop，则重连。
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(RECONNECT_BACKOFF);
            }
            Err(_) => {
                // 连不上（GUI 没开）/握手失败。退避后重试。
                // 用短轮询 sleep 检查 stop，保证能及时退出。
                for _ in 0..RECONNECT_BACKOFF.as_secs() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    }
}

/// 连一次 socket，握手并持续读帧直到断开。返回 Ok 表示"该次会话结束"（需重连）。
fn connect_and_run(
    socket_path: &Path,
    state: &Mutex<HashMap<String, IpcState>>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    // 设短超时，让 read 周期性返回以检查 stop 标志（GUI 长时间无广播时也能退出）。
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    // 握手：initialize。
    let init = json!({
        "type": "request",
        "requestId": uuid_str(),
        "method": "initialize",
        "params": { "clientType": "extension" },
    });
    stream.write_all(&encode(&init))?;
    stream.flush()?;

    // 读循环。不严格等待 initialize 响应——GUI 的 router 一旦接受连接就会
    // 开始推广播，握手响应和首批广播可能交织到达，统一按帧解析即可。
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let n = match stream.read(&mut chunk) {
            Ok(0) => return Ok(()), // 对端关闭
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // 读超时：检查 stop 后继续。
                continue;
            }
            Err(_) => return Ok(()), // 任何读错误都视作需要重连
        };
        buf.extend_from_slice(&chunk[..n]);

        let frames = match decode_frames(&mut buf) {
            Ok(f) => f,
            Err(_) => return Ok(()), // 非法帧长度：协议错位，重连
        };
        for frame in frames {
            if let Some(update) = parse_broadcast(&frame) {
                if let Ok(mut m) = state.lock() {
                    m.insert(update.thread_id.clone(), update);
                }
            }
        }
    }
}

// ─── 帧编解码（4 字节 LE 长度前缀 + JSON payload） ──────────────────────────

/// 编码一个 JSON 值为 length-prefixed 帧。
fn encode(obj: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(obj).unwrap_or_else(|_| b"null".to_vec());
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&payload);
    out
}

/// 从 buffer 前部解析所有完整帧，把已消费的字节移除。
///
/// 返回 `Err` 表示遇到非法帧长度（0 或 >256MiB），调用方应重置连接。
fn decode_frames(buf: &mut Vec<u8>) -> std::io::Result<Vec<Value>> {
    let mut frames = Vec::new();
    let mut consumed = 0;
    while buf.len() >= consumed + 4 {
        let len_bytes = &buf[consumed..consumed + 4];
        let length = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        if length == 0 || length > MAX_FRAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid frame length ({length})"),
            ));
        }
        let frame_end = consumed + 4 + length as usize;
        if buf.len() < frame_end {
            break; // 不完整，等更多数据
        }
        let payload = &buf[consumed + 4..frame_end];
        if let Ok(v) = serde_json::from_slice::<Value>(payload) {
            frames.push(v);
        }
        // 解析失败的单帧跳过（不致命），继续后续帧。
        consumed = frame_end;
    }
    if consumed > 0 {
        buf.drain(..consumed);
    }
    Ok(frames)
}

// ─── 广播解析 ────────────────────────────────────────────────────────────

/// 从一帧 JSON 解析出 thread 状态更新。非 `thread-stream-state-changed` 广播
/// 返回 `None`。
fn parse_broadcast(frame: &Value) -> Option<IpcState> {
    let method = frame.get("method").and_then(|v| v.as_str())?;
    if method != "thread-stream-state-changed" {
        return None;
    }
    // params = { status: ThreadStatus, threadId: string }
    let params = frame.get("params")?;
    let thread_id = params.get("threadId").and_then(|v| v.as_str())?.to_string();
    let status_value = params.get("status")?;
    let status = deserialize_status(status_value)?;
    let updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(IpcState {
        thread_id,
        status,
        updated_at,
    })
}

/// 把 status JSON 值反序列化为 ThreadStatus。解析失败返回 None（不致命）。
fn deserialize_status(v: &Value) -> Option<ThreadStatus> {
    // 手动按 `type` 字段匹配。不直接用 serde derive 是因为 `ThreadStatus` 的
    // `#[serde(rename_all = "camelCase")]` 对 tagged enum 的内部字段
    // （`active_flags`）不会重命名为 `activeFlags`，而 IPC 广播里用的正是
    // `activeFlags`。手动解析更稳，且对协议演进友好（未知 type → None 降级）。
    let ty = v.get("type").and_then(|t| t.as_str())?;
    match ty {
        "notLoaded" => Some(ThreadStatus::NotLoaded),
        "idle" => Some(ThreadStatus::Idle),
        "systemError" => Some(ThreadStatus::SystemError),
        "active" => Some(ThreadStatus::Active {
            active_flags: v
                .get("activeFlags")
                .and_then(|f| serde_json::from_value(f.clone()).ok())
                .unwrap_or_default(),
        }),
        // 未知 type（协议演进）→ None，observer 不覆盖该 thread。
        _ => None,
    }
}

// ─── 路径与辅助 ──────────────────────────────────────────────────────────

/// 默认 socket 路径：`$CODEX_HOME/ipc/ipc.sock`，`CODEX_HOME` 缺省为 `~/.codex`。
pub fn default_socket_path() -> PathBuf {
    let codex_home = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".codex")))
        .unwrap_or_else(|| PathBuf::from("/.codex"));
    codex_home.join("ipc").join("ipc.sock")
}

/// 简易 UUID v4 生成（无需 uuid crate）。IpcRouter 用 requestId 做关联，
/// 只要全局唯一即可，这里用时间戳 + 计数器 + 随机凑一个 8-4-4-4-12 串。
fn uuid_str() -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    // 用各部分填充 UUIDv4 形状（8-4-4-4-12）。各段截断到对应位数的掩码，
    // 保证 format 的最小宽度填充生效（u64 的 {:08x} 否则会输出 16 位）。
    let a = (now ^ seq.rotate_left(13)) & 0xffffffff; // 8 hex
    let b = seq.wrapping_mul(0x9e3779b97f4a7c15) & 0xffff; // 4 hex
    let b_lo = b & 0xfff; // 3 hex (version nibble '4' 前缀拼出第 4 段)
    let c = (now >> 16) & 0xffff; // 4 hex
    let d = (seq >> 8) & 0xffffffffffff; // 12 hex
    format!("{a:08x}-{b:04x}-4{b_lo:03x}-{c:04x}-{d:012x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let obj = json!({"type": "request", "method": "initialize", "params": {}});
        let bytes = encode(&obj);
        let mut buf = bytes.clone();
        let frames = decode_frames(&mut buf).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], obj);
        assert!(buf.is_empty()); // 全部消费
    }

    #[test]
    fn decode_multiple_frames_in_one_buffer() {
        let a = encode(&json!({"id": 1}));
        let b = encode(&json!({"id": 2}));
        let c = encode(&json!({"id": 3}));
        let mut buf = Vec::new();
        buf.extend_from_slice(&a);
        buf.extend_from_slice(&b);
        buf.extend_from_slice(&c);
        let frames = decode_frames(&mut buf).unwrap();
        assert_eq!(frames.len(), 3);
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_partial_frame_waits_for_more() {
        let obj = json!({"hello": "world"});
        let bytes = encode(&obj);
        // 只给前半段，应返回 0 帧且不消耗（等更多数据）。
        let half = &bytes[..bytes.len() / 2];
        let mut buf = half.to_vec();
        let frames = decode_frames(&mut buf).unwrap();
        assert!(frames.is_empty());
        // 补齐剩余部分后应能解出。
        buf.extend_from_slice(&bytes[bytes.len() / 2..]);
        let frames = decode_frames(&mut buf).unwrap();
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn decode_zero_length_is_error() {
        let mut buf = vec![0u8, 0, 0, 0]; // length = 0 → 非法
        assert!(decode_frames(&mut buf).is_err());
    }

    #[test]
    fn decode_oversized_length_is_error() {
        // length = 257 MiB → 超过上限
        let mut buf = (MAX_FRAME + 1u32).to_le_bytes().to_vec();
        // 补点假 payload 占位（不会真到那么大，decode 在读长度时就拒绝）
        buf.extend_from_slice(&[0u8; 4]);
        assert!(decode_frames(&mut buf).is_err());
    }

    #[test]
    fn parse_state_changed_active_working() {
        // active 无 flag → working 态的 ThreadStatus
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "threadId": "abc-123",
                "status": { "type": "active", "activeFlags": [] }
            }
        });
        let s = parse_broadcast(&frame).expect("应解析出状态");
        assert_eq!(s.thread_id, "abc-123");
        assert!(matches!(s.status, ThreadStatus::Active { ref active_flags } if active_flags.is_empty()));
    }

    #[test]
    fn parse_state_changed_active_waiting() {
        // active + waitingOnApproval → waiting 态
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "threadId": "def-456",
                "status": { "type": "active", "activeFlags": ["waitingOnApproval"] }
            }
        });
        let s = parse_broadcast(&frame).expect("应解析出状态");
        match s.status {
            ThreadStatus::Active { active_flags } => {
                assert!(active_flags.iter().any(|f| f == "waitingOnApproval"));
            }
            _ => panic!("应为 Active"),
        }
    }

    #[test]
    fn parse_state_changed_idle() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "threadId": "xyz",
                "status": { "type": "idle" }
            }
        });
        let s = parse_broadcast(&frame).expect("应解析出状态");
        assert!(matches!(s.status, ThreadStatus::Idle));
    }

    #[test]
    fn parse_ignores_other_broadcasts() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-following-changed",
            "params": { "conversationId": "x", "following": true }
        });
        assert!(parse_broadcast(&frame).is_none());
    }

    #[test]
    fn parse_ignores_non_broadcast() {
        // initialize 响应、client-discovery 等非状态广播
        let frame = json!({
            "type": "response",
            "method": "initialize",
            "result": { "clientId": "c1" }
        });
        assert!(parse_broadcast(&frame).is_none());
    }

    #[test]
    fn parse_unknown_status_type_degrades_to_none_or_known() {
        // codex 协议演进引入新 type：active/idle/notLoaded/systemError 之外的
        // 应优雅降级（返回 None，observer 不覆盖）。
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "threadId": "u",
                "status": { "type": "someNewFutureStatus", "extra": 1 }
            }
        });
        // 未知 type：deserialize_status 兜底返回 None → parse_broadcast 返回 None
        assert!(parse_broadcast(&frame).is_none());
    }

    #[test]
    fn default_socket_path_under_codex_home() {
        let p = default_socket_path();
        assert!(p.ends_with("ipc/ipc.sock"));
    }

    #[test]
    fn uuid_str_is_uuid_shaped() {
        let u = uuid_str();
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(parts.len(), 5, "UUID 应有 5 段: {u}");
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }
}
