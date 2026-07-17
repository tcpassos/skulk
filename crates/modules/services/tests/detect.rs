use std::sync::Arc;
use std::time::Duration;

use contract::*;
use engine::{Engine, MemLoot};
use net_services::Services;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

/// A fake server that greets every connection with `banner` (like SSH/FTP/SMTP).
async fn banner_server(banner: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let _ = sock.write_all(banner.as_bytes()).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
    });
    port
}

/// A fake HTTP server: waits for the request, then replies (like a real web server).
async fn http_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 256];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.0 200 OK\r\nServer: TestServer/1.0\r\n\r\nhi")
                .await;
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn detects_ssh_and_http() {
    let ssh_port = banner_server("SSH-2.0-TestSSH_9.9\r\n").await;
    let http_port = http_server().await;

    let implant = ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(Services));
    let engine = Arc::new(engine);
    let mut rx = engine.subscribe();

    let params = serde_json::json!({
        "target": "127.0.0.1",
        "ports": format!("{},{}", ssh_port, http_port),
        "timeout_ms": 1000
    });
    engine
        .handle(Envelope::new(
            Body::Command(Command::Invoke(Invoke {
                module: ModuleId::from("net.services"),
                action: "detect".to_string(),
                params: RawParams(params),
                timeout_ms: None,
            })),
            0,
        ))
        .await;

    let result = loop {
        match timeout(Duration::from_secs(8), rx.recv()).await {
            Ok(Ok(env)) => {
                if let Body::Result(r) = env.body {
                    if r.output.0.get("services").is_some() {
                        break r;
                    }
                }
            }
            _ => panic!("no detect result"),
        }
    };
    assert_eq!(result.status, TaskStatus::Ok);

    let services = result.output.0.get("services").unwrap().as_array().unwrap();
    let ssh = services
        .iter()
        .find(|s| s["port"].as_u64() == Some(ssh_port as u64))
        .expect("ssh port present");
    assert_eq!(ssh["service"], serde_json::json!("ssh"));
    assert!(ssh["banner"].as_str().unwrap().contains("TestSSH"));

    let http = services
        .iter()
        .find(|s| s["port"].as_u64() == Some(http_port as u64))
        .expect("http port present");
    assert_eq!(http["service"], serde_json::json!("http"));
    assert!(http["banner"].as_str().unwrap().contains("TestServer"));
}
