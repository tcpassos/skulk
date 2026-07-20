//! `net.services` — service/version detection (nmap's `-sV`, in native Rust).
//!
//! For each open port it grabs a greeting banner (SSH, FTP, SMTP, … announce
//! themselves) and, if the service instead waits for the client (HTTP), sends a
//! probe, then classifies what is listening. Unprivileged: plain TCP connects.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use contract::{
    ActionSpec, LogLevel, ModuleDescriptor, ModuleId, ParamSpec, RawParams, Severity, Tactic, ViewLine,
};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams, PortSpec};

pub struct Services;

const DEFAULT_PORTS: &[u16] = &[21, 22, 23, 25, 80, 110, 143, 443, 3306, 3389, 5432, 8080];
const GREET_MS: u64 = 600;

#[derive(Debug, Deserialize)]
struct DetectParams {
    target: String,
    #[serde(default)]
    ports: Option<PortSpec>,
    #[serde(default)]
    concurrency: Option<usize>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ServiceInfo {
    port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    banner: String,
}

#[derive(Debug, Serialize)]
struct DetectOutput {
    target: String,
    services: Vec<ServiceInfo>,
    scanned: usize,
    duration_ms: u64,
}

#[async_trait]
impl ImplantModule for Services {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("net.services"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: Some(Tactic::Discovery),
            actions: vec![ActionSpec {
                name: "detect".to_string(),
                description: Some(
                    "Identify service + version on open ports (banner / HTTP probe)".to_string(),
                ),
                params: vec![
                    ParamSpec::required("target", "host", "IP or hostname").with_example("192.168.0.1"),
                    ParamSpec::optional("ports", "port-spec", "ports to probe, e.g. 22,80,443")
                        .with_default("common service ports")
                        .with_example("1-1024"),
                    ParamSpec::optional("timeout_ms", "int", "connect/read timeout in ms")
                        .with_default("1500"),
                ],
                params_schema: None,
            }],
            requires: Vec::new(), // plain TCP connects — no special hardware
        }
    }

    async fn invoke(
        &self,
        ctx: &ModuleCtx,
        action: &str,
        params: RawParams,
    ) -> Result<RawParams, ModuleError> {
        if action != "detect" {
            return Err(ModuleError::Unsupported(format!(
                "net.services has no action '{action}'"
            )));
        }

        let params: DetectParams = params.parse()?;
        let ports = match &params.ports {
            None => DEFAULT_PORTS.to_vec(),
            Some(spec) => {
                let p = spec.resolve()?;
                if p.is_empty() { DEFAULT_PORTS.to_vec() } else { p }
            }
        };
        let total = ports.len();
        let concurrency = params.concurrency.unwrap_or(64).max(1);
        let dur = Duration::from_millis(params.timeout_ms.unwrap_or(1500));
        let target = params.target.clone();

        ctx.log(LogLevel::Info, format!("probing {total} ports on {target}"));
        let started = Instant::now();

        let probes = ports.into_iter().map(|port| {
            let target = target.clone();
            async move { probe(&target, port, dur).await }
        });

        let mut buffered = stream::iter(probes).buffer_unordered(concurrency);
        let mut services = Vec::new();
        let mut scanned = 0usize;
        while let Some(result) = buffered.next().await {
            scanned += 1;
            if let Some(info) = result {
                services.push(info);
            }
            if scanned % 16 == 0 || scanned == total {
                let pct = ((scanned * 100) / total.max(1)) as u8;
                ctx.progress(Some(pct), format!("{scanned}/{total}"));
                ctx.view(
                    "net.services",
                    vec![
                        ViewLine { label: "TARGET".into(), value: target.clone(), severity: None },
                        ViewLine {
                            label: "SCANNED".into(),
                            value: format!("{scanned}/{total}"),
                            severity: None,
                        },
                        ViewLine {
                            label: "FOUND".into(),
                            value: services.len().to_string(),
                            severity: if services.is_empty() { None } else { Some(Severity::Medium) },
                        },
                    ],
                );
            }
            if ctx.cancelled() {
                break;
            }
        }
        services.sort_by_key(|s| s.port);

        let output = DetectOutput {
            target: target.clone(),
            services,
            scanned,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        raw_params(&output)
    }
}

/// Probe one port: connect, read a greeting; if silent, speak HTTP; then classify.
async fn probe(target: &str, port: u16, dur: Duration) -> Option<ServiceInfo> {
    let mut stream = match timeout(dur, TcpStream::connect((target, port))).await {
        Ok(Ok(s)) => s,
        _ => return None, // closed / filtered / timed out
    };

    let mut buf = vec![0u8; 512];

    // 1) Many services greet on connect (SSH, FTP, SMTP, POP3, IMAP, VNC…).
    if let Some(n) = read_some(&mut stream, &mut buf, Duration::from_millis(GREET_MS)).await {
        let (service, banner) = classify(&first_line(&buf[..n]));
        return Some(ServiceInfo { port, service, banner });
    }

    // 2) Silent so far — try speaking HTTP.
    let req = format!(
        "GET / HTTP/1.0\r\nHost: {target}\r\nUser-Agent: skulk\r\nConnection: close\r\n\r\n"
    );
    let _ = stream.write_all(req.as_bytes()).await;
    if let Some(n) = read_some(&mut stream, &mut buf, dur).await {
        let head = String::from_utf8_lossy(&buf[..n]);
        if head.starts_with("HTTP/") {
            return Some(ServiceInfo {
                port,
                service: Some("http".to_string()),
                banner: http_summary(&head),
            });
        }
        let (service, banner) = classify(&first_line(&buf[..n]));
        return Some(ServiceInfo { port, service, banner });
    }

    // Open but silent.
    Some(ServiceInfo { port, service: None, banner: String::new() })
}

async fn read_some(stream: &mut TcpStream, buf: &mut [u8], dur: Duration) -> Option<usize> {
    match timeout(dur, stream.read(buf)).await {
        Ok(Ok(n)) if n > 0 => Some(n),
        _ => None,
    }
}

/// The first printable line of a byte slice.
fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().next().unwrap_or("");
    line.chars().filter(|c| !c.is_control()).take(200).collect::<String>().trim().to_string()
}

/// Guess a service from a greeting banner.
fn classify(banner: &str) -> (Option<String>, String) {
    let lower = banner.to_ascii_lowercase();
    let service = if banner.starts_with("SSH-") {
        Some("ssh")
    } else if banner.starts_with("HTTP/") {
        Some("http")
    } else if banner.starts_with("220") && lower.contains("ftp") {
        Some("ftp")
    } else if banner.starts_with("220") && (lower.contains("smtp") || lower.contains("esmtp")) {
        Some("smtp")
    } else if banner.starts_with("220") {
        Some("ftp/smtp")
    } else if banner.starts_with("+OK") {
        Some("pop3")
    } else if banner.starts_with("* OK") {
        Some("imap")
    } else if banner.starts_with("RFB ") {
        Some("vnc")
    } else {
        None
    };
    (service.map(str::to_string), banner.to_string())
}

fn http_summary(head: &str) -> String {
    let status = head.lines().next().unwrap_or("").trim().to_string();
    match head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("server:"))
    {
        Some(server) => format!("{status} | {}", server.trim()),
        None => status,
    }
}
