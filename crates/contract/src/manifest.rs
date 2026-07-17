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
    /// The MITRE ATT&CK tactic this module serves (the "phase"), orthogonal to
    /// the protocol-based id. Lets a UI group/filter by kill-chain phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tactic: Option<Tactic>,
}

/// A MITRE ATT&CK tactic (kill-chain phase). Serialized as the ATT&CK slug, e.g.
/// `"credential-access"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tactic {
    Reconnaissance,
    Discovery,
    CredentialAccess,
    LateralMovement,
    Collection,
    Exfiltration,
    CommandAndControl,
    Execution,
    Impact,
    /// Anything outside the standard set.
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Declared parameters, so a controller/UI can show help and build inputs
    /// without knowing the module. Empty -> the UI falls back to a raw
    /// `key=value` editor.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// Optional JSON Schema for advanced validation (rarely needed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params_schema: Option<String>,
}

/// A declared parameter of an action — enough to render help or a simple form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    /// Free-form type hint: `"host"`, `"port-spec"`, `"int"`, `"bool"`, `"string"`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

impl ParamSpec {
    pub fn required(name: &str, type_hint: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            required: true,
            type_hint: Some(type_hint.to_string()),
            description: Some(description.to_string()),
            default: None,
            example: None,
        }
    }

    pub fn optional(name: &str, type_hint: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            required: false,
            type_hint: Some(type_hint.to_string()),
            description: Some(description.to_string()),
            default: None,
            example: None,
        }
    }

    pub fn with_default(mut self, default: &str) -> Self {
        self.default = Some(default.to_string());
        self
    }

    pub fn with_example(mut self, example: &str) -> Self {
        self.example = Some(example.to_string());
        self
    }
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
