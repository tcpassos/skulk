//! Wire + bus contract for the implant engine.
//!
//! Every message on the external control socket AND on the internal event bus
//! is an [`Envelope`]. The socket adapter is a thin serde translator: the same
//! types are the bus payload and the wire message.
//!
//! Design stance (v1): the engine is driven live by an *external* agnostic
//! controller — an operator, a script, or an AI orchestrator that speaks this
//! protocol — never a brain embedded in the device. Offline autonomy
//! ("missions") is intentionally out of scope for v1 but reserved as an additive
//! [`Command`] variant so it can be added later without breaking the wire.

pub mod command;
pub mod envelope;
pub mod error;
pub mod event;
pub mod ids;
pub mod manifest;
pub mod response;

pub use command::{Command, Invoke, LootEntry, LootKind, LootQuery, RawParams, ShutdownMode};
pub use envelope::{Body, Envelope};
pub use error::{ErrorCode, ProtocolError};
pub use event::{Event, LogLevel, Severity, ViewLine, ViewManifest};
pub use ids::{MessageId, ModuleId, TaskId};
pub use manifest::{
    ActionSpec, Capability, ImplantInfo, Manifest, ModuleDescriptor, ParamSpec, Peripheral,
    PeripheralKind, Tactic,
};
pub use response::{Ack, TaskResult, TaskStatus};

/// Protocol version, carried in every [`Envelope`] for capability negotiation.
pub const PROTOCOL_VERSION: u16 = 1;
