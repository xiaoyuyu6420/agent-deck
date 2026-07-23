//! Codex ThreadStatus → SessionSnapshot mapper.
//! See docs/codex-integration.md for the status mapping table.

use agent_deck_protocol::{BackendId, DeckStatus, SessionSnapshot};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThread {
    pub id: String,
    /// Rollout's git/worktree session id. Distinct from `id` (the canonical
    /// thread id / rollout UUID) — kept for deserialization completeness but
    /// NOT used for session addressing (deep link / resume use `id`). See
    /// `map_thread`.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub status: ThreadStatus,
    /// Unix seconds (codex) — converted to ms for SessionSnapshot.
    #[serde(default)]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub recency_at: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadStatus {
    #[default]
    #[serde(rename = "notLoaded")]
    NotLoaded,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "systemError")]
    SystemError,
    #[serde(rename = "active")]
    Active {
        #[serde(default)]
        active_flags: Vec<String>,
    },
}

pub fn map_thread(t: &CodexThread) -> SessionSnapshot {
    let status = map_status(&t.status);
    let title = t
        .name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| t.preview.clone().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "(untitled)".into());
    let updated_sec = t.updated_at.or(t.recency_at).unwrap_or(0);
    let updated_at = updated_sec.saturating_mul(1000);
    SessionSnapshot {
        backend: BackendId::Codex,
        // Canonical codex session identifier is `thread.id` (the rollout UUID),
        // NOT `thread.session_id`. The latter is a git/worktree session id (see
        // `codex_turn_diff_event`: it carries both `thread_id` and `session_id`
        // as distinct fields). They coincide for most threads, but the value
        // deep-linking (`codex://threads/<id>`) and `thread/resume {threadId}`
        // expect is `thread.id`. Verified 2026-07-23 against the ChatGPT.app
        // renderer and the app-server schema.
        session_id: t.id.clone(),
        title,
        status,
        risk: None,
        detail: None,
        waiting_since: if status == DeckStatus::Waiting {
            Some(updated_at)
        } else {
            None
        },
        updated_at,
        workspace_path: t.cwd.clone(),
        project_category: None,
        project_label: None,
    }
}

pub fn map_status(status: &ThreadStatus) -> DeckStatus {
    match status {
        ThreadStatus::SystemError => DeckStatus::Error,
        ThreadStatus::Active { active_flags } => {
            let waiting = active_flags
                .iter()
                .any(|f| f == "waitingOnApproval" || f == "waitingOnUserInput");
            if waiting {
                DeckStatus::Waiting
            } else {
                DeckStatus::Working
            }
        }
        // notLoaded / idle: treat as idle so they don't crowd out real work,
        // unless they were recently completed (caller may filter by recency).
        ThreadStatus::NotLoaded | ThreadStatus::Idle => DeckStatus::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_without_flags_is_working() {
        let t = CodexThread {
            id: "t1".into(),
            session_id: None,
            name: Some("work".into()),
            preview: None,
            cwd: Some("/tmp".into()),
            status: ThreadStatus::Active {
                active_flags: vec![],
            },
            updated_at: Some(1_700_000_000),
            recency_at: None,
        };
        let s = map_thread(&t);
        assert_eq!(s.status, DeckStatus::Working);
        assert_eq!(s.backend, BackendId::Codex);
    }

    #[test]
    fn active_waiting_on_approval_is_waiting() {
        let t = CodexThread {
            id: "t2".into(),
            session_id: None,
            name: None,
            preview: Some("need approve".into()),
            cwd: None,
            status: ThreadStatus::Active {
                active_flags: vec!["waitingOnApproval".into()],
            },
            updated_at: Some(1_700_000_000),
            recency_at: None,
        };
        let s = map_thread(&t);
        assert_eq!(s.status, DeckStatus::Waiting);
        assert!(s.waiting_since.is_some());
    }

    #[test]
    fn system_error_is_error() {
        let t = CodexThread {
            id: "t3".into(),
            session_id: None,
            name: Some("boom".into()),
            preview: None,
            cwd: None,
            status: ThreadStatus::SystemError,
            updated_at: None,
            recency_at: None,
        };
        assert_eq!(map_thread(&t).status, DeckStatus::Error);
    }
}
