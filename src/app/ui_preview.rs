use super::*;
use slint::Model;

pub fn check(app: &mut App, ui: &LauncherWindow) -> Result<()> {
    let server = Server {
        id: "qa".into(),
        caption: "STALCRAFT: X".into(),
        ..Default::default()
    };
    app.apply_servers(ui, vec![server.clone()])?;
    app.control = Some(download::Control::default());
    ui.set_downloading(true);
    ensure!(
        app.refresh(ui).is_err(),
        "Refresh changed the active download"
    );
    ensure!(app.select(ui, 0).is_err(), "Server changed during download");
    for action in [
        "refresh",
        "logout",
        "browse",
        "repair",
        "delete",
        "save-settings",
        "rust-update",
    ] {
        ensure!(
            app.action(ui, action).is_err(),
            "{action} allowed during download"
        );
    }
    let result = app.event(
        ui,
        Event::DownloadDone(Err(anyhow::anyhow!("fixture failure")), false),
    );
    ensure!(
        result.is_err() && app.control.is_none() && !ui.get_downloading() && !ui.get_paused(),
        "Download error left stale controls"
    );
    app.busy(ui, true);
    ensure!(
        app.action(ui, "delete").is_err(),
        "Delete allowed during another task"
    );
    app.busy(ui, false);
    app.auth_generation = 2;
    app.auth = Some(AuthResult {
        kind: "OK".into(),
        true_login: Some("QA".into()),
        ..Default::default()
    });
    app.event(
        ui,
        Event::ServerMeta(1, Err(anyhow::anyhow!("Сессия истекла"))),
    )?;
    ensure!(
        app.auth.is_some(),
        "Stale session response signed out current account"
    );
    app.news_generation = 2;
    app.news = vec![News {
        title: "current".into(),
        ..Default::default()
    }];
    app.event(ui, Event::News("qa".into(), 1, Ok(vec![])))?;
    ensure!(
        app.news.len() == 1,
        "Stale news response replaced current list"
    );
    app.apply_servers(ui, vec![])?;
    ensure!(
        !ui.get_installed() && ui.get_install_path().is_empty(),
        "Empty server list retained install path"
    );
    app.auth = None;
    app.apply_servers(ui, vec![server])?;
    let original = app.config.clone();
    ui.set_legacy_root("".into());
    ui.set_autostart(
        platform::registry_string(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "ExboLauncherNew",
        )
        .is_some(),
    );
    app.save_settings(ui)?;
    ensure!(
        app.config.legacy_root.is_empty(),
        "Standalone settings require legacy installation"
    );
    app.optimization_action(ui, "opt-auto")?;
    let heap = ui
        .get_opt_rows()
        .iter()
        .position(|r| r.key == "heap_size_gb")
        .context("Heap control missing")?;
    ui.invoke_opt_edit(heap as i32, "3".into(), false);
    app.optimization_action(ui, "opt-save")?;
    ensure!(
        app.store
            .load()?
            .settings
            .optimization
            .profile
            .context("Profile not saved")?
            .heap_size_gb
            == 3,
        "Optimization save lost edited value"
    );
    ui.invoke_opt_edit(heap as i32, "invalid".into(), false);
    ensure!(
        app.optimization_action(ui, "opt-save").is_err(),
        "Invalid optimization was saved"
    );
    ensure!(
        app.store
            .load()?
            .settings
            .optimization
            .profile
            .context("Profile lost")?
            .heap_size_gb
            == 3,
        "Invalid edit overwrote saved profile"
    );
    app.optimization_action(ui, "opt-revert")?;
    ensure!(
        ui.get_opt_rows().row_data(heap).unwrap().value == "3",
        "Revert did not restore saved value"
    );
    if ui.get_opt_cpu_supported() {
        ui.set_opt_cpu_active(true);
        app.optimization_action(ui, "opt-cpu-clear")?;
        ensure!(
            app.optimization_action(ui, "opt-save").is_err(),
            "Empty CPU selection was saved"
        );
        app.optimization_action(ui, "opt-cpu:0")?;
        let selected = ui
            .get_opt_cpus()
            .row_data(0)
            .context("CPU controls missing")?
            .id as u16;
        app.optimization_action(ui, "opt-save")?;
        let saved = app.store.load()?.settings.optimization;
        ensure!(
            saved.cpu_affinity == Some(vec![selected]),
            "CPU selection not saved"
        );
        let bytes = saved.export(false, app.hardware)?;
        let imported = optimization::Settings::import(&bytes, &Default::default(), "CPU")?;
        optimization_panel::fill(ui, &imported, app.hardware);
        ensure!(
            ui.get_opt_cpu_active()
                && ui.get_opt_cpus().iter().filter(|cpu| cpu.checked).count() == 1,
            "CPU import lost selection"
        );
        app.optimization_action(ui, "opt-cpu-all")?;
        app.optimization_action(ui, "opt-revert")?;
        ensure!(
            ui.get_opt_cpus().iter().filter(|cpu| cpu.checked).count() == 1,
            "CPU revert lost saved selection"
        );
        ui.set_opt_cpu_active(false);
        app.optimization_action(ui, "opt-save")?;
        ensure!(
            app.store
                .load()?
                .settings
                .optimization
                .cpu_affinity
                .is_none(),
            "Disabling CPU restriction failed"
        );
    }
    app.opt_generation = 50;
    app.opt_round_complete = false;
    let search = tunnel::Control::default();
    app.opt_search = Some(search.clone());
    ui.set_opt_network_busy(true);
    app.event(
        ui,
        Event::Tunnels(49, Box::new(Err(anyhow::anyhow!("stale error")))),
    )?;
    ensure!(ui.get_opt_network_busy(), "Stale result ended a new search");
    app.event(
        ui,
        Event::TunnelProgress(
            49,
            Box::new(tunnel::Progress {
                done: 1,
                total: 1,
                entry: None,
            }),
        ),
    )?;
    ensure!(
        app.optimization_action(ui, "opt-tunnel-best").is_err(),
        "Provisional best was selected"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let entries = (0..31)
        .map(|index| {
            let samples = vec![
                tunnel::Sample {
                    time: now,
                    client: (index + 1) as f64,
                    server: 5.,
                    failed: false
                };
                if index == 0 { 1 } else { 10 }
            ];
            tunnel::rank(
                tunnel::Target {
                    pool: "qa".into(),
                    endpoint: tunnel::Endpoint {
                        name: format!("Node {index}"),
                        address: format!("node{index}.example:2000"),
                    },
                },
                &samples,
                0,
                1.,
                true,
                false,
            )
        })
        .collect();
    ui.set_opt_region(0);
    app.event(
        ui,
        Event::Tunnels(
            50,
            Box::new(Ok(tunnel::Results {
                region: "ru".into(),
                entries,
                cache: Default::default(),
            })),
        ),
    )?;
    ensure!(
        app.opt_targets.len() == 31 && ui.get_opt_tunnels().row_count() == 31,
        "Tunnel list was truncated"
    );
    ensure!(ui.get_opt_network_best(), "Completed best is disabled");
    app.optimization_action(ui, "opt-tunnel-best")?;
    ensure!(
        ui.get_opt_rows()
            .iter()
            .any(|r| r.key == "tunnel:ru" && r.value == "node1.example:2000"),
        "Best selected an under-sampled candidate"
    );
    ui.set_opt_search_mode(2);
    app.optimization_action(ui, "opt-mode")?;
    ensure!(
        ui.get_opt_tunnels().row_count() == 31,
        "Re-ranking lost results"
    );
    let previous_generation = app.opt_generation;
    app.opt_search = Some(search.clone());
    app.optimization_action(ui, "opt-cancel")?;
    ensure!(
        search.cancelled() && !ui.get_opt_network_busy() && !ui.get_opt_network_best(),
        "Cancellation did not reset controls"
    );
    app.event(
        ui,
        Event::Tunnels(
            previous_generation,
            Box::new(Err(anyhow::anyhow!("late cancellation"))),
        ),
    )?;
    ensure!(
        app.optimization_action(ui, "opt-tunnel:0").is_err(),
        "Cancelled provisional result was selectable"
    );
    app.opt_targets.clear();
    app.opt_result_region.clear();
    app.config = original;
    ui.set_autostart(app.config.settings.run_on_start);
    app.store.save(&app.config)?;
    optimization_panel::fill(ui, &app.config.settings.optimization, app.hardware);
    ui.set_opt_applied_active(app.config.settings.optimization.enabled);
    ui.set_error_message("".into());
    ui.set_page("auth".into());
    ui.set_auth_step("password".into());
    ui.set_status("Offline UI checks passed".into());
    println!(
        "UI_SMOKE_OK: busy guards, cancellation/error reset, stale responses, empty server list; optimization edit/save/restart/invalid/revert; CPU select/save/import/revert/disable; tunnel full-list/best/freshness/cancel/stale-event guards; standalone settings"
    );
    Ok(())
}

pub const COUNT: u8 = 32;

pub fn prepare(ui: &LauncherWindow, case: u8) -> &'static str {
    set_window_size(
        ui,
        if case == 0 || case == 8 || case == 10 || case == 16 {
            1180.
        } else {
            900.
        },
        if case == 0 || case == 8 || case == 10 || case == 16 {
            780.
        } else {
            600.
        },
    );
    ui.set_page("auth".into());
    ui.set_tab("overview".into());
    ui.set_auth_step("password".into());
    ui.set_error_message("".into());
    ui.set_license_required(false);
    ui.set_downloading(false);
    ui.set_en(false);
    ui.set_opt_network_busy(false);
    ui.set_opt_network_best(false);
    ui.set_status("Готов к работе".into());
    ui.set_progress_label("".into());
    ui.set_progress(0.);
    ui.set_confirm_action("".into());
    ui.set_accounts(strings(vec!["QA Player".into(), "Second account".into()]));
    ui.set_selected_account("QA Player".into());
    ui.set_new_password("".into());
    match case {
        0 => "auth.png",
        1 => {
            ui.set_error_message("Ошибка соединения с сервером. Проверьте подключение и повторите попытку. Дополнительные сведения о причине ошибки отображаются здесь.".into());
            "auth-min.png"
        }
        2 => {
            ui.set_auth_step("mail".into());
            "mail-min.png"
        }
        3 => {
            ui.set_auth_step("totp".into());
            "totp-min.png"
        }
        4 => {
            ui.set_auth_step("email".into());
            "email-min.png"
        }
        5 => {
            ui.set_auth_step("email-warning".into());
            ui.set_email_warning("Запрошена смена почты на example@example.org. До завершения: 5 дн. Подтвердите, что изменение выполнено вами.".into());
            "email-warning-min.png"
        }
        6 => {
            ui.set_auth_step("password-change".into());
            "password-change-min.png"
        }
        7 => {
            ui.set_auth_step("distributor".into());
            "steam-min.png"
        }
        8..=15 => {
            ui.set_page("main".into());
            ui.set_server_title("STALCRAFT: X".into());
            ui.set_server_description("Исследуй Зону. Найди свой путь.".into());
            ui.set_display_login("EXBO Player".into());
            ui.set_server_online("9999 / 10000 online".into());
            ui.set_installed(true);
            ui.set_progress(1.);
            ui.set_install_path(r"C:\Games\STALCRAFT".into());
            ui.set_servers(
                Rc::new(VecModel::from(
                    (0..12)
                        .map(|i| ServerRow {
                            id: format!("qa-{i}").into(),
                            caption: format!("STALCRAFT: X — сервер {i}").into(),
                            online: "9999 / 10000 online".into(),
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            match case {
                8 => "main.png",
                9 => {
                    ui.set_license_required(true);
                    ui.set_downloading(true);
                    ui.set_progress(0.5);
                    ui.set_progress_label("Загрузка: 7 B / 14 B · 1.00 KiB/s".into());
                    ui.set_error_message("Тестовая длинная ошибка: сервер недоступен. Повторите попытку позже или проверьте настройки подключения.".into());
                    "main-min-license-error.png"
                }
                10 => {
                    ui.set_tab("settings".into());
                    "settings.png"
                }
                11 => {
                    ui.set_tab("settings".into());
                    "settings-min.png"
                }
                12 => {
                    ui.set_tab("news".into());
                    ui.set_news(Rc::new(VecModel::from(vec![NewsRow { title: "Длинный заголовок новости о событиях и обновлениях игрового мира STALCRAFT: X".into(), body: "Большой текст новости. Подробности события, новые возможности и изменения игрового мира. ".repeat(6).into(), image: ui.get_news_fallback() }])).into());
                    "news-min.png"
                }
                13 => {
                    ui.set_confirm_action("repair".into());
                    ui.set_confirm_title("Восстановить все файлы?".into());
                    ui.set_confirm_body("Изменённые файлы, включая исключения, будут восстановлены из дистрибутива.".into());
                    "confirm-min.png"
                }
                14 => {
                    set_window_size(ui, 800., 480.);
                    ui.set_license_required(true);
                    ui.set_error_message("Проверка компоновки на небольшом экране".into());
                    "main-compact.png"
                }
                _ => {
                    set_window_size(ui, 800., 480.);
                    ui.set_tab("settings".into());
                    "settings-compact.png"
                }
            }
        }
        16..=23 => {
            ui.set_page("main".into());
            ui.set_tab("optimization".into());
            ui.set_opt_category(match case {
                18 => 1,
                19 => 2,
                20 => 3,
                21 | 23 => 4,
                _ => 0,
            });
            if case == 17 || case == 23 {
                set_window_size(ui, 800., 480.);
            }
            if case == 22 {
                ui.set_en(true);
            }
            if case == 20 {
                ui.set_opt_network_status("RU: показано 2 / 2. Измерено / measured.".into());
                ui.set_opt_tunnels(
                    Rc::new(VecModel::from(vec![
                        TunnelRow {
                            name: "Москва · Node 1".into(),
                            details:
                                "node.example:2000 · RTT 20 + 8 = 28 ms · jitter 1 ms · loss 0%"
                                    .into(),
                            available: true,
                        },
                        TunnelRow {
                            name: "Node 2".into(),
                            details: "Лимит подключений / Connection limit".into(),
                            available: false,
                        },
                    ]))
                    .into(),
                );
            }
            match case {
                16 => "optimization.png",
                17 => "optimization-compact.png",
                18 => "optimization-gc.png",
                19 => "optimization-jit.png",
                20 => "optimization-network.png",
                21 => "optimization-help.png",
                22 => "optimization-en.png",
                _ => "optimization-help-compact.png",
            }
        }
        24..=27 => {
            ui.set_page("main".into());
            ui.set_tab("optimization".into());
            ui.set_opt_category(5);
            ui.set_opt_cpu_supported(true);
            ui.set_opt_cpu_status("".into());
            ui.set_opt_cpu_active(case != 27);
            ui.set_opt_cpus(
                Rc::new(VecModel::from(
                    (0..16)
                        .map(|id| CpuRow {
                            id,
                            available: true,
                            checked: id >= 2,
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            if case == 25 {
                set_window_size(ui, 800., 480.);
            }
            if case == 26 {
                ui.set_en(true);
            }
            match case {
                24 => "optimization-cpu.png",
                25 => "optimization-cpu-compact.png",
                26 => "optimization-cpu-en.png",
                _ => "optimization-cpu-default.png",
            }
        }
        28..=31 => {
            ui.set_page("main".into());
            ui.set_tab("optimization".into());
            ui.set_opt_category(3);
            if case == 28 || case == 31 {
                set_window_size(ui, 1180., 780.);
            }
            if case == 29 {
                set_window_size(ui, 800., 480.);
            }
            ui.set_opt_network_busy(case == 29);
            ui.set_opt_network_best(case != 29);
            ui.set_en(case == 30);
            ui.set_opt_network_status(
                if case == 29 {
                    "Предварительно: 3 / 31 узлов · 10 проб на узел"
                } else {
                    "RU: 31 узел · 10 проб на узел. Все результаты доступны прокруткой."
                }
                .into(),
            );
            ui.set_opt_tunnels(Rc::new(VecModel::from((0..31).map(|index|TunnelRow{
                name:format!("Pool · Node {}",index+1).into(),
                details:if index==0 && case==30 {"Score 25.0 · node.example:2000 · median 25.0 ms · P90 25.0 ms · max 2000010.0 ms · replies 10/10 · loss 0% · last 10.0 + 2000000.0 ms · avg 10.0 + 200013.5 ms".into()}else{format!("Оценка / Score {:.1} · node{}.example:2000 · median 25.0 ms · P90 30.0 ms · max 35.0 ms · replies 10/10 · loss 0%{}",25.+index as f64,index+1,if case==29 {" · Предварительно / Provisional"}else{""}).into()},
                available:case!=29,
            }).collect::<Vec<_>>())).into());
            match case {
                28 => "optimization-network-series.png",
                29 => "optimization-network-pending.png",
                30 => "optimization-network-outlier.png",
                _ => "optimization-network-last.png",
            }
        }
        _ => unreachable!(),
    }
}
