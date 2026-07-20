//! `sys.temp` — CPU/SoC thermal-zone monitoring. Reads the Linux kernel's
//! standard thermal sysfs interface, present on every Raspberry Pi (and most
//! Linux SBCs) with no extra hardware or wiring. `watch` keeps polling and
//! publishing an ambient HUD slot until cancelled — pair with `skulk.toml`'s
//! `[hud].slots = ["temp"]` for an always-visible reading.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use contract::{ActionSpec, LogLevel, ModuleDescriptor, ModuleId, ParamSpec, RawParams, Severity};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams};

pub struct Temp;

/// The kernel's standard CPU thermal zone — present on every Raspberry Pi
/// (and most Linux SBCs), no extra wiring needed. A board with more than one
/// zone still exposes this one; it's always the SoC/CPU package.
const THERMAL_ZONE: &str = "/sys/class/thermal/thermal_zone0/temp";

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Params {
    warn_c: f64,
    crit_c: f64,
    interval_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self { warn_c: 70.0, crit_c: 80.0, interval_ms: 2000 }
    }
}

#[derive(Debug, Default, Serialize)]
struct Reading {
    celsius: f64,
}

/// Read and parse the thermal zone — millidegrees C as a plain integer
/// string, per the kernel's thermal sysfs ABI.
fn read_celsius() -> Result<f64, ModuleError> {
    let raw = std::fs::read_to_string(THERMAL_ZONE)
        .map_err(|e| ModuleError::Failed(format!("cannot read {THERMAL_ZONE}: {e}")))?;
    let millidegrees: i64 = raw
        .trim()
        .parse()
        .map_err(|e| ModuleError::Failed(format!("unexpected content in {THERMAL_ZONE}: {e}")))?;
    Ok(millidegrees as f64 / 1000.0)
}

/// HUD severity for a reading — `None` below `warn_c`, `Medium` at/above it,
/// `Critical` at/above `crit_c`.
fn severity_for(celsius: f64, warn_c: f64, crit_c: f64) -> Option<Severity> {
    if celsius >= crit_c {
        Some(Severity::Critical)
    } else if celsius >= warn_c {
        Some(Severity::Medium)
    } else {
        None
    }
}

#[async_trait]
impl ImplantModule for Temp {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("sys.temp"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: None, // a utility, not an ATT&CK technique
            actions: vec![
                ActionSpec {
                    name: "get".to_string(),
                    description: Some("One-shot CPU/SoC temperature reading".to_string()),
                    params: Vec::new(),
                    params_schema: None,
                },
                ActionSpec {
                    name: "watch".to_string(),
                    description: Some(
                        "Poll the temperature and publish it to the HUD's \"temp\" slot until cancelled"
                            .to_string(),
                    ),
                    params: vec![
                        ParamSpec::optional("warn_c", "int", "publish Medium severity at/above this \u{b0}C")
                            .with_default("70"),
                        ParamSpec::optional("crit_c", "int", "publish Critical severity at/above this \u{b0}C")
                            .with_default("80"),
                        ParamSpec::optional("interval_ms", "int", "polling interval").with_default("2000"),
                    ],
                    params_schema: None,
                },
            ],
            requires: Vec::new(), // best-effort sysfs read; fails cleanly if absent
        }
    }

    async fn invoke(
        &self,
        ctx: &ModuleCtx,
        action: &str,
        params: RawParams,
    ) -> Result<RawParams, ModuleError> {
        match action {
            "get" => {
                let celsius = read_celsius()?;
                raw_params(&Reading { celsius })
            }
            "watch" => {
                let p: Params = params.parse().unwrap_or_default();
                let interval = Duration::from_millis(p.interval_ms.max(250));
                let mut last = Reading::default();
                loop {
                    if ctx.cancelled() {
                        break;
                    }
                    match read_celsius() {
                        Ok(celsius) => {
                            ctx.widget(
                                "temp",
                                format!("{celsius:.0}C"),
                                severity_for(celsius, p.warn_c, p.crit_c),
                            );
                            last = Reading { celsius };
                        }
                        Err(e) => ctx.log(LogLevel::Warn, format!("temp read failed: {e}")),
                    }
                    tokio::time::sleep(interval).await;
                }
                ctx.widget("temp", "", None); // retract the slot once watching stops
                raw_params(&last)
            }
            other => Err(ModuleError::Unsupported(format!("sys.temp has no action '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_is_none_below_warn() {
        assert_eq!(severity_for(50.0, 70.0, 80.0), None);
    }

    #[test]
    fn severity_is_medium_from_warn_up_to_crit() {
        assert_eq!(severity_for(70.0, 70.0, 80.0), Some(Severity::Medium));
        assert_eq!(severity_for(79.9, 70.0, 80.0), Some(Severity::Medium));
    }

    #[test]
    fn severity_is_critical_at_and_above_crit() {
        assert_eq!(severity_for(80.0, 70.0, 80.0), Some(Severity::Critical));
        assert_eq!(severity_for(95.0, 70.0, 80.0), Some(Severity::Critical));
    }

    #[test]
    fn defaults_are_sane_pi_thresholds() {
        let p = Params::default();
        assert_eq!(p.warn_c, 70.0);
        assert_eq!(p.crit_c, 80.0);
        assert_eq!(p.interval_ms, 2000);
    }

    #[test]
    fn missing_params_fall_back_to_defaults() {
        let p: Params = RawParams::default().parse().unwrap_or_default();
        assert_eq!(p.interval_ms, 2000);
    }

    #[test]
    fn partial_params_keep_the_rest_at_default() {
        let p: Params = RawParams(serde_json::json!({ "warn_c": 60.0 })).parse().unwrap();
        assert_eq!(p.warn_c, 60.0);
        assert_eq!(p.crit_c, 80.0, "unset field keeps its default");
    }
}
