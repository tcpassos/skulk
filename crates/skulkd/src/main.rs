//! `skulkd` — the Skulk implant daemon. Wires the engine, its modules and the
//! socket transport into a single runnable process, driven by a TOML config.
//!
//! Usage:
//!   skulkd                 # load ./skulk.toml (defaults if absent)
//!   skulkd path/to.toml    # load a specific config

mod caps;
mod config;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use contract::ImplantInfo;
use engine::{Engine, RedbLoot};
#[cfg(feature = "mod-dns-recon")]
use dns_recon::DnsRecon;
#[cfg(feature = "mod-sysinfo")]
use example_sysinfo::SysInfo;
#[cfg(feature = "mod-portscan")]
use net_portscan::PortScan;
#[cfg(feature = "mod-services")]
use net_services::Services;
use transport::{run_dialer, run_listener, TransportConfig};

use config::{Config, Mode};

#[tokio::main]
async fn main() {
    let config_path = std::env::args().nth(1).unwrap_or_else(|| "skulk.toml".to_string());
    let config = match Config::load(&PathBuf::from(&config_path)) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("skulkd: {e}");
            std::process::exit(1);
        }
    };

    init_tracing(&config.log);

    // Loot path comes from config, but SKULK_LOOT overrides it for deployment/tests.
    let loot_path = std::env::var("SKULK_LOOT").unwrap_or_else(|_| config.loot.path.clone());

    let hardware = if config.implant.hardware.is_empty() {
        format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
    } else {
        config.implant.hardware.clone()
    };
    let implant = ImplantInfo {
        id: config.implant.id.clone(),
        hardware,
        firmware: env!("CARGO_PKG_VERSION").to_string(),
    };

    let capabilities = caps::detect();
    tracing::info!(?capabilities, "detected capabilities");

    let loot = match RedbLoot::open(&loot_path) {
        Ok(store) => Arc::new(store),
        Err(e) => {
            tracing::error!(path = %loot_path, "cannot open loot store: {e}");
            std::process::exit(1);
        }
    };

    let mut engine = Engine::new(implant, capabilities, loot);
    #[cfg(feature = "mod-sysinfo")]
    engine.register(Arc::new(SysInfo));
    #[cfg(feature = "mod-portscan")]
    engine.register(Arc::new(PortScan));
    #[cfg(feature = "mod-services")]
    engine.register(Arc::new(Services));
    #[cfg(feature = "mod-dns-recon")]
    engine.register(Arc::new(DnsRecon));
    engine.set_peripherals(config.peripherals.into_iter().map(Into::into).collect());
    let engine = Arc::new(engine);

    spawn_lcd(&engine, &config.display);

    if config.heartbeat_secs > 0 {
        engine.spawn_heartbeat(Duration::from_secs(config.heartbeat_secs));
    }

    let modules = engine.module_ids();
    tracing::info!(
        id = %config.implant.id,
        loot = %loot_path,
        heartbeat_secs = config.heartbeat_secs,
        ?modules,
        "implant ready"
    );

    let transport = TransportConfig::default();
    let addr = config.transport.addr.clone();
    let mode = config.transport.mode;
    let shutdown = engine.clone();

    // Serve until the transport ends (error) or a Shutdown command is received.
    tokio::select! {
        result = async {
            match mode {
                Mode::Dial => {
                    tracing::info!(%addr, "reverse tunnel: dialing controller");
                    run_dialer(engine, addr.clone(), transport).await;
                    Ok::<(), std::io::Error>(())
                }
                Mode::Listen => {
                    tracing::info!(%addr, "listening for controller");
                    run_listener(engine, &addr, transport).await
                }
            }
        } => {
            if let Err(e) = result {
                tracing::error!("transport error: {e}");
                std::process::exit(1);
            }
        }
        _ = shutdown.wait_for_shutdown() => {
            tracing::info!("shutdown requested by controller; stopping");
        }
    }
}

/// Bring up the on-device LCD, if `[display]` names a compiled-in driver.
/// Never fatal: a missing/misconfigured screen logs and leaves the daemon
/// running headless, the same "fail clean" stance as capability-gating.
#[cfg(all(feature = "lcd", target_os = "linux"))]
fn spawn_lcd(engine: &Arc<Engine>, disp: &config::DisplaySection) {
    // Named `disp`, not `display`: `tracing`'s field macros treat a bare
    // `display` identifier specially (it's also their `%`-sigil helper fn),
    // and a same-named local shadows it in a way that fails to compile.
    if disp.driver.is_empty() {
        return;
    }
    if disp.driver != "mipidsi" {
        tracing::warn!(driver = %disp.driver, "lcd: unknown driver, display stays off");
        return;
    }
    if disp.chip.is_empty() {
        tracing::warn!("lcd: [display].chip is required for driver 'mipidsi' (e.g. \"st7789\" or \"st7735s\")");
        return;
    }

    let to_u16 = |field: &str, value: u32| match u16::try_from(value) {
        Ok(v) => Some(v),
        Err(_) => {
            tracing::error!(field, value, "lcd: value out of range for a u16");
            None
        }
    };
    let (Some(width), Some(height), Some(offset_x), Some(offset_y)) = (
        to_u16("width", disp.width),
        to_u16("height", disp.height),
        to_u16("offset_x", disp.offset_x),
        to_u16("offset_y", disp.offset_y),
    ) else {
        return;
    };
    let mipidsi_config = lcd_render::MipidsiConfig {
        width,
        height,
        offset_x,
        offset_y,
        spi_bus: disp.spi_bus,
        spi_cs: disp.spi_cs,
        dc_gpio: disp.dc_gpio,
        rst_gpio: disp.rst_gpio,
        bl_gpio: disp.bl_gpio,
        bgr: disp.bgr,
    };

    match disp.chip.as_str() {
        "st7789" => match lcd_render::open_st7789(&mipidsi_config) {
            Ok(panel) => {
                tracing::info!(width, height, chip = "st7789", "lcd: display ready");
                lcd_render::spawn(engine.subscribe(), panel);
            }
            Err(e) => tracing::error!("lcd: cannot open display: {e}"),
        },
        "st7735s" => match lcd_render::open_st7735s(&mipidsi_config) {
            Ok(panel) => {
                tracing::info!(width, height, chip = "st7735s", "lcd: display ready");
                lcd_render::spawn(engine.subscribe(), panel);
            }
            Err(e) => tracing::error!("lcd: cannot open display: {e}"),
        },
        other => {
            tracing::warn!(chip = other, "lcd: unknown chip for driver 'mipidsi', display stays off");
        }
    }
}

#[cfg(not(all(feature = "lcd", target_os = "linux")))]
fn spawn_lcd(_engine: &Arc<Engine>, _display: &config::DisplaySection) {}

/// Initialise structured logging to stderr. `RUST_LOG` overrides the config filter.
fn init_tracing(filter: &str) {
    use tracing_subscriber::{fmt, EnvFilter};

    let env = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter));
    fmt()
        .with_env_filter(env)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
}
