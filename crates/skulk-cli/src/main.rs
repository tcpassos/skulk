//! `skulk` - a module-first command-line controller for an implant.
//!
//! The core shape mirrors the protocol 1:1:  <module> <action> <params>
//!   skulk net.port_scan scan target=10.0.0.1 ports=[1,1024] timeout=200
//!
//! Params infer their type (numbers/bools/lists stay typed, everything else is a
//! string); `--params-json '{...}'` is the escape hatch. Reserved words without a
//! dot (describe, loot, watch, ping, shutdown) are device-level operations.

use client::{Client, Outcome};
use contract::{
    Body, Command, Envelope, Event, Invoke, LootKind, LootQuery, ModuleId, RawParams, ShutdownMode,
    TaskId, TaskResult,
};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let addr = take_flag(&mut args, "--connect").unwrap_or_else(|| "127.0.0.1:9000".to_string());
    let params_json = take_flag(&mut args, "--params-json");

    let first = match args.first() {
        Some(f) if f == "help" || f == "-h" || f == "--help" => {
            print_usage();
            return Ok(());
        }
        Some(f) => f.clone(),
        None => {
            print_usage();
            return Ok(());
        }
    };

    let mut client = Client::connect(&addr)
        .await
        .map_err(|e| format!("cannot connect to {addr}: {e}"))?;

    // Module ids contain a dot; bare words are device-level commands.
    if first.contains('.') {
        invoke_module(&mut client, &args, params_json).await
    } else {
        match first.as_str() {
            "describe" | "modules" => describe(&mut client).await,
            "loot" => loot(&mut client, &mut args).await,
            "watch" => watch(&mut client).await,
            "ping" => ping(&mut client).await,
            "shutdown" => shutdown(&mut client, &mut args).await,
            other => Err(format!("unknown command '{other}' (try 'skulk help')")),
        }
    }
}

async fn invoke_module(
    client: &mut Client,
    args: &[String],
    params_json: Option<String>,
) -> Result<(), String> {
    let module = &args[0];
    let action = args
        .get(1)
        .ok_or_else(|| format!("usage: skulk {module} <action> [key=value ...]"))?;
    let params = build_params(&args[2..], params_json)?;

    let command = Command::Invoke(Invoke {
        module: ModuleId::from(module.as_str()),
        action: action.clone(),
        params,
        timeout_ms: None,
    });

    let outcome = client.run(command, print_event).await.map_err(|e| e.to_string())?;
    match outcome {
        Outcome::Result(r) => {
            print_result(&r);
            Ok(())
        }
        Outcome::Error(e) => Err(format!("{:?}: {}", e.code, e.message)),
    }
}

/// Turn `key=value` pairs into a JSON object, inferring each value's type
/// (JSON-parse, falling back to a string). `--params-json` overrides everything.
fn build_params(pairs: &[String], params_json: Option<String>) -> Result<RawParams, String> {
    if let Some(json) = params_json {
        let value: serde_json::Value =
            serde_json::from_str(&json).map_err(|e| format!("--params-json is not valid JSON: {e}"))?;
        return Ok(RawParams(value));
    }
    let mut map = serde_json::Map::new();
    for pair in pairs {
        let (key, raw) = pair
            .split_once('=')
            .ok_or_else(|| format!("bad parameter '{pair}' (expected key=value)"))?;
        let value = serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
        map.insert(key.to_string(), value);
    }
    Ok(RawParams(serde_json::Value::Object(map)))
}

async fn describe(client: &mut Client) -> Result<(), String> {
    let m = client.describe().await.map_err(|e| e.to_string())?;
    println!("implant: {} ({})  protocol v{}", m.implant.id, m.implant.hardware, m.protocol);
    let caps = if m.capabilities.is_empty() {
        "none".to_string()
    } else {
        format!("{:?}", m.capabilities)
    };
    println!("capabilities: {caps}");
    println!("modules:");
    for md in &m.modules {
        let requires = if md.requires.is_empty() {
            String::new()
        } else {
            format!("  requires {:?}", md.requires)
        };
        let tactic = md.tactic.as_ref().map(|t| format!("  [{t:?}]")).unwrap_or_default();
        println!("  {} v{}{}{}", md.id, md.version, tactic, requires);
        for a in &md.actions {
            let desc = a.description.as_deref().unwrap_or("");
            println!("    {}  -  {}", a.name, desc);
            for p in &a.params {
                let req = if p.required { "*" } else { " " };
                let ty = p.type_hint.as_deref().unwrap_or("");
                let d = p.description.as_deref().unwrap_or("");
                let mut extra = String::new();
                if let Some(dv) = &p.default {
                    extra.push_str(&format!("  [default: {dv}]"));
                }
                if let Some(ex) = &p.example {
                    extra.push_str(&format!("  [e.g. {ex}]"));
                }
                println!("      {req} {:<12} {:<9} {}{}", p.name, ty, d, extra);
            }
        }
    }
    Ok(())
}

async fn loot(client: &mut Client, args: &mut Vec<String>) -> Result<(), String> {
    let prefix = take_flag(args, "--prefix");
    let kind = take_flag(args, "--kind").and_then(|k| parse_kind(&k));
    let limit = take_flag(args, "--limit").and_then(|l| l.parse().ok());

    // A positional key left over after flag parsing (args[0] is "loot"
    // itself) fetches that item's content instead of listing the index.
    if let Some(key) = args.get(1) {
        let content = client.loot_fetch(key.clone()).await.map_err(|e| e.to_string())?;
        match std::str::from_utf8(&content.bytes) {
            Ok(text) => println!("{text}"),
            Err(_) => println!("(binary, {} B, kind {:?})", content.bytes.len(), content.kind),
        }
        return Ok(());
    }

    let entries = client
        .loot(LootQuery { prefix, kind, limit })
        .await
        .map_err(|e| e.to_string())?;
    if entries.is_empty() {
        println!("(no loot)");
    }
    for e in &entries {
        println!("  {}  {:?}  {} B", e.key, e.kind, e.size);
    }
    Ok(())
}

async fn watch(client: &mut Client) -> Result<(), String> {
    println!("watching events (Ctrl+C to stop)...");
    client.watch(print_event).await.map_err(|e| e.to_string())
}

async fn ping(client: &mut Client) -> Result<(), String> {
    match client.run(Command::Ping, |_| {}).await.map_err(|e| e.to_string())? {
        Outcome::Result(r) => {
            println!("pong: {}", r.output.0);
            Ok(())
        }
        Outcome::Error(e) => Err(e.message),
    }
}

async fn shutdown(client: &mut Client, args: &mut Vec<String>) -> Result<(), String> {
    let mode = if take_bool_flag(args, "--wipe") {
        ShutdownMode::Wipe
    } else {
        ShutdownMode::Graceful
    };
    match client.run(Command::Shutdown { mode }, print_event).await.map_err(|e| e.to_string())? {
        Outcome::Result(r) => {
            println!("{}", r.output.0);
            Ok(())
        }
        Outcome::Error(e) => Err(e.message),
    }
}

/// Human-readable one-liner per envelope (ASCII only, to survive any console).
fn print_event(env: &Envelope) {
    match &env.body {
        Body::Ack(a) => println!("  . task {} started", short(&a.task)),
        Body::Result(r) => println!("  = {:?}", r.status),
        Body::Error(e) => println!("  x {:?}: {}", e.code, e.message),
        Body::Command(_) => {}
        Body::Event(ev) => match ev {
            Event::Progress { pct, note, .. } => {
                let p = pct.map(|p| format!("{p}% ")).unwrap_or_default();
                println!("  . {p}{note}");
            }
            Event::Log { level, source, msg } => println!("  . [{level:?}] {source} {msg}"),
            Event::Alert { severity, source, msg } => println!("  ! [{severity:?}] {source} {msg}"),
            Event::LootStored { key, kind, size } => println!("  + loot {key} ({kind:?}, {size} B)"),
            Event::Heartbeat { seq } => println!("  ~ heartbeat {seq}"),
            Event::Sensor { source, .. } => println!("  . sensor {source}"),
            Event::ViewManifest(_) => {}
            Event::Widget(w) => {
                if !w.value.is_empty() {
                    println!("  . [{}] {}", w.slot, w.value);
                }
            }
        },
    }
}

fn print_result(r: &TaskResult) {
    println!("  = {:?}", r.status);
    let pretty =
        serde_json::to_string_pretty(&r.output.0).unwrap_or_else(|_| r.output.0.to_string());
    for line in pretty.lines() {
        println!("    {line}");
    }
}

fn short(task: &TaskId) -> String {
    task.0.to_string().chars().take(8).collect()
}

fn parse_kind(s: &str) -> Option<LootKind> {
    Some(match s {
        "hash" => LootKind::Hash,
        "handshake" => LootKind::Handshake,
        "credential" => LootKind::Credential,
        "pcap" => LootKind::Pcap,
        "telemetry" => LootKind::Telemetry,
        "file" => LootKind::File,
        "other" => LootKind::Other,
        _ => return None,
    })
}

fn take_flag(args: &mut Vec<String>, name: &str) -> Option<String> {
    if let Some(pos) = args.iter().position(|a| a == name) {
        if pos + 1 < args.len() {
            let value = args.remove(pos + 1);
            args.remove(pos);
            return Some(value);
        }
        args.remove(pos);
    }
    None
}

fn take_bool_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(pos) = args.iter().position(|a| a == name) {
        args.remove(pos);
        true
    } else {
        false
    }
}

fn print_usage() {
    println!("skulk - control an implant over the socket protocol\n");
    println!("USAGE");
    println!("  skulk [--connect ADDR] <command> [args]\n");
    println!("MODULE INVOCATION (module ids contain a dot)");
    println!("  skulk <module> <action> [key=value ...] [--params-json JSON]");
    println!("  e.g.  skulk net.port_scan scan target=10.0.0.1 ports=[1,1024] timeout=200\n");
    println!("  key=value infers the type: numbers, true/false and [lists] stay typed,");
    println!("  anything else is a string. Use --params-json '{{...}}' for full control.\n");
    println!("COMMANDS");
    println!("  describe                              list modules and capabilities");
    println!("  loot [--prefix P] [--kind K] [--limit N]   list captured loot");
    println!("  loot <key>                            print one loot item's content");
    println!("  watch                                 stream events live");
    println!("  ping                                  liveness check");
    println!("  shutdown [--wipe]                     stop the implant (--wipe clears loot)\n");
    println!("Default --connect is 127.0.0.1:9000.");
}
