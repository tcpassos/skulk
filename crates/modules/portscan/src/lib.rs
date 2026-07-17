//! `net.port_scan` — an unprivileged async TCP connect scan. The first module
//! that does something in the real world: it takes a typed target/port request,
//! streams progress, honours cancellation, and returns a structured result.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use contract::{ActionSpec, LogLevel, ModuleDescriptor, ModuleId, RawParams};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams};

pub struct PortScan;

const DEFAULT_PORTS: &[u16] = &[22, 80, 443, 445, 3389, 8080, 9000];

#[derive(Debug, Deserialize)]
struct ScanParams {
    /// Target IP or hostname.
    target: String,
    /// Explicit ports to scan.
    #[serde(default)]
    ports: Vec<u16>,
    /// Inclusive port range `[start, end]`, merged with `ports`.
    #[serde(default)]
    port_range: Option<[u16; 2]>,
    /// Max simultaneous connects (default 256).
    #[serde(default)]
    concurrency: Option<usize>,
    /// Per-port connect timeout (default 500 ms).
    #[serde(default)]
    connect_timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ScanOutput {
    target: String,
    open_ports: Vec<u16>,
    scanned: usize,
    duration_ms: u64,
}

fn resolve_ports(params: &ScanParams) -> Vec<u16> {
    let mut set: BTreeSet<u16> = params.ports.iter().copied().collect();
    if let Some([a, b]) = params.port_range {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        set.extend(lo..=hi);
    }
    if set.is_empty() {
        set.extend(DEFAULT_PORTS.iter().copied());
    }
    set.into_iter().collect()
}

#[async_trait]
impl ImplantModule for PortScan {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("net.port_scan"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            actions: vec![ActionSpec {
                name: "scan".to_string(),
                description: Some("Unprivileged TCP connect scan of a target".to_string()),
                params_schema: None,
            }],
            requires: Vec::new(), // a connect scan needs no special hardware
        }
    }

    async fn invoke(
        &self,
        ctx: &ModuleCtx,
        action: &str,
        params: RawParams,
    ) -> Result<RawParams, ModuleError> {
        if action != "scan" {
            return Err(ModuleError::Unsupported(format!(
                "net.port_scan has no action '{action}'"
            )));
        }

        let params: ScanParams = params.parse()?;
        let ports = resolve_ports(&params);
        let total = ports.len();
        let concurrency = params.concurrency.unwrap_or(256).max(1);
        let timeout = Duration::from_millis(params.connect_timeout_ms.unwrap_or(500));
        let target = params.target.clone();

        ctx.log(LogLevel::Info, format!("scanning {total} ports on {target}"));

        let started = Instant::now();
        let mut open = Vec::new();
        let mut scanned = 0usize;

        let probes = ports.into_iter().map(|port| {
            let target = target.clone();
            async move {
                let is_open = matches!(
                    tokio::time::timeout(timeout, TcpStream::connect((target.as_str(), port))).await,
                    Ok(Ok(_))
                );
                (port, is_open)
            }
        });

        let mut buffered = stream::iter(probes).buffer_unordered(concurrency);
        while let Some((port, is_open)) = buffered.next().await {
            scanned += 1;
            if is_open {
                open.push(port);
            }
            if scanned % 64 == 0 || scanned == total {
                let pct = ((scanned * 100) / total.max(1)) as u8;
                ctx.progress(Some(pct), format!("{scanned}/{total}"));
            }
            if ctx.cancelled() {
                break;
            }
        }
        open.sort_unstable();

        let output = ScanOutput {
            target: target.clone(),
            open_ports: open,
            scanned,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        raw_params(&output)
    }
}
