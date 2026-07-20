# Skulk

> *to skulk* — to move about stealthily, lying in wait. *a skulk* — the collective
> noun for a group of foxes.

A modular, transport-agnostic **pentest implant engine** in async Rust, built for
low-power single-board computers (Raspberry Pi Zero 2 W and up). Skulk is a *dumb
daemon with an external brain*: the device exposes a clean, language-agnostic
socket protocol and executes structured instructions; the intelligence
(operator, automation, or an AI orchestrator) lives entirely outside and speaks
that protocol.

## Design in one breath

- **Engine (`engine`)** — an async core: an event bus, a module registry, and a
  dispatcher that routes `Invoke{module, action, params}` to modules, enforcing
  **capability-gating** (a module declares the hardware it needs; the core refuses
  it cleanly if absent).
- **Contract (`contract`)** — the wire + bus protocol. Every message is an
  `Envelope`; the socket message and the internal bus event are the same object.
- **Modules (`ImplantModule`)** — self-contained capabilities that depend only on
  the `module-sdk` + `contract`, never on the engine. Each is opt-in at build time
  via a Cargo feature, so a firmware ships only what a given drop needs.
- **Transport (`transport`)** — a thin JSON-lines translator over TCP. Production
  is a reverse tunnel: the device dials out (it's behind NAT) and phones home.
- **Loot (`RedbLoot`)** — atomic, power-loss-resilient storage (redb). Survives a
  hard kill.
- **Operator (`client` + `skulk` CLI)** — a client library that speaks the
  protocol (depends only on `contract`) and a module-first CLI on top of it.

## Crates

```
contract      protocol types (Envelope / Command / Event / Manifest)
module-sdk    ImplantModule trait + ModuleCtx (what a module author builds against)
engine        event bus, registry, dispatcher, capability-gating, loot store
transport     socket adapter (TCP + JSON-lines, listen / reverse-dial)
client        async client library for the protocol
skulk-cli     the `skulk` command-line controller
skulkd        the daemon: engine + modules + transport in one process
modules/*     attack/recon modules (sys.info, net.ports, net.services,
              sys.temp, sys.battery, ...)
```

## Quickstart

```sh
cargo build
```

Run the daemon (reads `./skulk.toml`, defaults if absent; listens on
`127.0.0.1:9000`):

```sh
cargo run -p skulkd          # or: target/debug/skulkd
```

Drive it from another terminal with the `skulk` CLI — **module-first** syntax:

```sh
skulk describe                                        # what can this implant do?
skulk sys.info get
skulk net.ports scan target=10.0.0.1 ports=1-1024
skulk net.services detect target=10.0.0.1 ports=22,80,443
skulk sys.temp watch                                  # ambient HUD feed until cancelled
skulk loot
skulk watch                                           # stream events live
```

Module ids follow `<domain>.<subject> <verb>` (protocol-based, like nmap /
metasploit / bettercap); each module also declares a MITRE ATT&CK `tactic`.
Params infer their type (`timeout_ms=200` a number, `target=10.0.0.1` a string);
`--params-json '{...}'` is the escape hatch. Bare words without a dot
(`describe`, `loot`, `watch`, `ping`, `shutdown`) are device-level operations;
anything with a dot is a module invocation.

On Windows, `scripts/demo.ps1` starts the daemon and runs a scripted tour.

## Deploy to a Raspberry Pi

Releases are built by the manual `release.yml` workflow, which cross-compiles
`skulkd` for the Pi and publishes a rolling `latest-<target>` release (a tarball
with the binary, `skulk.toml`, the deploy script, and themes).

`scripts/skulk-deploy.sh` installs and updates it **without ever clobbering your
config**: the live config lives at `/etc/skulk/skulk.toml`, created once and
preserved across every upgrade, while updates only replace the binary.

```sh
# First install (on the Pi):
curl -fsSL https://raw.githubusercontent.com/tcpassos/skulk/main/scripts/skulk-deploy.sh -o skulk-deploy.sh
sudo bash skulk-deploy.sh          # a fork? prefix with SKULK_REPO=youruser/skulk

# Tailor the config once (id, listen addr, [display]/[[peripherals]]/[nav]):
sudo nano /etc/skulk/skulk.toml
sudo systemctl restart skulkd

# Update anytime after that — one command, config untouched:
sudo skulk-update
```

It runs `skulkd` as a **systemd service** (auto-start on boot, restart on crash;
root for GPIO/SPI and raw sockets). Logs: `journalctl -u skulkd -f`. A private
repo needs `GITHUB_TOKEN=…` in the environment (or `gh auth login`).

## Status

Foundation complete and tested (engine, transport, persistent loot, config,
tracing, capability detection, heartbeat, shutdown/wipe, per-module feature
flags, client + CLI). See [ROADMAP.md](ROADMAP.md) for the module backlog and the
hardware phase (LCD, operator TUI, tamper reflexes, cross-compile).

> For authorized security testing, research, and education only.
