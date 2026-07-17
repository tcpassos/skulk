# CLAUDE.md — Skulk

Guidance for AI assistants (Claude Code) working in this repo. Read this first.

## What Skulk is

A modular, transport-agnostic **pentest implant engine** in async Rust for
low-power SBCs (Raspberry Pi Zero 2 W and up). Design philosophy: a **dumb daemon
with an external brain** — the device exposes a language-agnostic socket protocol
and just executes structured instructions; all intelligence (operator, automation,
AI orchestrator) lives *outside* and speaks that protocol. For authorized
security testing, research and education only.

## Workspace map (dependency arrow always points inward, toward `contract`)

```
contract      protocol types: Envelope / Command / Event / Manifest / LootEntry. No internal deps.
module-sdk    ImplantModule trait, ModuleCtx, LootSink, ParseParams.   deps: contract
engine        event bus, module registry, dispatcher, capability-gating,
              loot stores (MemLoot / RedbLoot).                         deps: contract, module-sdk
transport     socket adapter — TCP + JSON-lines
              (serve_connection / run_listener / run_dialer).           deps: contract, engine
client        async client library (Client: connect/send/recv/run/
              watch/describe/loot). Proves the protocol is the boundary. deps: contract ONLY
skulk-cli     the `skulk` CLI (binary named `skulk`).                    deps: client, contract
skulkd        the daemon binary: engine + modules + transport.          deps: engine, transport, modules
crates/modules/*   attack/recon modules. e.g. example-sysinfo -> sys.info,
                   net-portscan -> net.ports, net-services -> net.services. deps: module-sdk, contract
```

**Rule:** modules never depend on `engine`; they depend on `module-sdk` + `contract`.
`client` depends only on `contract`.

## Build / test / run

```sh
cargo build
cargo test                    # full suite — keep it green
cargo run -p skulkd           # daemon; reads ./skulk.toml (defaults if absent); listens 127.0.0.1:9000
target/debug/skulk describe   # the CLI (see `skulk help`)
# slim firmware (only chosen modules):
cargo build -p skulkd --no-default-features --features mod-portscan
```
On Windows, `scripts/demo.ps1` runs an end-to-end tour (starts skulkd, drives it via `skulk`).

## Locked-in decisions (do not re-litigate)

- **The protocol (`contract`) is the boundary.** Same types on the wire and the
  internal bus (`Envelope`). The socket adapter is a dumb serde translator.
- **Modules:** static registration (no dynamic `.so`). The trait is object-safe
  via `RawParams` in/out + `async_trait`. Params are opaque to the core; the
  module parses them with `ParseParams::parse` into its own typed struct.
- **Capability-gating:** modules declare `requires: Vec<Capability>`; the core
  refuses invocation with a structured error if the device lacks them.
- **Lifecycle commands** (Ping/Describe/Loot/Shutdown) are "instant tasks" whose
  payload rides in the `Result.output` (Describe's `Manifest` is serialized there).
- **CLI syntax (`skulk`):** module-first `<module> <action> key=value`. A token
  with a `.` is a module; bare words are reserved verbs (describe/loot/watch/
  ping/shutdown). Params **infer type** (JSON-parse, fallback to string);
  `--params-json '{...}'` overrides. No `:=` (rejected: the shell eats quotes, so
  type must come from inference or a module's `params_schema`, not quoting).
- **Naming:** brand is **Skulk**; binaries `skulkd` (daemon) + `skulk` (CLI).
  Internal crate names (`contract`/`engine`/…) and domain vocabulary
  (`ImplantModule`, `ImplantInfo`, "implant") stay generic — they describe what
  the thing *is*; the brand is Skulk. The crates.io name `skulk` is taken by an
  unrelated micro-crate, so the CLI package is `skulk-cli` producing a `skulk`
  binary via `[[bin]]`.
- **UI:** modules never ship UI code. The operator TUI / on-device LCD render from
  the `Manifest` (menus/forms, grouped by the dotted id namespace) and from
  `Event` / `ViewManifest` (live views). `ActionSpec.params_schema` (JSON Schema)
  drives rich forms when a module provides one; otherwise a generic key=value
  editor. Extensibility is data-driven, not code-driven.

## Module taxonomy (convention)

Two orthogonal axes (approved with the user), following industry practice:

- **ID = `<domain>.<subject>`, invoked with a `<verb>` action** — protocol/service
  first, like metasploit/nmap/bettercap. `domain` = `net`/`dns`/`http`/`tls`/`smb`/
  `ssh`/`snmp`/`ftp`/`arp`/`dhcp`/`wifi`/`ble`/`usb`/`sys`… The TUI groups by the
  dotted namespace. Standard verbs: `discover`/`scan`/`detect`/`enum`/`sniff`/
  `capture`/`spoof`/`poison`/`relay`/`brute`/`inject`. Examples: `net.hosts discover`,
  `net.ports scan`, `net.services detect`, `dns.records enum`, `arp.cache spoof`.
- **`ModuleDescriptor.tactic`** = the MITRE ATT&CK tactic (`Discovery`,
  `CredentialAccess`, `LateralMovement`, `Collection`, `Exfiltration`, …) — the
  kill-chain "phase", orthogonal to the id, for grouping/filtering by phase.

Each action declares its params via `ActionSpec.params: Vec<ParamSpec>`.
Network modules take a `PortSpec` param (from `module-sdk`) for `ports`
(`"1-1024"` / `"22,80,443"` / `[22,80]` / `80`).

## Adding a module

1. New crate `crates/modules/<name>`, deps `module-sdk` + `contract` (never `engine`).
2. Define your own typed `Params`/`Output` (serde). Impl `ImplantModule`:
   `descriptor()` (id `family.name`, `actions`, `requires`) + async
   `invoke(ctx, action, params)`. Talk to the world only via `ctx`
   (`progress` / `log` / `alert` / `store_loot` / `cancelled`).
3. Register in `skulkd`: optional dep + a `mod-<name>` feature +
   `#[cfg(feature = "mod-<name>")] engine.register(Arc::new(YourModule))`.
4. Add a test (loopback pattern — see `crates/modules/portscan/tests/scan.rs`).
   `crates/modules/sysinfo` is the smallest complete example.

## Gotchas (learned the hard way)

- **`cargo test` does NOT refresh `target/debug/skulkd.exe`.** Run
  `cargo build -p skulkd` before live-testing the binary, or you run a stale build.
- **`Cargo.lock` is committed and `tokio` is pinned to 1.44.2.** This repo was
  built against an offline/stale crates.io index where tokio 1.53's `mio` dep
  didn't resolve once the `net` feature was on. Keep the lockfile; on a fresh
  machine with a current index you may `cargo update` if you want newer versions.
- **TOML:** top-level scalar keys (`log`, `heartbeat_secs`) must appear BEFORE any
  `[table]` in `skulk.toml`, else they parse as fields of the last table. Guarded
  by `config::tests::shipped_config_parses`.
- **CLI output is ASCII on purpose** (Windows PowerShell mangles unicode piped
  from a child process's stdout).
- **redb** loot ops run via `tokio::task::spawn_blocking` (redb is blocking).

## Where to look

- `ROADMAP.md` — the tracked backlog (by phase and by module domain) and status.
- `README.md` — human-facing overview and quickstart.

## Status & next

Foundation complete and tested: engine, transport, persistent loot, config,
tracing, capability detection, heartbeat, shutdown/wipe, per-module feature flags,
client + CLI. Next per `ROADMAP.md`: the operator **TUI** (ratatui, manifest-driven,
built on the `client` lib) or more attack modules (e.g. `dns_recon`).

Every nontrivial change gets a test; run `cargo test` and keep it green.
