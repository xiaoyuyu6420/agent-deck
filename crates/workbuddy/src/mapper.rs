//! Pure WorkBuddy jsonl-event → SessionSnapshot mapper.
//!
//! WorkBuddy appends one JSON object per line to
//! `~/.workbuddy/projects/<workspace>/<session-id>.jsonl`. Event `type`
//! values seen in the wild: `message`, `function_call`, `function_call_result`,
//! `reasoning`, `ai-title`, `custom-title`, `file-history-snapshot`. We only
//! decode the handful of fields the board cares about; unknown fields/lines
//! are ignored so the observer is forward-compatible with format drift.

use agent_deck_protocol::{BackendId, DeckStatus, Risk, SessionSnapshot};
use std::collections::HashSet;

/// A session is considered "recently touched" (→ Idle rather than Done) if its
/// last event landed inside this window. Mirrors the zcode observer's
/// RECENT_ACTIVITY heuristic: between tool calls the agent thinks/reads, so a
/// session with no live tool but fresh activity is an open conversation, not a
/// finished one.
const RECENT_WINDOW_MS: u64 = 5 * 60 * 1000;

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
    /// Generic title slot (covers `custom-title` events whose field is `title`).
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
    /// Tool-call lifecycle status (e.g. "completed").
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
#[derive(Debug, Clone, Default)]
pub struct SessionSignals {
    pub session_id: String,
    pub title: Option<String>,
    pub workspace_path: Option<String>,
    pub updated_at: u64,
    /// A tool call is still in flight (no matching result, not completed).
    pub active: bool,
    /// A pending tool call requested explicit approval.
    pub waiting: bool,
    /// Human-readable hint for the waiting tool, e.g. "Bash".
    pub detail: Option<String>,
}

/// Fold a session's decoded events into the signals the board needs.
pub fn aggregate(events: &[SessionEvent], session_id: &str) -> SessionSignals {
    let mut title: Option<String> = None;
    let mut workspace_path: Option<String> = None;
    let mut updated_at: u64 = 0;
    let mut resolved_calls: HashSet<String> = HashSet::new();
    let mut calls: Vec<&SessionEvent> = Vec::new();

    for ev in events {
        if ev.timestamp > updated_at {
            updated_at = ev.timestamp;
        }
        // Prefer a user-set custom title, then fall back to the AI title.
        if ev.event_type == "custom-title" {
            if let Some(t) = ev.title.as_ref().filter(|t| !t.is_empty()) {
                title = Some(t.clone());
            }
        } else if title.is_none() && ev.event_type == "ai-title" {
            if let Some(t) = ev.ai_title.as_ref().filter(|t| !t.is_empty()) {
                title = Some(t.clone());
            }
        }
        if workspace_path.is_none() {
            workspace_path = ev.cwd.clone().filter(|c| !c.is_empty());
        }
        match ev.event_type.as_str() {
            "function_call" => calls.push(ev),
            "function_call_result" => {
                if let Some(id) = &ev.call_id {
                    resolved_calls.insert(id.clone());
                }
            }
            _ => {}
        }
    }

    // A call is pending if it has no matching result row and isn't explicitly
    // completed. Pending + requires_approval ⇒ Waiting; pending otherwise ⇒
    // Working (tool executing right now).
    let mut waiting_detail: Option<String> = None;
    let mut active = false;
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
            // Keep the most recent approval-pending tool as the detail.
            if waiting_detail.is_none() {
                waiting_detail = ev.name.clone();
            }
        } else {
            active = true;
        }
    }
    let waiting = waiting_detail.is_some();

    SessionSignals {
        session_id: session_id.to_string(),
        title,
        workspace_path,
        updated_at,
        active,
        waiting,
        detail: waiting_detail,
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

/// Map aggregated signals to a deck status. `now` (ms epoch) gates the
/// Idle-vs-Done split: a session with no live tool but touched very recently
/// is treated as an open, idle conversation rather than finished.
pub fn infer_status(signals: &SessionSignals, now: u64) -> DeckStatus {
    if signals.waiting {
        return DeckStatus::Waiting;
    }
    if signals.active {
        return DeckStatus::Working;
    }
    if now.saturating_sub(signals.updated_at) <= RECENT_WINDOW_MS {
        return DeckStatus::Idle;
    }
    DeckStatus::Done
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
            Some(signals.updated_at)
        } else {
            None
        },
        updated_at: signals.updated_at,
        workspace_path: signals.workspace_path.clone(),
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

    #[test]
    fn idle_when_recently_touched_and_no_live_tool() {
        let now = 10_000_000;
        let events = vec![ev("message", now - 60_000)];
        let s = aggregate(&events, "s1");
        assert_eq!(infer_status(&s, now), DeckStatus::Idle);
    }

    #[test]
    fn done_when_quiet_and_old() {
        let now = 10_000_000;
        let events = vec![ev("message", now - 10 * RECENT_WINDOW_MS)];
        let s = aggregate(&events, "s1");
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn working_when_pending_unapproved_call() {
        let now = 10_000_000;
        let mut e = ev("function_call", now - 1_000);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.status = Some("running".into());
        let s = aggregate(&[e], "s1");
        assert_eq!(infer_status(&s, now), DeckStatus::Working);
        assert!(s.active);
        assert!(!s.waiting);
    }

    #[test]
    fn waiting_when_pending_call_requests_approval() {
        let now = 10_000_000;
        let mut e = ev("function_call", now - 1_000);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.arguments = Some(r#"{"command":"rm -rf /","requires_approval":true}"#.into());
        let s = aggregate(&[e], "s1");
        assert_eq!(infer_status(&s, now), DeckStatus::Waiting);
        assert_eq!(s.detail.as_deref(), Some("Bash"));
    }

    #[test]
    fn completed_call_does_not_count_as_pending() {
        let now = 10_000_000;
        // Far enough in the past to be outside the recent-activity window.
        let mut e = ev("function_call", now - 10 * RECENT_WINDOW_MS);
        e.call_id = Some("c1".into());
        e.name = Some("Bash".into());
        e.status = Some("completed".into());
        let s = aggregate(&[e], "s1");
        // old + completed + not recent → Done
        assert_eq!(infer_status(&s, now), DeckStatus::Done);
    }

    #[test]
    fn call_resolved_by_result_does_not_count_as_pending() {
        let now = 10_000_000;
        let mut call = ev("function_call", now - 60_000);
        call.call_id = Some("c1".into());
        call.name = Some("Read".into());
        let mut result = ev("function_call_result", now - 59_000);
        result.call_id = Some("c1".into());
        let s = aggregate(&[call, result], "s1");
        assert!(!s.active);
        assert!(!s.waiting);
    }

    #[test]
    fn title_prefers_custom_over_ai() {
        let mut ai = ev("ai-title", 100);
        ai.ai_title = Some("AI guess".into());
        let mut custom = ev("custom-title", 200);
        custom.title = Some("My title".into());
        let s = aggregate(&[ai, custom], "s1");
        assert_eq!(s.title.as_deref(), Some("My title"));
    }

    #[test]
    fn workspace_path_taken_from_cwd() {
        let mut e = ev("message", 100);
        e.cwd = Some("/Users/x/WorkBuddy/proj".into());
        let s = aggregate(&[e], "s1");
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
        let s = aggregate(&[e], "sid-1");
        let snap = map_session(&s, now);
        assert_eq!(snap.backend, BackendId::Workbuddy);
        assert_eq!(snap.session_id, "sid-1");
        assert_eq!(snap.title, "hello");
        assert_eq!(snap.workspace_path.as_deref(), Some("/tmp/p"));
        assert_eq!(snap.status, DeckStatus::Idle);
    }
}
