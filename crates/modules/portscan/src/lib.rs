//! `net.ports` — an unprivileged async TCP connect scan. The first module
//! that does something in the real world: it takes a typed target/port request,
//! streams progress, honours cancellation, and returns a structured result.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

use contract::{
    ActionSpec, LogLevel, ModuleDescriptor, ModuleId, ParamSpec, RawParams, Severity, Tactic,
    ViewLine,
};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams, PortSpec};

pub struct PortScan;

const DEFAULT_PORTS: &[u16] = &[22, 80, 443, 445, 3389, 8080, 9000];

#[derive(Debug, Deserialize)]
struct ScanParams {
    /// Target IP or hostname.
    target: String,
    /// Ports (a spec string, array, or number); omitted -> common ports.
    #[serde(default)]
    ports: Option<PortSpec>,
    /// Max simultaneous connects (default 256).
    #[serde(default)]
    concurrency: Option<usize>,
    /// Per-port connect timeout in ms (default 500).
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ScanOutput {
    target: String,
    open_ports: Vec<u16>,
    scanned: usize,
    duration_ms: u64,
}

fn resolve_ports(params: &ScanParams) -> Result<Vec<u16>, ModuleError> {
    match &params.ports {
        None => Ok(DEFAULT_PORTS.to_vec()),
        Some(spec) => {
            let ports = spec.resolve()?;
            Ok(if ports.is_empty() { DEFAULT_PORTS.to_vec() } else { ports })
        }
    }
}

#[async_trait]
impl ImplantModule for PortScan {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("net.ports"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: Some(Tactic::Discovery),
            actions: vec![ActionSpec {
                name: "scan".to_string(),
                description: Some("Unprivileged TCP connect scan of a target".to_string()),
                params: vec![
                    ParamSpec::required("target", "host", "IP or hostname to scan"),
                    ParamSpec::optional("ports", "port-spec", "range/list, e.g. 1-1024 or 22,80,443")
                        .with_default("common ports")
                        .with_example("1-1024"),
                    ParamSpec::optional("timeout_ms", "int", "per-port connect timeout in ms")
                        .with_default("500"),
                ],
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
                "net.ports has no action '{action}'"
            )));
        }

        let params: ScanParams = params.parse()?;
        let ports = resolve_ports(&params)?;
        let total = ports.len();
        let concurrency = params.concurrency.unwrap_or(256).max(1);
        let timeout = Duration::from_millis(params.timeout_ms.unwrap_or(500));
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
                ctx.view(
                    "net.ports",
                    vec![
                        ViewLine { label: "TARGET".into(), value: target.clone(), severity: None },
                        ViewLine {
                            label: "SCANNED".into(),
                            value: format!("{scanned}/{total}"),
                            severity: None,
                        },
                        ViewLine {
                            label: "OPEN".into(),
                            value: open.len().to_string(),
                            severity: if open.is_empty() { None } else { Some(Severity::Medium) },
                        },
                    ],
                );
                // Ambient HUD slot: a live open-port count that shows over any
                // screen while the scan runs, independent of the tactical view
                // above. The operator lists "ports" in `[hud].slots` to see it.
                ctx.widget(
                    "ports",
                    open.len().to_string(),
                    if open.is_empty() { None } else { Some(Severity::Medium) },
                );
            }
            if ctx.cancelled() {
                break;
            }
        }
        // Retract the transient slot now the scan is done.
        ctx.widget("ports", "", None);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_params(json: serde_json::Value) -> ScanParams {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn spec_range() {
        let p = scan_params(serde_json::json!({ "target": "x", "ports": "1-5" }));
        assert_eq!(resolve_ports(&p).unwrap(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn spec_mixed_list_and_range() {
        let p = scan_params(serde_json::json!({ "target": "x", "ports": "22,80,100-102" }));
        assert_eq!(resolve_ports(&p).unwrap(), vec![22, 80, 100, 101, 102]);
    }

    #[test]
    fn array_and_single_and_default() {
        let a = scan_params(serde_json::json!({ "target": "x", "ports": [443, 80] }));
        assert_eq!(resolve_ports(&a).unwrap(), vec![80, 443]);
        let one = scan_params(serde_json::json!({ "target": "x", "ports": 8080 }));
        assert_eq!(resolve_ports(&one).unwrap(), vec![8080]);
        let none = scan_params(serde_json::json!({ "target": "x" }));
        assert_eq!(resolve_ports(&none).unwrap(), DEFAULT_PORTS.to_vec());
    }

    #[test]
    fn bad_port_errors() {
        let p = scan_params(serde_json::json!({ "target": "x", "ports": "1-abc" }));
        assert!(resolve_ports(&p).is_err());
    }
}
