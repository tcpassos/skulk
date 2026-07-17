//! Runtime configuration, loaded from a TOML file. Every field is optional and
//! falls back to a sane default, so a bare `implantd` with no config still runs.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub implant: ImplantConfig,
    pub transport: TransportSection,
    pub loot: LootConfig,
    /// Log filter in `RUST_LOG` syntax, e.g. `"info"` or `"engine=debug,info"`.
    pub log: String,
    /// Heartbeat interval in seconds; `0` disables the heartbeat.
    pub heartbeat_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            implant: ImplantConfig::default(),
            transport: TransportSection::default(),
            loot: LootConfig::default(),
            log: "info".to_string(),
            heartbeat_secs: 30,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImplantConfig {
    pub id: String,
    /// Hardware label; empty means auto-detect as `os/arch`.
    pub hardware: String,
}

impl Default for ImplantConfig {
    fn default() -> Self {
        Self { id: "implant-dev".to_string(), hardware: String::new() }
    }
}

#[derive(Debug, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Accept inbound connections (dev / USB-OTG).
    Listen,
    /// Dial out to a controller and keep reconnecting (production reverse tunnel).
    Dial,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TransportSection {
    pub mode: Mode,
    pub addr: String,
}

impl Default for TransportSection {
    fn default() -> Self {
        Self { mode: Mode::Listen, addr: "127.0.0.1:9000".to_string() }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LootConfig {
    /// On-disk path of the redb loot store; overridable at runtime via `SKULK_LOOT`.
    pub path: String,
}

impl Default for LootConfig {
    fn default() -> Self {
        Self { path: "skulk-loot.redb".to_string() }
    }
}

impl Config {
    /// Load configuration from `path`. A missing file is not an error — defaults
    /// are used. Only a malformed file fails.
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("cannot read config {}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The config shipped at the repo root must actually parse. Guards against
    /// TOML pitfalls like top-level keys placed after a [table] header.
    #[test]
    fn shipped_config_parses() {
        let text = include_str!("../../../skulk.toml");
        let cfg: Config = toml::from_str(text).expect("shipped skulk.toml must parse");
        assert_eq!(cfg.transport.mode, Mode::Listen);
        assert_eq!(cfg.transport.addr, "127.0.0.1:9000");
        assert_eq!(cfg.heartbeat_secs, 30);
        assert_eq!(cfg.log, "info");
    }

    #[test]
    fn empty_config_uses_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.transport.addr, "127.0.0.1:9000");
        assert_eq!(cfg.loot.path, "skulk-loot.redb");
        assert_eq!(cfg.heartbeat_secs, 30);
    }
}
