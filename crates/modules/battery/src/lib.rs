//! `sys.battery` — INA219 current/voltage sensor over I2C, converted to an
//! approximate charge percentage. `watch` keeps polling and publishing an
//! ambient HUD slot until cancelled — pair with `skulk.toml`'s
//! `[hud].slots = ["battery"]` for an always-visible reading. Requires
//! `Capability::I2c`; real I2C access is Linux-only (see [`ina219`]).

mod ina219;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use contract::{ActionSpec, Capability, LogLevel, ModuleDescriptor, ModuleId, ParamSpec, RawParams, Severity};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams};

pub struct Battery;

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Params {
    i2c_bus: u8,
    address: u16,
    voltage_min: f64,
    voltage_max: f64,
    warn_pct: f64,
    crit_pct: f64,
    interval_ms: u64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            i2c_bus: 1,
            address: 0x43,
            voltage_min: 3.0,
            voltage_max: 4.2,
            warn_pct: 25.0,
            crit_pct: 10.0,
            interval_ms: 2000,
        }
    }
}

#[derive(Debug, Default, Serialize)]
struct Reading {
    percent: f64,
    voltage: f64,
}

/// Linear voltage -> percent estimate between `voltage_min` (0%) and
/// `voltage_max` (100%) — a deliberate simplification of a true Li-ion
/// discharge curve (which sags nonlinearly near the low end); good enough
/// for an ambient HUD gauge, not a precise fuel gauge.
fn voltage_to_percent(v: f64, voltage_min: f64, voltage_max: f64) -> f64 {
    if voltage_max <= voltage_min {
        return 0.0;
    }
    ((v - voltage_min) / (voltage_max - voltage_min) * 100.0).clamp(0.0, 100.0)
}

/// HUD severity for a reading — `None` above `warn_pct`, `Medium` at/below
/// it, `Critical` at/below `crit_pct`.
fn severity_for(percent: f64, warn_pct: f64, crit_pct: f64) -> Option<Severity> {
    if percent <= crit_pct {
        Some(Severity::Critical)
    } else if percent <= warn_pct {
        Some(Severity::Medium)
    } else {
        None
    }
}

#[async_trait]
impl ImplantModule for Battery {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("sys.battery"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: None, // a utility, not an ATT&CK technique
            actions: vec![
                ActionSpec {
                    name: "get".to_string(),
                    description: Some("One-shot battery voltage/percentage reading (INA219)".to_string()),
                    params: Vec::new(),
                    params_schema: None,
                },
                ActionSpec {
                    name: "watch".to_string(),
                    description: Some(
                        "Poll the battery and publish it to the HUD's \"battery\" slot until cancelled"
                            .to_string(),
                    ),
                    params: vec![
                        ParamSpec::optional("i2c_bus", "int", "I2C bus index").with_default("1"),
                        ParamSpec::optional("address", "int", "INA219 I2C address (decimal)")
                            .with_default("67"),
                        ParamSpec::optional("voltage_min", "int", "voltage at 0%").with_default("3.0"),
                        ParamSpec::optional("voltage_max", "int", "voltage at 100%").with_default("4.2"),
                        ParamSpec::optional("warn_pct", "int", "publish Medium severity at/below this %")
                            .with_default("25"),
                        ParamSpec::optional("crit_pct", "int", "publish Critical severity at/below this %")
                            .with_default("10"),
                        ParamSpec::optional("interval_ms", "int", "polling interval").with_default("2000"),
                    ],
                    params_schema: None,
                },
            ],
            requires: vec![Capability::I2c],
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
                let p: Params = params.parse().unwrap_or_default();
                let (voltage, _current) = ina219::read(p.i2c_bus, p.address)?;
                let percent = voltage_to_percent(voltage, p.voltage_min, p.voltage_max);
                raw_params(&Reading { percent, voltage })
            }
            "watch" => {
                let p: Params = params.parse().unwrap_or_default();
                let interval = Duration::from_millis(p.interval_ms.max(250));
                let mut last = Reading::default();
                loop {
                    if ctx.cancelled() {
                        break;
                    }
                    match ina219::read(p.i2c_bus, p.address) {
                        Ok((voltage, _current)) => {
                            let percent = voltage_to_percent(voltage, p.voltage_min, p.voltage_max);
                            ctx.widget(
                                "battery",
                                format!("{percent:.0}%"),
                                severity_for(percent, p.warn_pct, p.crit_pct),
                            );
                            last = Reading { percent, voltage };
                        }
                        Err(e) => ctx.log(LogLevel::Warn, format!("battery read failed: {e}")),
                    }
                    tokio::time::sleep(interval).await;
                }
                ctx.widget("battery", "", None); // retract the slot once watching stops
                raw_params(&last)
            }
            other => Err(ModuleError::Unsupported(format!("sys.battery has no action '{other}'"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_interpolates_linearly() {
        assert_eq!(voltage_to_percent(3.0, 3.0, 4.2), 0.0);
        assert_eq!(voltage_to_percent(4.2, 3.0, 4.2), 100.0);
        assert!((voltage_to_percent(3.6, 3.0, 4.2) - 50.0).abs() < 0.01);
    }

    #[test]
    fn percent_clamps_outside_the_configured_range() {
        assert_eq!(voltage_to_percent(2.5, 3.0, 4.2), 0.0);
        assert_eq!(voltage_to_percent(5.0, 3.0, 4.2), 100.0);
    }

    #[test]
    fn percent_handles_a_degenerate_range() {
        assert_eq!(voltage_to_percent(3.7, 4.0, 4.0), 0.0, "max <= min must not divide by zero");
    }

    #[test]
    fn severity_is_none_above_warn() {
        assert_eq!(severity_for(50.0, 25.0, 10.0), None);
    }

    #[test]
    fn severity_is_medium_from_warn_down_to_crit() {
        assert_eq!(severity_for(25.0, 25.0, 10.0), Some(Severity::Medium));
        assert_eq!(severity_for(11.0, 25.0, 10.0), Some(Severity::Medium));
    }

    #[test]
    fn severity_is_critical_at_and_below_crit() {
        assert_eq!(severity_for(10.0, 25.0, 10.0), Some(Severity::Critical));
        assert_eq!(severity_for(0.0, 25.0, 10.0), Some(Severity::Critical));
    }

    #[test]
    fn defaults_match_this_projects_reference_hardware() {
        let p = Params::default();
        assert_eq!(p.i2c_bus, 1);
        assert_eq!(p.address, 0x43);
        assert_eq!(p.interval_ms, 2000);
    }

    #[test]
    fn partial_params_keep_the_rest_at_default() {
        let p: Params = RawParams(serde_json::json!({ "warn_pct": 30.0 })).parse().unwrap();
        assert_eq!(p.warn_pct, 30.0);
        assert_eq!(p.crit_pct, 10.0, "unset field keeps its default");
    }
}
