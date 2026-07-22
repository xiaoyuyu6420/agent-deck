//! Agent Deck shared protocol — single source of truth.
//! Ported from packages/protocol/src/index.ts

use serde::{Deserialize, Serialize};

pub const SLOT_COUNT: usize = 8;
pub const DONE_TTL_MS: u64 = 5 * 60 * 1000;
pub const URGENCY_FULL_WAIT_MS: u64 = 2 * 60 * 1000;
pub const WORKING_LONG_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendId {
    Zcode,
    Codex,
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
