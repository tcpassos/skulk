use std::sync::Arc;
use std::time::Duration;

use contract::*;
use engine::{Engine, MemLoot};
use tokio::time::timeout;

#[tokio::test]
async fn heartbeat_ticks_on_the_bus() {
    let implant = ImplantInfo {
        id: "t".to_string(),
        hardware: "t".to_string(),
        firmware: "0".to_string(),
    };
    let engine = Arc::new(Engine::new(implant, Vec::new(), Arc::new(MemLoot::default())));
    let mut rx = engine.subscribe();
    engine.spawn_heartbeat(Duration::from_millis(20));

    let seen = timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(env) => {
                    if matches!(env.body, Body::Event(Event::Heartbeat { .. })) {
                        return true;
                    }
                }
                Err(_) => return false,
            }
        }
    })
    .await;

    assert_eq!(seen, Ok(true), "expected a heartbeat event within 2s");
}
