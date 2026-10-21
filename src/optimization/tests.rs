use crate::{
    optimization::tunnel,
    optimization::{self, Hardware, Settings},
};
use std::{collections::BTreeMap, net::UdpSocket, time::Duration};
fn hw(gb: u64, threads: usize, mts: u32) -> Hardware {
    Hardware {
        total_mb: gb * 1024,
        free_mb: gb * 800,
        threads,
        memory_mts: mts,
        large_pages: false,
    }
}

#[test]
fn existing_launcher_config_keeps_optimization_disabled() {
    let config: crate::model::Config = serde_json::from_str(r#"{"settings":{"ram":0.5}}"#).unwrap();
    assert!(!config.settings.optimization.enabled);
    assert!(config.settings.optimization.tunnels.is_empty());
    assert!(config.settings.optimization.cpu_affinity.is_none());
}

#[test]
fn cpu_profiles_roundtrip_and_keep_legacy_jvm_json_compatible() {
    let hardware = hw(32, 16, 3600);
    let current = Settings {
        cpu_affinity: Some(vec![1, 3, 5]),
        ..Default::default()
    };
    let bytes = current.export(false, hardware).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        2
    );
    assert_eq!(
        Settings::import(&bytes, &Settings::default(), "full").unwrap(),
        current
    );
    let legacy = Settings::import(
        br#"{"format":"exbo-launcher-optimization","version":1,"settings":{}}"#,
        &current,
        "legacy",
    )
    .unwrap();
    assert!(legacy.cpu_affinity.is_none());
    let flat = current.export(true, hardware).unwrap();
    let flat_value: serde_json::Value = serde_json::from_slice(&flat).unwrap();
    assert!(flat_value.get("cpu_affinity").is_none());
    let imported = Settings::import(&flat, &current, "JVM").unwrap();
    assert_eq!(imported.cpu_affinity, current.cpu_affinity);
    assert!(!imported.enabled);
    for cpus in [vec![], vec![0, 0], vec![64]] {
        let mut invalid = current.clone();
        invalid.cpu_affinity = Some(cpus);
        assert!(invalid.export(false, hardware).is_err());
        let json = serde_json::json!({"format":"exbo-launcher-optimization","version":2,"settings":invalid});
        assert!(
            Settings::import(&serde_json::to_vec(&json).unwrap(), &current, "invalid").is_err()
        );
    }
    let mut one_cpu = current;
    one_cpu.cpu_affinity = Some(vec![5]);
    let auto = one_cpu.profile_for(hardware);
    assert_eq!(auto.parallel_gc_threads, 1);
    assert_eq!(auto.conc_gc_threads, 1);
}
#[test]
fn automatic_profile_matches_upstream_memory_tiers_and_logical_threads() {
    for (gb, heap, pre_touch) in [
        (6, 3, false),
        (8, 4, false),
        (12, 5, true),
        (16, 6, true),
        (32, 6, true),
    ] {
        let p = optimization::automatic(hw(gb, 6, 2666));
        assert_eq!(
            (
                p.heap_size_gb,
                p.pre_touch,
                p.parallel_gc_threads,
                p.conc_gc_threads
            ),
            (heap, pre_touch, 4, 2)
        );
        assert_eq!(
            (
                p.max_gc_pause_millis,
                p.g1_new_size_percent,
                p.g1_mixed_gc_count_target,
                p.g1_rset_updating_pause_time_percent
            ),
            (150, 30, 4, 5)
        );
        p.validate_hardware(hw(gb, 6, 2666)).unwrap();
    }
    let p = optimization::automatic(hw(32, 32, 6200));
    assert_eq!(
        (
            p.parallel_gc_threads,
            p.conc_gc_threads,
            p.max_gc_pause_millis,
            p.g1_new_size_percent
        ),
        (10, 5, 100, 33)
    );
    assert!(!p.use_large_pages);
}
#[test]
fn every_profile_parameter_has_bilingual_help_and_valid_defaults() {
    let profile = optimization::automatic(hw(32, 16, 3600));
    let value = serde_json::to_value(&profile).unwrap();
    assert_eq!(
        optimization::parameters().len(),
        value.as_object().unwrap().len()
    );
    for field in optimization::parameters() {
        assert!(value.get(&field.key).is_some());
        assert!(!field.label_ru.is_empty() && !field.label_en.is_empty());
        assert!(field.help_ru.len() > 40 && field.help_en.len() > 40);
    }
    optimization::aikar().validate().unwrap();
    profile.validate().unwrap();
}
#[test]
fn full_profile_roundtrip_includes_all_regions_and_no_account_data() {
    let mut settings = Settings {
        enabled: true,
        name: "Мой профиль".into(),
        profile: Some(optimization::automatic(hw(32, 16, 3600))),
        search_mode: 3,
        ..Default::default()
    };
    for region in optimization::REGIONS {
        settings
            .tunnels
            .insert(region.into(), "trusted.example.org:2000".into());
    }
    settings
        .excluded_pools
        .insert("ru".into(), vec!["Excluded pool".into()]);
    let bytes = settings.export(false, hw(32, 16, 3600)).unwrap();
    assert_eq!(
        Settings::import(&bytes, &Settings::default(), "ignored").unwrap(),
        settings
    );
    let document = String::from_utf8(bytes).unwrap();
    for word in [
        "session",
        "password",
        "protected_session",
        "user_id",
        "active_account",
    ] {
        assert!(!document.contains(word));
    }
}
#[test]
fn upstream_flat_json_import_export_is_lossless_and_does_not_enable_itself() {
    let bytes = include_bytes!("../../assets/optimization/aikar.json");
    let settings = Settings::import(bytes, &Settings::default(), "aikar").unwrap();
    assert!(!settings.enabled);
    assert_eq!(settings.profile.as_ref().unwrap(), &optimization::aikar());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(bytes).unwrap(),
        serde_json::from_slice::<serde_json::Value>(
            &settings.export(true, hw(32, 16, 3600)).unwrap()
        )
        .unwrap()
    );
}
#[test]
fn import_rejects_unknown_fields_types_versions_and_bad_combinations() {
    let base = serde_json::to_value(optimization::aikar()).unwrap();
    for (key, value) in [
        ("arbitrary_command", serde_json::json!("cmd.exe")),
        ("heap_size_gb", serde_json::json!(-1)),
        ("pre_touch", serde_json::json!("true")),
        ("g1_heap_region_size_mb", serde_json::json!(3)),
        ("g1_new_size_percent", serde_json::json!(99)),
        ("compile_threshold_scaling", serde_json::json!(99.5)),
        ("max_node_limit", serde_json::json!(1000)),
        ("conc_gc_threads", serde_json::json!(64)),
    ] {
        let mut v = base.clone();
        v[key] = value;
        assert!(
            Settings::import(
                &serde_json::to_vec(&v).unwrap(),
                &Settings::default(),
                "bad"
            )
            .is_err(),
            "{key}"
        );
    }
    assert!(Settings::import(&vec![b' '; 256 * 1024 + 1], &Settings::default(), "big").is_err());
    assert!(
        Settings::import(
            br#"{"format":"exbo-launcher-optimization","version":99,"settings":{}}"#,
            &Settings::default(),
            "new"
        )
        .is_err()
    );
}
#[test]
fn memory_and_large_page_checks_do_not_silently_change_imported_values() {
    let mut p = optimization::aikar();
    p.heap_size_gb = 32;
    assert!(p.validate_hardware(hw(16, 16, 3600)).is_err());
    assert_eq!(p.heap_size_gb, 32);
    p.heap_size_gb = 6;
    p.use_large_pages = true;
    assert!(p.validate_hardware(hw(32, 16, 3600)).is_err());
    let mut available = hw(32, 16, 3600);
    available.large_pages = true;
    p.validate_hardware(available).unwrap();
}
#[test]
fn tuning_replaces_conflicting_flags_without_changing_game_arguments() {
    let args: Vec<String> = [
        "java.exe",
        "-cp",
        "-Xmx-is-a-classpath.jar",
        "-XX:+UseSerialGC",
        "-Xmx1g",
        "-XX:+AlwaysPreTouch",
        "-XX:+UseStringDeduplication",
        "main.Game",
        "--session",
        "fixture-secret",
        "-Xmx-application-argument",
    ]
    .map(str::to_owned)
    .into();
    let mut p = optimization::automatic(hw(8, 6, 2666));
    p.pre_touch = false;
    p.use_string_deduplication = false;
    p.use_dynamic_number_of_gc_threads = false;
    let merged = optimization::inject(&args, &p.flags(), true).unwrap();
    assert_eq!(&merged[..3], &args[..3]);
    for flag in [
        "-XX:-AlwaysPreTouch",
        "-XX:-UseStringDeduplication",
        "-XX:-UseDynamicNumberOfGCThreads",
    ] {
        assert!(merged.iter().any(|value| value == flag));
    }
    let boundary = merged.iter().position(|v| v == "main.Game").unwrap();
    assert_eq!(&merged[boundary..], &args[7..]);
    for old in [
        "-Xmx1g",
        "-XX:+UseSerialGC",
        "-XX:+AlwaysPreTouch",
        "-XX:+UseStringDeduplication",
    ] {
        assert!(!merged.iter().any(|v| v == old));
    }
    assert_eq!(
        merged
            .iter()
            .filter(|v| v.starts_with("-Xmx")
                && **v != "-Xmx-is-a-classpath.jar"
                && **v != "-Xmx-application-argument")
            .count(),
        1
    );
}
#[test]
fn jar_and_module_entrypoints_receive_flags_before_the_entrypoint() {
    for tail in [
        vec!["-jar", "game.jar", "--token", "fixture"],
        vec!["--module", "game/main", "-Xmx-app"],
    ] {
        let args: Vec<_> = std::iter::once("java.exe")
            .chain(tail.iter().copied())
            .map(str::to_owned)
            .collect();
        let result = optimization::inject(&args, &["-Xmx4g".into()], true).unwrap();
        assert_eq!(result[1], "-Xmx4g");
        assert_eq!(&result[2..], &args[1..]);
    }
    assert!(
        optimization::inject(&["java.exe".into(), "-cp".into()], &["-Xmx4g".into()], true).is_err()
    );
}
#[test]
fn network_only_override_preserves_all_jvm_and_session_arguments() {
    let args: Vec<String> = [
        "java.exe",
        "-Xmx3g",
        "-Droxy_address_override.ru=old.example:2000",
        "Game",
        "--token",
        "fixture",
    ]
    .map(str::to_owned)
    .into();
    let settings = Settings {
        tunnels: BTreeMap::from([("ru".into(), "new.example:2000".into())]),
        ..Default::default()
    };
    let result = optimization::inject(&args, &settings.flags(hw(32, 16, 3600)), false).unwrap();
    assert_eq!(
        result,
        vec![
            "java.exe",
            "-Xmx3g",
            "-Droxy_address_override.ru=new.example:2000",
            "Game",
            "--token",
            "fixture"
        ]
    );
}
#[test]
fn invalid_tunnel_addresses_and_unknown_regions_are_rejected() {
    for value in [
        "host",
        "host:0",
        "host:65535",
        "host:2000 --agent",
        "https://host:2000",
        "host:2000/path",
        "-flag:2000",
        "host:2000\n",
    ] {
        assert!(tunnel::parse_address(value).is_err(), "{value}");
    }
    assert_eq!(
        tunnel::parse_address("[::1]:2000").unwrap(),
        ("::1".into(), 2000)
    );
    let settings = Settings {
        tunnels: BTreeMap::from([("unknown".into(), "node.example:2000".into())]),
        ..Default::default()
    };
    assert!(settings.validate().is_err());
}
#[test]
fn roxy_probe_checks_uuid_big_endian_latency_and_connection_limit() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let mut token = [0; 16];
        let (n, peer) = socket.recv_from(&mut token).unwrap();
        assert_eq!(n, 16);
        socket.send_to(&[0; 21], peer).unwrap();
        let mut response = token.to_vec();
        response.extend(123i32.to_be_bytes());
        response.push(1);
        socket.send_to(&response, peer).unwrap();
    });
    let result = tunnel::probe(&format!("127.0.0.1:{}", port - 1), Duration::from_secs(1)).unwrap();
    assert_eq!(result.server, 123.);
    assert!(result.limited);
    assert!(result.client >= 0.);
    server.join().unwrap();
}
#[test]
fn probe_timeout_does_not_make_cached_node_selectable() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    assert!(
        tunnel::probe(
            &format!("127.0.0.1:{}", port - 1),
            Duration::from_millis(80)
        )
        .is_err()
    );
    let target = tunnel::Target {
        pool: "pool".into(),
        endpoint: tunnel::Endpoint {
            name: "node".into(),
            address: "node.example:2000".into(),
        },
    };
    let result = tunnel::rank(target, &[], 0, 1., false, false);
    assert!(!result.available && result.score.is_infinite());
}
#[test]
fn generated_flags_are_accepted_by_installed_game_java9_when_available() {
    let java = std::path::Path::new(r"C:\EXBO\EXBO\runtime\stalcraft\win64\java\bin\java.exe");
    if !java.is_file() {
        return;
    }
    let p = optimization::automatic(hw(32, 16, 3600));
    let flags: Vec<_> = p
        .flags()
        .into_iter()
        .filter(|f| !f.starts_with("-Xmx") && !f.starts_with("-Xms") && f != "-XX:+AlwaysPreTouch")
        .collect();
    let output = crate::platform::hidden(
        std::process::Command::new(java)
            .args(flags)
            .args(["-Xmx128m", "-Xms128m", "-version"]),
    )
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        optimization::java_major(&String::from_utf8_lossy(&output.stderr)),
        Some(9)
    );
}

#[test]
fn hardware_memory_probe_prefers_configured_speed_and_handles_truncation() {
    let mut record = vec![0u8; 0x22];
    record[0] = 17;
    record[1] = 0x22;
    record[12..14].copy_from_slice(&16384u16.to_le_bytes());
    record[0x15..0x17].copy_from_slice(&3600u16.to_le_bytes());
    record[0x20..0x22].copy_from_slice(&2666u16.to_le_bytes());
    let mut raw = vec![0u8; 8];
    raw[4..8].copy_from_slice(&((record.len() + 2) as u32).to_le_bytes());
    raw.extend(record);
    raw.extend([0, 0]);
    assert_eq!(optimization::memory_speed(&raw), 2666);
    for length in 0..raw.len() {
        assert_eq!(optimization::memory_speed(&raw[..length]), 0);
    }
}
#[test]
fn catalog_validation_and_paired_vm_options_preserve_boundaries() {
    let json = serde_json::json!({"mode":"roxy","clientToTunnelRttWeight":1.0,"pools":[{"name":"pool","tunnels":[{"name":"node","address":"node.example:2000"}]}]});
    let catalog: tunnel::Catalog = serde_json::from_value(json.clone()).unwrap();
    tunnel::validate_catalog(&catalog).unwrap();
    for (key, value) in [
        ("mode", serde_json::json!("direct")),
        ("clientToTunnelRttWeight", serde_json::json!(1001)),
    ] {
        let mut invalid = json.clone();
        invalid[key] = value;
        let catalog: tunnel::Catalog = serde_json::from_value(invalid).unwrap();
        assert!(tunnel::validate_catalog(&catalog).is_err());
    }
    let args: Vec<String> = [
        "java.exe",
        "--add-reads",
        "java.base=ALL-UNNAMED",
        "-Xmn1g",
        "Game",
        "--token",
        "fixture",
    ]
    .map(str::to_owned)
    .into();
    let result = optimization::inject(&args, &optimization::aikar().flags(), true).unwrap();
    assert_eq!(&result[..3], &args[..3]);
    assert!(!result.contains(&"-Xmn1g".to_owned()));
    let entry = result.iter().position(|v| v == "Game").unwrap();
    assert_eq!(&result[entry..], &args[4..]);
}

#[test]
fn exens_command_keeps_platform_and_child_before_jvm_flags() {
    let command = r#""<java>/java/bin/ExensLauncher.exe" -platform exbo "<java>/java/bin/stalzone.exe" -classpath "classes.jar;libs.jar;modassets" -Xmx<mem>m -XX:MaxGCPauseMillis=3 GameMain --session fixture-session --gameDir "<cd>""#;
    let args = crate::launch::bake(
        command,
        std::path::Path::new(r"C:\Game Folder\stalcraft"),
        "fixture-user",
        "fixture-display",
        "fixture-session",
        2304,
    )
    .unwrap();
    let mut profile = optimization::aikar();
    profile.heap_size_gb = 5;
    let result = optimization::inject(&args, &profile.flags(), true).unwrap();
    assert_eq!(&result[..4], &args[..4], "anti-cheat argv was corrupted");
    let main = result.iter().position(|v| v == "GameMain").unwrap();
    let old_main = args.iter().position(|v| v == "GameMain").unwrap();
    assert_eq!(&result[main..], &args[old_main..]);
    assert!(result[4..main].contains(&"-Xmx5g".into()));
    assert!(!result.contains(&"-Xmx2304m".into()));
    assert!(!result.contains(&"-XX:MaxGCPauseMillis=3".into()));
    assert_eq!(&result[4..6], &args[4..6]);
}

#[test]
fn exens_network_only_override_preserves_both_native_and_jvm_arguments() {
    let args: Vec<String> = [
        "ExensLauncher.exe",
        "-platform",
        "exbo",
        "stalzone.exe",
        "-Xmx2304m",
        "-Droxy_address_override.ru=old.example:2000",
        "-cp",
        "classes.jar;libs.jar",
        "Main",
        "--session",
        "fixture token",
        "-Droxy_address_override.ru=application-value",
    ]
    .map(str::to_owned)
    .into();
    let result = optimization::inject(
        &args,
        &["-Droxy_address_override.ru=new.example:2000".into()],
        false,
    )
    .unwrap();
    assert_eq!(&result[..5], &args[..5]);
    assert!(!result.contains(&args[5]));
    assert_eq!(&result[result.len() - 4..], &args[8..]);
    assert_eq!(optimization::jvm_target_index(&result).unwrap(), 3);
    assert_eq!(optimization::inject(&args, &[], true).unwrap(), args);
}

#[test]
fn exens_malformed_prefix_is_rejected_instead_of_guessing_the_java_boundary() {
    for command in [
        "ExensLauncher.exe",
        "ExensLauncher.exe -platform exbo",
        "ExensLauncher.exe -unknown exbo stalzone.exe Main",
        "ExensLauncher.exe -platform -bad stalzone.exe Main",
        "ExensLauncher.exe -platform exbo SupportTool.exe Main",
    ] {
        let args = crate::launch::parse_args(command).unwrap();
        assert!(
            optimization::inject(&args, &["-Xmx5g".into()], true).is_err(),
            "{command}"
        );
    }
    let args = crate::launch::parse_args(
        "EXENSLAUNCHER.EXE --platform steam stalzonew.exe -jar game.jar --session fixture",
    )
    .unwrap();
    let result = optimization::inject(&args, &["-Xmx5g".into()], true).unwrap();
    assert_eq!(&result[..4], &args[..4]);
    assert_eq!(
        &result[4..],
        ["-Xmx5g", "-jar", "game.jar", "--session", "fixture"]
    );
}
