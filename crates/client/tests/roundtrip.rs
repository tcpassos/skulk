use std::sync::Arc;

use client::{Client, Outcome};
use contract::*;
use engine::{Engine, MemLoot};
use example_sysinfo::SysInfo;
use tokio::net::TcpListener;
use transport::{serve_connection, TransportConfig};

/// Start an in-process implant (engine + transport) on an ephemeral port.
async fn spawn_implant() -> String {
    let implant = ImplantInfo {
        id: "t".to_string(),
        hardware: "t".to_string(),
        firmware: "0".to_string(),
    };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(SysInfo));
    let engine = Arc::new(engine);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let engine = engine.clone();
            tokio::spawn(async move {
                serve_connection(engine, stream, TransportConfig::default()).await;
            });
        }
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_describes_and_invokes() {
    let addr = spawn_implant().await;
    let mut client = Client::connect(&addr).await.unwrap();

    let manifest = client.describe().await.unwrap();
    assert!(
        manifest.modules.iter().any(|m| m.id == ModuleId::from("sys.info")),
        "manifest should list sys.info"
    );

    let mut saw_progress = false;
    let outcome = client
        .run(
            Command::Invoke(Invoke {
                module: ModuleId::from("sys.info"),
                action: "get".to_string(),
                params: RawParams::default(),
                timeout_ms: None,
            }),
            |env| {
                if matches!(env.body, Body::Event(Event::Progress { .. })) {
                    saw_progress = true;
                }
            },
        )
        .await
        .unwrap();

    match outcome {
        Outcome::Result(r) => assert_eq!(r.status, TaskStatus::Ok),
        Outcome::Error(e) => panic!("unexpected protocol error: {}", e.message),
    }
    assert!(saw_progress, "the client should have observed progress events");

    // The invoke stored loot; the typed loot() helper should see it.
    let loot = client.loot(LootQuery::default()).await.unwrap();
    assert!(loot.iter().any(|e| e.key == "sysinfo/last"));

    // And its actual content is fetchable by key.
    let content = client.loot_fetch("sysinfo/last").await.unwrap();
    assert_eq!(content.kind, LootKind::Telemetry);
    assert!(serde_json::from_slice::<serde_json::Value>(&content.bytes).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_loot_fetch_rejects_an_unknown_key() {
    let addr = spawn_implant().await;
    let mut client = Client::connect(&addr).await.unwrap();

    let err = client.loot_fetch("no/such/key").await.unwrap_err();
    assert!(err.to_string().contains("no/such/key"));
}
