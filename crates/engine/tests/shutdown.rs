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

/// Read outbound envelopes until a Result whose output is a JSON array (the shape
/// of a Loot query result), skipping other results/events.
async fn next_loot(rx: &mut Receiver<Envelope>) -> Vec<serde_json::Value> {
    let got = timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(env) = rx.recv().await {
                if let Body::Result(r) = &env.body {
                    if let Some(arr) = r.output.0.as_array() {
                        return arr.clone();
                    }
                }
            }
        }
    })
    .await;
    got.expect("expected a loot result")
}

/// Drain until any Result arrives — used to wait for an async task (and its loot
/// write) to finish before querying.
async fn wait_for_result(rx: &mut Receiver<Envelope>) {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(env) = rx.recv().await {
                if matches!(env.body, Body::Result(_)) {
                    return;
                }
            }
        }
    })
    .await
    .expect("expected a task result");
}

#[tokio::test]
async fn wipe_clears_loot_and_signals_shutdown() {
    let implant = ImplantInfo {
        id: "t".to_string(),
        hardware: "t".to_string(),
        firmware: "0".to_string(),
    };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(SysInfo));
    let engine = Arc::new(engine);
    let mut rx = engine.subscribe();

    // Store loot by invoking sys.info, then confirm one loot item exists.
    engine
        .handle(command(Command::Invoke(Invoke {
            module: ModuleId::from("sys.info"),
            action: "get".to_string(),
            params: RawParams::default(),
            timeout_ms: None,
        })))
        .await;
    // Wait for the invoke to finish so its loot write has committed.
    wait_for_result(&mut rx).await;
    engine.handle(command(Command::Loot(LootQuery::default()))).await;
    assert_eq!(next_loot(&mut rx).await.len(), 1, "loot present before wipe");

    // Shutdown with wipe: the signal fires and the loot is cleared.
    let waiter = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.wait_for_shutdown().await })
    };
    engine.handle(command(Command::Shutdown { mode: ShutdownMode::Wipe })).await;
    timeout(Duration::from_secs(2), waiter)
        .await
        .expect("shutdown signal should fire")
        .expect("waiter task ok");

    engine.handle(command(Command::Loot(LootQuery::default()))).await;
    assert_eq!(next_loot(&mut rx).await.len(), 0, "loot wiped after shutdown");
}
