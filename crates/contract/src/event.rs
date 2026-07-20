use serde::{Deserialize, Serialize};

use crate::command::{LootKind, RawParams};
use crate::ids::{ModuleId, TaskId};

/// Implant -> controller: unsolicited, asynchronous notification. This is also
/// the payload that sensors, progress, alerts and the on-device UI publish on
/// the internal bus — the socket adapter simply forwards the ones meant to
/// leave the device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Progress of a running task.
    Progress {
        task: TaskId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pct: Option<u8>,
        note: String,
    },
    /// Diagnostic log line from a module or the core.
    Log {
        level: LogLevel,
        source: ModuleId,
        msg: String,
    },
    /// Notable event worth surfacing on the tactical LCD.
    Alert {
        severity: Severity,
        source: ModuleId,
        msg: String,
    },
    /// A loot item was persisted.
    LootStored {
        key: String,
        kind: LootKind,
        size: u64,
    },
    /// A raw sensor reading (module-defined payload).
    Sensor {
        source: String,
        reading: RawParams,
    },
    /// Periodic liveness signal from the core, so a controller can tell the
    /// implant is still alive even when idle.
    Heartbeat {
        seq: u64,
    },
    /// Drives the dual on-device UI (LCD + TUI).
    ViewManifest(ViewManifest),
    /// Updates one slot of the on-device HUD/status band — a small keyed
    /// indicator (a value + severity) any module can publish independent of
    /// whether its full [`ViewManifest`] is the screen in focus. The LCD
    /// draws the latest value per slot over whatever screen is active; the
    /// theme maps the slot name to an icon. Unlike a `ViewManifest` (one
    /// module's whole screen), the HUD composites many modules' slots.
    Widget(WidgetUpdate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/// A minimal, renderer-agnostic view description. The LCD and the TUI both
/// subscribe to this and render it their own way — the "View Manifest" that
/// keeps the dual UI in sync without either renderer touching the modules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewManifest {
    pub screen: String,
    #[serde(default)]
    pub lines: Vec<ViewLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewLine {
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
}

/// One update to a HUD slot. The renderer keeps the latest update per `slot`
/// and draws them as a compact status band. An empty `value` clears the slot
/// (removes it from the band) — how a module retracts a transient indicator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetUpdate {
    /// Stable slot name, e.g. `"battery"` / `"temp"` / `"alert"`. Also the
    /// key a theme maps to an icon asset; the operator's HUD config decides
    /// which slots show and in what order.
    pub slot: String,
    /// Short text shown next to the icon, e.g. `"42%"`. Empty clears the slot.
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
}
