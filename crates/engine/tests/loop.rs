use std::sync::Arc;

use async_trait::async_trait;
use contract::*;
use engine::{Engine, MemLoot};
use example_sysinfo::SysInfo;
use module_sdk::{ImplantModule, ModuleCtx, ModuleError};
use tokio::sync::broadcast::Receiver;
use tokio::time::{timeout, Duration};

fn command(c: Command) -> Envelope {
    Envelope::new(Body::Command(c), 0)
}

/// Collect outbound envelopes until `done` matches one (or a 2s idle timeout).
async fn drain_until(
    rx: &mut Receiver<Envelope>,
    mut done: impl FnMut(&Envelope) -> bool,
) -> Vec<Envelope> {
    let mut out = Vec::new();
    while let Ok(Ok(env)) = timeout(Duration::from_secs(2), rx.recv()).await {
        let stop = done(&env);
        out.push(env);
        if stop {
            break;
        }
    }
    out
}

/// A stub module that requires a capability the test device does not have.
struct NeedsMonitor;

#[async_trait]
impl ImplantModule for NeedsMonitor {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("wifi.stub"),
            version: "0.0.0".to_string(),
            tactic: None,
            actions: Vec::new(),
            requires: vec![Capability::MonitorMode],
        }
    }
    async fn invoke(
        &self,
        _ctx: &ModuleCtx,
        _action: &str,
        _params: RawParams,
    ) -> Result<RawParams, ModuleError> {
        Ok(RawParams::default())
    }
}

fn build_engine() -> Arc<Engine> {
    let implant = ImplantInfo {
        id: "implant-01".to_string(),
        hardware: "Raspberry Pi Zero 2 W".to_string(),
        firmware: "0.1.0".to_string(),
    };
    // Note: capabilities are empty — the device has no monitor-mode radio.
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(SysInfo));
    engine.register(Arc::new(NeedsMonitor));
    Arc::new(engine)
}

#[tokio::test]
async fn invoke_runs_module_and_streams_events() {
    let engine = build_engine();
    let mut rx = engine.subscribe();

    let inbound = command(Command::Invoke(Invoke {
        module: ModuleId::from("sys.info"),
        action: "get".to_string(),
        params: RawParams::default(),
        timeout_ms: None,
    }));
    let cause = inbound.id;
    engine.handle(inbound).await;

    let msgs = drain_until(&mut rx, |e| matches!(e.body, Body::Result(_))).await;

    assert!(msgs.iter().any(|e| matches!(e.body, Body::Ack(_))), "expected an Ack");
    assert!(
        msgs.iter().any(|e| matches!(e.body, Body::Event(Event::Progress { .. }))),
        "expected a Progress event"
    );
    assert!(
        msgs.iter().any(|e| matches!(e.body, Body::Event(Event::LootStored { .. }))),
        "expected a LootStored event"
    );
    let result = msgs.iter().find(|e| matches!(e.body, Body::Result(_))).unwrap();
    match &result.body {
        Body::Result(r) => assert_eq!(r.status, TaskStatus::Ok),
        _ => unreachable!(),
    }
    assert!(
        msgs.iter().all(|e| e.correlate == Some(cause)),
        "every reply correlates to the command"
    );
}

#[tokio::test]
async fn capability_gating_rejects_before_running() {
    let engine = build_engine();
    let mut rx = engine.subscribe();

    let inbound = command(Command::Invoke(Invoke {
        module: ModuleId::from("wifi.stub"),
        action: "start".to_string(),
        params: RawParams::default(),
        timeout_ms: None,
    }));
    let cause = inbound.id;
    engine.handle(inbound).await;

    let msgs = drain_until(&mut rx, |e| matches!(e.body, Body::Error(_))).await;
    let err = msgs.iter().find(|e| matches!(e.body, Body::Error(_))).expect("expected an Error");
    match &err.body {
        Body::Error(pe) => assert_eq!(pe.code, ErrorCode::MissingCapability),
        _ => unreachable!(),
    }
    assert_eq!(err.correlate, Some(cause));
}

#[tokio::test]
async fn describe_returns_manifest() {
    let engine = build_engine();
    let mut rx = engine.subscribe();

    engine.handle(command(Command::Describe)).await;

    let msgs = drain_until(&mut rx, |e| matches!(e.body, Body::Result(_))).await;
    let result = msgs
        .iter()
        .find_map(|e| match &e.body {
            Body::Result(r) => Some(r),
            _ => None,
        })
        .unwrap();
    let manifest: Manifest = serde_json::from_value(result.output.0.clone()).unwrap();
    assert_eq!(manifest.modules.len(), 2);
    assert_eq!(manifest.protocol, PROTOCOL_VERSION);
}
