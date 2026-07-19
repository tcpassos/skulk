//! Runtime configuration, loaded from a TOML file. Every field is optional and
//! falls back to a sane default, so a bare `implantd` with no config still runs.

use std::collections::HashMap;
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
    /// On-device LCD, if any (the `lcd` feature). Empty `driver` means none.
    pub display: DisplaySection,
    /// Physical buttons/indicators/encoders wired to the device, surfaced in
    /// the `Manifest` and consulted by the LCD's navigation.
    pub peripherals: Vec<PeripheralConfig>,
    /// Operator override of the LCD's peripheral-name -> nav-action mapping
    /// (`lcd_render::NavMap`'s highest-precedence layer, above a theme's own
    /// `[nav]`). Peripheral name -> one of `"up"`/`"down"`/`"select"`/`"back"`.
    pub nav: HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            implant: ImplantConfig::default(),
            transport: TransportSection::default(),
            loot: LootConfig::default(),
            nav: HashMap::new(),
            log: "info".to_string(),
            heartbeat_secs: 30,
            display: DisplaySection::default(),
            peripherals: Vec::new(),
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

#[derive(Debug, Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum DisplayInterface {
    Spi,
    I2c,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplaySection {
    /// Which `lcd-render` backend to drive, e.g. `"mipidsi"`. Empty means no
    /// physical display is attached — the `lcd` feature, if compiled in,
    /// stays idle.
    pub driver: String,
    /// Controller chip within `driver`, e.g. `"st7789"` (Waveshare 1.14"
    /// LCD Module) or `"st7735s"` (Waveshare 1.44" LCD HAT). Ignored by
    /// drivers that only support one chip.
    pub chip: String,
    pub width: u32,
    pub height: u32,
    /// Offset of the visible panel within the controller's addressable
    /// framebuffer — most small SPI TFTs (including the Waveshare 1.14"
    /// this was built against) need a nonzero value or the image is
    /// shifted/cropped. Start at 0 and adjust against the real hardware;
    /// the same values keep working regardless of `rotation` below.
    pub offset_x: u32,
    pub offset_y: u32,
    /// Clockwise rotation in degrees: 0 (default), 90, 180, or 270.
    pub rotation: u16,
    pub interface: DisplayInterface,
    /// SPI bus/chip-select index (maps to rppal's `Bus`/`SlaveSelect`).
    pub spi_bus: u8,
    pub spi_cs: u8,
    /// BCM GPIO numbers for the data/command, reset, and backlight lines.
    pub dc_gpio: u8,
    pub rst_gpio: u8,
    pub bl_gpio: u8,
    /// Panel subpixel order. Most boards are RGB (the default); some,
    /// including the Waveshare 1.44" LCD HAT / ST7735S, need BGR or colors
    /// come out swapped.
    pub bgr: bool,
}

impl Default for DisplaySection {
    fn default() -> Self {
        Self {
            driver: String::new(),
            chip: String::new(),
            width: 0,
            height: 0,
            offset_x: 0,
            offset_y: 0,
            rotation: 0,
            interface: DisplayInterface::Spi,
            spi_bus: 0,
            spi_cs: 0,
            dc_gpio: 0,
            rst_gpio: 0,
            bl_gpio: 0,
            bgr: false,
        }
    }
}

/// A `[[peripherals]]` entry: physical wiring, config-local (kept separate
/// from `contract::Peripheral` so a typo in `skulk.toml` is caught by
/// `deny_unknown_fields` — the wire type stays lenient on purpose).
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PeripheralConfig {
    pub name: String,
    pub kind: PeripheralKindConfig,
    /// One GPIO pin for a button/indicator, two for a rotary encoder's
    /// quadrature pair.
    pub gpio: Vec<u8>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeripheralKindConfig {
    Button,
    Indicator,
    RotaryEncoder,
}

impl From<PeripheralConfig> for contract::Peripheral {
    fn from(p: PeripheralConfig) -> Self {
        let kind = match p.kind {
            PeripheralKindConfig::Button => contract::PeripheralKind::Button,
            PeripheralKindConfig::Indicator => contract::PeripheralKind::Indicator,
            PeripheralKindConfig::RotaryEncoder => contract::PeripheralKind::RotaryEncoder,
        };
        contract::Peripheral { name: p.name, kind, gpio: p.gpio }
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
        assert_eq!(cfg.display.driver, "", "no display by default");
        assert!(cfg.peripherals.is_empty());
        assert!(cfg.nav.is_empty());
    }

    #[test]
    fn display_and_peripherals_parse() {
        let text = r#"
            [display]
            driver = "mipidsi"
            chip = "st7789"
            width = 240
            height = 135
            offset_x = 40
            offset_y = 53
            rotation = 180
            interface = "spi"
            spi_bus = 0
            spi_cs = 0
            dc_gpio = 25
            rst_gpio = 27
            bl_gpio = 24
            bgr = true

            [[peripherals]]
            name = "btn_a"
            kind = "button"
            gpio = [17]

            [[peripherals]]
            name = "encoder"
            kind = "rotary_encoder"
            gpio = [5, 6]

            [nav]
            btn_a = "up"
        "#;
        let cfg: Config = toml::from_str(text).expect("display + peripherals config must parse");
        assert_eq!(cfg.display.driver, "mipidsi");
        assert_eq!(cfg.display.chip, "st7789");
        assert_eq!(cfg.display.width, 240);
        assert_eq!(cfg.display.height, 135);
        assert_eq!(cfg.display.offset_x, 40);
        assert_eq!(cfg.display.offset_y, 53);
        assert_eq!(cfg.display.rotation, 180);
        assert_eq!(cfg.display.interface, DisplayInterface::Spi);
        assert_eq!(cfg.display.bl_gpio, 24);
        assert!(cfg.display.bgr);

        assert_eq!(cfg.peripherals.len(), 2);
        assert_eq!(cfg.peripherals[0].name, "btn_a");
        assert_eq!(cfg.peripherals[0].gpio, vec![17]);
        assert_eq!(cfg.peripherals[1].gpio, vec![5, 6]);

        let converted: contract::Peripheral = cfg.peripherals[0].clone().into();
        assert_eq!(converted.kind, contract::PeripheralKind::Button);

        assert_eq!(cfg.nav.get("btn_a"), Some(&"up".to_string()));
    }
}
