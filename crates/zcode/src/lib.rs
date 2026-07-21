//! ZCode backend: mapper + sqlite observer.
//! Ported from packages/host/src/backends/zcode/*

mod mapper;
mod observer;

pub use mapper::{infer_risk, map_zcode_row, ZcodeRow};
pub use observer::{SqliteObserver, SqliteObserverOptions};
