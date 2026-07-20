# Roadmap & Backlog — Skulk (motor de pentest embarcado)

Backlog rastreável do projeto. Fonte de verdade do "o que falta". Marque com `[x]` ao concluir.

- **Status:** `[x]` feito · `[~]` em progresso · `[ ]` pendente
- **Dificuldade:** `easy` · `medium` · `hard` · `frontier` (greenfield / ponto fino do ecossistema)
- **Alvo:** Pi Zero 2 W e variantes (aarch64/armv7)
- **Regra de ouro:** Rust-first · FFI-C só onde o ecossistema é fino · nunca embrulhar motor rival (bettercap etc.)
- **Referências:** dossiê de mercado completo em `scratchpad/dossier.html`. Âncoras técnicas: **AngryOxide** (wifi nativo), **hudsucker** (MITM HTTPS), **bettercap** (arquitetura), **P4wnP1** (USB gadget).

---

## Fases (ordem de prova de valor)

- **Fase 0 — Fundação** — ✅ feito
- **Fase 1 — Fechar o loop ao vivo** — socket adapter + loot real + detecção de capability + 1º módulo de recon real
- **Fase 2 — Identidade no device** — LCD + TUI + ViewManifest + reflexos de sobrevivência
- **Fase 3 — Amplitude de ataque** — suíte de recon, creds/MITM, wifi (stack AngryOxide)
- **Fase 4 — Rádio, USB, autonomia offline e módulos de fronteira**

---

## Fase 0 — Fundação (feito)

- [x] Crate `contract` — Envelope/Command/Event/Result/Manifest + capability-gating (5 testes de round-trip)
- [x] Crate `module-sdk` — trait `ImplantModule`, `ModuleCtx`, `Cancel`, `LootSink`, `ModuleError`, `ParseParams`
- [x] Crate `engine` — event bus (broadcast), registro de módulos, dispatcher, capability-gating
- [x] Módulo-exemplo `example-sysinfo` + teste end-to-end do loop (3 testes)
- [x] Decisões travadas: registro estático · trait object-safe (RawParams + async_trait) · ModuleCtx mínimo

---

## Infra do motor (gaps não-módulo)

### Transporte & controle agnóstico
- [x] `medium` **Socket adapter / túnel reverso** — crate `transport`: TCP + JSON-lines, modo disca-pra-fora (`run_dialer`, com reconexão) + modo escuta (`run_listener`), egresso filtra `ViewManifest` por padrão, trata `Lagged`. Testado end-to-end (Describe/Invoke sobre TCP real). *Falta: WebSocket/gRPC/QUIC.*
- [ ] `medium` **Cripto de canal + identidade do device** — mTLS ou Noise + identidade assinada → `rustls`, `snow`, `ed25519-dalek`, `chacha20poly1305`
- [x] `easy` **Reconexão/backoff do túnel** (`run_dialer`) + **heartbeat** (`Event::Heartbeat` periódico via `Engine::spawn_heartbeat`, intervalo em config)
- [x] `easy` **Binário `skulkd`** — junta engine + módulos + transporte; config `skulk.toml`. Testado ao vivo: cliente TCP → Describe/Invoke → respostas em streaming.

### Persistência
- [x] `medium` **Loot store real** (`RedbLoot` em `engine`) — redb single-file ACID via `spawn_blocking`; layout `[kind_byte]++payload`. Testado automatizado (reabertura) E ao vivo (loot sobreviveu a kill+restart do processo). `MemLoot` mantido pra testes. Config de path via `IMPLANT_LOOT`.
- [x] `easy` **Loot por referência** — `ctx.store_loot` grava bulk no store, wire carrega só `LootStored{key}` + `Loot` query retorna refs. Convenção habilitada.
- [x] `medium` **Fetch de conteúdo do loot** — `Command::LootFetch{key}` (separado de `Loot`, que continua só metadados) retorna `LootContent{key,kind,bytes}` via `LootSink::get`; `ErrorCode::NotFound` pra chave inexistente. `client`/CLI (`skulk loot <key>`)/TUI/LCD — todos conseguem ver o conteúdo agora, não só o índice. Testado ponta-a-ponta (engine, client, TUI, LCD). Design **unificado** entre TUI e LCD após relato ao vivo de confusão com dois paradigmas diferentes: loot vive dentro da árvore MODULES/`Menu` como um grupo "loot" no final (`Row::Loot`, `set_loot`/`note_loot`) nos dois, Up/Down/Enter navegam tudo igual, sem foco/tela separados — o TUI chegou a ter um painel `Focus::Loot` dedicado numa primeira passada, mas foi substituído por isso.
- [x] `easy` **Histórico de loot (versão simples)** — `sys.info`/`dns.records` trocaram a chave fixa (sobrescrevia a cada execução) por `module_sdk::timestamped_key(prefix)` (`<prefix>/<millis>`); `Command::Loot{prefix}`/`skulk loot --prefix` já alcançam todo o histórico sem nenhuma mudança de protocolo. `engine::loot` passou a ordenar decrescente (mais recente primeiro), então `limit` mantém os mais novos; TUI/LCD capam a lista ambiente em `RECENT_LOOT_LIMIT` (20) pra não crescer pra sempre, e itens novos (`Event::LootStored`) entram no topo da lista, não no fim. Versão com agrupamento em 2 níveis (família → snapshots) considerada e **adiada de propósito** — nenhum módulo hoje gera loot em volume que justifique.

### Build, config & bootstrap
- [x] `medium` **Feature-flags por módulo** — deps de módulo opcionais + `#[cfg(feature)]` no skulkd; provado ao vivo (build enxuto = só `net.port_scan`, default = ambos)
- [x] `medium` **Detecção de capabilities em runtime** — `caps::detect()` (Linux sonda `/sys/class/udc` + bluetooth; vazio em não-Linux) → `Manifest.capabilities`
- [x] `easy` **Config/bootstrap** — `implant.toml` (TOML, todos os campos com default) + override `IMPLANT_LOOT`; testes de parse do arquivo real
- [x] `easy` **Logging/tracing** — `tracing` estruturado (engine/transport/skulkd) + `EnvFilter` (`RUST_LOG`/config), saída em stderr
- [~] `medium` **Pipeline de cross-compile/deploy** para aarch64/armv7 (Pi Zero 2 W) — `cross` (Docker) buildando do Windows confirmado ao vivo: `cross build --target armv7-unknown-linux-gnueabihf -p skulkd [--features lcd]` produz um ELF ARM 32-bit real (o Pi Zero 2 W de teste roda Raspberry Pi OS 32-bit — `uname -m` = `armv7l`, não aarch64). Workflow `.github/workflows/release.yml` (manual, `workflow_dispatch`) builda em CI e publica um release "rolante" `latest-<target>` por arquitetura (tarball com o binário `--release` + `skulk.toml`) — extensível pra outros targets adicionando uma linha na matrix. *Falta: rodar o workflow pelo menos uma vez de verdade (nunca disparado ainda) e automatizar o `curl`/instalação no lado do Pi.*

### UI / operador
- [x] `medium` **Lib `client` + CLI `skulk`** — cliente do protocolo (depende só de `contract`): `connect/send/recv/run/watch/describe/loot`. Sintaxe module-first `<module> <action> key=value` com inferência de tipo (+ `--params-json`) e verbos `describe/loot/watch/ping/shutdown`. Teste automatizado + ao vivo.
- [~] `medium` **TUI de operador** (`skulk-tui`) — dashboard `ratatui`+`crossterm` sobre a lib `client`. FEITO: árvore de módulos do Manifest (agrupada por namespace, disponibilidade por capability), painéis Events/Tasks/Loot ao vivo, linha de comando module-first, painel **DETAIL** com o help dos params (do `ParamSpec`), Enter pré-preenche os params obrigatórios. Client ganhou `split()`. Falta: formulário editável campo-a-campo, config local do operador (favoritos/recipes/keybindings), painel de ViewManifest.
- [x] `easy` **Params auto-descritos** — `ActionSpec.params: Vec<ParamSpec>` no contrato (nome/tipo/obrigatório/default/exemplo); módulos declaram; `skulk describe` e a TUI mostram o help. `net.port_scan` aceita `ports` flexível e `timeout_ms`.
- [~] `medium` **Renderer LCD** (`lcd-render`) — consumidor in-process de `Engine::subscribe()` (nunca socket), desenha `Event::ViewManifest` via `embedded-graphics`. FEITO: `ctx.view()` em `module-sdk` (+ demo em `net.ports scan`), `Peripheral`/`PeripheralKind` no `Manifest` (fiação de GPIO auto-descrita), sistema de tema data-driven (`theme.toml` + assets `.bmp` via `tinybmp`, sem recompilar — pensado para temas da comunidade tipo Flipper/Pwnagotchi), backend `mipidsi`+`rppal` pro ST7789/ST7735S (feature `driver-mipidsi`, Linux-only), camada de input plugável (`InputSource`, `GpioButtons` via `input-gpio`) + `NavMap` (override do operador > tema > sem binding). **Menu on-device** (`menu::{Row, Menu, Screen, App}`): qualquer botão mapeado abre uma lista navegável e agrupada dos módulos/ações do Manifest; Select invoca a ação selecionada quando ela não tem params obrigatórios; sem peripherals configurados cai em `NoInput` e o comportamento fica idêntico ao modo só-visualização de antes. `Renderer::draw_menu`/`draw_app` + `run_app`/`spawn_app` unificam bus (ViewManifest) e input via `tokio::select!`; `skulkd::spawn_lcd` liga tudo (`engine.manifest()` agora público, `build_input` escolhe botões reais ou `NoInput`). **Barra de HUD/status** (`hud::Hud` + `Renderer::draw_hud`/`draw_frame`): uma faixa fina no topo que compõe indicadores pequenos e chaveados que qualquer módulo publica via `ctx.widget` (`Event::Widget`/`WidgetUpdate { slot, value, severity }`) por cima de qualquer tela; o operador lista os slots em `[hud].slots` e o **tema mapeia cada slot a um ícone `.bmp`** (entrada `[assets]` com a chave = nome do slot), caindo pra texto quando não há tema/asset. **Tema agora é carregado de disco** via `[display].theme` (`Theme::load`, fail-clean pro default) — dirige paleta, ícones do HUD e nav de fallback; exemplo em `themes/example/theme.toml`. Demo: `net.ports` publica um slot `ports` ao vivo. Tudo testado (simulador `embedded-graphics-simulator`, loopback ponta-a-ponta, + cross-check `--target armv7-unknown-linux-gnueabihf`). **Widgets de entrada tipada** (`form::{Widget, Form}` + `Screen::Form`): Select numa ação cujos params obrigatórios são todos "spinnáveis" abre um wizard de spinners dirigido por `ParamSpec` (`host`→4 octetos, `int`/`port`→stepper com min/max, `port-spec`→range início/fim, `bool`→toggle, `allowed`/`enum`→picker), tudo nos 4 nav-actions (Up/Down edita, Select avança/submete, Back volta/cancela); param obrigatório de texto livre deixa a ação browse-only. `ParamSpec` ganhou `min`/`max`/`allowed` (serde-default, retrocompatível). **Validado ao vivo** num Pi Zero 2 W + Waveshare joystick/3-botões LCD HAT, tanto o 1.14" (ST7789) quanto o 1.44" (ST7735S) — offset/rotação corrigidos (ver gotchas do `mipidsi` no CLAUDE.md) e pinagem cruzada com o fork do RaspyJack. `NavAction` ganhou `Left`/`Right` (move o cursor do `Form` entre campos sem o efeito de submeter/cancelar do Select/Back nas pontas; ignorado no `Menu`), todo `Widget` limitado (octetos/`Number`/`Range`) agora dá a volta em vez de travar no limite, e segurar Up/Down repete a ação num timer que acelera (`HeldButton`, ~450ms + 160ms→20ms) em vez de exigir um clique por unidade. Dois **módulos de sensor ambiente** alimentam o HUD: `sys.temp` (thermal sysfs do kernel, sem hardware extra, no `default`) e `sys.battery` (INA219 via I2C, exige a nova `Capability::I2c` + feature opt-in `mod-battery`, driver Linux-only cruzado com o fork do RaspyJack); cada um tem `get` (uma leitura) e `watch` (poll infinito até cancelar); `skulkd` auto-invoca `watch` no boot pra qualquer sensor cujo slot esteja listado em `[hud].slots` — esse é o único switch, sem flag de config separada. *Falta: decidir se a paleta do skulk-tui migra pro mesmo `Theme`.*

### Sobrevivência (reflexos locais, independentes do controlador)
- [ ] `medium` **Sensor de tamper** (acelerômetro/PIR) → `Alert` → gatilho de wipe → `rppal`/`linux-embedded-hal`, `evdev`, driver do sensor
- [ ] `easy` **Telemetria de energia** (undervoltage → flush)
- [x] `easy` **Self-wipe do loot** — `Command::Shutdown{Wipe}` limpa o redb (`LootSink::clear`) e para o daemon via `Notify`; testado. *Falta: `zeroize` de segredos em RAM (na fase de cripto).*

### Autonomia offline (adiado, mas o contrato já reserva)
- [ ] `hard` **`Mission`** — variante aditiva de `Command`: fila de comandos executada sem o controlador + store-and-forward do loot

---

## Backlog de módulos (do levantamento de mercado)

### Recon (15)
- [ ] `easy` **`net.hosts` discover** (host_discovery) — `pnet_datalink` (ARP) + `surge-ping` (ICMP) + `rtnetlink`. Requer `Capability::RawSocket` (a criar) — roda no Pi.
- [x] `medium` **`net.ports` scan** (port_scan) — connect-scan async `tokio`, `PortSpec` flexível, progresso, cancelamento. Testado ao vivo. *Falta variante SYN half-open (`pnet`).*
- [x] `medium` **`net.services` detect** (service_fp) — detecção de serviço via banner + probe HTTP, nativo `tokio`, tática Discovery. Testado (fake SSH/HTTP no loopback). *Falta: TLS/cert fingerprint (`rustls`+`x509-parser`) e porte de assinaturas nmap-service-probes.*
- [ ] `medium` **passive_sniff** — `pnet_datalink` (ou `pcap` FFI) + `etherparse` + `pcap-file`
- [ ] `medium` **mdns_ssdp_llmnr_harvest** — `mdns-sd` + `ssdp-client` + listener `hickory-proto`
- [ ] `easy` **passive_os_fingerprint** — `huginn-net` (p0f + JA4, reusa a captura)
- [x] `easy` **`dns.records` enum** (dns_recon) — `hickory-resolver` (A/AAAA/NS/MX/TXT/SOA/CNAME) + `hickory-client` AXFR probe against each discovered nameserver, successful transfer alerts + zones stored as loot. Tested against a loopback fake nameserver (UDP + TCP AXFR).
- [ ] `medium` **dhcp_recon** — `dhcproto` + `pnet` + tabela Fingerbank option-55
- [ ] `hard` **wifi_recon** — stack AngryOxide (ver domínio WiFi)
- [ ] `medium` **ble_recon** — `bluer`/`btleplug`
- [ ] `medium` **snmp_enum** — `snmp2` (+ `rasn-snmp`)
- [ ] `medium` **netbios_smb_enum** — `netbios-parser` + `smb`
- [ ] `easy` **traceroute_topology** — `tracert` / `pnet`+`socket2`
- [ ] `easy` **upnp_device_enum** — `ssdp-client` + `quick-xml`
- [ ] `frontier` **os_fingerprint (ativo)** — `pnet`+`socket2` + porte nmap-os-db (sem crate)

### Credenciais & MITM (16)
- [ ] `hard` **responder_llmnr_nbtns_mdns** — `tokio`+`socket2` multicast + `hickory-proto` + `sspi` (NTLM)
- [ ] `easy` **arp_spoof** — `pnet_datalink` + `pnet_packet` + `rtnetlink`
- [ ] `medium` **dns_spoof** — `hickory-server` + `nfq` (NFQUEUE)
- [ ] `medium` **dhcp_spoof** — `dhcproto` + `socket2`
- [ ] `hard` **dhcp6_spoof_mitm6** — `dhcproto`(v6) + `socket2` + `hickory-server` → *ver oportunidades*
- [ ] `medium` **icmp_ra_redirect_spoof** — `pnet_packet` + `socket2`
- [ ] `frontier` **ntlm_relay** — `sspi` + `smb2`/`ldap3` → *ver oportunidades*
- [ ] `medium` **wpad_rogue_proxy** — `hyper`/`axum` + `hudsucker` + `sspi`
- [ ] `medium` **captive_portal / evil_portal** — `axum` + `hickory-server` + `dhcproto` + `rcgen`
- [ ] `hard` **sslstrip_http_downgrade** — plugin `hudsucker` + `lol_html` + `nfq`
- [ ] `medium` **https_proxy_intercept** — `hudsucker` (CA por-SNI, HTTP/2, hooks) — *o melhor caminho pronto*
- [ ] `medium` **http_content_injection** — `hudsucker` + `lol_html`
- [ ] `medium` **cleartext_credential_sniffer** — `afpacket`/`pcap` + `etherparse` + `regex` + `sspi`
- [ ] `easy` **tls_mitm_ca** (suporte) — `rcgen` + `rustls` (CA/cache único compartilhado)
- [ ] `medium` **mitm_substrate_forwarding** (suporte) — `rtnetlink` + `nfq` + `rustables`⚠
- [ ] `easy` **cam_table_flood** — `pnet_datalink` (baixo rendimento em switch moderno)

### WiFi / 802.11 (6) — stack: `nl80211-ng`+`neli`+`libwifi`+`radiotap` + AF_PACKET (`nix`/`libc`)
- [ ] `medium` **wifi_recon** — monitor + channel-hop + parse de beacons
- [ ] `medium` **deauth** — `libwifi` monta frame + radiotap TX manual + AF_PACKET sendto
- [ ] `hard` **handshake_capture** — captura EAPOL + FSM M1–M4 + `pcap-file` (hc22000)
- [ ] `hard` **pmkid_capture** — FSM de associação ativa (auth→assoc→EAPOL M1→PMKID KDE)
- [ ] `frontier` **evil_twin_rogue_ap** — soft-AP KARMA em Rust + `dhcproto`/`hickory`/`axum` → *ver oportunidades*
- [ ] `frontier` **wpa_wps (pixie-dust)** — detecção `libwifi` + EAP-WSC; crack: FFI `pixiewps` → *ver oportunidades*

### Bluetooth & rádio (7)
- [ ] `easy` **ble_scan** — `btleplug` + `bluer`
- [ ] `easy` **ble_enum** (GATT) — `btleplug`/`bluer`
- [ ] `hard` **bt_classic_recon** — HCI cru (`socket2`+`nix`); FFI `libbluetooth` p/ SDP
- [ ] `hard` **hid_over_2.4ghz (MouseJack)** — `rusb`/`nusb` portando RFStorm do `bettercap/nrf24`
- [ ] `hard` **subghz_recon** — `soapysdr` (FFI) / `rtl-sdr-rs` / `cc1101` (spidev)
- [ ] `medium` **ble_spoof_hid_inject** — `bluer` (LEAdvertisement + GATT server) / `bluster`
- [ ] `frontier` **ble_sniff_follow** — `serialport` c/ firmware Sniffle (nRF52840)

### Exfil & transporte C2 (do conhecimento — confirmar crates)
- [ ] `medium` **reverse_tunnel** — (ver Infra › Transporte)
- [ ] `hard` **dns_tunnel** — `hickory-proto` + encoder de chunking próprio (sem lib Rust madura)
- [ ] `easy` **https_beacon** — `reqwest`+`rustls` (domain fronting)
- [ ] `easy` **icmp_tunnel** — `pnet`/`socket2`
- [ ] `easy` **webhook_exfil** (Discord/Telegram) — `reqwest`

### USB / acesso físico (do conhecimento — mais OS-facility que crate)
- [ ] `medium` **usb_ethernet_gadget** — configfs `libcomposite` ecm/rndis → roda recon/responder sobre usb0
- [ ] `medium` **usb_hid_inject (BadUSB)** — HID gadget `/dev/hidgN` + parser DuckyScript
- [ ] `easy` **usb_mass_storage** — `f_mass_storage` (exfil/autorun)
- [ ] `easy` **duckyscript_parser** (suporte, compartilhado com mousejack)

---

## Oportunidades de contribuição original (pontos finos do ecossistema)

Onde o Rust não tem equivalente pronto — valor alto, diferencial do projeto:

- [ ] **ntlm_relay** — não existe um `ntlmrelayx` em Rust
- [ ] **mitm6 em Rust** — todos os blocos existem (`dhcproto`+`socket2`+`hickory-server`+`sspi`), ninguém montou
- [ ] **evil_twin soft-AP** — beacon/probe-response KARMA em Rust puro (greenfield mas viável)
- [ ] **wpa_wps pixie-dust** — reimplementar Pixie-Dust em `RustCrypto` (hoje só C via `pixiewps`)

---

## Confirmar antes de depender (crates marcados "unverified" na pesquisa)

- [ ] `rustables` (programação nftables) — checar API/manutenção atual
- [ ] `rupnp` (SOAP UPnP) · `mac_oui`/`oui` · `gattrs` · `bluetooth-serial-port`
- [ ] `afpacket` TPACKET_V3 ring · matcher `nmap-os-db`/`nmap-service-probes` (portar você mesmo)
