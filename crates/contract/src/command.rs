use serde::{Deserialize, Serialize};

use crate::ids::{ModuleId, TaskId};

/// Controller -> implant instruction. The core handles the lifecycle variants
/// itself; for [`Command::Invoke`] it only routes by `module` and never
/// interprets the payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Liveness check.
    Ping,
    /// Ask the implant to return its [`crate::Manifest`].
    Describe,
    /// Invoke a module action.
    Invoke(Invoke),
    /// Cancel a running task.
    Cancel { task: TaskId },
    /// Query stored loot.
    Loot(LootQuery),
    /// Fetch one loot item's bytes by key. Deliberately separate from
    /// [`Command::Loot`] (which only ever returns metadata) — bulk content
    /// only leaves the device when a specific key is asked for by name, not
    /// as a side effect of listing.
    LootFetch { key: String },
    /// Shut the implant down.
    Shutdown { mode: ShutdownMode },
    // Reserved: offline autonomy ("Mission") is a future additive variant here —
    // a queued list of commands the device runs without the controller.
}

/// A module invocation. `params` is opaque to the core and deserialized by the
/// target module into its own typed request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Invoke {
    /// Target module, e.g. `"wifi.deauth"`.
    pub module: ModuleId,
    /// Module-defined verb.
    pub action: String,
    /// Opaque, module-defined parameters.
    #[serde(default)]
    pub params: RawParams,
    /// Optional per-invocation timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Opaque, module-defined payload. JSON in v1; the newtype isolates the wire so
/// a future move to bytes+encoding (e.g. protobuf) touches nothing else.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawParams(pub serde_json::Value);

/// Filter for a loot query.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LootQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<LootKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Category of a stored loot item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LootKind {
    Hash,
    Handshake,
    Credential,
    Pcap,
    Telemetry,
    File,
    Other,
}

/// Metadata for one stored loot item, as returned by a [`Command::Loot`] query.
/// Bulk bytes stay on the device; the wire only carries this reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootEntry {
    pub key: String,
    pub kind: LootKind,
    pub size: u64,
}

/// One loot item's actual bytes, as returned by [`Command::LootFetch`]. JSON
/// array-of-numbers encoding for now (fine at today's sizes — small JSON
/// blobs); worth revisiting (e.g. base64) once a module stores something
/// `Pcap`/`File`-sized.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LootContent {
    pub key: String,
    pub kind: LootKind,
    pub bytes: Vec<u8>,
}

/// How the implant should shut down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMode {
    Graceful,
    Reboot,
    /// Zeroize loot before powering off (tamper / self-destruct).
    Wipe,
}
