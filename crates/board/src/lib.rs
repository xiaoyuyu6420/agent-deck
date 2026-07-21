//! Board layer: theme, slot allocator, session board.
//! Ported from packages/host/src/board/*

mod session_board;
mod slot_allocator;
mod theme;

pub use session_board::SessionBoard;
pub use slot_allocator::{allocate_slots, AllocatedSlot, ScoredSession, SlotAllocatorOptions};
pub use theme::{paint, ThemeInput, ThemeOutput, ThemePalette, CODEX_THEME};
