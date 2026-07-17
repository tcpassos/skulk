use serde::{Deserialize, Serialize};

use crate::ids::ModuleId;

/// Full self-description returned in answer to [`crate::Command::Describe`].
/// Lets a controller (or an AI orchestrator) discover what this implant can do
/// without prior knowledge of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub protocol: u16,
    pub implant: ImplantInfo,
    pub modules: Vec<ModuleDescriptor>,
    /// Everything the hardware currently offers.
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImplantInfo {
    pub id: String,
    pub hardware: String,
    pub firmware: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleDescriptor {
    pub id: ModuleId,
    pub version: String,
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    /// Capabilities this module requires. The core refuses to invoke it — with a
    /// structured [`crate::ErrorCode::MissingCapability`] — unless every one is
    /// present, instead of crashing mid-attack.
    #[serde(default)]
    pub requires: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional JSON Schema (as a string) describing this action's params, for
    /// validation and controller-side UIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_schema: Option<String>,
}

/// A hardware or platform capability a module can require. `Other` keeps the set
/// open without a protocol bump.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    MonitorMode,
    PacketInjection,
    AccessPoint,
    Bluetooth,
    Nrf24,
    Sdr,
    UsbGadget,
    UsbHid,
    MassStorage,
    Accelerometer,
    Display,
    Other(String),
}
