//! Pure ZCode row → SessionSnapshot mapper.
//! Ported from packages/host/src/backends/zcode/mapper.ts

use agent_deck_protocol::{BackendId, DeckStatus, Risk, SessionSnapshot};

#[derive(Debug, Clone)]
pub struct ZcodeRow {
    pub task_id: String,
    pub title: Option<String>,
    pub task_status: Option<String>,
    pub workspace_path: Option<String>,
    pub updated_at: u64,
    pub waiting: i64,
    pub detail: Option<String>,
    /// True when this session has a tool_usage row with `status='running'`,
    /// `completed_at IS NULL`, AND `started_at` within the last few minutes —
    /// i.e. the agent is actively executing a tool right now. The recency
    /// window is essential: ZCode 3.4.2 does NOT always write `completed_at`
    /// when a tool finishes, leaving zombie `running` rows forever; without
    /// the window a finished session would be pinned to Working indefinitely.
    /// This matters because `task_status` is NOT live either: ZCode writes
    /// `completed` when a turn ends and never flips it back to `running` when
    /// the user resumes, so a `completed` task that is still being driven
    /// shows a spinner in the ZCode UI but looks `done` from `task_status`
    /// alone. `active` mirrors that live signal from the only table written in
    /// real time (`tool_usage`), bounded by a window to dodge zombies.
    pub active: bool,
}

pub fn map_zcode_row(row: &ZcodeRow) -> SessionSnapshot {
    let status = map_status(row.task_status.as_deref(), row.waiting == 1, row.active);
    let risk = if status == DeckStatus::Waiting {
        Some(infer_risk(row.detail.as_deref()))
    } else {
        None
    };
    SessionSnapshot {
        backend: BackendId::Zcode,
        session_id: row.task_id.clone(),
        title: row
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "(untitled)".into()),
        status,
        risk,
        detail: row.detail.clone().filter(|d| !d.is_empty()),
        waiting_since: if status == DeckStatus::Waiting {
            Some(row.updated_at)
        } else {
            None
        },
        updated_at: row.updated_at,
        workspace_path: row.workspace_path.clone(),
        project_category: None,
        project_label: None,
    }
}

fn map_status(task_status: Option<&str>, waiting: bool, active: bool) -> DeckStatus {
    // `task_status` is a write-once-per-turn field: ZCode sets it to
    // `completed`/`error` when a turn ends and NEVER flips it back to `running`
    // when the conversation resumes. So both `completed` and `error` can describe
    // a session that is actively running a tool right now. The `active` flag
    // (derived from tool_usage) is the only live signal. When active, a resumed
    // session shows Working regardless of the stale task_status; only a truly
    // idle session honors the terminal status.
    match task_status {
        Some("error") => {
            if active {
                DeckStatus::Working
            } else {
                DeckStatus::Error
            }
        }
        Some("completed") => {
            if active {
                DeckStatus::Working
            } else {
                DeckStatus::Done
            }
        }
        Some("running") => {
            if waiting {
                DeckStatus::Waiting
            } else {
                DeckStatus::Working
            }
        }
        _ => DeckStatus::Idle,
    }
}

pub fn infer_risk(detail: Option<&str>) -> Risk {
    let Some(detail) = detail else {
        return Risk::Medium;
    };
    if detail.is_empty() {
        return Risk::Medium;
    }
    let d = detail.to_lowercase();

    let high = [
        "shell",
        "bash",
        "git push",
        "git reset",
        "rm ",
        "rm-",
        "delete",
        "destroy",
        "force",
        "sudo",
        "chmod",
        "mv ",
        "unlink",
    ];
    for kw in high {
        if d.contains(kw) {
            return Risk::High;
        }
    }

    let low = [
        "userinteraction",
        "askuser",
        "read",
        "grep",
        "glob",
        "list",
        "todo",
        "view",
        "ls ",
    ];
    for kw in low {
        if d.contains(kw) {
            return Risk::Low;
        }
    }

    Risk::Medium
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, waiting: i64, detail: Option<&str>) -> ZcodeRow {
        row_with_active(status, waiting, detail, false)
    }

    fn row_with_active(status: &str, waiting: i64, detail: Option<&str>, active: bool) -> ZcodeRow {
        ZcodeRow {
            task_id: "sess_test".into(),
            title: Some("test".into()),
            task_status: Some(status.into()),
            workspace_path: Some("/tmp".into()),
            updated_at: 1000,
            waiting,
            detail: detail.map(str::to_string),
            active,
        }
    }

    #[test]
    fn running_no_waiting_is_working() {
        let s = map_zcode_row(&row("running", 0, None));
        assert_eq!(s.status, DeckStatus::Working);
        assert!(s.risk.is_none());
    }

    #[test]
    fn running_waiting_bash_is_high() {
        let s = map_zcode_row(&row("running", 1, Some("Bash: shell")));
        assert_eq!(s.status, DeckStatus::Waiting);
        assert_eq!(s.risk, Some(Risk::High));
        assert_eq!(s.waiting_since, Some(1000));
    }

    #[test]
    fn completed_idle_is_done() {
        // completed + no tool in flight → genuinely finished.
        assert_eq!(
            map_zcode_row(&row("completed", 0, None)).status,
            DeckStatus::Done
        );
    }

    #[test]
    fn completed_but_active_is_working() {
        // Regression: a `completed` task whose conversation resumed and is now
        // running a tool must show Working, not Done. task_status stays
        // `completed` across turns, so the live tool_usage signal is the only
        // way to know it's still going — this is what the ZCode UI spinner
        // reflects.
        assert_eq!(
            map_zcode_row(&row_with_active("completed", 0, None, true)).status,
            DeckStatus::Working
        );
    }

    #[test]
    fn error_is_error() {
        // error + no activity → genuinely errored.
        assert_eq!(
            map_zcode_row(&row("error", 0, None)).status,
            DeckStatus::Error
        );
    }

    #[test]
    fn error_but_active_is_working() {
        // Regression: a session that errored on a previous turn but has since
        // resumed (tool_usage shows activity) must show Working, not Error.
        // task_status stays `error` across resumes, just like `completed`.
        assert_eq!(
            map_zcode_row(&row_with_active("error", 0, None, true)).status,
            DeckStatus::Working
        );
    }

    #[test]
    fn askuser_is_low_risk() {
        assert_eq!(
            infer_risk(Some("AskUserQuestion: userInteraction")),
            Risk::Low
        );
    }
}
