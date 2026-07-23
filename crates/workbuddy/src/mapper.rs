//! Pure WorkBuddy jsonl-event → SessionSnapshot mapper.
//!
//! WorkBuddy appends one JSON object per line to
//! `~/.workbuddy/projects/<workspace>/<session-id>.jsonl`. Event `type`
//! values seen in the wild: `message`, `function_call`, `function_call_result`,
//! `reasoning`, `ai-title`, `custom-title`, `file-history-snapshot`. We only
//! decode the handful of fields the board cares about; unknown fields/lines
//! are ignored so the observer is forward-compatible with format drift.
//!
//! Status is inferred (WorkBuddy has no external live status API). The mapper
//! layers live signals with recency windows so we match the desktop UI without
//! latching onto zombie rows.
//!
//! ## Done vs Idle semantics (fixed 2026-07-23, decay moved 2026-07-23)
//!
//! Earlier versions had these inverted: recent activity → Idle, old → Done.
//! The correct semantics are:
//!
//! - **Done** = a turn completed (assistant `status=completed`), **or** a
//!   streaming turn that stalled after interrupt. Green on the key.
//! - **Idle** = conversation is open/dormant and has never completed a turn
//!   that the mapper can see as Done. Dim white on the key.
//!
//! Done → Idle **decay is NOT done here**. Host-core applies a view-aware TTL:
//! unopened Done stays green until a long force-idle window; after the user
//! opens the key from Agent Deck, a shorter TTL starts. Mapper only reports
//! the raw completion signal so host can own "did the user look?".
//!
//! Soft Working signals (streaming text / thinking) use a short window and the
//! jsonl file mtime: when the user hits Stop, WorkBuddy stops appending and
//! the incomplete row freezes — we must not stay on `run` for 5 minutes.

use agent_deck_protocol::{BackendId, DeckStatus, Risk, SessionSnapshot};
use std::collections::HashSet;

/// Hard live-work: pending tools may legitimately run for a while without new
/// jsonl lines (result arrives later). Zombie pending tools age out here.
const ACTIVE_WINDOW_MS: u64 = 5 * 60 * 1000;

/// Soft live-work: streaming assistant (`incomplete`) and "user just spoke,
/// model still thinking". When the user interrupts, the jsonl freezes — a
/// short window is what makes Stop drop off `run` quickly.
const STREAMING_WINDOW_MS: u64 = 30 * 1000;

/// Approval-pending tools may sit longer than a normal tool call (user away
/// from keyboard). Keep Waiting a bit longer before falling back to Done/Idle.
const WAITING_WINDOW_MS: u64 = 30 * 60 * 1000;

/// One deserialized line from a WorkBuddy `<session-id>.jsonl` file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SessionEvent {
    #[serde(default)]
    pub timestamp: u64,
    #[serde(rename = "type", default)]
    pub event_type: String,
    /// `ai-title` events carry the model-generated title here.
    #[serde(default, rename = "aiTitle")]
    pub ai_title: Option<String>,
    /// `custom-title` events use `customTitle` in real WorkBuddy jsonl
    /// (verified 2026-07-23). Keep plain `title` as a fallback for older/
    /// alternate shapes.
    #[serde(default, rename = "customTitle")]
    pub custom_title: Option<String>,
    /// Generic title slot (legacy / alternate shapes).
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default, rename = "callId")]
    pub call_id: Option<String>,
    /// Tool name on `function_call` events (e.g. "Bash", "Edit").
    #[serde(default)]
    pub name: Option<String>,
    /// Tool-call lifecycle status (e.g. "completed") or message status
    /// (`completed` / `incomplete` on assistant messages).
    #[serde(default)]
    pub status: Option<String>,
    /// JSON-encoded tool arguments string; may embed `requires_approval`.
    #[serde(default)]
    pub arguments: Option<String>,
    /// `message` events carry a role (user/assistant).
    #[serde(default)]
    pub role: Option<String>,
}

/// Signals distilled from one session's event stream.
///
/// Timestamps are kept raw so `infer_status` can apply recency windows against
/// `now` — that keeps zombie tools / stranded incomplete messages from
/// permanently pinning a session as Working.
#[derive(Debug, Clone, Default)]
pub struct SessionSignals {
    pub session_id: String,
    pub title: Option<String>,
    pub workspace_path: Option<String>,
    pub updated_at: u64,
    /// Newest pending tool that does NOT require approval (ms epoch).
    pub pending_tool_at: Option<u64>,
    /// Newest pending tool that requires approval (ms epoch).
    pub pending_approval_at: Option<u64>,
    /// Timestamp of the chronologically last `message` with `role=user`.
    pub last_user_at: Option<u64>,
    /// Timestamp of the chronologically last `message` with `role=assistant`.
    pub last_assistant_at: Option<u64>,
    /// True when the last assistant message has `status=incomplete` (still
    /// streaming). Stranded incompletes from earlier turns are ignored by
    /// only consulting the last assistant message.
    pub last_assistant_incomplete: bool,
    /// Optional wall-clock freshness from the jsonl file mtime. When the user
    /// hits Stop, WorkBuddy freezes the incomplete row and stops appending;
    /// mtime is what proves the stream is still alive.
    pub file_mtime_ms: Option<u64>,
    /// Human-readable hint for the waiting tool, e.g. "Bash".
    pub detail: Option<String>,
}

/// Fold a session's decoded events into the signals the board needs.
///
/// `file_mtime_ms` is optional wall-clock freshness from the jsonl path. Soft
/// Working signals (streaming / thinking) require it to still be recent so a
/// user Stop does not leave the key on `run`.
pub fn aggregate(
    events: &[SessionEvent],
    session_id: &str,
    file_mtime_ms: Option<u64>,
) -> SessionSignals {
    // Track custom vs AI titles separately so renames always win: the latest
    // custom-title overrides any earlier ai-title, and a later ai-title still
    // updates when the user never set a custom name. (Old code only kept the
    // first title and read custom-title from the wrong field `title` instead
    // of real WorkBuddy's `customTitle`.)
    let mut custom_title: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut workspace_path: Option<String> = None;
    let mut updated_at: u64 = 0;
    let mut resolved_calls: HashSet<String> = HashSet::new();
    let mut calls: Vec<&SessionEvent> = Vec::new();
    let mut last_user_at: Option<u64> = None;
    let mut last_assistant_at: Option<u64> = None;
    let mut last_assistant_incomplete = false;

    for ev in events {
        if ev.timestamp > updated_at {
            updated_at = ev.timestamp;
        }
        if ev.event_type == "custom-title" {
            let t = ev
                .custom_title
                .as_ref()
                .or(ev.title.as_ref())
                .filter(|t| !t.is_empty());
            if let Some(t) = t {
                custom_title = Some(t.clone());
            }
        } else if ev.event_type == "ai-title" {
            if let Some(t) = ev.ai_title.as_ref().filter(|t| !t.is_empty()) {
                ai_title = Some(t.clone());
            }
        }
        // Prefer the newest non-empty cwd — task workspaces can be created
        // after the first event and later lines carry the real path.
        if let Some(cwd) = ev.cwd.as_ref().filter(|c| !c.is_empty()) {
            workspace_path = Some(cwd.clone());
        }

        match ev.event_type.as_str() {
            "function_call" => calls.push(ev),
            "function_call_result" => {
                if let Some(id) = &ev.call_id {
                    resolved_calls.insert(id.clone());
                }
            }
            "message" => match ev.role.as_deref() {
                Some("user") => {
                    last_user_at = Some(max_ts(last_user_at, ev.timestamp));
                }
                Some("assistant") => {
                    // Track the chronologically last assistant message only —
                    // older incomplete rows left in the log must not pin status.
                    if last_assistant_at.map(|t| ev.timestamp >= t).unwrap_or(true) {
                        last_assistant_at = Some(ev.timestamp);
                        last_assistant_incomplete = ev.status.as_deref() == Some("incomplete");
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // A call is pending if it has no matching result row and isn't explicitly
    // completed. We keep the newest pending-of-each-kind timestamp; recency
    // filtering happens in `infer_status` so zombies age out.
    let mut pending_tool_at: Option<u64> = None;
    let mut pending_approval_at: Option<u64> = None;
    let mut waiting_detail: Option<String> = None;
    for ev in &calls {
        let resolved = ev
            .call_id
            .as_deref()
            .map(|id| resolved_calls.contains(id))
            .unwrap_or(false);
        let done = ev.status.as_deref() == Some("completed");
        if resolved || done {
            continue;
        }
        if requires_approval(&ev.arguments) {
            pending_approval_at = Some(max_ts(pending_approval_at, ev.timestamp));
            // Prefer the newest approval-pending tool as the detail.
            if waiting_detail.is_none()
                || pending_approval_at.is_some_and(|t| ev.timestamp >= t)
            {
                waiting_detail = ev.name.clone();
            }
        } else {
            pending_tool_at = Some(max_ts(pending_tool_at, ev.timestamp));
        }
    }

    SessionSignals {
        session_id: session_id.to_string(),
        // User rename always wins over model-generated title.
        title: custom_title.or(ai_title),
        workspace_path,
        updated_at,
        pending_tool_at,
        pending_approval_at,
        last_user_at,
        last_assistant_at,
        last_assistant_incomplete,
        file_mtime_ms,
        detail: waiting_detail,
    }
}

/// Soft Working needs both a semantic signal and recent file writes.
/// Event timestamps alone are not enough after Stop: WorkBuddy freezes the
/// incomplete row without rewriting it to `completed`.
fn stream_is_live(signals: &SessionSignals, now: u64) -> bool {
    within(now, signals.file_mtime_ms.or(Some(signals.updated_at)), STREAMING_WINDOW_MS)
}

fn max_ts(prev: Option<u64>, ts: u64) -> u64 {
    prev.map(|p| p.max(ts)).unwrap_or(ts)
}

fn within(now: u64, ts: Option<u64>, window_ms: u64) -> bool {
    match ts {
        Some(t) => now.saturating_sub(t) <= window_ms,
        None => false,
    }
}

/// Decode the `requires_approval` flag from a tool's JSON arguments string.
fn requires_approval(arguments: &Option<String>) -> bool {
    let Some(args) = arguments else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v.get("requires_approval").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// Map aggregated signals to a deck status.
///
/// Priority (high → low):
///
/// 1. **Waiting** — recent approval-pending tool
/// 2. **Working** — recent pending tool, OR (still-live) streaming /
///    thinking soft signals
/// 3. **Done** — last assistant turn completed recently, **or** a stalled
///    interrupted stream whose file stopped updating. Green flash that decays.
/// 4. **Idle** — open/dormant conversation, not recently completed, not
///    currently running. The steady state for bound sessions.
pub fn infer_status(signals: &SessionSignals, now: u64) -> DeckStatus {
    // 1) Waiting on the user (approval). Longer window than active tools.
    if within(now, signals.pending_approval_at, WAITING_WINDOW_MS) {
        return DeckStatus::Waiting;
    }

    // 2a) Tool currently executing (fresh pending function_call). Tools can
    // legitimately run for minutes without new jsonl lines, so use the longer
    // ACTIVE_WINDOW and do NOT require file mtime freshness.
    if within(now, signals.pending_tool_at, ACTIVE_WINDOW_MS) {
        return DeckStatus::Working;
    }

    let live = stream_is_live(signals, now);

    // 2b) Assistant message still streaming AND the jsonl is still being
    // written. After user Stop, incomplete freezes and mtime goes stale → not
    // Working.
    if signals.last_assistant_incomplete
        && within(now, signals.last_assistant_at, STREAMING_WINDOW_MS)
        && live
    {
        return DeckStatus::Working;
    }

    // 2c) User just sent a message and no assistant reply has started/finished
    // yet — the model is thinking. Require live file writes so a cancelled
    // turn does not pin Working for minutes.
    if within(now, signals.last_user_at, STREAMING_WINDOW_MS) && live {
        let user_ts = signals.last_user_at.unwrap_or(0);
        let asst_ts = signals.last_assistant_at.unwrap_or(0);
        if user_ts >= asst_ts {
            return DeckStatus::Working;
        }
    }

    // 3a) Done: the last assistant turn completed. Host-core owns Done→Idle
    // decay (open-aware TTL); mapper reports completion without a short TTL.
    if signals.last_assistant_at.is_some() && !signals.last_assistant_incomplete {
        return DeckStatus::Done;
    }

    // 3b) Done: interrupted stream. WorkBuddy leaves status=incomplete after
    // Stop; once the file is no longer live we treat the freeze point as a
    // completion so the key goes green instead of staying on run. Host still
    // applies open-aware TTL on top of this Done.
    if signals.last_assistant_incomplete
        && signals.last_assistant_at.is_some()
        && !live
    {
        return DeckStatus::Done;
    }

    // 4) Idle: conversation is open/dormant and has no completed/interrupted
    // turn the mapper can treat as Done (e.g. title-only / brand new).
    DeckStatus::Idle
}

pub fn map_session(signals: &SessionSignals, now: u64) -> SessionSnapshot {
    let status = infer_status(signals, now);
    let risk = if status == DeckStatus::Waiting {
        Some(infer_risk(signals.detail.as_deref()))
    } else {
        None
    };
    SessionSnapshot {
        backend: BackendId::Workbuddy,
        session_id: signals.session_id.clone(),
        title: signals
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "(untitled)".into()),
        status,
        risk,
        detail: signals.detail.clone().filter(|d| !d.is_empty()),
        waiting_since: if status == DeckStatus::Waiting {
            signals.pending_approval_at.or(Some(signals.updated_at))
        } else {
            None
        },
        // For Done, prefer the completion/interrupt timestamp so host open-aware
        // TTL measures from the actual turn end, not a later title/cwd event.
        updated_at: if status == DeckStatus::Done {
            signals
                .last_assistant_at
                .unwrap_or(signals.updated_at)
        } else {
            signals.updated_at
        },
        workspace_path: signals.workspace_path.clone(),
        // Filled by the observer from workbuddy.db + path heuristics.
        project_category: None,
        project_label: None,
    }
}

/// Keyword heuristic over the waiting tool name → risk tier. Mirrors the zcode
/// mapper's intent: destructive tools escalate, read-only tools stay low.
pub fn infer_risk(detail: Option<&str>) -> Risk {
    let Some(detail) = detail else {
        return Risk::Medium;
    };
    if detail.is_empty() {
        return Risk::Medium;
    }
    let d = detail.to_lowercase();
    if d.contains("bash")
        || d.contains("edit")
        || d.contains("write")
        || d.contains("remove")
        || d.contains("delete")
        || d.contains("multiedit")
    {
        Risk::High
    } else if d.contains("read") || d.contains("glob") || d.contains("grep") {
        Risk::Low
    } else {
        Risk::Medium
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event_type: &str, ts: u64) -> SessionEvent {
        SessionEvent {
            timestamp: ts,
            event_type: event_type.into(),
            ai_title: None,
            custom_title: None,
            title: None,
            session_id: None,
            cwd: None,
            call_id: None,
            name: None,
            status: None,
            arguments: None,
            role: None,
        }
    }

    fn user(ts: u64) -> SessionEvent {
        let mut e = ev("message", ts);
        e.role = Some("user".into());
        e
    }

    fn assistant(ts: u64, status: &str) -> SessionEvent {
        let mut e = ev("message", ts);
        e.role = Some("assistant".into());
        e.status = Some(status.into());
        e
    }

    /// Aggregate with an explicit file mtime. Soft Working requires a fresh
    /// mtime (`STREAMING_WINDOW_MS`); hard Working (pending tools) does not.
    fn agg(events: &[SessionEvent], id: &str, file_mtime_ms: Option<u64>) -> SessionSignals {
        aggregate(events, id, file_mtime_ms)
    }

    // ── Working signals ──

    #[test]
    fn working_when_pending_unapproved_call() {
        let now = 10_000_000;
        let mut e = ev("function_call", now - 1_000);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.status = Some("running".into());
        // Pending tools stay Working even if the file mtime is a bit stale —
        // tools can run without intermediate jsonl writes.
        let s = agg(&[e], "s1", Some(now - 60_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Working);
        assert!(s.pending_tool_at.is_some());
        assert!(s.pending_approval_at.is_none());
    }

    #[test]
    fn working_when_assistant_message_is_incomplete() {
        // The "缓解脑雾的方法" case: model is streaming text, no live tool.
        let now = 10_000_000;
        let events = vec![
            user(now - 30_000),
            assistant(now - 5_000, "incomplete"),
        ];
        let s = agg(&events, "s1", Some(now - 1_000));
        assert!(s.last_assistant_incomplete);
        assert_eq!(infer_status(&s, now), DeckStatus::Working);
    }

    #[test]
    fn working_when_user_just_spoke_and_no_reply_yet() {
        // Gap between user message and first assistant/tool event: model is
        // thinking. Without this, the key stays Idle/Done for the whole think phase.
        let now = 10_000_000;
        let events = vec![
            user(now - 120_000),
            assistant(now - 100_000, "completed"),
            user(now - 2_000),
            ev("file-history-snapshot", now - 1_500),
        ];
        let s = agg(&events, "s1", Some(now - 1_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Working);
    }

    #[test]
    fn working_when_user_speaks_into_done_session() {
        // Regression: session was Done (completed 3 min ago), user sends new
        // message → must become Working, NOT Idle.
        let now = 10_000_000;
        let events = vec![
            user(now - 200_000),
            assistant(now - 180_000, "completed"), // completed 3 min ago → Done
            user(now - 3_000),                     // user just spoke → Working!
        ];
        let s = agg(&events, "s1", Some(now - 1_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Working);
    }

    // ── Waiting signals ──

    #[test]
    fn waiting_when_pending_call_requests_approval() {
        let now = 10_000_000;
        let mut e = ev("function_call", now - 1_000);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.arguments = Some(r#"{"command":"rm -rf /","requires_approval":true}"#.into());
        let s = agg(&[e], "s1", Some(now));
        assert_eq!(infer_status(&s, now), DeckStatus::Waiting);
        assert_eq!(s.detail.as_deref(), Some("Bash"));
    }

    #[test]
    fn waiting_survives_longer_than_active_tool_window() {
        // User stepped away for 10 minutes — still Waiting, not aged out.
        let now = 10_000_000;
        let mut e = ev("function_call", now - 10 * 60 * 1000);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.arguments = Some(r#"{"requires_approval":true}"#.into());
        let s = agg(&[e], "s1", Some(now - 10 * 60 * 1000));
        assert_eq!(infer_status(&s, now), DeckStatus::Waiting);
    }

    // ── Done signals ──

    #[test]
    fn done_when_assistant_just_completed() {
        // Turn just finished → green flash.
        let now = 10_000_000;
        let events = vec![
            user(now - 60_000),
            assistant(now - 5_000, "completed"),
        ];
        let s = agg(&events, "s1", Some(now - 5_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn completed_stays_done_without_mapper_ttl() {
        // Mapper no longer applies DONE_TTL; host-core owns open-aware decay.
        // An old completed turn must still surface as Done here.
        let now = 100_000_000;
        let completed_at = now - 10 * 60 * 60 * 1000; // 10h ago
        let events = vec![
            user(completed_at - 60_000),
            assistant(completed_at, "completed"),
        ];
        let s = agg(&events, "s1", Some(completed_at));
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn interrupted_incomplete_becomes_done_not_working() {
        // User hit Stop: incomplete freezes, jsonl stops appending.
        // After STREAMING_WINDOW the key must leave `run` and go green Done.
        let now = 10_000_000;
        let freeze = now - 45_000; // frozen 45s ago (> 30s streaming window)
        let events = vec![user(freeze - 10_000), assistant(freeze, "incomplete")];
        let s = agg(&events, "s1", Some(freeze));
        assert!(s.last_assistant_incomplete);
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    // ── Idle signals ──

    #[test]
    fn old_completed_conversation_is_done_not_idle() {
        // Mapper reports Done for any completed turn; host may later decay.
        let now = 10_000_000;
        let events = vec![
            user(now - 600_000),
            assistant(now - 500_000, "completed"),
        ];
        let s = agg(&events, "s1", Some(now - 500_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn idle_when_no_assistant_at_all() {
        // Brand new conversation with only a title event → Idle.
        let now = 10_000_000;
        let mut e = ev("ai-title", now - 1_000);
        e.ai_title = Some("hello".into());
        let s = agg(&[e], "s1", Some(now - 1_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Idle);
    }

    // ── Zombie / edge cases ──

    #[test]
    fn zombie_pending_tool_ages_out_to_done_when_completed() {
        // Abandoned sessions leave function_call rows without a matching result
        // for days. Those must not pin Working forever; with a completed turn
        // the mapper surfaces Done (host may later decay to Idle).
        let now = 10_000_000;
        let mut e = ev("function_call", now - 10 * ACTIVE_WINDOW_MS);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        let a = assistant(now - 10 * ACTIVE_WINDOW_MS, "completed");
        let s = agg(&[e, a], "s1", Some(now - 10 * ACTIVE_WINDOW_MS));
        assert!(s.pending_tool_at.is_some());
        // Zombie tool aged out of ACTIVE_WINDOW → Done from the completed turn.
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn stranded_incomplete_becomes_done_not_working() {
        // An old incomplete left after Stop must not pin Working forever.
        // Once the file is frozen, mapper reports Done; host owns Idle decay.
        let now = 10_000_000;
        let events = vec![
            user(now - 20 * ACTIVE_WINDOW_MS),
            assistant(now - 20 * ACTIVE_WINDOW_MS, "incomplete"),
        ];
        let s = agg(&events, "s1", Some(now - 20 * ACTIVE_WINDOW_MS));
        assert!(s.last_assistant_incomplete);
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn completed_call_does_not_count_as_pending() {
        let now = 10_000_000;
        let mut e = ev("function_call", now - 10 * ACTIVE_WINDOW_MS);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.status = Some("completed".into());
        let s = agg(&[e], "s1", Some(now - 10 * ACTIVE_WINDOW_MS));
        assert!(s.pending_tool_at.is_none());
    }

    #[test]
    fn call_resolved_by_result_does_not_count_as_pending() {
        let now = 10_000_000;
        let mut call = ev("function_call", now - 60_000);
        call.call_id = Some("c1".into());
        call.name = Some("Read".into());
        let mut result = ev("function_call_result", now - 59_000);
        result.call_id = Some("c1".into());
        let s = agg(&[call, result], "s1", Some(now - 59_000));
        assert!(s.pending_tool_at.is_none());
        assert!(s.pending_approval_at.is_none());
    }

    #[test]
    fn last_assistant_wins_over_older_incomplete() {
        // Stream can contain incomplete then a later completed for the same
        // turn — only the chronologically last assistant status matters.
        let now = 10_000_000;
        let events = vec![
            user(now - 30_000),
            assistant(now - 20_000, "incomplete"),
            assistant(now - 5_000, "completed"),
        ];
        let s = agg(&events, "s1", Some(now - 5_000));
        assert!(!s.last_assistant_incomplete);
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn completed_reply_after_user_is_done_not_idle() {
        // Key fix: completed turn recently → Done (green), not Idle (gray).
        let now = 10_000_000;
        let events = vec![
            user(now - 30_000),
            assistant(now - 5_000, "completed"),
        ];
        let s = agg(&events, "s1", Some(now - 5_000));
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    // ── Metadata / risk ──

    #[test]
    fn title_prefers_custom_over_ai() {
        // Real WorkBuddy field is `customTitle`, not `title`.
        let mut ai = ev("ai-title", 100);
        ai.ai_title = Some("AI guess".into());
        let mut custom = ev("custom-title", 200);
        custom.custom_title = Some("My title".into());
        let s = agg(&[ai, custom], "s1", Some(200));
        assert_eq!(s.title.as_deref(), Some("My title"));
    }

    #[test]
    fn title_rename_overrides_previous_ai_title() {
        // User renames after the model already set an ai-title.
        let mut ai = ev("ai-title", 100);
        ai.ai_title = Some("打招呼".into());
        let mut custom = ev("custom-title", 300);
        custom.custom_title = Some("新名字".into());
        let s = agg(&[ai, custom], "s1", Some(300));
        assert_eq!(s.title.as_deref(), Some("新名字"));
    }

    #[test]
    fn later_ai_title_updates_when_no_custom() {
        // Model may re-title; without a custom-title, keep the latest ai title.
        let mut a1 = ev("ai-title", 100);
        a1.ai_title = Some("旧标题".into());
        let mut a2 = ev("ai-title", 200);
        a2.ai_title = Some("新标题".into());
        let s = agg(&[a1, a2], "s1", Some(200));
        assert_eq!(s.title.as_deref(), Some("新标题"));
    }

    #[test]
    fn custom_title_legacy_title_field_still_works() {
        let mut custom = ev("custom-title", 100);
        custom.title = Some("legacy field".into());
        let s = agg(&[custom], "s1", Some(100));
        assert_eq!(s.title.as_deref(), Some("legacy field"));
    }

    #[test]
    fn workspace_path_takes_latest_cwd() {
        let mut e1 = ev("message", 100);
        e1.cwd = Some("/tmp/old".into());
        let mut e2 = ev("message", 200);
        e2.cwd = Some("/Users/munich/WorkBuddy/2026-07-23-16-37-26".into());
        let s = agg(&[e1, e2], "s1", Some(200));
        assert_eq!(
            s.workspace_path.as_deref(),
            Some("/Users/munich/WorkBuddy/2026-07-23-16-37-26")
        );
    }

    #[test]
    fn workspace_path_taken_from_cwd() {
        let mut e = ev("message", 100);
        e.cwd = Some("/Users/x/WorkBuddy/proj".into());
        let s = agg(&[e], "s1", Some(100));
        assert_eq!(
            s.workspace_path.as_deref(),
            Some("/Users/x/WorkBuddy/proj")
        );
    }

    #[test]
    fn risk_heuristics() {
        assert_eq!(infer_risk(Some("Bash")), Risk::High);
        assert_eq!(infer_risk(Some("Edit")), Risk::High);
        assert_eq!(infer_risk(Some("Read")), Risk::Low);
        assert_eq!(infer_risk(Some("Grep")), Risk::Low);
        assert_eq!(infer_risk(Some("WebFetch")), Risk::Medium);
        assert_eq!(infer_risk(None), Risk::Medium);
    }

    #[test]
    fn map_session_sets_workbuddy_backend() {
        let now = 10_000_000;
        let mut e = ev("ai-title", now - 1_000);
        e.ai_title = Some("hello".into());
        e.cwd = Some("/tmp/p".into());
        let s = agg(&[e], "s1", Some(now - 1_000));
        let snap = map_session(&s, now);
        assert_eq!(snap.backend, BackendId::Workbuddy);
        assert_eq!(snap.session_id, "s1");
        assert_eq!(snap.title, "hello");
        assert_eq!(snap.workspace_path.as_deref(), Some("/tmp/p"));
        assert_eq!(snap.status, DeckStatus::Idle);
    }

    #[test]
    fn map_session_streaming_is_working() {
        let now = 10_000_000;
        let events = vec![user(now - 5_000), assistant(now - 1_000, "incomplete")];
        let s = agg(&events, "brainfog", Some(now - 500));
        let snap = map_session(&s, now);
        assert_eq!(snap.status, DeckStatus::Working);
        assert_eq!(snap.session_id, "brainfog");
    }
}
