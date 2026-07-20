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
contract      protocol types: Envelope / Command / Event / Manifest /
              LootEntry / LootContent. No internal deps.
module-sdk    ImplantModule trait, ModuleCtx, LootSink (put/query/get),
              ParseParams.                                              deps: contract
engine        event bus, module registry, dispatcher, capability-gating,
              loot stores (MemLoot / RedbLoot); Engine::manifest()/
              loot_query()/loot_get() are direct in-process reads for
              consumers that bypass the wire protocol (the on-device LCD). deps: contract, module-sdk
transport     socket adapter — TCP + JSON-lines
              (serve_connection / run_listener / run_dialer).           deps: contract, engine
client        async client library (Client: connect/send/recv/run/
              watch/describe/loot/loot_fetch). Proves the protocol is
              the boundary.                                             deps: contract ONLY
skulk-cli     the `skulk` CLI (binary named `skulk`).                    deps: client, contract
skulkd        the daemon binary: engine + modules + transport.          deps: engine, transport, modules
lcd-render    on-device LCD: in-process consumer of Engine::subscribe()
              (never a socket client), draws Event::ViewManifest via
              embedded-graphics; community themes (theme.toml + .bmp),
              physical drivers (mipidsi/rppal) behind Linux-only, opt-in
              feature flags so the Windows dev build never needs them.
              run_app()/spawn_app() also drive a browsable on-device menu
              (built from the Manifest) toggled by physical buttons
              (InputSource/NavMap), invoking param-less actions straight
              from the screen; NoInput is the zero-buttons fallback.
              A HUD band (Hud + draw_hud) composites small keyed indicators
              any module publishes via ctx.widget (Event::Widget) over
              whatever screen is active; the theme maps slot name -> icon.
              Params-taking actions open a button-driven spinner Form
              (form::{Widget,Form}) built from ParamSpec type_hints
              (host->octets, int/port->stepper, port-spec->range, bool,
              enum/allowed->picker); free-text params stay browse-only.    deps: engine, contract
crates/modules/*   attack/recon modules. e.g. example-sysinfo -> sys.info,
                   net-portscan -> net.ports, net-services -> net.services,
                   sys-temp -> sys.temp, sys-battery -> sys.battery.        deps: module-sdk, contract
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
# cross-compile for a Pi Zero 2 W from Windows (needs Docker + `cross`;
# confirm the real target with `uname -m` on the device -- armv7l here,
# not aarch64, since it runs the 32-bit OS):
cross build --target armv7-unknown-linux-gnueabihf -p skulkd --features lcd
```
On Windows, `scripts/demo.ps1` runs an end-to-end tour (starts skulkd, drives it via `skulk`).

`.github/workflows/release.yml` runs that same cross-build in CI, manually
(`workflow_dispatch`, so it doesn't fire on every push) — one matrix entry per
target architecture, each publishing/overwriting its own standing
`latest-<target>` GitHub release (a tarball with the release binary,
`skulk.toml`, `scripts/skulk-deploy.sh`, and `themes/`), so the Pi always has one
stable URL to pull the newest build from for its own arch. Add a target by adding
a matrix line.

**Pi deploy/update: `scripts/skulk-deploy.sh`** (idempotent; installs itself as
`skulk-update`). Separates binary from config: binary at `/opt/skulk/skulkd`,
live config at `/etc/skulk/skulk.toml` created ONCE from the shipped example and
**never overwritten by updates** (so operator edits — peripherals/nav/listen
addr — survive). Runs `skulkd` as a systemd service (root, for GPIO/SPI/raw
sockets). Repo defaults to `tcpassos/skulk`, overridable via `SKULK_REPO` (saved
to `/etc/skulk/deploy.env`); target auto-detected from `uname -m`.

## Locked-in decisions (do not re-litigate)

- **The protocol (`contract`) is the boundary.** Same types on the wire and the
  internal bus (`Envelope`). The socket adapter is a dumb serde translator.
- **Modules:** static registration (no dynamic `.so`). The trait is object-safe
  via `RawParams` in/out + `async_trait`. Params are opaque to the core; the
  module parses them with `ParseParams::parse` into its own typed struct.
- **Capability-gating:** modules declare `requires: Vec<Capability>`; the core
  refuses invocation with a structured error if the device lacks them.
- **Lifecycle commands** (Ping/Describe/Loot/LootFetch/Shutdown) are "instant
  tasks" whose payload rides in the `Result.output` (Describe's `Manifest` is
  serialized there). `Loot` only ever returns metadata (`LootEntry`: key/kind/
  size) — deliberately never bulk bytes, so listing loot can never itself leak
  it. `LootFetch{key}` is the separate, explicit command that returns one
  item's actual bytes (`LootContent`), so bulk content only ever leaves the
  device when asked for by key, never as a side effect of browsing.
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
- **Windows builds need the GNU ABI, not MSVC** (Visual Studio's `link.exe`
  isn't available in every environment this project is built in) — but
  `rust-toolchain.toml` deliberately pins a **bare** `channel = "1.88.0"`,
  no target-triple suffix. A suffixed channel (e.g.
  `"1.88.0-x86_64-pc-windows-gnu"`) is read on *every* platform this project
  builds on, including the Pi itself, and fails there with "target tuple in
  channel name" — this shipped once and broke the Pi build. The GNU-ABI
  requirement is a per-machine concern instead: `rustup override set
  1.88.0-x86_64-pc-windows-gnu` in the repo directory on a Windows box that
  needs it, plus a mingw-w64 `gcc`/linker (e.g. WinLibs) on `PATH`. If VS
  Code/rust-analyzer reports proc-macro errors like `unsupported metadata
  version`, it's loading a `.dll` in `target/debug/deps` built by a
  *different* toolchain than the one its proc-macro server was resolved
  against — reload the rust-analyzer workspace after any toolchain change.
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
- **`tracing`'s field macros special-case a bare `display` identifier**
  (it's also their `%`-sigil helper fn) — a local variable/param named
  `display` used inside `tracing::warn!(... = %display.field, ...)` fails to
  compile with a confusing "no field on type `fn(_) -> DisplayValue<_>`"
  error. Name it something else (`disp`, `cfg`, ...).
- **Cross-check Linux-only code from Windows without real hardware**: install
  a Linux target (`rustup target add aarch64-unknown-linux-gnu --toolchain
  1.88.0-x86_64-pc-windows-gnu`) and run `cargo check --target
  aarch64-unknown-linux-gnu` (add `--features ...` for whatever's behind a
  `target_os = "linux"` gate, e.g. `lcd-render`'s `driver-mipidsi`/
  `input-gpio`). This won't link (no cross-linker here), but `check` still
  type-checks real usage of Linux-only crates (`rppal`, `mipidsi`, ...)
  against their actual APIs — caught the `display`-vs-`tracing` gotcha above
  before it ever reached real hardware.
- **Rapid alternating `cargo check`/`--target ...` invocations can corrupt the
  incremental build cache** — symptoms look like a real regression (a type
  that's definitely in the source reports as "not found"), but a full crate
  actually recompiles from scratch when checked alone. Fix: `cargo clean -p
  <affected crates>` (not a full clean) and rebuild.
- **`mipidsi`'s `display_size()`/`display_offset()` want the panel's NATIVE
  (as-if-rotation-were-0) dimensions/offset, not the final on-screen shape**
  — confirmed by reading `mipidsi` 0.10's actual source
  (`set_address_window`, `MemoryMapping::from_orientation`): it internally
  re-derives the per-rotation offset from `FRAMEBUFFER_SIZE - (display_size +
  offset)`, using whatever raw width/height/offset you passed in. Feed it the
  FINAL (already-rotated) shape instead and this math silently underflows/
  goes wrong, showing up as a garbage line on one screen edge and a clipped
  line on the opposite edge, and a large stray offset on rotations that
  reverse an axis (e.g. 180). This only bites panels that are natively
  non-square: the Waveshare 1.14" LCD Module's glass is natively PORTRAIT
  (135x240) even though `skulk.toml` can drive it in landscape — so
  `[display]` there needs `width=135 height=240 offset_x=52 offset_y=40`,
  NOT the on-screen 240x135, with `rotation=90` (or `270`) to actually swap
  the axes into landscape (`0`/`180` stay portrait — they can't produce this
  panel's landscape image no matter the offset). The Waveshare 1.44" LCD
  HAT (ST7735S, this project's actual current hardware) is natively square
  (128x128) so this width/height trap doesn't apply to it, but it still
  needs a nonzero offset: its GRAM is 132x162 (`ST7735s::FRAMEBUFFER_SIZE`),
  bigger than the 128x128 visible glass. `offset_x=2 offset_y=1` at
  `rotation=90` is hardware-confirmed: decoding the actual MADCTL byte this
  repo's RaspyJack fork's ST7735S driver (`LCD_1in44.py`) writes for its
  default orientation shows it's `mipidsi`'s `Rotation::Deg90` (NOT `Deg0` —
  an easy wrong guess, made once already, since `LCD_1in44.py`'s `LCD_X`/
  `LCD_Y` constants look like they're "the native offset" but are actually
  already Deg90-adjusted), and at that rotation `(2, 1)` reproduces
  `LCD_1in44.py`'s applied values exactly. Being square, `display_size` is
  width/height-trap-free at every rotation, but the OFFSET is not
  interchangeable across rotations the way the "native offset, re-derived
  automatically" framing above implies in the abstract: `0`/`180` swap in a
  *different* `MemoryMapping` (reverse-without-swap vs swap-without-full-
  reverse), so they need their own on-hardware check — don't assume they're
  fine just because `90` is.

## Where to look

- `ROADMAP.md` — the tracked backlog (by phase and by module domain) and status.
- `README.md` — human-facing overview and quickstart.

## Status & next

Foundation complete and tested: engine, transport, persistent loot, config,
tracing, capability detection, heartbeat, shutdown/wipe, per-module feature flags,
client + CLI, operator TUI (field-editable forms), recon modules (net.ports,
net.services, dns.records). The **LCD renderer** (`lcd-render`) is code-complete
and cross-target-verified (contract/engine plumbing, `ctx.view()`, theme system,
`mipidsi`/`rppal` ST7789/ST7735S backend, GPIO input + nav-mapping) — including
the on-device **menu** (`menu::{Row, Menu, Screen, App}` + `Renderer::draw_menu`/
`draw_app` + `run_app`/`spawn_app`, wired into `skulkd::spawn_lcd` via
`build_input`): any bound button opens a browsable, grouped list of the
Manifest's modules/actions, Select invokes whichever row has no required
params, and the screen falls back to the tactical `ViewManifest` view when
there's nothing to browse (no peripherals configured -> `NoInput`, the loop
behaves exactly like the view-only path). This resolves the "should the LCD
navigate the Manifest or stay view-only" open question from Phase 5's design
in favor of "navigate" — lives in `skulkd`, not a separate binary, since it
owns the same SPI/GPIO bus exclusively anyway. A cross-screen **HUD band**
(`hud::Hud` + `Renderer::draw_hud`/`draw_frame`) composites small keyed
indicators any module publishes via `ctx.widget` (`Event::Widget` /
`WidgetUpdate { slot, value, severity }`) over whatever screen is active; the
operator lists which slots show in `[hud].slots`, and the **theme maps each
slot name to a `.bmp` icon** (an `[assets]` entry keyed by the slot name),
falling back to text when no theme/asset is present. A theme is now actually
**loaded from disk** when `[display].theme` names a directory (`Theme::load`,
fail-clean to `Theme::default()`), so themes drive the palette, HUD icons, and
a fallback nav map — a runnable reference lives in `themes/example/theme.toml`.
Params-taking actions are now runnable from the device too: Select on a
menu row whose required params are all spinner-fillable opens a **typed input
form** (`form::{Widget, Field, Form}`, `Screen::Form`) — a wizard of
button-driven spinners built from each `ParamSpec` (`host` → four octet
spinners, `int`/`port` → bounded stepper, `port-spec` → start/end range,
`bool` → toggle, `allowed`/`enum` → scrollable picker), folded onto the same
four nav actions (Up/Down edit, Select advances/submits, Back retreats/
cancels). An action with a *required free-text* param (`string`/`mac`) stays
browse-only (menu marker `-`; `+` marks form-able, none marks run-now).
`ParamSpec` gained optional `min`/`max`/`allowed` (serde-default,
back-compatible) to drive the spinners. **Now live-tested on real hardware**
— a Pi Zero 2 W + the Waveshare joystick/3-button LCD HAT, confirmed working
on both the 1.14" (ST7789) and 1.44" (ST7735S) variants (`--features lcd`;
see `[display]`/`[hud]`/`[[peripherals]]`/`[nav]` in `skulk.toml`, and the
`mipidsi`-offset/rotation and RaspyJack-cross-checked-offset gotchas above).
`NavAction` grew `Left`/`Right` (bound to the joystick's left/right presses),
so a `Form`'s cursor moves between fields/edit points independently of
`Select`/`Back`'s submit/cancel at the ends — `menu::App::apply_nav` ignores
them on the plain vertical `Menu` screen (no horizontal axis there), though
`Screen::TextView` (below) gives them a second meaning: page up/down. Every
bounded `Widget` (octets, `Number`, `Range`) now wraps past its min/max
instead of clamping, and holding `Up`/`Down` auto-repeats the step on an
accelerating timer (`lib.rs`'s `HeldButton`, ~450ms initial delay then
160ms→20ms) instead of needing one press per unit — dialling in a value like
a port past 6000 no longer means hundreds of individual clicks. Two new
**ambient sensor modules** feed the HUD: `sys.temp` (kernel thermal sysfs,
no extra hardware, in `default`) and `sys.battery` (INA219 over I2C, needs
the new `Capability::I2c` and the opt-in `mod-battery` feature, Linux-only
real I2C access behind `crates/modules/battery/src/ina219.rs`'s
`#[cfg(target_os = "linux")]` — register layout/calibration cross-checked
against this repo's RaspyJack fork's own INA219 driver). Each has a one-shot
`get` action and a `watch` action that polls forever, publishing to the HUD
until cancelled; `skulkd::main` auto-invokes `watch` at boot for any
compiled-in sensor whose slot name is listed in `[hud].slots` — that's the
only on/off switch, no separate config flag (capability-gating still
refuses `sys.battery` cleanly with no I2C bus present).

Loot content (not just its `key`/`kind`/`size` index) is now fetchable and
viewable end to end, closing a real protocol gap: `Command::Loot` only ever
returned metadata, and there was no way to see actual bytes remotely at all
before this. The new `Command::LootFetch{key}` (→ `LootContent{key, kind,
bytes}`, `ErrorCode::NotFound` if the key's gone) is deliberately separate
from `Loot` — bulk content only ever leaves the device when asked for by a
specific key, never as a side effect of listing. `LootSink` grew a matching
`get(key)`; `Engine::loot_query()`/`loot_get()` are direct in-process
pass-throughs for consumers that bypass the wire (same exemption as
`manifest()`) — the on-device LCD uses these directly, the wire commands'
dispatch handlers use them too, so the two paths can't disagree. `client`
gained `loot_fetch()`; the CLI's `loot` verb now doubles as `skulk loot
<key>` (prints the content, or a `(binary, N B, ...)` note if it isn't
UTF-8) alongside the existing listing form. `skulk-tui`'s loot browsing
first shipped as a separate `Focus::Loot` panel, but that gave the TUI and
the LCD two different navigation metaphors for the same feature and that
was confusing in practice (a live user report) — it's now unified the same
way the LCD works: loot lives in the MODULES tree as a trailing "loot"
group (`Row::Loot`, `App::set_loot`/`note_loot`, same rebuild-on-change
design as the LCD's `Menu`), Up/Down/Enter navigate everything (module
actions and loot alike) with no separate mode, and `l` is back to being a
plain manual refresh (no focus switch). The small LOOT panel is now a
passive, always-visible mirror of the same list — no selection of its own.
Fetched content still shows in the larger middle panel shared with
DETAIL/FORM, opened whenever `App.loot_content` is `Some` regardless of
`Focus`. On the LCD specifically — the part that most needed this, to not
fall behind RaspyJack's own loot/log browsing — loot entries are a
live-updating trailing "loot" group *inside* the existing `Menu` (no new
button/screen-entry needed; `Menu::set_loot`/`note_loot` rebuild it, backed
by an `Engine::loot_query()` backfill at `run_app` startup plus live
`Event::LootStored`), and Select on one opens a new, deliberately generic
`Screen::TextView` (`textview::TextView`: title + lines + scroll, pure
state, no `embedded-graphics` — same split as `menu`/`form`) rather than a
loot-specific screen, since a scrollable text viewer is obviously reusable
later (full task output, a log tail, help text) and genericizing it cost
nothing extra. Best-effort UTF-8 decode; anything else (or a key that
vanished between listing and fetching, e.g. a wipe) shows an explanatory
placeholder line instead of failing silently. This did require widening
`App::apply_nav`'s return type from a plain `Option<Command>` to a new
`NavOutcome` (`None`/`Invoke`/`FetchLoot`) — `apply_nav` stays synchronous
and I/O-free (the actual `Engine::loot_get().await` happens in `run_app`,
via the new `apply_nav_outcome` helper it shares with the existing
Invoke-dispatch path), preserving the "pure state machine" split the
menu/form code already had.

Loot history: `sys.info get` and `dns.records enum` used to store under a
fixed key (`"sysinfo/last"`, `"dns/axfr/<zone>/<label>"`), so a second run
silently overwrote the first — no history, ever. They now build their key
with the new `module_sdk::timestamped_key(prefix)` (`<prefix>/<millis-since-
epoch>`), so every run keeps its own loot entry; `Command::Loot{prefix:
...}` (or the CLI's `skulk loot --prefix ...`) already reaches the full
history with zero engine/protocol changes, since millisecond-epoch suffixes
stay the same digit width for centuries and so sort chronologically as
plain strings with no padding needed. `engine::loot`'s shared `filter()` now
sorts **descending** (newest-first) instead of ascending, so `LootQuery.limit`
naturally keeps the most recent items instead of the oldest. Both ambient
UIs (the TUI's MODULES tree, the LCD's `Menu`) cap their auto-refreshed loot
list at a `RECENT_LOOT_LIMIT` (20) for exactly this reason — a long-running
daemon's loot would otherwise grow the on-screen list forever — and a
brand-new `Event::LootStored` item is now inserted at the *front* of the
list (not pushed to the back), matching that newest-first order; deeper
history past the cap is still fully reachable, just via `skulk loot --prefix
...` on a real terminal rather than the constrained on-device views. A
grouped/drill-down browse (family → individual snapshots, one more
navigation level) was considered and deliberately deferred — no module
today produces loot at a volume where the flat capped list actually falls
short, so building that now would be solving a hypothetical problem.

Also still open: whether the TUI's own colors should move onto the same
`lcd_render::Theme` system instead of `skulk-tui/src/ui.rs`'s hardcoded
consts. Cross-compiling
from the Windows dev machine via `cross` (Docker) is confirmed working
end-to-end: the test Pi Zero 2 W runs Raspberry Pi OS 32-bit (`uname -m` ->
`armv7l`), so the target is `armv7-unknown-linux-gnueabihf`, not aarch64 —
`cross build --target armv7-unknown-linux-gnueabihf -p skulkd --features
lcd` produces a real ARM ELF binary. On-device install/update is automated
by `scripts/skulk-deploy.sh` (config-preserving; see the deploy note above),
so it is no longer a manual `scp`.

Every nontrivial change gets a test; run `cargo test` and keep it green.
