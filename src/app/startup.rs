use super::*;

pub(crate) fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--apply-update") {
        return updater::apply(&args);
    }
    if let Some(i) = args.iter().position(|a| a == "--inspect-manifest") {
        let bytes = fs::read(args.get(i + 1).context("Укажите файл метаданных")?)?;
        let meta = download::Metadata::parse(&bytes)?;
        let torrent = meta.torrent();
        let parsed = librqbit::torrent_from_bytes(&torrent)?;
        println!(
            "files={}; total={}; infohash={}",
            meta.files.len(),
            meta.total,
            parsed.info_hash.as_string()
        );
        return Ok(());
    }
    let preview = args
        .iter()
        .position(|a| a == "--preview-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    let data = args
        .iter()
        .position(|a| a == "--data-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                .join("ExboLauncherNew")
        });
    let store = storage::Store::new(data)?;
    if args.iter().any(|arg| arg == "--register-protocol") {
        protocol::register(&store.root)?;
        println!("EXBO_PROTOCOL_OK: EXBO Launcher New");
        return Ok(());
    }
    let link = protocol::link_argument(&args)?;
    #[cfg(windows)]
    let _instance = match platform::InstanceGuard::acquire(&store.root)? {
        Some(guard) => guard,
        None => {
            let port = fs::read_to_string(store.root.join("ipc-port"))?.parse()?;
            ipc::forward(port, link.as_deref().unwrap_or("exbo://wake"))?;
            return Ok(());
        }
    };
    let mut config = store.load_with_import(!args.iter().any(|a| a == "--no-legacy-import"))?;
    if let Some(id) = args
        .iter()
        .position(|a| a == "--server")
        .and_then(|i| args.get(i + 1))
    {
        storage::safe_id(id)?;
        config.selected_server = id.clone();
    }
    if args.iter().any(|a| a == "--diagnose") {
        let bytes = platform::hwid(&store.root)?;
        println!(
            "HWID provider: OK ({} bytes); clients: {}; accounts: {}; embedded bootstrap: {}; legacy installation required: false",
            bytes.len() / 2,
            config.clients.len(),
            config.accounts.len(),
            !support::bootstrap_hash().is_empty()
        );
        return Ok(());
    }
    slint::platform::set_platform(Box::new(
        i_slint_backend_winit::Backend::new_with_renderer_by_name(
            preview.as_ref().map(|_| "software"),
        )?,
    ))
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ui = LauncherWindow::new()?;
    ui.set_app_version(env!("CARGO_PKG_VERSION").into());
    set_window_size(&ui, 1180., 780.);
    let (tx, rx) = mpsc::channel();
    let (link_tx, link_rx) = mpsc::channel();
    let ipc = ipc::Ipc::start(link_tx)?;
    storage::atomic_write(
        &store.root.join("ipc-port"),
        ipc.port.to_string().as_bytes(),
    )?;
    let api = if preview.is_some() {
        Err(anyhow::anyhow!("Preview"))
    } else {
        api::Api::new(&config, &store.root)
    };
    if preview.is_none()
        && let Err(e) = &api
    {
        error(&ui, e);
    }
    let mut tray = if preview.is_none() {
        Some(tray())
    } else {
        None
    };
    let tray_available = tray
        .as_ref()
        .and_then(|t| t.as_ref().ok())
        .is_some_and(|(icon, _, _)| icon.rect().is_some());
    let app = Rc::new(RefCell::new(App {
        hardware: optimization::Hardware::detect(),
        opt_targets: Vec::new(),
        opt_result_region: String::new(),
        opt_search: None,
        opt_generation: 0,
        opt_round_complete: false,
        store,
        config: config.clone(),
        api: api.ok(),
        auth: None,
        pending: None,
        pending_password: String::new(),
        servers: Vec::new(),
        news: Vec::new(),
        sender: tx,
        control: None,
        child: None,
        socket_port: ipc.port,
        busy: false,
        tray_available,
        update: None,
        polling: false,
        seeder: None,
        auth_generation: 0,
        news_generation: 0,
        quit_requested: false,
    }));
    ui.set_login(config.active_account.clone().into());
    ui.set_auto_login(config.auto_login);
    ui.set_legacy_root(config.legacy_root.clone().into());
    ui.set_update_feed(config.settings.update_feed.clone().into());
    ui.set_en(config.settings.language == "en");
    ui.set_ram(config.settings.ram as f32);
    ui.set_tray(config.settings.tray);
    ui.set_autostart(config.settings.run_on_start);
    ui.set_start_on_downloaded(config.settings.start_on_downloaded);
    ui.set_verify_files_on_launch(config.settings.verify_files_on_launch);
    ui.set_defender(config.settings.defender);
    ui.set_p2p(config.settings.p2p as i32);
    ui.set_exit_mode(config.settings.exit_new as i32);
    ui.set_download_limit(config.settings.download.to_string().into());
    ui.set_upload_limit(config.settings.upload.to_string().into());
    optimization_panel::fill(&ui, &config.settings.optimization, app.borrow().hardware);
    ui.set_opt_applied_active(config.settings.optimization.enabled);
    let weak = ui.as_weak();
    ui.on_opt_edit(move |index, value, checked| {
        if let Some(ui) = weak.upgrade() {
            optimization_panel::edit(&ui, index.max(0) as usize, value, checked);
        }
    });
    app.borrow().accounts(&ui);
    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_action(move |action| {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(e) = a.borrow_mut().action(&ui, &action) {
            error(&ui, e);
        }
    });
    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_select_server(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        if let Err(e) = a.borrow_mut().select(&ui, index.max(0) as usize) {
            error(&ui, e);
        }
    });
    let weak = ui.as_weak();
    ui.on_drag_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().with_winit_window(|w| {
                let _ = w.drag_window();
            });
        }
    });
    let weak = ui.as_weak();
    ui.on_minimize_window(move || {
        if let Some(ui) = weak.upgrade() {
            ui.window().set_minimized(true);
        }
    });
    let weak = ui.as_weak();
    ui.on_maximize_window(move || {
        if let Some(ui) = weak.upgrade() {
            let w = ui.window();
            w.set_maximized(!w.is_maximized());
        }
    });
    let weak = ui.as_weak();
    let a = app.clone();
    ui.on_close_window(move || {
        if let Some(ui) = weak.upgrade() {
            if a.borrow().config.settings.tray && a.borrow().tray_available {
                let _ = ui.hide();
            } else {
                a.borrow_mut().quit_requested = true;
            }
        }
    });
    let a = app.clone();
    ui.window().on_close_requested(move || {
        if !a.borrow().config.settings.tray || !a.borrow().tray_available {
            a.borrow_mut().quit_requested = true;
        }
        slint::CloseRequestResponse::HideWindow
    });
    let timer = slint::Timer::default();
    let weak = ui.as_weak();
    let a = app.clone();
    let last_poll = std::cell::Cell::new(Instant::now());
    let last_tray_check = std::cell::Cell::new(Instant::now());
    let update_checked = std::cell::Cell::new(false);
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(50),
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut app = a.borrow_mut();
            if last_tray_check.get().elapsed() >= Duration::from_secs(1) {
                app.tray_available = tray
                    .as_ref()
                    .and_then(|t| t.as_ref().ok())
                    .is_some_and(|(icon, _, _)| icon.rect().is_some());
                last_tray_check.set(Instant::now());
            }
            while let Ok(event) = rx.try_recv() {
                if let Err(e) = app.event(&ui, event) {
                    error(&ui, e);
                }
            }
            while let Ok(link) = link_rx.try_recv() {
                restore_window(&ui);
                if let Err(e) = app.link(&ui, &link) {
                    error(&ui, e);
                }
            }
            if let Some(child) = &mut app.child
                && let Ok(Some(exit)) = child.try_wait()
            {
                app.child = None;
                if app.config.settings.exit_new == 1 {
                    app.quit_requested = true;
                }
                ui.set_running(false);
                ui.set_status(format!("Игра завершена: {exit}").into());
                let _ = ui.show();
                ui.window().set_minimized(false);
            }
            let memory = if app.config.settings.optimization.enabled {
                app.config
                    .settings
                    .optimization
                    .profile_for(app.hardware)
                    .heap_size_gb as u64
                    * 1024
            } else {
                platform::heap_size(ui.get_ram() as f64)
            };
            ui.set_ram_label(format!("{memory} MiB").into());
            if !update_checked.get()
                && !app.config.settings.update_feed.is_empty()
                && !app.busy
                && app.control.is_none()
                && app.child.is_none()
            {
                update_checked.set(true);
                if let Err(e) = app.action(&ui, "check-update") {
                    error(&ui, e);
                }
            }
            if last_poll.get().elapsed() >= Duration::from_secs(30)
                && app.auth.is_some()
                && !app.busy
                && !app.polling
                && app.control.is_none()
                && app.child.is_none()
                && let (Ok(api), Ok((login, token))) = (app.api(), app.credentials())
            {
                let tx = app.sender.clone();
                app.polling = true;
                let generation = app.auth_generation;
                last_poll.set(Instant::now());
                std::thread::spawn(move || {
                    let _ = tx.send(Event::ServerMeta(
                        generation,
                        api.server_meta(&login, &token),
                    ));
                });
            }
            if let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv()
                && let Some(Ok((_, show, quit))) = &tray
            {
                if event.id == *show {
                    restore_window(&ui);
                }
                if event.id == *quit {
                    if let Some(c) = &app.control {
                        c.cancel();
                    }
                    app.quit_requested = true;
                }
            }
            while let Ok(event) = tray_icon::TrayIconEvent::receiver().try_recv() {
                if restores_from_tray(&event) {
                    restore_window(&ui);
                }
            }
            if app.quit_requested {
                drop(tray.take());
                let _ = slint::quit_event_loop();
            }
        },
    );
    ui.show()?;
    if preview.is_none() {
        ui.window().with_winit_window(|w| {
            if let Some(monitor) = w.current_monitor() {
                let scale = ui.window().scale_factor() as f64;
                let size = monitor.size().to_logical::<f32>(scale);
                let width = (size.width - 48.).clamp(800., 1180.);
                let height = (size.height - 64.).clamp(480., 780.);
                set_window_size(&ui, width, height);
                let origin = monitor.position();
                ui.window().set_position(slint::PhysicalPosition::new(
                    origin.x + ((size.width - width) * scale as f32 / 2.) as i32,
                    origin.y + ((size.height - height) * scale as f32 / 2.) as i32,
                ));
            }
        });
    }
    let preview_timer = slint::Timer::default();
    if let Some(dir) = preview {
        fs::create_dir_all(&dir)?;
        if args.iter().any(|a| a == "--ui-smoke") {
            ui_preview::check(&mut app.borrow_mut(), &ui)?;
        }
        ui.set_error_message("".into());
        ui_preview::prepare(&ui, 0);
        let weak = ui.as_weak();
        let preview_app = app.clone();
        let frame = std::cell::Cell::new(0u8);
        preview_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(700),
            move || {
                let Some(ui) = weak.upgrade() else { return };
                let phase = frame.get();
                if phase.is_multiple_of(2) {
                    if matches!(phase / 2, 1 | 11 | 31) {
                        ui.window()
                            .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                                position: slint::LogicalPosition::new(
                                    700.,
                                    if phase / 2 == 31 { 520. } else { 300. },
                                ),
                                delta_x: 0.,
                                delta_y: -10000.,
                            });
                    }
                    ui.window().with_winit_window(|w| w.request_redraw());
                    frame.set(phase + 1);
                    return;
                }
                if let Ok(buffer) = ui.window().take_snapshot() {
                    let case = phase / 2;
                    let name = [
                        "auth.png",
                        "auth-min.png",
                        "mail-min.png",
                        "totp-min.png",
                        "email-min.png",
                        "email-warning-min.png",
                        "password-change-min.png",
                        "steam-min.png",
                        "main.png",
                        "main-min-license-error.png",
                        "settings.png",
                        "settings-min.png",
                        "news-min.png",
                        "confirm-min.png",
                        "main-compact.png",
                        "settings-compact.png",
                        "optimization.png",
                        "optimization-compact.png",
                        "optimization-gc.png",
                        "optimization-jit.png",
                        "optimization-network.png",
                        "optimization-help.png",
                        "optimization-en.png",
                        "optimization-help-compact.png",
                        "optimization-cpu.png",
                        "optimization-cpu-compact.png",
                        "optimization-cpu-en.png",
                        "optimization-cpu-default.png",
                        "optimization-network-series.png",
                        "optimization-network-pending.png",
                        "optimization-network-outlier.png",
                        "optimization-network-last.png",
                    ][case as usize];
                    let mut pixels = buffer.as_bytes().to_vec();
                    for pixel in pixels.chunks_exact_mut(4) {
                        pixel[3] = 255;
                    }
                    let _ = ::image::save_buffer(
                        dir.join(name),
                        &pixels,
                        buffer.width(),
                        buffer.height(),
                        ::image::ColorType::Rgba8,
                    );
                }
                let next = phase / 2 + 1;
                if next < ui_preview::COUNT {
                    ui_preview::prepare(&ui, next);
                } else {
                    preview_app.borrow_mut().quit_requested = true;
                }
                frame.set(phase + 1);
            },
        );
    } else if config.auto_login
        && !config.active_account.is_empty()
        && let Err(e) = app.borrow_mut().auth_request(&ui, true)
    {
        error(&ui, e);
    }
    if let Some(link) = link
        && let Err(e) = app.borrow_mut().link(&ui, &link)
    {
        error(&ui, e);
    }
    if args.iter().any(|a| a == "--autostart") && tray_available {
        ui.hide()?;
    }
    slint::run_event_loop_until_quit()?;
    if let Some(c) = &app.borrow().control {
        c.cancel();
    }
    timer.stop();
    drop(ipc);
    Ok(())
}
