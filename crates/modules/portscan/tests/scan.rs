use std::sync::Arc;
use std::time::Duration;

use contract::*;
use engine::{Engine, MemLoot};
use net_portscan::PortScan;
use tokio::net::TcpListener;
use tokio::time::timeout;

fn build_engine() -> Arc<Engine> {
    let implant = ImplantInfo {
        id: "t".to_string(),
        hardware: "t".to_string(),
        firmware: "0".to_string(),
    };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(PortScan));
    Arc::new(engine)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scan_finds_an_open_port() {
    // A real listener on an ephemeral port; keep accepting so connects succeed.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let open_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {}
    });

    let engine = build_engine();
    let mut rx = engine.subscribe();

    let params = serde_json::json!({
        "target": "127.0.0.1",
        "ports": [open_port, open_port.wrapping_sub(1)],
        "timeout_ms": 300
    });
    let invoke = Envelope::new(
        Body::Command(Command::Invoke(Invoke {
            module: ModuleId::from("net.ports"),
            action: "scan".to_string(),
            params: RawParams(params),
            timeout_ms: None,
        })),
        0,
    );
    engine.handle(invoke).await;

    let mut views: Vec<ViewManifest> = Vec::new();
    let mut widgets: Vec<WidgetUpdate> = Vec::new();
    let result = loop {
        match timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(env)) => match env.body {
                Body::Result(r) => break r,
                Body::Event(Event::ViewManifest(v)) => views.push(v),
                Body::Event(Event::Widget(w)) => widgets.push(w),
                _ => {}
            },
            _ => panic!("no result received"),
        }
    };

    assert_eq!(result.status, TaskStatus::Ok);
    assert!(!views.is_empty(), "expected at least one ViewManifest while scanning");
    assert!(views.iter().all(|v| v.screen == "net.ports"));
    // The scan publishes an ambient "ports" HUD slot, and retracts it (empty
    // value) once done.
    assert!(widgets.iter().any(|w| w.slot == "ports"), "expected a 'ports' HUD widget update");
    assert_eq!(widgets.last().map(|w| w.value.as_str()), Some(""), "last update should clear the slot");
    let open: Vec<u16> = result
        .output
        .0
        .get("open_ports")
        .and_then(|v| v.as_array())
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u16)
        .collect();
    assert!(open.contains(&open_port), "expected {open_port} open, got {open:?}");
}
