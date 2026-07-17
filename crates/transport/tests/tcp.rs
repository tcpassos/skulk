use std::sync::Arc;
use std::time::Duration;

use contract::*;
use engine::{Engine, MemLoot};
use example_sysinfo::SysInfo;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use transport::{serve_connection, TransportConfig};

fn build_engine() -> Arc<Engine> {
    let implant = ImplantInfo {
        id: "implant-01".to_string(),
        hardware: "Raspberry Pi Zero 2 W".to_string(),
        firmware: "0.1.0".to_string(),
    };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(SysInfo));
    Arc::new(engine)
}

async fn read_until<R>(
    lines: &mut tokio::io::Lines<R>,
    pred: impl Fn(&Envelope) -> bool,
) -> Option<Envelope>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    loop {
        match timeout(Duration::from_secs(3), lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(env) = serde_json::from_str::<Envelope>(&line) {
                    if pred(&env) {
                        return Some(env);
                    }
                }
            }
            _ => return None,
        }
    }
}

async fn send(stream: &mut TcpStream, env: &Envelope) {
    let mut buf = serde_json::to_vec(env).unwrap();
    buf.push(b'\n');
    stream.write_all(&buf).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn describe_and_invoke_over_tcp() {
    let engine = build_engine();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    // Accept one connection and serve it.
    let srv = engine.clone();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve_connection(srv, stream, TransportConfig::default()).await;
    });

    let mut client = TcpStream::connect(addr).await.unwrap();

    // 1. Describe over the wire → the Manifest comes back.
    send(&mut client, &Envelope::new(Body::Command(Command::Describe), 0)).await;

    let (read, mut write) = client.into_split();
    let mut lines = BufReader::new(read).lines();

    let described = read_until(&mut lines, |e| matches!(e.body, Body::Result(_)))
        .await
        .expect("expected a Result for Describe");
    let manifest: Manifest = match described.body {
        Body::Result(r) => serde_json::from_value(r.output.0).unwrap(),
        _ => unreachable!(),
    };
    assert!(
        manifest.modules.iter().any(|m| m.id == ModuleId::from("sys.info")),
        "manifest should list sys.info"
    );

    // 2. Invoke sys.info/get → a Result Ok eventually arrives over the same socket.
    let invoke = Envelope::new(
        Body::Command(Command::Invoke(Invoke {
            module: ModuleId::from("sys.info"),
            action: "get".to_string(),
            params: RawParams::default(),
            timeout_ms: None,
        })),
        0,
    );
    let mut buf = serde_json::to_vec(&invoke).unwrap();
    buf.push(b'\n');
    write.write_all(&buf).await.unwrap();

    let ok = read_until(&mut lines, |e| {
        matches!(&e.body, Body::Result(r) if r.status == TaskStatus::Ok)
    })
    .await;
    assert!(ok.is_some(), "expected a Result Ok over the wire");
}
