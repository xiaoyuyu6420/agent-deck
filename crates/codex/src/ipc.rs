//! Codex 桌面 GUI 的实时 thread 状态观察器（via `~/.codex/ipc/ipc.sock`）。
//!
//! ## 为什么需要这个模块
//!
//! 独立 spawn 的 `codex app-server` 子进程看不到 GUI 里正在跑的会话的实时
//! 状态：thread 的 live 状态是 app-server **进程私有内存**，GUI 用的是它自己
//! 的 app-server 进程，两者内存隔离。所以 `CodexObserver` 的 `thread/list` 全是
//! `notLoaded`。
//!
//! 解决办法：连 GUI 的 IpcRouter，**注册成 stream follower**，接收
//! `thread-stream-state-changed` 的 snapshot/patches，从
//! `conversationState.threadRuntimeStatus` 读 working/waiting。
//!
//! ## 协议（2026-07-23 阶段 0 定稿，见 docs/codex-integration.md）
//!
//! - Unix domain socket，同 uid 无鉴权。
//! - 帧格式：**4 字节小端长度前缀**（= payload 字节数）+ UTF-8 JSON。
//! - 握手：`initialize {clientType:"extension"}` → `{result:{clientId}}`。
//! - **Follower 注册（关键）**：
//!   1. owner 发来 `thread-stream-following-changed{following:true}`（邀请）
//!   2. 我们**主动广播**同样 method（不带 targetClientIds）
//!   3. owner 立刻推 `thread-stream-state-changed` snapshot（仅 target 我们）
//! - payload：`params.{conversationId, hostId, change:{type:snapshot|patches,...}}`
//!   状态字段在 `change.conversationState.threadRuntimeStatus`（与 app-server
//!   `ThreadStatus` 同构）。
//!
//! ## 降级
//!
//! best-effort：GUI 没开 → `status_of` 恒 `None` → observer 回退 notLoaded。
//! 任何协议/解析失败不 panic、不阻塞主流程。
//!
//! **跨平台**：Unix domain socket 仅 Unix 可用。`spawn`/`run_loop`/
//! `connect_and_run`/`announce_following` 及它们调用的纯解析函数在非 Unix
//! 平台下不编译（Windows 返回 no-op watcher）。`#[allow(dead_code)]` 抑制
//! 非 Unix 下这些函数「未使用」的告警。

#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use crate::mapper::ThreadStatus;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 单帧 payload 上限（256 MiB），与 GUI 帧解析器一致。
const MAX_FRAME: u32 = 256 * 1024 * 1024;
/// 连接/重连失败后的退避间隔。
const RECONNECT_BACKOFF: Duration = Duration::from_secs(5);

/// 单个 thread 的最新实时状态快照。
#[derive(Debug, Clone)]
pub struct IpcState {
    pub thread_id: String,
    pub status: ThreadStatus,
    /// 收到该状态时的 Unix 时间戳（秒）。诊断用。
    #[allow(dead_code)]
    pub updated_at: u64,
}

/// 后台线程连 IpcRouter、注册 follower、接收 stream 状态、维护 thread 状态表。
pub struct IpcStateWatcher {
    state: Arc<Mutex<HashMap<String, IpcState>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl IpcStateWatcher {
    /// 启动后台 watcher。立即返回；连接/announce/解析都在后台线程。
    ///
    /// On non-Unix platforms (Windows) the real-time ipc channel (Unix domain
    /// socket) does not exist, so this returns a no-op watcher whose
    /// `status_of` always yields `None`. The observer then falls back to the
    /// static `thread/list` poll.
    #[cfg(unix)]
    pub fn spawn(socket_path: PathBuf) -> Self {
        let state: Arc<Mutex<HashMap<String, IpcState>>> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
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

    /// Windows / non-Unix stub: no real-time watcher. `status_of` is always
    /// `None`; the observer uses `thread/list` polling only.
    #[cfg(not(unix))]
    #[allow(clippy::new_without_default)]
    pub fn spawn(_socket_path: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            stop: Arc::new(AtomicBool::new(true)),
            join: None,
        }
    }

    /// 查询某个 thread 的最新实时状态。`None` = 无实时数据。
    pub fn status_of(&self, thread_id: &str) -> Option<ThreadStatus> {
        self.state
            .lock()
            .ok()
            .and_then(|m| m.get(thread_id).map(|s| s.status.clone()))
    }

    /// 是否已收到过至少一条 stream 状态（announce 成功后的 snapshot 也算）。
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
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

#[cfg(unix)]
fn run_loop(socket_path: PathBuf, state: Arc<Mutex<HashMap<String, IpcState>>>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match connect_and_run(&socket_path, &state, &stop) {
            Ok(()) => {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(RECONNECT_BACKOFF);
            }
            Err(_) => {
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

#[cfg(unix)]
fn connect_and_run(
    socket_path: &Path,
    state: &Mutex<HashMap<String, IpcState>>,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(socket_path)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let init = json!({
        "type": "request",
        "requestId": uuid_str(),
        "method": "initialize",
        "params": { "clientType": "extension" },
    });
    stream.write_all(&encode(&init))?;
    stream.flush()?;

    let mut client_id: Option<String> = None;
    // 已向 owner announce following=true 的 conversationId（重连后清空，会再 announce）。
    let mut followed: HashSet<String> = HashSet::new();
    // following 邀请若早于 initialize 响应到达，先记下，拿到 clientId 后再 flush。
    let mut pending_follow: Vec<(String, String)> = Vec::new();
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let n = match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => return Ok(()),
        };
        buf.extend_from_slice(&chunk[..n]);

        let frames = match decode_frames(&mut buf) {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };

        for frame in frames {
            // initialize 响应：记下 clientId（announce 需要 sourceClientId）。
            if frame.get("type").and_then(|v| v.as_str()) == Some("response")
                && frame.get("method").and_then(|v| v.as_str()) == Some("initialize")
                && frame.get("resultType").and_then(|v| v.as_str()) == Some("success")
            {
                if let Some(id) = frame
                    .pointer("/result/clientId")
                    .and_then(|v| v.as_str())
                {
                    client_id = Some(id.to_string());
                    // flush 握手前积压的 follow 邀请
                    if let Some(cid) = client_id.as_deref() {
                        for (conv, host) in pending_follow.drain(..) {
                            let _ = announce_following(&mut stream, cid, &conv, &host);
                            followed.insert(conv);
                        }
                    }
                }
            }

            let method = frame.get("method").and_then(|v| v.as_str()).unwrap_or("");
            match method {
                "thread-stream-following-changed" => {
                    // owner 邀请我们 follow → 主动 announce，才能进 followerClientIds。
                    if let Some((conv, host, following)) = parse_following_params(&frame) {
                        if following {
                            if let Some(cid) = client_id.as_deref() {
                                let _ = announce_following(&mut stream, cid, &conv, &host);
                                followed.insert(conv);
                            } else {
                                pending_follow.push((conv, host));
                            }
                        } else {
                            followed.remove(&conv);
                        }
                    }
                }
                "thread-stream-following-status-requested" => {
                    // owner 询问谁在 follow → 若我们已 follow 该会话，再 announce 一次。
                    if let Some((conv, host)) = parse_status_requested_params(&frame) {
                        if followed.contains(&conv) {
                            if let Some(cid) = client_id.as_deref() {
                                let _ = announce_following(&mut stream, cid, &conv, &host);
                            }
                        }
                    }
                }
                "thread-stream-state-changed" => {
                    if let Some(update) = parse_state_changed(&frame) {
                        if let Ok(mut m) = state.lock() {
                            m.insert(update.thread_id.clone(), update);
                        }
                    }
                }
                "client-discovery-request" => {
                    // 我们不处理任何 request method；老实回 canHandle=false，
                    // 避免 router 侧 pending discovery 挂起。
                    if let Some(rid) = frame.get("requestId").and_then(|v| v.as_str()) {
                        let resp = json!({
                            "type": "client-discovery-response",
                            "requestId": rid,
                            "response": { "canHandle": false },
                        });
                        let _ = stream.write_all(&encode(&resp));
                        let _ = stream.flush();
                    }
                }
                _ => {}
            }
        }
    }
}

/// 主动广播 following=true，让 owner 把我们登记为 stream follower。
#[cfg(unix)]
fn announce_following(
    stream: &mut UnixStream,
    client_id: &str,
    conversation_id: &str,
    host_id: &str,
) -> std::io::Result<()> {
    let msg = json!({
        "type": "broadcast",
        "method": "thread-stream-following-changed",
        "sourceClientId": client_id,
        "params": {
            "conversationId": conversation_id,
            "hostId": host_id,
            "following": true,
        },
        "version": 1,
    });
    stream.write_all(&encode(&msg))?;
    stream.flush()
}

fn parse_following_params(frame: &Value) -> Option<(String, String, bool)> {
    let params = frame.get("params")?;
    let conv = params.get("conversationId")?.as_str()?.to_string();
    let host = params
        .get("hostId")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    let following = params.get("following")?.as_bool()?;
    Some((conv, host, following))
}

fn parse_status_requested_params(frame: &Value) -> Option<(String, String)> {
    let params = frame.get("params")?;
    let conv = params.get("conversationId")?.as_str()?.to_string();
    let host = params
        .get("hostId")
        .and_then(|v| v.as_str())
        .unwrap_or("local")
        .to_string();
    Some((conv, host))
}

// ─── 帧编解码 ───────────────────────────────────────────────────────────────

fn encode(obj: &Value) -> Vec<u8> {
    let payload = serde_json::to_vec(obj).unwrap_or_else(|_| b"null".to_vec());
    let mut out = (payload.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&payload);
    out
}

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
            break;
        }
        let payload = &buf[consumed + 4..frame_end];
        if let Ok(v) = serde_json::from_slice::<Value>(payload) {
            frames.push(v);
        }
        consumed = frame_end;
    }
    if consumed > 0 {
        buf.drain(..consumed);
    }
    Ok(frames)
}

// ─── state-changed 解析 ─────────────────────────────────────────────────────

/// 解析 `thread-stream-state-changed`：从 snapshot 或 patches 提取 ThreadStatus。
fn parse_state_changed(frame: &Value) -> Option<IpcState> {
    if frame.get("method").and_then(|v| v.as_str()) != Some("thread-stream-state-changed") {
        return None;
    }
    let params = frame.get("params")?;
    let thread_id = params
        .get("conversationId")
        .or_else(|| params.get("threadId"))
        .and_then(|v| v.as_str())?
        .to_string();
    let change = params.get("change")?;
    let change_type = change.get("type").and_then(|v| v.as_str()).unwrap_or("");

    let status = match change_type {
        "snapshot" => {
            let cs = change.get("conversationState")?;
            status_from_conversation_state(cs)?
        }
        "patches" => {
            // patches 结构随版本变；尽量从中抠 threadRuntimeStatus / requests / inProgress。
            // 抠不到就返回 None，保留状态表里上一帧（snapshot）的值。
            let patches = change.get("patches")?;
            status_from_patches(patches)?
        }
        _ => return None,
    };

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

/// 从 conversationState 推导 ThreadStatus。
///
/// 优先级：
/// 1. `threadRuntimeStatus`（与 app-server ThreadStatus 同构）
/// 2. `requests` 非空 → Waiting
/// 3. 末 turn `status==inProgress` → Working
fn status_from_conversation_state(cs: &Value) -> Option<ThreadStatus> {
    let has_requests = cs
        .get("requests")
        .and_then(|r| r.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let turn_in_progress = last_turn_in_progress(cs);

    let mut status = cs
        .get("threadRuntimeStatus")
        .and_then(deserialize_status)
        .or_else(|| {
            // 无 threadRuntimeStatus 时用 turn/requests 兜底。
            if has_requests {
                Some(ThreadStatus::Active {
                    active_flags: vec!["waitingOnApproval".into()],
                })
            } else if turn_in_progress {
                Some(ThreadStatus::Active {
                    active_flags: vec![],
                })
            } else {
                None
            }
        })?;

    // 补强：status 是 idle/notLoaded 但有未决请求或 turn 在跑 → 提升。
    match &status {
        ThreadStatus::Idle | ThreadStatus::NotLoaded => {
            if has_requests {
                status = ThreadStatus::Active {
                    active_flags: vec!["waitingOnApproval".into()],
                };
            } else if turn_in_progress {
                status = ThreadStatus::Active {
                    active_flags: vec![],
                };
            }
        }
        ThreadStatus::Active { active_flags } if has_requests => {
            let waiting = active_flags
                .iter()
                .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput");
            if !waiting {
                let mut flags = active_flags.clone();
                flags.push("waitingOnApproval".into());
                status = ThreadStatus::Active {
                    active_flags: flags,
                };
            }
        }
        _ => {}
    }

    Some(status)
}

fn last_turn_in_progress(cs: &Value) -> bool {
    cs.get("turns")
        .and_then(|t| t.as_array())
        .and_then(|arr| arr.last())
        .and_then(|t| t.get("status"))
        .and_then(|s| s.as_str())
        == Some("inProgress")
}

/// 从 patches 数组/对象里尽量提取状态。失败返回 None（保留旧状态）。
fn status_from_patches(patches: &Value) -> Option<ThreadStatus> {
    // 1) 直接搜 threadRuntimeStatus 节点
    if let Some(trs) = find_key_value(patches, "threadRuntimeStatus") {
        if let Some(s) = deserialize_status(trs) {
            // 若 patches 里同时有非空 requests，提升为 waiting
            let has_requests = find_key_value(patches, "requests")
                .and_then(|r| r.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            return Some(elevate_if_requests(s, has_requests));
        }
    }

    // 2) 搜 inProgress（patch 常为 {path:".../status", value:"inProgress"}）
    if json_contains_string_field(patches, "status", "inProgress")
        || json_contains_string_field(patches, "value", "inProgress")
    {
        return Some(ThreadStatus::Active {
            active_flags: vec![],
        });
    }

    // 3) 搜 waiting 标志
    if json_contains_string(patches, "waitingOnApproval")
        || json_contains_string(patches, "waitingOnUserInput")
    {
        let flag = if json_contains_string(patches, "waitingOnUserInput") {
            "waitingOnUserInput"
        } else {
            "waitingOnApproval"
        };
        return Some(ThreadStatus::Active {
            active_flags: vec![flag.into()],
        });
    }

    // 4) 裸 ThreadStatus 对象（type: active/idle/...）
    if let Some(s) = find_thread_status_object(patches) {
        return Some(s);
    }

    None
}

fn elevate_if_requests(status: ThreadStatus, has_requests: bool) -> ThreadStatus {
    if !has_requests {
        return status;
    }
    match status {
        ThreadStatus::Active { active_flags } => {
            let waiting = active_flags
                .iter()
                .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput");
            if waiting {
                ThreadStatus::Active { active_flags }
            } else {
                let mut flags = active_flags;
                flags.push("waitingOnApproval".into());
                ThreadStatus::Active {
                    active_flags: flags,
                }
            }
        }
        ThreadStatus::Idle | ThreadStatus::NotLoaded => ThreadStatus::Active {
            active_flags: vec!["waitingOnApproval".into()],
        },
        other => other,
    }
}

fn deserialize_status(v: &Value) -> Option<ThreadStatus> {
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
        _ => None,
    }
}

/// 深度查找 key 对应的值。
fn find_key_value<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            if let Some(found) = map.get(key) {
                return Some(found);
            }
            for val in map.values() {
                if let Some(found) = find_key_value(val, key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(found) = find_key_value(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}

fn find_thread_status_object(v: &Value) -> Option<ThreadStatus> {
    if let Some(s) = deserialize_status(v) {
        // 避免把任意 {type:"..."} 误判：active 必须带 activeFlags 字段（可空数组），
        // 或 type 明确是 idle/notLoaded/systemError。
        match &s {
            ThreadStatus::Active { .. } => {
                if v.get("activeFlags").is_some() {
                    return Some(s);
                }
            }
            _ => return Some(s),
        }
    }
    match v {
        Value::Object(map) => {
            for val in map.values() {
                if let Some(s) = find_thread_status_object(val) {
                    return Some(s);
                }
            }
            None
        }
        Value::Array(arr) => {
            for item in arr {
                if let Some(s) = find_thread_status_object(item) {
                    return Some(s);
                }
            }
            None
        }
        _ => None,
    }
}

fn json_contains_string(v: &Value, needle: &str) -> bool {
    match v {
        Value::String(s) => s == needle,
        Value::Array(a) => a.iter().any(|x| json_contains_string(x, needle)),
        Value::Object(m) => m.values().any(|x| json_contains_string(x, needle)),
        _ => false,
    }
}

fn json_contains_string_field(v: &Value, key: &str, value: &str) -> bool {
    match v {
        Value::Object(m) => {
            if m.get(key).and_then(|x| x.as_str()) == Some(value) {
                return true;
            }
            m.values().any(|x| json_contains_string_field(x, key, value))
        }
        Value::Array(a) => a
            .iter()
            .any(|x| json_contains_string_field(x, key, value)),
        _ => false,
    }
}

// ─── 路径与辅助 ─────────────────────────────────────────────────────────────

pub fn default_socket_path() -> PathBuf {
    let codex_home = std::env::var("CODEX_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| agent_deck_protocol::home_dir().join(".codex"));
    codex_home.join("ipc").join("ipc.sock")
}

fn uuid_str() -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let a = (now ^ seq.rotate_left(13)) & 0xffffffff;
    let b = seq.wrapping_mul(0x9e3779b97f4a7c15) & 0xffff;
    let b_lo = b & 0xfff;
    let c = (now >> 16) & 0xffff;
    let d = (seq >> 8) & 0xffffffffffff;
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
        assert!(buf.is_empty());
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
        let half = &bytes[..bytes.len() / 2];
        let mut buf = half.to_vec();
        let frames = decode_frames(&mut buf).unwrap();
        assert!(frames.is_empty());
        buf.extend_from_slice(&bytes[bytes.len() / 2..]);
        let frames = decode_frames(&mut buf).unwrap();
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn decode_zero_length_is_error() {
        let mut buf = vec![0u8, 0, 0, 0];
        assert!(decode_frames(&mut buf).is_err());
    }

    #[test]
    fn decode_oversized_length_is_error() {
        let mut buf = (MAX_FRAME + 1u32).to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 4]);
        assert!(decode_frames(&mut buf).is_err());
    }

    #[test]
    fn parse_snapshot_active_working() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "abc-123",
                "hostId": "local",
                "change": {
                    "type": "snapshot",
                    "revision": 1,
                    "conversationState": {
                        "id": "abc-123",
                        "threadRuntimeStatus": { "type": "active", "activeFlags": [] },
                        "requests": [],
                        "turns": []
                    }
                }
            }
        });
        let s = parse_state_changed(&frame).expect("应解析 snapshot");
        assert_eq!(s.thread_id, "abc-123");
        assert!(matches!(s.status, ThreadStatus::Active { ref active_flags } if active_flags.is_empty()));
    }

    #[test]
    fn parse_snapshot_active_waiting() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "def-456",
                "hostId": "local",
                "change": {
                    "type": "snapshot",
                    "revision": 2,
                    "conversationState": {
                        "threadRuntimeStatus": {
                            "type": "active",
                            "activeFlags": ["waitingOnApproval"]
                        },
                        "requests": []
                    }
                }
            }
        });
        let s = parse_state_changed(&frame).expect("应解析 waiting");
        match s.status {
            ThreadStatus::Active { active_flags } => {
                assert!(active_flags.iter().any(|f| f == "waitingOnApproval"));
            }
            _ => panic!("应为 Active"),
        }
    }

    #[test]
    fn parse_snapshot_idle() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "xyz",
                "hostId": "local",
                "change": {
                    "type": "snapshot",
                    "revision": 1,
                    "conversationState": {
                        "threadRuntimeStatus": { "type": "idle" },
                        "requests": [],
                        "turns": []
                    }
                }
            }
        });
        let s = parse_state_changed(&frame).expect("应解析 idle");
        assert!(matches!(s.status, ThreadStatus::Idle));
    }

    #[test]
    fn parse_snapshot_requests_elevate_to_waiting() {
        // threadRuntimeStatus=idle 但 requests 非空 → Waiting
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "req-1",
                "change": {
                    "type": "snapshot",
                    "conversationState": {
                        "threadRuntimeStatus": { "type": "idle" },
                        "requests": [{ "id": "r1", "type": "commandApproval" }]
                    }
                }
            }
        });
        let s = parse_state_changed(&frame).expect("应提升为 waiting");
        match s.status {
            ThreadStatus::Active { active_flags } => {
                assert!(active_flags.iter().any(|f| f == "waitingOnApproval"));
            }
            other => panic!("应为 Active/waiting, got {other:?}"),
        }
    }

    #[test]
    fn parse_snapshot_turn_in_progress_is_working() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "turn-1",
                "change": {
                    "type": "snapshot",
                    "conversationState": {
                        "threadRuntimeStatus": { "type": "idle" },
                        "requests": [],
                        "turns": [{ "status": "completed" }, { "status": "inProgress" }]
                    }
                }
            }
        });
        let s = parse_state_changed(&frame).expect("turn inProgress → working");
        assert!(matches!(s.status, ThreadStatus::Active { ref active_flags } if active_flags.is_empty()));
    }

    #[test]
    fn parse_patches_with_thread_runtime_status() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "p-1",
                "change": {
                    "type": "patches",
                    "baseRevision": 1,
                    "revision": 2,
                    "patches": [
                        { "op": "replace", "path": "/threadRuntimeStatus",
                          "value": { "type": "active", "activeFlags": [] } }
                    ]
                }
            }
        });
        let s = parse_state_changed(&frame).expect("patches 应抠出 active");
        assert!(matches!(s.status, ThreadStatus::Active { .. }));
    }

    #[test]
    fn parse_patches_in_progress_string() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "p-2",
                "change": {
                    "type": "patches",
                    "patches": [{ "path": "/turns/0/status", "value": "inProgress" }]
                }
            }
        });
        let s = parse_state_changed(&frame).expect("inProgress → working");
        assert!(matches!(s.status, ThreadStatus::Active { ref active_flags } if active_flags.is_empty()));
    }

    #[test]
    fn parse_ignores_following_changed() {
        // following-changed 不由 parse_state_changed 处理（主循环单独处理 announce）。
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-following-changed",
            "params": { "conversationId": "x", "following": true, "hostId": "local" }
        });
        assert!(parse_state_changed(&frame).is_none());
    }

    #[test]
    fn parse_ignores_legacy_status_thread_id_shape() {
        // 旧错误假设的 payload 形状：不应再被当成有效状态。
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "threadId": "abc-123",
                "status": { "type": "active", "activeFlags": [] }
            }
        });
        // 无 change 字段 → None
        assert!(parse_state_changed(&frame).is_none());
    }

    #[test]
    fn parse_unknown_status_type_degrades() {
        let frame = json!({
            "type": "broadcast",
            "method": "thread-stream-state-changed",
            "params": {
                "conversationId": "u",
                "change": {
                    "type": "snapshot",
                    "conversationState": {
                        "threadRuntimeStatus": { "type": "someNewFutureStatus" }
                    }
                }
            }
        });
        assert!(parse_state_changed(&frame).is_none());
    }

    #[test]
    fn parse_following_params_ok() {
        let frame = json!({
            "method": "thread-stream-following-changed",
            "params": {
                "conversationId": "c1",
                "hostId": "local",
                "following": true
            }
        });
        let (c, h, f) = parse_following_params(&frame).unwrap();
        assert_eq!(c, "c1");
        assert_eq!(h, "local");
        assert!(f);
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

    #[test]
    fn announce_following_encode_shape() {
        // 不连真实 socket，只验证我们发出的 announce 帧 JSON 形状正确。
        let msg = json!({
            "type": "broadcast",
            "method": "thread-stream-following-changed",
            "sourceClientId": "cid-1",
            "params": {
                "conversationId": "conv-1",
                "hostId": "local",
                "following": true,
            },
            "version": 1,
        });
        let bytes = encode(&msg);
        let mut buf = bytes;
        let frames = decode_frames(&mut buf).unwrap();
        assert_eq!(frames[0]["method"], "thread-stream-following-changed");
        assert_eq!(frames[0]["params"]["following"], true);
        assert_eq!(frames[0]["params"]["conversationId"], "conv-1");
    }
}
