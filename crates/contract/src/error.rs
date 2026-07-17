use serde::{Deserialize, Serialize};

use crate::ids::ModuleId;

/// Implant -> controller: a command could not be processed. Structured so the
/// external brain can react programmatically instead of parsing strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<ModuleId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No module with the requested id is registered.
    UnknownModule,
    /// The module exists but a required [`crate::Capability`] is absent.
    MissingCapability,
    /// `params` did not match the module's expected shape.
    InvalidParams,
    /// The module does not support the requested action.
    Unsupported,
    /// The task exceeded its timeout.
    Timeout,
    /// The implant is at capacity.
    Busy,
    /// Unexpected internal failure.
    Internal,
}
