//! The engine: an outbound event bus, a module registry, and the dispatcher that
//! turns an inbound [`Command`] into acks, events, results and errors. The socket
//! adapter (later) is a thin serde translator on top of [`Engine::handle`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use tokio::sync::{broadcast, mpsc, Notify};

use contract::*;
use module_sdk::{Cancel, ImplantModule, LootSink, ModuleCtx, ModuleError};

mod loot;
pub use loot::{MemLoot, RedbLoot};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The running implant engine.
pub struct Engine {
    implant: ImplantInfo,
    capabilities: Vec<Capability>,
    modules: HashMap<ModuleId, Arc<dyn ImplantModule>>,
    loot: Arc<dyn LootSink>,
    outbound: broadcast::Sender<Envelope>,
    tasks: Arc<Mutex<HashMap<TaskId, Cancel>>>,
    shutdown: Arc<Notify>,
}

impl Engine {
    pub fn new(implant: ImplantInfo, capabilities: Vec<Capability>, loot: Arc<dyn LootSink>) -> Self {
        let (outbound, _rx) = broadcast::channel(256);
        Self {
            implant,
            capabilities,
            modules: HashMap::new(),
            loot,
            outbound,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Register a module (static registration; call before serving).
    pub fn register(&mut self, module: Arc<dyn ImplantModule>) -> &mut Self {
        let id = module.descriptor().id;
        self.modules.insert(id, module);
        self
    }

    /// Ids of all registered modules, sorted — reflects the compiled-in feature set.
    pub fn module_ids(&self) -> Vec<ModuleId> {
        let mut ids: Vec<ModuleId> = self.modules.keys().cloned().collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids
    }

    /// Subscribe to the outbound bus. The socket adapter, the LCD and the TUI
    /// all do this — the same [`Envelope`] stream feeds every consumer.
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.outbound.subscribe()
    }

    /// Publish an unsolicited event on the outbound bus (not a reply to a command).
    pub fn publish_event(&self, event: Event) {
        let env = Envelope::new(Body::Event(event), now_ms());
        let _ = self.outbound.send(env);
    }

    /// Spawn a periodic heartbeat so controllers can detect liveness while idle.
    pub fn spawn_heartbeat(self: &Arc<Self>, interval: Duration) {
        let engine = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            let mut seq = 0u64;
            loop {
                ticker.tick().await;
                engine.publish_event(Event::Heartbeat { seq });
                seq = seq.wrapping_add(1);
            }
        });
    }

    /// Resolve once a `Shutdown` command has been received, so the daemon can stop.
    pub async fn wait_for_shutdown(&self) {
        self.shutdown.notified().await;
    }

    fn emit(&self, body: Body, correlate: MessageId) {
        let env = Envelope::new(body, now_ms()).in_reply_to(correlate);
        let _ = self.outbound.send(env);
    }

    /// Handle one inbound message. The device only acts on commands.
    pub async fn handle(&self, inbound: Envelope) {
        let cause = inbound.id;
        if let Body::Command(cmd) = inbound.body {
            self.dispatch(cmd, cause).await;
        }
    }

    async fn dispatch(&self, cmd: Command, cause: MessageId) {
        match cmd {
            Command::Ping => {
                let out = RawParams(json!({ "pong": true, "protocol": PROTOCOL_VERSION }));
                self.emit(Body::Result(instant(out)), cause);
            }
            Command::Describe => {
                let out = RawParams(serde_json::to_value(self.manifest()).unwrap_or_default());
                self.emit(Body::Result(instant(out)), cause);
            }
            Command::Invoke(inv) => self.invoke(inv, cause).await,
            Command::Cancel { task } => {
                if let Some(cancel) = self.tasks.lock().unwrap().get(&task) {
                    cancel.cancel();
                }
                // The running task observes the flag and emits its own
                // Result{ status: Cancelled }.
            }
            Command::Loot(query) => {
                let entries = self.loot.query(&query).await.unwrap_or_default();
                let out = RawParams(serde_json::to_value(entries).unwrap_or_default());
                self.emit(Body::Result(instant(out)), cause);
            }
            Command::Shutdown { mode } => {
                tracing::warn!(?mode, "shutdown requested");
                self.emit(
                    Body::Event(Event::Alert {
                        severity: Severity::High,
                        source: ModuleId::from("core"),
                        msg: format!("shutdown requested: {mode:?}"),
                    }),
                    cause,
                );
                let wiped = if mode == ShutdownMode::Wipe {
                    match self.loot.clear().await {
                        Ok(()) => {
                            tracing::warn!("loot store wiped");
                            true
                        }
                        Err(e) => {
                            tracing::error!("wipe failed: {e}");
                            false
                        }
                    }
                } else {
                    false
                };
                self.emit(
                    Body::Result(instant(RawParams(
                        json!({ "shutdown": true, "mode": format!("{mode:?}"), "wiped": wiped }),
                    ))),
                    cause,
                );
                // Signal the daemon to stop. Actual reboot/poweroff is a
                // hardware-phase concern handled by the process owner.
                self.shutdown.notify_one();
            }
        }
    }

    async fn invoke(&self, inv: Invoke, cause: MessageId) {
        let module = match self.modules.get(&inv.module) {
            Some(m) => m.clone(),
            None => {
                tracing::warn!(module = %inv.module, "invoke for unknown module");
                self.emit(
                    Body::Error(ProtocolError {
                        code: ErrorCode::UnknownModule,
                        message: format!("no module '{}'", inv.module),
                        module: Some(inv.module),
                    }),
                    cause,
                );
                return;
            }
        };

        // Capability-gating: refuse before running if the hardware is missing
        // anything the module declared it needs.
        let requires = module.descriptor().requires;
        let mut missing = Vec::new();
        for req in &requires {
            if !self.capabilities.contains(req) {
                missing.push(req.clone());
            }
        }
        if !missing.is_empty() {
            tracing::warn!(module = %inv.module, ?missing, "invoke rejected: missing capability");
            self.emit(
                Body::Error(ProtocolError {
                    code: ErrorCode::MissingCapability,
                    message: format!("{} requires {:?}", inv.module, missing),
                    module: Some(inv.module),
                }),
                cause,
            );
            return;
        }

        let task = TaskId::new();
        let cancel = Cancel::new();
        self.tasks.lock().unwrap().insert(task, cancel.clone());
        self.emit(Body::Ack(Ack { task }), cause);

        let outbound = self.outbound.clone();
        let loot = self.loot.clone();
        let tasks = self.tasks.clone();
        let module_id = inv.module;
        let action = inv.action;
        let params = inv.params;

        tracing::info!(module = %module_id, action = %action, "invoke dispatched");

        tokio::spawn(async move {
            // Forward the module's events onto the bus as they happen, each
            // correlated to the causing command.
            let (etx, mut erx) = mpsc::unbounded_channel::<Event>();
            let forwarder = {
                let outbound = outbound.clone();
                tokio::spawn(async move {
                    while let Some(ev) = erx.recv().await {
                        let env = Envelope::new(Body::Event(ev), now_ms()).in_reply_to(cause);
                        let _ = outbound.send(env);
                    }
                })
            };

            let ctx = ModuleCtx::new(module_id, task, etx, loot, cancel);
            let outcome = module.invoke(&ctx, &action, params).await;
            drop(ctx); // closes the event channel so the forwarder drains and ends
            let _ = forwarder.await;

            let result = match outcome {
                Ok(output) => TaskResult { task, status: TaskStatus::Ok, output },
                Err(ModuleError::Cancelled) => {
                    TaskResult { task, status: TaskStatus::Cancelled, output: RawParams::default() }
                }
                Err(err) => TaskResult {
                    task,
                    status: TaskStatus::Error,
                    output: RawParams(json!({ "error": err.to_string() })),
                },
            };
            let env = Envelope::new(Body::Result(result), now_ms()).in_reply_to(cause);
            let _ = outbound.send(env);
            tasks.lock().unwrap().remove(&task);
        });
    }

    fn manifest(&self) -> Manifest {
        Manifest {
            protocol: PROTOCOL_VERSION,
            implant: self.implant.clone(),
            modules: self.modules.values().map(|m| m.descriptor()).collect(),
            capabilities: self.capabilities.clone(),
        }
    }
}

/// Lifecycle commands (Ping/Describe/Loot/Shutdown) are modelled as instant
/// tasks whose payload rides in the result output.
fn instant(output: RawParams) -> TaskResult {
    TaskResult { task: TaskId::new(), status: TaskStatus::Ok, output }
}

