//! `dns.records` — standard DNS record enumeration plus an AXFR (zone
//! transfer) probe against every nameserver discovered for the domain. A
//! nameserver that honours an unauthenticated AXFR leaks the entire zone —
//! a classic, high-value DNS misconfiguration finding.

use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::join_all;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

use contract::{
    ActionSpec, LogLevel, LootKind, ModuleDescriptor, ModuleId, ParamSpec, RawParams, Severity,
    Tactic,
};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams};

use hickory_client::client::{Client, ClientHandle};
use hickory_client::proto::op::ResponseCode;
use hickory_client::proto::rr::{Name, Record, RecordType};
use hickory_client::proto::runtime::TokioRuntimeProvider;
use hickory_client::proto::tcp::TcpClientStream;
use hickory_resolver::config::{NameServerConfigGroup, ResolverConfig};
use hickory_resolver::name_server::TokioConnectionProvider;
use hickory_resolver::TokioResolver;

pub struct DnsRecon;

const DEFAULT_RECORD_TYPES: &[&str] = &["A", "AAAA", "NS", "MX", "TXT", "SOA", "CNAME"];
const DEFAULT_TIMEOUT_MS: u64 = 4000;
const DEFAULT_RESOLVER_PORT: u16 = 53;
const DEFAULT_AXFR_PORT: u16 = 53;

#[derive(Debug, Deserialize)]
struct EnumParams {
    domain: String,
    #[serde(default)]
    resolver: Option<String>,
    #[serde(default)]
    record_types: Option<RecordTypeSpec>,
    #[serde(default)]
    axfr: Option<bool>,
    #[serde(default)]
    axfr_port: Option<u16>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Record types to query: a comma-separated spec string (CLI-friendly) or an
/// explicit JSON array. Mirrors `module_sdk::PortSpec`'s flexible-input idiom.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RecordTypeSpec {
    Spec(String),
    List(Vec<String>),
}

impl RecordTypeSpec {
    fn resolve(&self) -> Result<Vec<RecordType>, ModuleError> {
        let names: Vec<String> = match self {
            RecordTypeSpec::List(v) => v.clone(),
            RecordTypeSpec::Spec(s) => {
                s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).collect()
            }
        };
        names
            .iter()
            .map(|n| {
                RecordType::from_str(&n.to_ascii_uppercase())
                    .map_err(|_| ModuleError::InvalidParams(format!("unknown record type '{n}'")))
            })
            .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
struct DnsRecordOut {
    name: String,
    rtype: String,
    ttl: u32,
    data: String,
}

impl From<&Record> for DnsRecordOut {
    fn from(r: &Record) -> Self {
        Self {
            name: r.name().to_string(),
            rtype: r.record_type().to_string(),
            ttl: r.ttl(),
            data: r.data().to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct AxfrAttempt {
    server: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loot_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl AxfrAttempt {
    fn failed(server: String, status: &str, detail: impl Into<String>) -> Self {
        Self { server, status: status.to_string(), record_count: None, loot_key: None, detail: Some(detail.into()) }
    }
}

#[derive(Debug, Serialize)]
struct EnumOutput {
    domain: String,
    records: Vec<DnsRecordOut>,
    name_servers: Vec<String>,
    axfr: Vec<AxfrAttempt>,
    duration_ms: u64,
}

#[async_trait]
impl ImplantModule for DnsRecon {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("dns.records"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: Some(Tactic::Discovery),
            actions: vec![ActionSpec {
                name: "enum".to_string(),
                description: Some(
                    "Enumerate standard DNS records and probe each nameserver for an open AXFR zone transfer"
                        .to_string(),
                ),
                params: vec![
                    ParamSpec::required("domain", "domain", "target domain, e.g. example.com"),
                    ParamSpec::optional("resolver", "host", "DNS server to query, e.g. 8.8.8.8 or 8.8.8.8:53")
                        .with_default("system resolver"),
                    ParamSpec::optional("record_types", "string-list", "record types to query")
                        .with_default("A,AAAA,NS,MX,TXT,SOA,CNAME")
                        .with_example("A,MX,TXT"),
                    ParamSpec::optional("axfr", "bool", "attempt zone transfer against each discovered nameserver")
                        .with_default("true"),
                    ParamSpec::optional("axfr_port", "int", "nameserver port for the AXFR probe")
                        .with_default("53"),
                    ParamSpec::optional("timeout_ms", "int", "per-query / per-AXFR-attempt timeout in ms")
                        .with_default("4000"),
                ],
                params_schema: None,
            }],
            requires: Vec::new(), // plain UDP/TCP DNS queries — no special hardware
        }
    }

    async fn invoke(
        &self,
        ctx: &ModuleCtx,
        action: &str,
        params: RawParams,
    ) -> Result<RawParams, ModuleError> {
        if action != "enum" {
            return Err(ModuleError::Unsupported(format!("dns.records has no action '{action}'")));
        }

        let params: EnumParams = params.parse()?;
        let requested = params.domain.trim();
        if requested.is_empty() {
            return Err(ModuleError::InvalidParams("domain must not be empty".into()));
        }
        let domain = fqdn(requested);

        let record_types = match &params.record_types {
            Some(spec) => spec.resolve()?,
            None => DEFAULT_RECORD_TYPES
                .iter()
                .map(|s| RecordType::from_str(s).expect("valid default record type"))
                .collect(),
        };
        let want_axfr = params.axfr.unwrap_or(true);
        let axfr_port = params.axfr_port.unwrap_or(DEFAULT_AXFR_PORT);
        let timeout = Duration::from_millis(params.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));

        let resolver = build_resolver(params.resolver.as_deref(), timeout)?;

        ctx.log(LogLevel::Info, format!("enumerating {} record types for {domain}", record_types.len()));
        let started = Instant::now();

        let lookups = record_types.iter().copied().map(|rt| {
            let resolver = &resolver;
            let domain = domain.clone();
            async move { (rt, resolver.lookup(domain, rt).await) }
        });
        let mut records = Vec::new();
        for (rt, result) in join_all(lookups).await {
            match result {
                Ok(lookup) => records.extend(lookup.records().iter().map(DnsRecordOut::from)),
                Err(e) => ctx.log(LogLevel::Debug, format!("{rt} lookup for {domain}: {e}")),
            }
        }
        ctx.progress(Some(40), format!("{} records found", records.len()));

        // NS discovery for the AXFR probe, independent of whether NS is in `record_types`.
        let ns_names: Vec<Name> = match resolver.lookup(domain.clone(), RecordType::NS).await {
            Ok(lookup) => {
                lookup.records().iter().filter_map(|r| r.data().as_ns().map(|ns| ns.0.clone())).collect()
            }
            Err(e) => {
                ctx.log(LogLevel::Debug, format!("NS lookup for {domain}: {e}"));
                Vec::new()
            }
        };
        let name_servers: Vec<String> = ns_names.iter().map(|n| n.to_string()).collect();
        ctx.progress(Some(60), format!("{} nameservers", name_servers.len()));

        let axfr = if want_axfr && !ns_names.is_empty() {
            let attempts = ns_names
                .iter()
                .map(|ns| try_axfr(ctx, &resolver, ns.clone(), domain.clone(), axfr_port, timeout));
            join_all(attempts).await
        } else {
            Vec::new()
        };
        ctx.progress(Some(100), "done");

        let output = EnumOutput {
            domain,
            records,
            name_servers,
            axfr,
            duration_ms: started.elapsed().as_millis() as u64,
        };
        raw_params(&output)
    }
}

/// Force a fully-qualified name so the resolver never appends the host's own
/// search domains to a target the operator explicitly typed out.
fn fqdn(domain: &str) -> String {
    if domain.ends_with('.') { domain.to_string() } else { format!("{domain}.") }
}

fn parse_resolver(s: &str) -> Result<(IpAddr, u16), ModuleError> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok((addr.ip(), addr.port()));
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok((ip, DEFAULT_RESOLVER_PORT));
    }
    Err(ModuleError::InvalidParams(format!("bad resolver address '{s}', expected ip or ip:port")))
}

fn build_resolver(resolver: Option<&str>, timeout: Duration) -> Result<TokioResolver, ModuleError> {
    let mut builder = match resolver {
        Some(s) => {
            let (ip, port) = parse_resolver(s)?;
            let group = NameServerConfigGroup::from_ips_clear(&[ip], port, true);
            let config = ResolverConfig::from_parts(None, vec![], group);
            TokioResolver::builder_with_config(config, TokioConnectionProvider::default())
        }
        None => TokioResolver::builder_tokio()
            .map_err(|e| ModuleError::Failed(format!("cannot read system DNS config: {e}")))?,
    };
    builder.options_mut().timeout = timeout;
    Ok(builder.build())
}

/// Resolve one nameserver's address and attempt a full zone transfer against it.
async fn try_axfr(
    ctx: &ModuleCtx,
    resolver: &TokioResolver,
    ns_name: Name,
    zone: String,
    port: u16,
    timeout: Duration,
) -> AxfrAttempt {
    let label = ns_name.to_string();

    let ip = match resolver.lookup_ip(ns_name).await {
        Ok(lookup) => match lookup.iter().next() {
            Some(ip) => ip,
            None => return AxfrAttempt::failed(label, "error", "nameserver has no A/AAAA record"),
        },
        Err(e) => return AxfrAttempt::failed(label, "error", format!("cannot resolve nameserver: {e}")),
    };

    let addr = SocketAddr::new(ip, port);
    let server = format!("{addr} ({label})");

    match tokio::time::timeout(timeout, run_axfr(addr, zone.clone(), timeout)).await {
        Err(_) => AxfrAttempt::failed(server, "timeout", format!("no response within {timeout:?}")),
        Ok(Err(detail)) => AxfrAttempt::failed(server, "refused", detail),
        Ok(Ok(records)) => {
            let count = records.len();
            ctx.alert(
                Severity::High,
                format!("AXFR zone transfer succeeded against {server} for {zone} — {count} records leaked"),
            );
            // Timestamped, not a fixed per-target key: re-transferring the
            // same zone keeps its own snapshot instead of overwriting the
            // last one.
            let key = module_sdk::timestamped_key(&format!(
                "dns/axfr/{}/{}",
                zone.trim_end_matches('.'),
                label.trim_end_matches('.')
            ));
            let bytes = serde_json::to_vec(&records).unwrap_or_default();
            if let Err(e) = ctx.store_loot(LootKind::Other, &key, bytes).await {
                ctx.log(LogLevel::Warn, format!("failed to store AXFR loot for {server}: {e}"));
            }
            AxfrAttempt {
                server,
                status: "success".to_string(),
                record_count: Some(count),
                loot_key: Some(key),
                detail: None,
            }
        }
    }
}

async fn run_axfr(addr: SocketAddr, zone: String, timeout: Duration) -> Result<Vec<DnsRecordOut>, String> {
    let name = Name::from_str(&zone).map_err(|e| format!("bad zone name: {e}"))?;

    let (stream, sender) = TcpClientStream::new(addr, None, Some(timeout), TokioRuntimeProvider::new());
    let (mut client, bg) = Client::with_timeout(stream, sender, timeout, None)
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    tokio::spawn(bg);

    let mut xfr = client.zone_transfer(name, None);
    let mut records = Vec::new();
    let mut first = true;
    while let Some(item) = xfr.next().await {
        let response = item.map_err(|e| e.to_string())?;
        if first {
            first = false;
            let code = response.response_code();
            if code != ResponseCode::NoError {
                return Err(format!("server replied {code}"));
            }
        }
        records.extend(response.answers().iter().map(DnsRecordOut::from));
    }

    if records.is_empty() {
        return Err("empty response (not authoritative for this zone?)".to_string());
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(json: serde_json::Value) -> EnumParams {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn record_types_comma_spec() {
        let p = spec(serde_json::json!({ "domain": "x", "record_types": "a, mx,TXT" }));
        let types = p.record_types.unwrap().resolve().unwrap();
        assert_eq!(types, vec![RecordType::A, RecordType::MX, RecordType::TXT]);
    }

    #[test]
    fn record_types_array_spec() {
        let p = spec(serde_json::json!({ "domain": "x", "record_types": ["NS", "SOA"] }));
        let types = p.record_types.unwrap().resolve().unwrap();
        assert_eq!(types, vec![RecordType::NS, RecordType::SOA]);
    }

    #[test]
    fn record_types_unknown_errors() {
        let p = spec(serde_json::json!({ "domain": "x", "record_types": "BOGUS" }));
        assert!(p.record_types.unwrap().resolve().is_err());
    }

    #[test]
    fn fqdn_appends_trailing_dot() {
        assert_eq!(fqdn("example.com"), "example.com.");
        assert_eq!(fqdn("example.com."), "example.com.");
    }

    #[test]
    fn parse_resolver_ip_only_defaults_port() {
        assert_eq!(parse_resolver("8.8.8.8").unwrap(), ("8.8.8.8".parse().unwrap(), 53));
    }

    #[test]
    fn parse_resolver_ip_and_port() {
        assert_eq!(parse_resolver("8.8.8.8:5353").unwrap(), ("8.8.8.8".parse().unwrap(), 5353));
    }

    #[test]
    fn parse_resolver_rejects_garbage() {
        assert!(parse_resolver("not-an-ip").is_err());
    }
}
