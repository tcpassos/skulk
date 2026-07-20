//! What a module author builds against: the [`ImplantModule`] trait, the
//! [`ModuleCtx`] handle, and the small support types around them.
//!
//! A module depends on this crate and on `contract` — never on the engine. That
//! keeps the dependency arrow pointing inward: modules know nothing about the
//! bus, the socket, or where loot is stored.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use contract::{
    Event, LogLevel, LootKind, LootQuery, ModuleDescriptor, ModuleId, RawParams, Severity, TaskId,
    ViewLine, ViewManifest, WidgetUpdate,
};

/// Re-exported so module authors can name it via `module_sdk::LootEntry`.
pub use contract::LootEntry;

/// A pluggable capability of the implant. Authors implement this and register
/// the module with the engine.
#[async_trait]
pub trait ImplantModule: Send + Sync {
    /// Static self-description: id, actions, and required capabilities. Feeds the
    /// manifest and the engine's capability-gating.
    fn descriptor(&self) -> ModuleDescriptor;

    /// Do the work for one action. `params` is opaque — parse it into your own
    /// type with [`ParseParams::parse`]. The returned value becomes the task
    /// output. Talk to the outside world only through `ctx`.
    async fn invoke(
        &self,
        ctx: &ModuleCtx,
        action: &str,
        params: RawParams,
    ) -> Result<RawParams, ModuleError>;

    /// Optional one-time setup when the module is loaded.
    async fn on_load(&self) -> Result<(), ModuleError> {
        Ok(())
    }
}

/// Per-invocation handle: the only way a module reaches the outside world.
/// Scoped to one `(module, task)`, so events and loot are attributed for you —
/// the module never fills in `source` or `task` itself.
pub struct ModuleCtx {
    module: ModuleId,
    task: TaskId,
    events: UnboundedSender<Event>,
    loot: Arc<dyn LootSink>,
    cancel: Cancel,
}

impl ModuleCtx {
    pub fn new(
        module: ModuleId,
        task: TaskId,
        events: UnboundedSender<Event>,
        loot: Arc<dyn LootSink>,
        cancel: Cancel,
    ) -> Self {
        Self { module, task, events, loot, cancel }
    }

    pub fn task(&self) -> TaskId {
        self.task
    }

    pub fn module(&self) -> &ModuleId {
        &self.module
    }

    /// Report progress of the running task.
    pub fn progress(&self, pct: Option<u8>, note: impl Into<String>) {
        let _ = self
            .events
            .send(Event::Progress { task: self.task, pct, note: note.into() });
    }

    /// Surface a tactical alert (also drives the on-device LCD).
    pub fn alert(&self, severity: Severity, msg: impl Into<String>) {
        let _ = self.events.send(Event::Alert {
            severity,
            source: self.module.clone(),
            msg: msg.into(),
        });
    }

    /// Emit a diagnostic log line.
    pub fn log(&self, level: LogLevel, msg: impl Into<String>) {
        let _ = self.events.send(Event::Log {
            level,
            source: self.module.clone(),
            msg: msg.into(),
        });
    }

    /// Push a live tactical view (drives the on-device LCD and, if the
    /// controller opted in, a remote TUI). `screen` is a short label for
    /// what's being shown; `lines` replaces the previous view wholesale.
    pub fn view(&self, screen: impl Into<String>, lines: Vec<ViewLine>) {
        let _ = self
            .events
            .send(Event::ViewManifest(ViewManifest { screen: screen.into(), lines }));
    }

    /// Update one slot of the on-device HUD/status band. Unlike [`view`], this
    /// is a small keyed indicator that persists across screen changes and
    /// composites with other modules' slots — the theme maps `slot` to an
    /// icon. An empty `value` clears the slot. Independent of whether this
    /// module's tactical view is the one on screen.
    ///
    /// [`view`]: ModuleCtx::view
    pub fn widget(&self, slot: impl Into<String>, value: impl Into<String>, severity: Option<Severity>) {
        let _ = self.events.send(Event::Widget(WidgetUpdate {
            slot: slot.into(),
            value: value.into(),
            severity,
        }));
    }

    /// Persist a loot item. The engine emits the `LootStored` event for you and
    /// keeps the bulk bytes out of the wire — the controller gets a reference.
    pub async fn store_loot(
        &self,
        kind: LootKind,
        key: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Result<(), ModuleError> {
        let key = key.into();
        let size = bytes.len() as u64;
        self.loot
            .put(kind, &key, bytes)
            .await
            .map_err(|e| ModuleError::Failed(e.to_string()))?;
        let _ = self.events.send(Event::LootStored { key, kind, size });
        Ok(())
    }

    /// Cooperative cancellation: long-running actions should poll this.
    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// Cancellation flag shared between the engine and a running task.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Where loot goes. The engine provides the implementation (in-memory now,
/// sled/redb later); modules only ever see this trait.
#[async_trait]
pub trait LootSink: Send + Sync {
    async fn put(&self, kind: LootKind, key: &str, bytes: Vec<u8>) -> Result<(), LootError>;
    async fn query(&self, query: &LootQuery) -> Result<Vec<LootEntry>, LootError>;
    /// Fetch one item's actual bytes by key — `None` if no such key exists.
    /// Distinct from `query`, which only ever returns metadata.
    async fn get(&self, key: &str) -> Result<Option<(LootKind, Vec<u8>)>, LootError>;
    /// Remove all stored loot — the self-wipe / tamper reflex.
    async fn clear(&self) -> Result<(), LootError>;
}

#[derive(Debug)]
pub struct LootError(pub String);

impl std::fmt::Display for LootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LootError {}

/// What a module can fail with. Distinct from the protocol/dispatch errors the
/// engine raises itself (unknown module, missing capability).
#[derive(Debug)]
pub enum ModuleError {
    InvalidParams(String),
    Unsupported(String),
    Failed(String),
    Cancelled,
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::InvalidParams(m) => write!(f, "invalid params: {m}"),
            ModuleError::Unsupported(m) => write!(f, "unsupported: {m}"),
            ModuleError::Failed(m) => write!(f, "failed: {m}"),
            ModuleError::Cancelled => f.write_str("cancelled"),
        }
    }
}
impl std::error::Error for ModuleError {}

impl From<serde_json::Error> for ModuleError {
    fn from(e: serde_json::Error) -> Self {
        ModuleError::InvalidParams(e.to_string())
    }
}

/// Parse opaque [`RawParams`] into a module's own typed request.
pub trait ParseParams {
    fn parse<T: DeserializeOwned>(&self) -> Result<T, ModuleError>;
}

impl ParseParams for RawParams {
    fn parse<T: DeserializeOwned>(&self) -> Result<T, ModuleError> {
        serde_json::from_value(self.0.clone()).map_err(ModuleError::from)
    }
}

/// Build [`RawParams`] from a serializable output.
pub fn raw_params<T: Serialize>(value: &T) -> Result<RawParams, ModuleError> {
    Ok(RawParams(serde_json::to_value(value).map_err(ModuleError::from)?))
}

/// Build a loot key ending in a sortable timestamp: `<prefix>/<millis-since-epoch>`.
/// Every call to `store_loot` with a key built this way gets its own entry
/// instead of overwriting one fixed key — `Command::Loot{prefix}` can still
/// reach every past snapshot, not just the latest, and the engine's newest-
/// first ordering (see `engine::loot`) shows the most recent one first.
/// Millisecond epoch timestamps stay the same digit width for centuries, so
/// plain string sorting already sorts chronologically — no zero-padding
/// needed. Prefer this over a fixed key for anything an operator might want
/// history of; keep a fixed key only for state that's meaningless to keep
/// more than one of.
pub fn timestamped_key(prefix: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{prefix}/{millis}")
}

/// A flexible port specification a network module can accept as a parameter: a
/// spec string (`"1-1024"`, `"22,80,443"`, `"80,1000-1010"`), an explicit array
/// (`[22, 80, 443]`), or a single number (`80`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PortSpec {
    Spec(String),
    List(Vec<u16>),
    One(u16),
}

impl PortSpec {
    /// Expand into a sorted, de-duplicated list of ports.
    pub fn resolve(&self) -> Result<Vec<u16>, ModuleError> {
        let mut set = std::collections::BTreeSet::new();
        match self {
            PortSpec::One(p) => {
                set.insert(*p);
            }
            PortSpec::List(v) => set.extend(v.iter().copied()),
            PortSpec::Spec(spec) => {
                for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    if let Some((a, b)) = part.split_once('-') {
                        let lo = parse_port(a)?;
                        let hi = parse_port(b)?;
                        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
                        set.extend(lo..=hi);
                    } else {
                        set.insert(parse_port(part)?);
                    }
                }
            }
        }
        Ok(set.into_iter().collect())
    }
}

fn parse_port(s: &str) -> Result<u16, ModuleError> {
    s.trim()
        .parse()
        .map_err(|_| ModuleError::InvalidParams(format!("bad port '{}'", s.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamped_key_is_prefixed_and_sortable() {
        let a = timestamped_key("sysinfo");
        assert!(a.starts_with("sysinfo/"));
        let suffix = a.strip_prefix("sysinfo/").unwrap();
        assert!(suffix.chars().all(|c| c.is_ascii_digit()), "timestamp suffix must be all digits: {suffix}");
    }

    #[test]
    fn timestamped_key_keeps_a_multi_segment_prefix_intact() {
        let key = timestamped_key("dns/axfr/example.com/ns1");
        assert!(key.starts_with("dns/axfr/example.com/ns1/"));
    }

    #[test]
    fn successive_calls_sort_chronologically_as_plain_strings() {
        let first = timestamped_key("x");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = timestamped_key("x");
        assert!(first < second, "later timestamp must sort after the earlier one as a string");
    }
}
