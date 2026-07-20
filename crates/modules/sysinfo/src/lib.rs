//! Reference module: the smallest complete example of the contributor workflow.
//! It defines its own params/output types, a descriptor, and an `invoke` that
//! talks only through [`ModuleCtx`]. It depends on `module-sdk` + `contract` and
//! knows nothing about the engine.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use contract::{ActionSpec, LogLevel, LootKind, ModuleDescriptor, ModuleId, ParamSpec, RawParams};
use module_sdk::{raw_params, ImplantModule, ModuleCtx, ModuleError, ParseParams};

pub struct SysInfo;

/// The module's own request type — the engine never sees this; it deserializes
/// from the opaque `RawParams`.
#[derive(Debug, Default, Deserialize)]
struct GetParams {
    #[serde(default)]
    verbose: bool,
}

/// The module's own result type — serialized back into the task output.
#[derive(Debug, Serialize)]
struct HostInfo {
    os: String,
    arch: String,
    family: String,
    verbose: bool,
}

#[async_trait]
impl ImplantModule for SysInfo {
    fn descriptor(&self) -> ModuleDescriptor {
        ModuleDescriptor {
            id: ModuleId::from("sys.info"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            tactic: None, // a utility, not an ATT&CK technique
            actions: vec![ActionSpec {
                name: "get".to_string(),
                description: Some("Report basic host info".to_string()),
                params: vec![
                    ParamSpec::optional("verbose", "bool", "include extra detail").with_default("false"),
                ],
                params_schema: None,
            }],
            requires: Vec::new(), // needs no special hardware
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
                ctx.log(LogLevel::Info, "collecting host info");
                ctx.progress(Some(50), "reading env");

                let p: GetParams = params.parse().unwrap_or_default();
                let info = HostInfo {
                    os: std::env::consts::OS.to_string(),
                    arch: std::env::consts::ARCH.to_string(),
                    family: std::env::consts::FAMILY.to_string(),
                    verbose: p.verbose,
                };

                let bytes = serde_json::to_vec(&info).unwrap_or_default();
                // Timestamped, not a fixed key: each run keeps its own
                // snapshot instead of overwriting the last one.
                ctx.store_loot(LootKind::Telemetry, module_sdk::timestamped_key("sysinfo"), bytes).await?;
                ctx.progress(Some(100), "done");

                raw_params(&info)
            }
            other => Err(ModuleError::Unsupported(format!("sys.info has no action '{other}'"))),
        }
    }
}
