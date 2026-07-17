use serde::{Deserialize, Serialize};

use crate::command::RawParams;
use crate::ids::TaskId;

/// Implant -> controller: a command was accepted and a task started. Sent
/// immediately so a long-running action doesn't block the channel; the terminal
/// [`TaskResult`] follows later, and [`crate::Event::Progress`] pings in between.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    pub task: TaskId,
}

/// Implant -> controller: terminal outcome of a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub task: TaskId,
    pub status: TaskStatus,
    /// Module-defined structured result (opaque to the core). Bulk data (pcaps,
    /// large scans) is stored as loot and referenced by key, not embedded here.
    #[serde(default)]
    pub output: RawParams,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Ok,
    Error,
    Cancelled,
    Timeout,
}
