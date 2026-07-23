//! Agent Deck shared protocol — single source of truth.
//! Ported from packages/protocol/src/index.ts

use serde::{Deserialize, Serialize};

pub const SLOT_COUNT: usize = 8;
/// How long a Done key stays green after the user opens it from Agent Deck.
/// Unopened Done sessions keep green until `DONE_TTL_UNOPENED_MS` instead.
pub const DONE_TTL_MS: u64 = 5 * 60 * 1000;
/// Max time an unopened Done session stays green before forced Idle.
pub const DONE_TTL_UNOPENED_MS: u64 = 12 * 60 * 60 * 1000;
pub const URGENCY_FULL_WAIT_MS: u64 = 2 * 60 * 1000;
pub const WORKING_LONG_MS: u64 = 5 * 60 * 1000;

/// Cross-platform user home directory.
///
/// Priority: `HOME` (macOS/Linux/mingw) → `USERPROFILE` (Windows) → `"."`.
/// On Windows `HOME` is usually unset, so without this fallback every
/// `~/.zcode` / `~/.workbuddy` / `~/.codex` / `~/.agent-deck` path would
/// resolve to the current working directory.
pub fn home_dir() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendId {
    Zcode,
    Codex,
    Workbuddy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeckStatus {
    Off,
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

impl DeckStatus {
    pub fn priority(self) -> u8 {
        match self {
            DeckStatus::Waiting => 5,
            DeckStatus::Error => 4,
            DeckStatus::Working => 3,
            DeckStatus::Done => 2,
            DeckStatus::Idle => 1,
            DeckStatus::Off => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn boost(self) -> f64 {
        match self {
            Risk::Low => 0.0,
            Risk::Medium => 0.25,
            Risk::High => 0.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedFx {
    Solid,
    Breathe,
    BlinkSlow,
    BlinkFast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMode {
    Plan,
    Act,
    Review,
}

/// Bind-picker grouping for a session's workspace.
/// WorkBuddy uses all three; other backends typically only set Project (or none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectCategory {
    /// Real project folder the user chose (e.g. ~/Desktop/modjing).
    Project,
    /// Ad-hoc WorkBuddy "任务" (timestamp folder under ~/WorkBuddy, is_playground).
    Task,
    /// Scheduled / background automation runs.
    Automation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub backend: BackendId,
    pub session_id: String,
    pub title: String,
    pub status: DeckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<Risk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<u64>,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// Bind-picker section (project / task / automation). Optional for backends
    /// that don't classify workspaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_category: Option<ProjectCategory>,
    /// Human label for the bind-picker row (task title, automation name, folder).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedSlot {
    pub i: usize,
    pub rgb: Option<[u8; 3]>,
    pub br: u8,
    pub fx: LedFx,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedFrame {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub slots: Vec<LedSlot>,
}

impl LedFrame {
    pub fn new(slots: Vec<LedSlot>) -> Self {
        Self {
            msg_type: "leds".into(),
            slots,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotBinding {
    pub i: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<BackendId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: DeckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    /// True when this slot is manually pinned to a session (survives recompute).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardState {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub slots: Vec<SlotBinding>,
    pub focus: usize,
    pub mode: PolicyMode,
}

impl BoardState {
    pub fn new(slots: Vec<SlotBinding>, focus: usize, mode: PolicyMode) -> Self {
        Self {
            msg_type: "board".into(),
            slots,
            focus,
            mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Action {
    Focus {
        i: usize,
    },
    Accept {
        #[serde(default)]
        i: Option<usize>,
    },
    Reject {
        #[serde(default)]
        i: Option<usize>,
    },
    Stop {
        #[serde(default)]
        i: Option<usize>,
    },
    StopAll,
    FreezeAll,
    Unfreeze,
    SetMode {
        mode: PolicyMode,
    },
    Send {
        #[serde(default)]
        i: Option<usize>,
        text: String,
    },
    /// Pin a session to slot `i`. `session_id = None` unpins the slot.
    Pin {
        i: usize,
        #[serde(default)]
        session_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_priority_order() {
        assert!(DeckStatus::Waiting.priority() > DeckStatus::Error.priority());
        assert!(DeckStatus::Error.priority() > DeckStatus::Working.priority());
        assert!(DeckStatus::Working.priority() > DeckStatus::Done.priority());
    }

    #[test]
    fn risk_boost_values() {
        assert_eq!(Risk::Low.boost(), 0.0);
        assert_eq!(Risk::Medium.boost(), 0.25);
        assert_eq!(Risk::High.boost(), 0.5);
    }
}
