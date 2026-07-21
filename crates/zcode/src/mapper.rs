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
}

pub fn map_zcode_row(row: &ZcodeRow) -> SessionSnapshot {
    let status = map_status(row.task_status.as_deref(), row.waiting == 1);
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
    }
}

fn map_status(task_status: Option<&str>, waiting: bool) -> DeckStatus {
    match task_status {
        Some("error") => DeckStatus::Error,
        Some("completed") => DeckStatus::Done,
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
        ZcodeRow {
            task_id: "sess_test".into(),
            title: Some("test".into()),
            task_status: Some(status.into()),
            workspace_path: Some("/tmp".into()),
            updated_at: 1000,
            waiting,
            detail: detail.map(str::to_string),
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
    fn completed_is_done() {
        assert_eq!(
            map_zcode_row(&row("completed", 0, None)).status,
            DeckStatus::Done
        );
    }

    #[test]
    fn error_is_error() {
        assert_eq!(
            map_zcode_row(&row("error", 0, None)).status,
            DeckStatus::Error
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
