use std::sync::Arc;
use std::time::Duration;

use contract::*;
use engine::{Engine, MemLoot};
use example_sysinfo::SysInfo;
use tokio::sync::broadcast::Receiver;
use tokio::time::timeout;

fn command(c: Command) -> Envelope {
    Envelope::new(Body::Command(c), 0)
}

async fn next_body(rx: &mut Receiver<Envelope>) -> Body {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(env) = rx.recv().await {
                if matches!(env.body, Body::Result(_) | Body::Error(_)) {
                    return env.body;
                }
            }
        }
    })
    .await
    .expect("expected a Result or Error")
}

#[tokio::test]
async fn fetches_the_bytes_of_an_existing_key() {
    let implant = ImplantInfo { id: "t".to_string(), hardware: "t".to_string(), firmware: "0".to_string() };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(SysInfo));
    let engine = Arc::new(engine);
    let mut rx = engine.subscribe();

    engine
        .handle(command(Command::Invoke(Invoke {
            module: ModuleId::from("sys.info"),
            action: "get".to_string(),
            params: RawParams::default(),
            timeout_ms: None,
        })))
        .await;
    // Drain the invoke's own Result before issuing the fetch.
    next_body(&mut rx).await;

    // sys.info stores under a timestamped key (module_sdk::timestamped_key),
    // not a fixed one -- look up whatever key it actually used.
    let entries = engine.loot_query(&LootQuery { prefix: Some("sysinfo/".to_string()), ..Default::default() }).await;
    let key = entries.first().expect("sys.info should have stored one loot entry").key.clone();

    engine.handle(command(Command::LootFetch { key: key.clone() })).await;
    match next_body(&mut rx).await {
        Body::Result(r) => {
            let content: LootContent = serde_json::from_value(r.output.0).unwrap();
            assert_eq!(content.key, key);
            assert_eq!(content.kind, LootKind::Telemetry);
            assert!(!content.bytes.is_empty());
            // The stored bytes are the sysinfo JSON -- decodes back cleanly.
            assert!(serde_json::from_slice::<serde_json::Value>(&content.bytes).is_ok());
        }
        other => panic!("expected a Result, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_key_returns_a_not_found_error() {
    let implant = ImplantInfo { id: "t".to_string(), hardware: "t".to_string(), firmware: "0".to_string() };
    let engine = Arc::new(Engine::new(implant, Vec::new(), Arc::new(MemLoot::default())));
    let mut rx = engine.subscribe();

    engine.handle(command(Command::LootFetch { key: "no/such/key".to_string() })).await;
    match next_body(&mut rx).await {
        Body::Error(e) => assert_eq!(e.code, ErrorCode::NotFound),
        other => panic!("expected an Error, got {other:?}"),
    }
}
