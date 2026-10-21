use crate::{
    api,
    download::{self, FileEntry, Metadata},
    launch,
    model::{Config, Server, Settings},
    storage::{self, JavaReader, Store},
};
use sha1::{Digest, Sha1};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("exbo-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture(paths: &[&str], data: &[u8]) -> Metadata {
    let all: Vec<u8> = paths.iter().flat_map(|_| data.iter().copied()).collect();
    Metadata {
        total: all.len() as u64,
        comment: "test".into(),
        trackers: vec!["http://127.0.0.1:1/announce".into()],
        piece_length: 4,
        name: "test".into(),
        files: paths
            .iter()
            .map(|p| FileEntry {
                path: p.to_string(),
                size: data.len() as u64,
                hash: Sha1::digest(data).into(),
            })
            .collect(),
        pieces: all.chunks(4).map(|p| Sha1::digest(p).into()).collect(),
    }
}

#[cfg(windows)]
#[test]
fn optimized_launch_reaches_java_through_native_exens_wrapper() {
    let temp = Temp::new();
    let root = temp.0.join("Game folder with spaces");
    let bin = root.join("win64").join("java").join("bin");
    fs::create_dir_all(&bin).unwrap();
    let source = temp.0.join("launch_stub.rs");
    fs::write(
        &source,
        include_str!("../../scripts/fixtures/launch_stub.rs"),
    )
    .unwrap();
    let wrapper = bin.join("ExensLauncher.exe");
    let output = crate::platform::hidden(
        std::process::Command::new("rustc")
            .args(["--edition", "2024", "--crate-name", "launch_stub"])
            .arg(&source)
            .arg("-o")
            .arg(&wrapper),
    )
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::copy(&wrapper, bin.join("stalzone.exe")).unwrap();
    fs::copy(&wrapper, bin.join("java.exe")).unwrap();
    let command = r#""<java>/java/bin/ExensLauncher.exe" -platform exbo "<java>/java/bin/stalzone.exe" -cp "classes.jar;libs.jar;modassets" -Xmx<mem>m -XX:MaxGCPauseMillis=3 GameMain --session "<sess>" --gameDir "<cd>""#;
    let mut settings = Settings::default();
    let available = crate::optimization::cpu::available_mask().ok();
    let cpu_choices = if let Some(mask) = available {
        let indices: Vec<_> = (0..64u16).filter(|cpu| mask & (1u64 << cpu) != 0).collect();
        let mut choices = vec![None, Some(vec![indices[0]])];
        if indices.len() >= 3 {
            choices.push(Some(vec![indices[0], *indices.last().unwrap()]));
        }
        choices
    } else {
        vec![None]
    };
    for (enabled, cpus) in [false, true]
        .into_iter()
        .flat_map(|enabled| cpu_choices.iter().cloned().map(move |cpus| (enabled, cpus)))
    {
        let probe = bin.join("fixture-probe.txt");
        if probe.exists() {
            fs::remove_file(probe).unwrap();
        }
        settings.optimization.cpu_affinity = cpus.clone();
        let mut profile = crate::optimization::aikar();
        profile.heap_size_gb = 1;
        profile.metaspace_mb = 64;
        profile.reserved_code_cache_size_mb = 128;
        profile.pre_touch = false;
        settings.optimization.enabled = enabled;
        settings.optimization.profile = Some(profile);
        let mut child = launch::start(
            command,
            &root,
            "fixture user",
            "Тест",
            "fixture session",
            &settings,
            &root.join("logs"),
        )
        .unwrap();
        assert!(child.wait().unwrap().success());
        if let Some(available) = available {
            let expected = cpus.as_ref().map_or(available, |cpus| {
                cpus.iter().fold(0u64, |mask, &cpu| mask | (1u64 << cpu))
            });
            for name in ["fixture-wrapper-cpu.txt", "fixture-java-cpu.txt"] {
                assert_eq!(
                    fs::read_to_string(root.join(name))
                        .unwrap()
                        .parse::<u64>()
                        .unwrap(),
                    expected
                );
            }
            assert_eq!(
                crate::optimization::cpu::available_mask().unwrap(),
                available,
                "Launcher CPU affinity changed"
            );
        }
        let wrapper_args = fs::read_to_string(root.join("fixture-wrapper-args.txt")).unwrap();
        let wrapper_args: Vec<_> = wrapper_args.lines().collect();
        assert_eq!(
            &wrapper_args[..3],
            &[
                "-platform",
                "exbo",
                bin.join("stalzone.exe").to_str().unwrap()
            ]
        );
        let java_args = fs::read_to_string(root.join("fixture-java-args.txt")).unwrap();
        let java_args: Vec<_> = java_args.lines().collect();
        let main = java_args.iter().position(|v| *v == "GameMain").unwrap();
        assert_eq!(&java_args[..2], &["-cp", "classes.jar;libs.jar;modassets"]);
        assert_eq!(
            &java_args[main..],
            &[
                "GameMain",
                "--session",
                "fixture session",
                "--gameDir",
                root.to_str().unwrap()
            ]
        );
        if enabled {
            assert!(java_args[..main].contains(&"-Xmx1g"));
            assert!(!java_args.contains(&"-XX:MaxGCPauseMillis=3"));
            assert_eq!(
                fs::read_to_string(bin.join("fixture-probe.txt")).unwrap(),
                "java.exe -version"
            );
        } else {
            assert!(java_args[..main].contains(&"-XX:MaxGCPauseMillis=3"));
            assert!(!bin.join("fixture-probe.txt").exists());
        }
    }
    fs::remove_file(bin.join("java.exe")).unwrap();
    fs::remove_file(bin.join("fixture-probe.txt")).unwrap();
    let before = fs::read(root.join("fixture-wrapper-args.txt")).unwrap();
    assert!(crate::optimization::validate_runtime(&wrapper).is_err());
    assert_eq!(
        fs::read(root.join("fixture-wrapper-args.txt")).unwrap(),
        before
    );
    assert!(!bin.join("fixture-probe.txt").exists());
}
fn utf(out: &mut Vec<u8>, text: &str) {
    let mut data = Vec::new();
    for u in text.encode_utf16() {
        match u {
            0 => data.extend([0xc0, 0x80]),
            1..=0x7f => data.push(u as u8),
            0x80..=0x7ff => data.extend([(0xc0 | (u >> 6)) as u8, (0x80 | (u & 63)) as u8]),
            _ => data.extend([
                (0xe0 | (u >> 12)) as u8,
                (0x80 | ((u >> 6) & 63)) as u8,
                (0x80 | (u & 63)) as u8,
            ]),
        }
    }
    out.extend((data.len() as u16).to_be_bytes());
    out.extend(data);
}
fn binary(meta: &Metadata) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(meta.total.to_be_bytes());
    utf(&mut out, &meta.comment);
    out.extend((meta.trackers.len() as u32).to_be_bytes());
    for t in &meta.trackers {
        utf(&mut out, t);
    }
    utf(&mut out, "");
    out.extend(meta.piece_length.to_be_bytes());
    utf(&mut out, &meta.name);
    out.extend((meta.files.len() as u32).to_be_bytes());
    for f in &meta.files {
        utf(&mut out, &f.path);
        out.extend(f.size.to_be_bytes());
        out.extend(f.hash);
    }
    out.extend((meta.pieces.len() as u32).to_be_bytes());
    for p in &meta.pieces {
        out.extend(p);
    }
    out
}

type MockResponse = (u16, Vec<(&'static str, String)>, Vec<u8>);
fn mock(
    responses: Vec<MockResponse>,
) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let mut socket = loop {
                if let Ok((socket, _)) = listener.accept() {
                    break socket;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "expected HTTP request"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            let mut b = [0; 1];
            while !request.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut b).unwrap();
                request.push(b[0]);
                assert!(request.len() < 16384);
            }
            tx.send(String::from_utf8(request).unwrap()).unwrap();
            let extra = headers
                .into_iter()
                .map(|(k, v)| format!("{k}: {v}\r\n"))
                .collect::<String>();
            write!(
                socket,
                "HTTP/1.1 {status} OK\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",
                body.len()
            )
            .unwrap();
            socket.write_all(&body).unwrap();
        }
    });
    (url, rx, thread)
}

#[test]
fn fresh_standalone_profile_does_not_import_existing_exbo_data() {
    let temp = Temp::new();
    let store = Store::new(temp.0.join("new-user")).unwrap();
    let first = store.load_with_import(false).unwrap();
    assert!(first.legacy_root.is_empty());
    assert!(first.clients.is_empty() && first.accounts.is_empty());
    assert!(!first.auto_login && first.active_account.is_empty());
    assert!(uuid::Uuid::parse_str(&first.user_id).is_ok());
    assert_eq!(store.load().unwrap().user_id, first.user_id);
    assert!(!store.root.join("bootstrap.jar").exists());
}

#[test]
fn embedded_native_resources_are_repaired_without_external_installation() {
    let temp = Temp::new();
    let dir = crate::support::native_dir(&temp.0).unwrap();
    let bridge = fs::read(dir.join("exbo-hwid.exe")).unwrap();
    let dll = fs::read(dir.join("jmt.dll")).unwrap();
    fs::write(dir.join("keep.txt"), "user file").unwrap();
    fs::write(dir.join("exbo-hwid.exe"), vec![0; bridge.len()]).unwrap();
    fs::remove_file(dir.join("jmt.dll")).unwrap();
    assert_eq!(crate::support::native_dir(&temp.0).unwrap(), dir);
    assert_eq!(fs::read(dir.join("exbo-hwid.exe")).unwrap(), bridge);
    assert_eq!(fs::read(dir.join("jmt.dll")).unwrap(), dll);
    assert_eq!(
        fs::read_to_string(dir.join("keep.txt")).unwrap(),
        "user file"
    );
    assert!(crate::support::native_dir(std::path::Path::new("relative")).is_err());
}

#[cfg(windows)]
#[test]
fn standalone_api_uses_embedded_identity_with_empty_or_invalid_legacy_path() {
    let temp = Temp::new();
    let store = Store::new(temp.0.join("profile")).unwrap();
    let mut config = store.load_with_import(false).unwrap();
    for legacy in ["", r"Z:\nonexistent\official-installation"] {
        config.legacy_root = legacy.into();
        let (endpoint, requests, http) =
            mock(vec![(200, vec![], br#"{"type":"TEST_RESPONSE"}"#.to_vec())]);
        let api = api::Api::testing_autonomous(endpoint, &config, &store.root).unwrap();
        assert_eq!(
            api.auth("standalone-fixture", "fixture password", None, true)
                .unwrap()
                .kind,
            "TEST_RESPONSE"
        );
        let request = requests.recv_timeout(Duration::from_secs(3)).unwrap();
        let target = request.split_whitespace().nth(1).unwrap();
        let url = url::Url::parse(&format!("http://localhost{target}")).unwrap();
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("bootstrap").unwrap(),
            "9d665c84d90a06af94679bde05994fd775a20cd"
        );
        assert_eq!(query.get("hwid").unwrap().len(), 80);
        assert_eq!(query.get("id").unwrap(), &config.user_id);
        assert!(!query.contains_key("legacy_root"));
        http.join().unwrap();
    }
}

#[test]
fn java_modified_utf8_roundtrip() {
    let mut bytes = Vec::new();
    utf(&mut bytes, "Привет\0🦀");
    assert_eq!(JavaReader::new(&bytes).utf().unwrap(), "Привет\0🦀");
}
#[test]
fn rejects_truncated_utf8() {
    assert!(JavaReader::new(&[0, 2, 0xe0, 0xa0]).utf().is_err());
}
#[test]
fn java_big_integer_hex_removes_padding() {
    assert_eq!(api::java_hex(&[0, 0, 1, 2]), "102");
    assert_eq!(api::java_hex(&[0; 20]), "0");
}
#[test]
fn metadata_roundtrip() {
    let meta = fixture(&["data/файл.txt", "other/file.bin"], b"1234567");
    let parsed = Metadata::parse(&binary(&meta)).unwrap();
    assert_eq!(parsed.files.len(), 2);
    assert_eq!(parsed.total, 14);
    assert_eq!(parsed.pieces, meta.pieces);
}

#[test]
fn torrent_api_uses_little_endian_length_before_gzip() {
    let expected = binary(&fixture(&["data/missing.bin"], b"restore this file"));
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&expected).unwrap();
    let mut body = (expected.len() as u32).to_le_bytes().to_vec();
    body.extend(gzip.finish().unwrap());
    let (endpoint, requests, thread) = mock(vec![(200, vec![], body)]);
    let actual = api::Api::testing(endpoint).torrent_data("fixture", "test-token", "test");
    assert!(
        requests
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .starts_with("GET /torrentData?")
    );
    thread.join().unwrap();
    assert_eq!(actual.unwrap(), expected);
}

#[test]
fn torrent_api_reports_expired_session_instead_of_oversized_metadata() {
    let (url, requests, http) = mock(vec![(
        200,
        vec![],
        br#"{"success":false,"data":null}"#.to_vec(),
    )]);
    let error = api::Api::testing(url)
        .torrent_data("user", "test-token", "test")
        .unwrap_err();
    requests.recv().unwrap();
    http.join().unwrap();
    assert!(error.to_string().contains("Сессия истекла"));
}

#[test]
fn torrent_api_rejects_oversized_and_truncated_envelopes() {
    for body in [
        u32::MAX.to_le_bytes().to_vec(),
        vec![1, 0],
        vec![1, 0, 0, 0, 0x1f, 0x8b],
    ] {
        let (url, requests, http) = mock(vec![(200, vec![], body)]);
        assert!(
            api::Api::testing(url)
                .torrent_data("user", "test-token", "test")
                .is_err()
        );
        requests.recv().unwrap();
        http.join().unwrap();
    }
}
#[test]
fn rejects_bad_metadata_sizes() {
    let mut bytes = binary(&fixture(&["file"], b"data"));
    bytes[7] = 99;
    assert!(Metadata::parse(&bytes).is_err());
    let len = bytes.len();
    assert!(Metadata::parse(&bytes[..len - 2]).is_err());
}
#[test]
fn rejects_path_escape_and_windows_devices() {
    for p in [
        "../x",
        "/x",
        "a/../x",
        "C:/x",
        "a\\x",
        "a:x",
        "a//x",
        "CON.txt",
        "folder/NUL",
        "a/COM1.txt",
        "a/.",
        "a/..",
        "file.",
        "bad?.bin",
        "bad|name",
        "bad\nname",
    ] {
        assert!(storage::relative_path(p).is_err(), "{p}");
    }
    assert!(storage::relative_path("data/текст.bin").is_ok());
}
#[test]
fn duplicate_case_insensitive_paths_rejected() {
    assert!(Metadata::parse(&binary(&fixture(&["a.txt", "A.txt"], b"x"))).is_err());
}
#[test]
fn torrent_resolves_true_paths_and_preserves_wire_infohash() {
    let meta = fixture(&["a/one.bin", "b/two.bin"], b"same");
    let bytes = meta.torrent();
    let parsed = librqbit::torrent_from_bytes(&bytes).unwrap();
    let info_start = bytes.windows(6).position(|w| w == b"4:info").unwrap() + 6;
    let expected = Sha1::digest(&bytes[info_start..bytes.len() - 1]);
    assert_eq!(parsed.info_hash.0.as_slice(), expected.as_slice());
    let files = parsed.info.data.files.as_ref().unwrap();
    assert_eq!(
        files[0]
            .path
            .iter()
            .map(|p| std::str::from_utf8(p.as_ref()).unwrap())
            .collect::<Vec<_>>(),
        vec!["a", "one.bin"]
    );
    assert_ne!(files[0].path, files[1].path);
    parsed.info.data.clone().validate().unwrap();
}
#[test]
fn command_substitution_matches_java_utf16_and_spaces() {
    let result = launch::bake(
        "\"<java>/java/bin/java.exe\" -Xmx<mem>M -Duser=\"<usr>\" <sess> <dlogin>",
        &PathBuf::from(r"C:\Games Folder"),
        "test user",
        "Тест",
        "session/+=",
        2048,
    )
    .unwrap();
    assert_eq!(result[0], r"C:\Games Folder\win64\java\bin\java.exe");
    assert_eq!(result[1], "-Xmx2048M");
    assert_eq!(result[2], "-Duser=test user");
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&result[4])
            .unwrap(),
        [0xfe, 0xff, 0x04, 0x22, 0x04, 0x35, 0x04, 0x41, 0x04, 0x42]
    );
}
#[test]
fn command_parser_rejects_bad_quotes() {
    assert!(launch::parse_args("a \"broken").is_err());
    assert_eq!(
        launch::parse_args("one   \"\" two").unwrap(),
        ["one", "", "two"]
    );
}
#[test]
fn token_envelope_and_session_error() {
    assert_eq!(
        api::unwrap_token(serde_json::json!({"success":true,"data":"[1,2]"})).unwrap(),
        serde_json::json!([1, 2])
    );
    assert!(api::unwrap_token(serde_json::json!({"success":false,"preventRelog":true})).is_err());
}
#[test]
fn auth_request_encodes_credentials_without_query_in_errors() {
    let (url, rx, server) = mock(vec![(
        200,
        vec![],
        br#"{"type":"WRONG_CREDENTIALS"}"#.to_vec(),
    )]);
    let api = api::Api::testing(url);
    let r = api.auth("user+a@b.ru", "pa&ss=word", None, true).unwrap();
    assert_eq!(r.kind, "WRONG_CREDENTIALS");
    let request = rx.recv().unwrap();
    assert!(request.contains("login=user%2Ba%40b.ru"));
    assert!(request.contains("pass=pa%26ss%3Dword"));
    assert!(request.contains("protocol_version=3"));
    server.join().unwrap();
}
#[test]
fn auth_pow_and_nice_secret_are_bounded() {
    let hash = api::java_hex(&Sha1::digest(b"00000"));
    let (url, rx, server) = mock(vec![
        (
            200,
            vec![],
            format!(r#"{{"type":"NEED_SECRET","secretHash":"{hash}"}}"#).into_bytes(),
        ),
        (200, vec![], br#"{"type":"NICE_SECRET"}"#.to_vec()),
        (200, vec![], br#"{"type":"GOOGLE_AUTH"}"#.to_vec()),
    ]);
    let r = api::Api::testing(url)
        .auth("user", "password", None, true)
        .unwrap();
    assert_eq!(r.kind, "GOOGLE_AUTH");
    rx.recv().unwrap();
    let request = rx.recv().unwrap();
    assert!(request.contains("answer=00000"));
    assert!(request.contains("new_protocol=true"));
    assert!(!rx.recv().unwrap().contains("start=true"));
    server.join().unwrap();
}
#[test]
fn server_json_uses_original_fields() {
    let r:Server=serde_json::from_value(serde_json::json!({"id":"stalcraft","caption":"STALCRAFT","runCommand":"java <sess>","onlineMax":120,"onlineCurrent":99,"httpSeeds":["http://localhost"]})).unwrap();
    assert_eq!(r.online_current, 99);
    assert_eq!(r.http_seeds.len(), 1);
}

#[test]
fn mail_verification_resend_uses_its_own_original_auth_route() {
    fn parameters<'a>(step: &str, email: &'a str, resend: bool) -> Vec<(&'static str, &'a str)> {
        api::auth_step_parameters(
            step,
            "12345",
            email,
            "new-password",
            "old-password",
            "5123",
            resend,
        )
        .unwrap()
    }
    assert_eq!(parameters("mail", "", true), vec![("regenerate", "true")]);
    assert_eq!(
        parameters("email-code", "", true),
        vec![("action", "confirm_existed")]
    );
    assert_eq!(
        parameters("email-code", "qa@example.org", true),
        vec![
            ("action", "confirm_new_mail_initial"),
            ("mail", "qa@example.org"),
            ("socket_port", "5123")
        ]
    );
    assert_eq!(
        parameters("email-code", "", false),
        vec![
            ("action", "confirm_new_mail_code"),
            ("mail", "null"),
            ("code", "12345"),
            ("socket_port", "5123")
        ]
    );
    assert!(api::auth_step_parameters("unknown", "", "", "", "", "", false).is_err());
}
#[test]
fn config_atomic_backup_and_dpapi_roundtrip() {
    let temp = Temp::new();
    let store = Store::new(temp.0.clone()).unwrap();
    let mut config = Config::default();
    store.save(&config).unwrap();
    config.active_account = "test".into();
    store.save(&config).unwrap();
    assert!(temp.0.join("config.bak").exists());
    assert_eq!(store.load().unwrap().active_account, "test");
    let secret = "session-0123";
    let encrypted = crate::platform::protect(secret).unwrap();
    assert!(!encrypted.contains(secret));
    assert_eq!(crate::platform::unprotect(&encrypted).unwrap(), secret);
}

#[test]
fn launch_verification_is_opt_in_and_survives_restart() {
    let legacy: Settings = serde_json::from_str(r#"{"tray":true,"ram":0.5}"#).unwrap();
    assert!(!Settings::default().verify_files_on_launch);
    assert!(!legacy.verify_files_on_launch);
    let temp = Temp::new();
    let store = Store::new(temp.0.clone()).unwrap();
    let mut config = Config::default();
    config.settings.verify_files_on_launch = true;
    store.save(&config).unwrap();
    assert!(store.load().unwrap().settings.verify_files_on_launch);
    config.settings.verify_files_on_launch = false;
    store.save(&config).unwrap();
    assert!(!store.load().unwrap().settings.verify_files_on_launch);
}

#[test]
fn skipping_routine_verification_preserves_game_updates() {
    let settings = Settings::default();
    assert!(!settings.verify_before_launch(None, "current"));
    assert!(!settings.verify_before_launch(Some("current"), "current"));
    assert!(settings.verify_before_launch(Some("older"), "current"));
    let enabled = Settings {
        verify_files_on_launch: true,
        ..settings
    };
    assert!(enabled.verify_before_launch(None, "current"));
    assert!(enabled.verify_before_launch(Some("current"), "current"));
    let temp = Temp::new();
    let store = Store::new(temp.0.clone()).unwrap();
    assert!(store.installed_version("test").unwrap().is_none());
    store.record_installed_version("test", "current").unwrap();
    assert_eq!(
        store.installed_version("test").unwrap().as_deref(),
        Some("current")
    );
}

#[test]
fn tray_hover_and_single_click_do_not_restore_the_window() {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
    let id: tray_icon::TrayIconId = "test-tray".into();
    let position = Default::default();
    let rect = Default::default();
    for event in [
        TrayIconEvent::Enter {
            id: id.clone(),
            position,
            rect,
        },
        TrayIconEvent::Move {
            id: id.clone(),
            position,
            rect,
        },
        TrayIconEvent::Leave {
            id: id.clone(),
            position,
            rect,
        },
        TrayIconEvent::Click {
            id: id.clone(),
            position,
            rect,
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
        },
        TrayIconEvent::Click {
            id: id.clone(),
            position,
            rect,
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
        },
        TrayIconEvent::Click {
            id: id.clone(),
            position,
            rect,
            button: MouseButton::Right,
            button_state: MouseButtonState::Up,
        },
        TrayIconEvent::DoubleClick {
            id: id.clone(),
            position,
            rect,
            button: MouseButton::Right,
        },
    ] {
        assert!(!crate::app::view::restores_from_tray(&event), "{event:?}");
    }
    assert!(crate::app::view::restores_from_tray(
        &TrayIconEvent::DoubleClick {
            id,
            position,
            rect,
            button: MouseButton::Left
        }
    ));
}
#[test]
fn http_duplicate_content_is_downloaded_once_and_installed_twice() {
    let data = b"same file data";
    let meta = fixture(&["a/one.bin", "b/two.bin"], data);
    let (url, rx, http) = mock(vec![(200, vec![], data.to_vec())]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    let server = Server {
        id: "test".into(),
        version: "version".into(),
        http_seeds: vec![url.clone()],
        ..Default::default()
    };
    download::update(
        &api::Api::testing(url),
        &store,
        &server,
        &meta,
        &root,
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &reporter(),
        false,
    )
    .unwrap();
    assert_eq!(fs::read(root.join("a/one.bin")).unwrap(), data);
    assert_eq!(fs::read(root.join("b/two.bin")).unwrap(), data);
    assert!(
        rx.recv()
            .unwrap()
            .contains(&api::java_hex(&meta.files[0].hash))
    );
    http.join().unwrap();
}
#[test]
fn http_resume_uses_range_and_verifies_hash() {
    let data = b"1234567890";
    let meta = fixture(&["file.bin"], data);
    let (url, rx, http) = mock(vec![(
        206,
        vec![("Content-Range", "bytes 4-9/10".into())],
        data[4..].to_vec(),
    )]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let stage = store.root.join("downloads/test");
    fs::create_dir_all(&stage).unwrap();
    fs::write(
        stage.join(format!("{}.part", api::java_hex(&meta.files[0].hash))),
        &data[..4],
    )
    .unwrap();
    let root = temp.0.join("game");
    download::update(
        &api::Api::testing(url.clone()),
        &store,
        &Server {
            id: "test".into(),
            http_seeds: vec![url],
            ..Default::default()
        },
        &meta,
        &root,
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &reporter(),
        false,
    )
    .unwrap();
    assert_eq!(fs::read(root.join("file.bin")).unwrap(), data);
    assert!(
        rx.recv()
            .unwrap()
            .to_lowercase()
            .contains("range: bytes=4-9")
    );
    http.join().unwrap();
}
#[test]
fn corrupted_download_keeps_existing_client_file() {
    let meta = fixture(&["file.bin"], b"good");
    let (url, rx, http) = mock(vec![
        (200, vec![], b"evil".to_vec()),
        (200, vec![], b"evil".to_vec()),
    ]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file.bin"), b"old!").unwrap();
    assert!(
        download::update(
            &api::Api::testing(url.clone()),
            &store,
            &Server {
                id: "test".into(),
                http_seeds: vec![url],
                ..Default::default()
            },
            &meta,
            &root,
            &Settings {
                p2p: 2,
                ..Default::default()
            },
            &download::Control::default(),
            &reporter(),
            false
        )
        .is_err()
    );
    assert_eq!(fs::read(root.join("file.bin")).unwrap(), b"old!");
    rx.recv().unwrap();
    rx.recv().unwrap();
    http.join().unwrap();
}
#[test]
fn soft_exclusions_are_preserved() {
    let meta = fixture(&["config/options.txt"], b"default");
    let temp = Temp::new();
    let root = temp.0.join("game");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(root.join("config/options.txt"), b"custom").unwrap();
    let store = Store::new(temp.0.join("state")).unwrap();
    download::update(
        &api::Api::testing("http://127.0.0.1:1".into()),
        &store,
        &Server {
            id: "test".into(),
            exclusions: vec!["config".into()],
            ..Default::default()
        },
        &meta,
        &root,
        &Settings::default(),
        &download::Control::default(),
        &reporter(),
        false,
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("config/options.txt")).unwrap(),
        b"custom"
    );
}
#[test]
fn cancellation_does_not_modify_installation() {
    let temp = Temp::new();
    let control = download::Control::default();
    control.cancel();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    assert!(
        download::update(
            &api::Api::testing("http://127.0.0.1:1".into()),
            &store,
            &Server {
                id: "test".into(),
                ..Default::default()
            },
            &fixture(&["file"], b"data"),
            &root,
            &Settings::default(),
            &control,
            &reporter(),
            false
        )
        .is_err()
    );
    assert!(!root.join("file").exists());
}
#[test]
fn deletion_keeps_unmanaged_data() {
    let temp = Temp::new();
    fs::write(temp.0.join("game.bin"), b"data").unwrap();
    fs::write(temp.0.join("save.dat"), b"save").unwrap();
    assert_eq!(
        download::remove_client(&temp.0, &fixture(&["game.bin"], b"data")).unwrap(),
        1
    );
    assert!(temp.0.join("save.dat").exists());
}
#[test]
fn ipc_matches_original_newline_protocol() {
    let (tx, rx) = mpsc::channel();
    let ipc = crate::ipc::Ipc::start(tx).unwrap();
    crate::ipc::forward(
        ipc.port,
        "exbo://auth?action=distributor_auth&verify_token=test",
    )
    .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        "exbo://auth?action=distributor_auth&verify_token=test"
    );
}

#[test]
#[cfg(windows)]
fn relative_profile_and_browser_callback_share_one_instance() {
    let relative =
        PathBuf::from("target").join(format!("callback-profile-{}", uuid::Uuid::new_v4()));
    let cleanup = Temp(relative.clone());
    let original = Store::new(relative).unwrap();
    assert!(original.root.is_absolute());
    let callback = Store::new(std::path::absolute(&original.root).unwrap()).unwrap();
    assert_eq!(original.root, callback.root);
    let guard = crate::platform::InstanceGuard::acquire(&original.root)
        .unwrap()
        .unwrap();
    assert!(
        crate::platform::InstanceGuard::acquire(&callback.root)
            .unwrap()
            .is_none()
    );
    drop(guard);
    drop(cleanup);
}

#[test]
fn steam_browser_callback_keeps_pending_session_and_encoded_token() {
    let (tx, rx) = mpsc::channel();
    let ipc = crate::ipc::Ipc::start(tx).unwrap();
    let pending = serde_json::json!({
        "type": "DISTRIBUTOR_PENDING", "token": "steam", "session": "pending-session",
        "loginURL": "https://example.org/steam"
    });
    let complete = serde_json::json!({"type": "OK", "trueLogin": "fixture-user", "session": "account-session"});
    let (url, requests, http) = mock(vec![
        (200, vec![], serde_json::to_vec(&pending).unwrap()),
        (200, vec![], serde_json::to_vec(&complete).unwrap()),
    ]);
    let api = api::Api::testing(url);
    let pending = api.distributor("steam", ipc.port, None).unwrap();
    assert_eq!(pending.kind, "DISTRIBUTOR_PENDING");
    let link = format!(
        "exbo://127.0.0.1:{}/?action=distributor_auth&verify_token=a%2Bb%26c%3D",
        ipc.port
    );
    let args = vec!["launcher.exe".into(), "--link".into(), link.clone()];
    crate::ipc::forward(
        ipc.port,
        &crate::protocol::link_argument(&args).unwrap().unwrap(),
    )
    .unwrap();
    let delivered = rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(delivered, link);
    let token = crate::protocol::callback_url(&delivered)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == "verify_token")
        .unwrap()
        .1
        .into_owned();
    let result = api
        .distributor(
            pending.token.as_deref().unwrap(),
            ipc.port,
            Some((pending.session.as_deref().unwrap(), &token)),
        )
        .unwrap();
    assert_eq!(result.kind, "OK");
    http.join().unwrap();
    let requests: Vec<_> = requests.try_iter().collect();
    assert_eq!(requests.len(), 2);
    for (index, request) in requests.iter().enumerate() {
        let target = request.split_whitespace().nth(1).unwrap();
        let parsed = url::Url::parse(&format!("http://localhost{target}")).unwrap();
        let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(parsed.path(), "/distributor");
        assert_eq!(query["socket_port"], ipc.port.to_string());
        assert_eq!(query["distributor"], "steam");
        if index == 1 {
            assert_eq!(query["distributor_session"], "pending-session");
            assert_eq!(query["verify_token"], "a+b&c=");
        }
    }
}
#[test]
fn pause_can_be_cancelled() {
    let control = download::Control::default();
    control.pause(true);
    let c = control.clone();
    let t = std::thread::spawn(move || c.checkpoint());
    std::thread::sleep(Duration::from_millis(100));
    control.cancel();
    control.pause(false);
    assert_eq!(control.state(), 2);
    assert!(t.join().unwrap().is_err());
}

#[test]
fn missing_file_is_restored_through_original_api_envelope() {
    let data = b"missing file contents";
    let meta = fixture(&["keep.bin", "restore.bin"], data);
    let bytes = binary(&meta);
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&bytes).unwrap();
    let mut body = (bytes.len() as u32).to_le_bytes().to_vec();
    body.extend(gzip.finish().unwrap());
    let (url, requests, http) = mock(vec![(200, vec![], body), (200, vec![], data.to_vec())]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep.bin"), data).unwrap();
    fs::write(root.join("restore.bin"), data).unwrap();
    fs::remove_file(root.join("restore.bin")).unwrap();
    let server = Server {
        id: "test".into(),
        version: api::java_hex(&Sha1::digest(&bytes)),
        http_seeds: vec![url.clone()],
        ..Default::default()
    };
    let api = api::Api::testing(url);
    let received = download::metadata(
        &api,
        &store,
        &temp.0.join("legacy"),
        &server,
        "user",
        "test-token",
    )
    .unwrap();
    let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = progress.clone();
    let report: download::Reporter = Arc::new(move |p| capture.lock().unwrap().push(p));
    download::update(
        &api,
        &store,
        &server,
        &received,
        &root,
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &report,
        false,
    )
    .unwrap();
    http.join().unwrap();
    assert_eq!(requests.try_iter().count(), 2);
    assert_eq!(fs::read(root.join("keep.bin")).unwrap(), data);
    assert_eq!(fs::read(root.join("restore.bin")).unwrap(), data);
    assert_eq!(
        fs::read_to_string(store.root.join("test.installed")).unwrap(),
        server.version
    );
    let values = progress.lock().unwrap();
    assert!(values.iter().all(|p| p.done <= p.total));
    assert!(
        values
            .iter()
            .filter(|p| matches!(p.phase, download::Phase::Checking))
            .all(|p| p.total == meta.total)
    );
    assert!(
        values
            .iter()
            .filter(|p| matches!(p.phase, download::Phase::Downloading))
            .all(|p| p.total == data.len() as u64)
    );
    assert!(matches!(
        values.last().unwrap().phase,
        download::Phase::Ready
    ));
}

#[test]
fn failed_http_attempt_does_not_inflate_progress() {
    let data = b"correct";
    let meta = fixture(&["a.bin", "b.bin"], data);
    let (url, requests, http) = mock(vec![
        (200, vec![], b"invalid".to_vec()),
        (200, vec![], data.to_vec()),
    ]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
    let capture = progress.clone();
    let report: download::Reporter = Arc::new(move |p| capture.lock().unwrap().push(p));
    download::update(
        &api::Api::testing(url.clone()),
        &store,
        &Server {
            id: "test".into(),
            http_seeds: vec![url],
            ..Default::default()
        },
        &meta,
        &temp.0.join("game"),
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &report,
        false,
    )
    .unwrap();
    http.join().unwrap();
    assert_eq!(requests.try_iter().count(), 2);
    assert!(progress.lock().unwrap().iter().all(|p| p.done <= p.total));
}

#[test]
fn range_with_wrong_total_is_rejected_before_writing() {
    let data = b"1234567890";
    let meta = fixture(&["file.bin"], data);
    let response = (
        206,
        vec![("Content-Range", "bytes 0-9/9999".into())],
        data.to_vec(),
    );
    let (url, requests, http) = mock(vec![response.clone(), response]);
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    let outcome = download::update(
        &api::Api::testing(url.clone()),
        &store,
        &Server {
            id: "test".into(),
            http_seeds: vec![url],
            ..Default::default()
        },
        &meta,
        &root,
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &reporter(),
        false,
    );
    http.join().unwrap();
    assert!(outcome.is_err());
    assert_eq!(requests.try_iter().count(), 2);
    assert!(!root.join("file.bin").exists());
}

#[test]
fn zero_length_file_needs_no_network_or_p2p() {
    let temp = Temp::new();
    let store = Store::new(temp.0.join("state")).unwrap();
    let root = temp.0.join("game");
    download::update(
        &api::Api::testing("http://127.0.0.1:1".into()),
        &store,
        &Server {
            id: "test".into(),
            ..Default::default()
        },
        &fixture(&["empty.bin"], b""),
        &root,
        &Settings {
            p2p: 2,
            ..Default::default()
        },
        &download::Control::default(),
        &reporter(),
        false,
    )
    .unwrap();
    assert_eq!(fs::metadata(root.join("empty.bin")).unwrap().len(), 0);
    assert!(store.root.join("test.installed").is_file());
}

#[test]
fn manifest_rejects_file_used_as_directory_and_conflicting_content_size() {
    assert!(Metadata::parse(&binary(&fixture(&["a", "A/b"], b"data"))).is_err());
    let mut meta = fixture(&["a", "b"], b"data");
    meta.files[1].size += 1;
    meta.total += 1;
    assert!(
        Metadata::parse(&binary(&meta))
            .unwrap_err()
            .to_string()
            .contains("SHA-1")
    );
}

#[test]
fn progress_labels_keep_small_file_sizes_and_checking_separate() {
    assert_eq!(crate::app::view::size_label(1024), "1.00 KiB");
    assert_eq!(crate::app::view::size_label(47_370_390_379), "44.12 GiB");
    let mut p = download::Progress {
        phase: download::Phase::Downloading,
        done: 7,
        total: 14,
        rate: 1024,
        message: String::new(),
    };
    assert_eq!(
        crate::app::view::progress_label(&p, false),
        "Загрузка: 7 B / 14 B · 1.00 KiB/s"
    );
    assert_eq!(
        crate::app::view::progress_label(&p, true),
        "Загрузка: 7 B / 14 B · 0 B/s"
    );
    p.phase = download::Phase::Checking;
    assert!(!crate::app::view::progress_label(&p, false).contains("/s"));
}
#[test]
fn real_exbo_manifest_parses_when_available() {
    let path = PathBuf::from(r"C:\EXBO\EXBO\runtime\stalcraft.torrent.bin");
    if !path.exists() {
        return;
    }
    let bytes = fs::read(path).unwrap();
    let meta = Metadata::parse(&bytes).unwrap();
    assert!(meta.files.len() > 1000);
    let torrent = meta.torrent();
    let parsed = librqbit::torrent_from_bytes(&torrent).unwrap();
    parsed.info.data.clone().validate().unwrap();
}

fn reporter() -> download::Reporter {
    Arc::new(|_| {})
}

#[test]
fn bcrypt_matches_original_java_implementation() {
    use base64::Engine;
    let first = format!("{:x}", md5::Md5::digest(b"fixture-password"));
    let second = format!("{:x}", md5::Md5::digest(first.as_bytes()));
    let salt: [u8; 16] = std::array::from_fn(|i| i as u8);
    let hash = bcrypt::hash_with_salt(second, 10, salt)
        .unwrap()
        .format_for_version(bcrypt::Version::TwoY);
    assert_eq!(
        hash,
        "$2y$10$..CA.uOD/eaGAOmJB.yMBunbNlDTRJClVX7IUmI73xWTunBEjn5ZW"
    );
    assert_eq!(
        bcrypt::BASE_64.decode("..CA.uOD/eaGAOmJB.yMBu").unwrap(),
        salt
    );
}
#[test]
fn update_staging_checks_sha256_without_touching_executable() {
    use sha2::Sha256;
    let payload = b"MZ fixture executable";
    let (url, rx, http) = mock(vec![(200, vec![], payload.to_vec())]);
    let temp = Temp::new();
    let release = crate::updater::Release {
        version: "99.0.0".into(),
        url: url.clone(),
        sha256: format!("{:x}", Sha256::digest(payload)),
    };
    let staged = crate::updater::stage(&api::Api::testing(url), &release, &temp.0).unwrap();
    assert_eq!(fs::read(staged).unwrap(), payload);
    rx.recv().unwrap();
    http.join().unwrap();
}
#[test]
fn corrupt_update_is_rejected() {
    let (url, rx, http) = mock(vec![(200, vec![], b"MZ corrupt executable".to_vec())]);
    let temp = Temp::new();
    let release = crate::updater::Release {
        version: "99.0.0".into(),
        url: url.clone(),
        sha256: "0".repeat(64),
    };
    assert!(crate::updater::stage(&api::Api::testing(url), &release, &temp.0).is_err());
    assert!(!temp.0.join("updates/ExboLauncherNew.new.exe").exists());
    rx.recv().unwrap();
    http.join().unwrap();
}

#[test]
fn p2p_transfer_uses_original_infohash_and_read_only_seed() {
    use librqbit::{
        AddTorrent, AddTorrentOptions, ListenerOptions, Session, SessionOptions,
        storage::StorageFactoryExt,
    };
    let temp = Temp::new();
    let seed = temp.0.join("seed");
    let target = temp.0.join("target");
    fs::create_dir_all(seed.join("a")).unwrap();
    fs::create_dir_all(seed.join("b")).unwrap();
    fs::write(seed.join("a/one.bin"), b"same").unwrap();
    fs::write(seed.join("b/two.bin"), b"same").unwrap();
    let meta = fixture(&["a/one.bin", "b/two.bin"], b"same");
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let factory = crate::seeding::ReadOnlyFactory {
            root: seed.clone(),
            paths: Arc::new(meta.files.iter().map(|f| f.path.clone()).collect()),
            control: download::Control::default(),
        };
        let listener = ListenerOptions {
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            ipv4_only: true,
            ..Default::default()
        };
        let s = Session::new_with_opts(
            seed.clone(),
            SessionOptions {
                dht: None,
                listen: Some(listener),
                disable_trackers: true,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let sh = s
            .add_torrent(
                AddTorrent::from_bytes(meta.torrent()),
                Some(AddTorrentOptions {
                    output_folder: Some(seed.to_string_lossy().into()),
                    overwrite: true,
                    only_files: Some(vec![]),
                    storage_factory: Some(factory.boxed()),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_handle()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while matches!(
            sh.stats().state,
            librqbit::TorrentStatsState::Initializing { .. }
        ) {
            assert!(std::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(sh.stats().error.is_none(), "{:?}", sh.stats());
        let l = Session::new_with_opts(
            target.clone(),
            SessionOptions {
                dht: None,
                disable_trackers: true,
                disable_local_service_discovery: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let lh = l
            .add_torrent(
                AddTorrent::from_bytes(meta.torrent()),
                Some(AddTorrentOptions {
                    output_folder: Some(target.to_string_lossy().into()),
                    overwrite: true,
                    initial_peers: Some(vec![s.listen_addr().unwrap()]),
                    ..Default::default()
                }),
            )
            .await
            .unwrap()
            .into_handle()
            .unwrap();
        let completed =
            tokio::time::timeout(Duration::from_secs(15), lh.wait_until_completed()).await;
        l.stop().await;
        s.stop().await;
        completed.unwrap().unwrap();
    });
    assert_eq!(fs::read(target.join("a/one.bin")).unwrap(), b"same");
    assert_eq!(fs::read(target.join("b/two.bin")).unwrap(), b"same");
    assert_eq!(fs::read(seed.join("a/one.bin")).unwrap(), b"same");
}

#[test]
fn news_json_accepts_original_image_link_and_null() {
    let a: crate::model::News = serde_json::from_value(
        serde_json::json!({"title":"News","image":"default/default.png","image_link":null}),
    )
    .unwrap();
    assert!(a.image_link.is_empty());
    let b: crate::model::News =
        serde_json::from_value(serde_json::json!({"image_link":"https://example.org/news.png"}))
            .unwrap();
    assert_eq!(b.image_link, "https://example.org/news.png");
}
