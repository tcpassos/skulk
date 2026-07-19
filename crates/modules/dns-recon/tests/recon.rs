//! Loopback pattern: a tiny hand-rolled authoritative nameserver (UDP for
//! standard queries, TCP for AXFR) so the module's plumbing — resolver ->
//! record parsing -> NS discovery -> zone transfer -> loot -> alert — is
//! proven against the real wire protocol without touching the network.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use contract::*;
use dns_recon::DnsRecon;
use engine::{Engine, MemLoot};
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{A, MX, SOA, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::time::timeout;

fn zone_records(zone: &Name, ns_name: &Name) -> Vec<Record> {
    let admin = Name::from_ascii("admin.example.test.").unwrap();
    let mail = Name::from_ascii("mail.example.test.").unwrap();
    vec![
        Record::from_rdata(zone.clone(), 300, RData::SOA(SOA::new(ns_name.clone(), admin, 1, 3600, 600, 86400, 300))),
        Record::from_rdata(zone.clone(), 300, RData::NS(hickory_proto::rr::rdata::NS(ns_name.clone()))),
        Record::from_rdata(zone.clone(), 300, RData::A(A::new(203, 0, 113, 10))),
        Record::from_rdata(zone.clone(), 300, RData::MX(MX::new(10, mail))),
        Record::from_rdata(zone.clone(), 300, RData::TXT(TXT::new(vec!["hello-world".to_string()]))),
        // Glue: lets the module resolve the nameserver's own IP for the AXFR probe.
        Record::from_rdata(ns_name.clone(), 300, RData::A(A::new(127, 0, 0, 1))),
    ]
}

fn answer_for(records: &[Record], query: &Query) -> Message {
    let mut msg = Message::new();
    msg.set_message_type(MessageType::Response);
    msg.set_op_code(OpCode::Query);
    msg.set_authoritative(true);
    msg.set_response_code(ResponseCode::NoError);
    msg.add_query(query.clone());
    let matching: Vec<Record> = records
        .iter()
        .filter(|r| r.name() == query.name() && r.record_type() == query.query_type())
        .cloned()
        .collect();
    msg.insert_answers(matching);
    msg
}

/// Spawns a fake authoritative nameserver on one loopback port (UDP for
/// normal lookups, TCP for AXFR) and returns that port plus its record set,
/// so the AXFR handler can also serve the full zone.
async fn spawn_fake_nameserver(zone: Name, ns_name: Name) -> u16 {
    let records = zone_records(&zone, &ns_name);

    let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = udp.local_addr().unwrap().port();
    let tcp = TcpListener::bind(("127.0.0.1", port)).await.unwrap();

    let udp_records = records.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 512];
        loop {
            let (n, from) = match udp.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => break,
            };
            let Ok(req) = Message::from_vec(&buf[..n]) else { continue };
            let Some(query) = req.queries().first() else { continue };
            let mut resp = answer_for(&udp_records, query);
            resp.set_id(req.id());
            if let Ok(bytes) = resp.to_vec() {
                let _ = udp.send_to(&bytes, from).await;
            }
        }
    });

    let axfr_records = records;
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = tcp.accept().await else { break };
            let axfr_records = axfr_records.clone();
            tokio::spawn(async move {
                let mut len_buf = [0u8; 2];
                if sock.read_exact(&mut len_buf).await.is_err() {
                    return;
                }
                let len = u16::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                if sock.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let Ok(req) = Message::from_vec(&buf) else { return };
                let Some(query) = req.queries().first().cloned() else { return };

                let mut resp = Message::new();
                resp.set_id(req.id());
                resp.set_message_type(MessageType::Response);
                resp.set_op_code(OpCode::Query);
                resp.set_authoritative(true);
                resp.set_response_code(ResponseCode::NoError);
                resp.add_query(query);
                // AXFR framing: leading and trailing SOA around the rest of the zone.
                let soa = axfr_records.iter().find(|r| r.record_type() == RecordType::SOA).unwrap().clone();
                let mut answers = vec![soa.clone()];
                answers.extend(axfr_records.iter().filter(|r| r.record_type() != RecordType::SOA).cloned());
                answers.push(soa);
                resp.insert_answers(answers);

                if let Ok(bytes) = resp.to_vec() {
                    let mut framed = (bytes.len() as u16).to_be_bytes().to_vec();
                    framed.extend(bytes);
                    let _ = sock.write_all(&framed).await;
                }
            });
        }
    });

    port
}

async fn run_enum(engine: &Arc<Engine>, params: serde_json::Value) -> TaskResult {
    let mut rx = engine.subscribe();
    engine
        .handle(Envelope::new(
            Body::Command(Command::Invoke(Invoke {
                module: ModuleId::from("dns.records"),
                action: "enum".to_string(),
                params: RawParams(params),
                timeout_ms: None,
            })),
            0,
        ))
        .await;

    loop {
        match timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Ok(env)) => {
                if let Body::Result(r) = env.body {
                    return r;
                }
            }
            other => panic!("no result: {other:?}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enumerates_records_and_transfers_zone() {
    let zone = Name::from_ascii("example.test.").unwrap();
    let ns_name = Name::from_ascii("ns1.example.test.").unwrap();
    let port = spawn_fake_nameserver(zone.clone(), ns_name.clone()).await;
    let resolver_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let implant = ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(DnsRecon));
    let engine = Arc::new(engine);

    let result = run_enum(
        &engine,
        serde_json::json!({
            "domain": "example.test",
            "resolver": resolver_addr.to_string(),
            "axfr_port": port,
            "timeout_ms": 2000,
        }),
    )
    .await;

    assert_eq!(result.status, TaskStatus::Ok, "{:?}", result.output);
    let out = result.output.0;

    let records = out["records"].as_array().unwrap();
    let has = |rtype: &str, needle: &str| {
        records.iter().any(|r| r["rtype"] == rtype && r["data"].as_str().unwrap().contains(needle))
    };
    assert!(has("A", "203.0.113.10"), "records: {records:?}");
    assert!(has("MX", "mail.example.test"), "records: {records:?}");
    assert!(has("TXT", "hello-world"), "records: {records:?}");
    assert!(has("SOA", "ns1.example.test"), "records: {records:?}");

    let name_servers = out["name_servers"].as_array().unwrap();
    assert!(name_servers.iter().any(|n| n.as_str().unwrap().contains("ns1.example.test")));

    let axfr = out["axfr"].as_array().unwrap();
    assert_eq!(axfr.len(), 1, "axfr attempts: {axfr:?}");
    assert_eq!(axfr[0]["status"], "success");
    // 6 zone records, with the SOA duplicated per AXFR framing (leading + trailing).
    assert_eq!(axfr[0]["record_count"], 7);
    let loot_key = axfr[0]["loot_key"].as_str().unwrap().to_string();

    let mut rx = engine.subscribe();
    engine
        .handle(Envelope::new(
            Body::Command(Command::Loot(LootQuery { prefix: Some(loot_key.clone()), kind: None, limit: None })),
            0,
        ))
        .await;
    let entries = loop {
        match timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(env)) => {
                if let Body::Result(r) = env.body {
                    break r.output.0;
                }
            }
            other => panic!("no loot result: {other:?}"),
        }
    };
    let entries = entries.as_array().unwrap();
    assert!(entries.iter().any(|e| e["key"] == loot_key), "loot entries: {entries:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn axfr_can_be_disabled() {
    let zone = Name::from_ascii("example.test.").unwrap();
    let ns_name = Name::from_ascii("ns1.example.test.").unwrap();
    let port = spawn_fake_nameserver(zone, ns_name).await;
    let resolver_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let implant = ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() };
    let mut engine = Engine::new(implant, Vec::new(), Arc::new(MemLoot::default()));
    engine.register(Arc::new(DnsRecon));
    let engine = Arc::new(engine);

    let result = run_enum(
        &engine,
        serde_json::json!({
            "domain": "example.test",
            "resolver": resolver_addr.to_string(),
            "axfr": false,
            "record_types": "A",
            "timeout_ms": 2000,
        }),
    )
    .await;

    assert_eq!(result.status, TaskStatus::Ok, "{:?}", result.output);
    let out = result.output.0;
    assert_eq!(out["axfr"].as_array().unwrap().len(), 0);
    assert_eq!(out["records"].as_array().unwrap().len(), 1);
}
