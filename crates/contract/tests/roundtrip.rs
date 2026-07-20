use contract::*;
use serde_json::json;

fn roundtrip(env: &Envelope) -> Envelope {
    let s = serde_json::to_string(env).expect("serialize");
    serde_json::from_str(&s).expect("deserialize")
}

#[test]
fn invoke_command_roundtrips() {
    let cmd = Command::Invoke(Invoke {
        module: ModuleId::from("wifi.deauth"),
        action: "start".into(),
        params: RawParams(json!({ "bssid": "aa:bb:cc:dd:ee:ff", "count": 10 })),
        timeout_ms: Some(30_000),
    });
    let env = Envelope::new(Body::Command(cmd), 1_700_000_000_000);
    assert_eq!(roundtrip(&env), env);
}

#[test]
fn event_and_result_roundtrip() {
    let event = Envelope::new(
        Body::Event(Event::LootStored {
            key: "hash/dc01$".into(),
            kind: LootKind::Hash,
            size: 512,
        }),
        1_700_000_000_001,
    );
    assert_eq!(roundtrip(&event), event);

    let cause = MessageId::new();
    let result = Envelope::new(
        Body::Result(TaskResult {
            task: TaskId::new(),
            status: TaskStatus::Ok,
            output: RawParams(json!({ "captured": 1 })),
        }),
        1_700_000_000_002,
    )
    .in_reply_to(cause);
    let back = roundtrip(&result);
    assert_eq!(back, result);
    assert_eq!(back.correlate, Some(cause));
}

#[test]
fn view_manifest_drives_dual_ui() {
    let env = Envelope::new(
        Body::Event(Event::ViewManifest(ViewManifest {
            screen: "status".into(),
            lines: vec![
                ViewLine { label: "LINK".into(), value: "UP".into(), severity: Some(Severity::Info) },
                ViewLine { label: "HASHES".into(), value: "3".into(), severity: Some(Severity::High) },
            ],
        })),
        1_700_000_000_003,
    );
    assert_eq!(roundtrip(&env), env);
}

#[test]
fn param_spec_bounds_and_allowed_roundtrip() {
    let numeric = ParamSpec::required("port", "port", "target port").with_range(1, 65535);
    let s = serde_json::to_string(&numeric).expect("serialize");
    let back: ParamSpec = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back, numeric);
    assert_eq!(back.min, Some(1));
    assert_eq!(back.max, Some(65535));

    let choice = ParamSpec::optional("mode", "enum", "scan mode").with_allowed(["connect", "syn"]);
    let s = serde_json::to_string(&choice).expect("serialize");
    let back: ParamSpec = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back.allowed, vec!["connect", "syn"]);

    // A plain spec omits all three new fields on the wire (backward compatible).
    let plain = ParamSpec::required("target", "host", "IP or hostname");
    let s = serde_json::to_string(&plain).expect("serialize");
    assert!(!s.contains("min"), "unset numeric bounds must be skipped");
    assert!(!s.contains("allowed"), "empty allowed must be skipped");
    // And an old-protocol spec with none of the fields still deserializes.
    let old = json!({ "name": "target", "required": true, "type_hint": "host" });
    let back: ParamSpec = serde_json::from_value(old).expect("deserialize legacy spec");
    assert!(back.min.is_none() && back.max.is_none() && back.allowed.is_empty());
}

#[test]
fn widget_update_roundtrips() {
    let env = Envelope::new(
        Body::Event(Event::Widget(WidgetUpdate {
            slot: "battery".into(),
            value: "42%".into(),
            severity: Some(Severity::Low),
        })),
        1_700_000_000_005,
    );
    assert_eq!(roundtrip(&env), env);

    // An update with no severity must omit the field and still round-trip.
    let plain = Envelope::new(
        Body::Event(Event::Widget(WidgetUpdate {
            slot: "temp".into(),
            value: "51C".into(),
            severity: None,
        })),
        1_700_000_000_006,
    );
    let s = serde_json::to_string(&plain).expect("serialize");
    assert!(!s.contains("severity"), "None severity should be skipped on the wire");
    assert_eq!(roundtrip(&plain), plain);
}

#[test]
fn manifest_carries_capability_gating() {
    let manifest = Manifest {
        protocol: PROTOCOL_VERSION,
        implant: ImplantInfo {
            id: "implant-01".into(),
            hardware: "Raspberry Pi Zero 2 W".into(),
            firmware: "0.1.0".into(),
        },
        modules: vec![ModuleDescriptor {
            id: ModuleId::from("wifi.deauth"),
            version: "0.1.0".into(),
            tactic: Some(Tactic::CredentialAccess),
            actions: vec![ActionSpec {
                name: "start".into(),
                description: Some("Inject deauth frames".into()),
                params: vec![ParamSpec::required("bssid", "mac", "target access point")],
                params_schema: None,
            }],
            requires: vec![Capability::MonitorMode, Capability::PacketInjection],
        }],
        capabilities: vec![Capability::MonitorMode, Capability::Other("gps".into())],
        peripherals: vec![
            Peripheral { name: "btn_a".into(), kind: PeripheralKind::Button, gpio: vec![17] },
            Peripheral {
                name: "encoder".into(),
                kind: PeripheralKind::RotaryEncoder,
                gpio: vec![5, 6],
            },
        ],
    };
    let s = serde_json::to_string(&manifest).expect("serialize");
    let back: Manifest = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back, manifest);
}

#[test]
fn manifest_without_peripherals_field_defaults_to_empty() {
    // Backward compatibility: an implant on an older protocol version that
    // never serialized `peripherals` must still deserialize cleanly.
    let json = json!({
        "protocol": PROTOCOL_VERSION,
        "implant": { "id": "x", "hardware": "x", "firmware": "0" },
        "modules": [],
        "capabilities": [],
    });
    let manifest: Manifest = serde_json::from_value(json).expect("deserialize");
    assert!(manifest.peripherals.is_empty());
}

#[test]
fn peripheral_kind_other_roundtrips() {
    let p = Peripheral { name: "gps_led".into(), kind: PeripheralKind::Other("gps".into()), gpio: vec![24] };
    let s = serde_json::to_string(&p).expect("serialize");
    let back: Peripheral = serde_json::from_str(&s).expect("deserialize");
    assert_eq!(back, p);
}

#[test]
fn unknown_module_error_roundtrips() {
    let env = Envelope::new(
        Body::Error(ProtocolError {
            code: ErrorCode::MissingCapability,
            message: "wifi.deauth requires monitor_mode".into(),
            module: Some(ModuleId::from("wifi.deauth")),
        }),
        1_700_000_000_004,
    );
    assert_eq!(roundtrip(&env), env);
}
