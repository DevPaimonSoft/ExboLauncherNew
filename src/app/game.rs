use super::*;

impl App {
    pub(super) fn refresh(&mut self, ui: &LauncherWindow) -> Result<()> {
        ensure!(
            !self.busy && self.control.is_none() && self.child.is_none(),
            "Клиент занят"
        );
        let api = self.api()?;
        let (login, token) = self.credentials()?;
        let tx = self.sender.clone();
        self.busy(ui, true);
        ui.set_status("Получение серверов…".into());
        std::thread::spawn(move || {
            let _ = tx.send(Event::Servers(api.servers(&login, &token)));
        });
        Ok(())
    }

    pub(super) fn apply_servers(
        &mut self,
        ui: &LauncherWindow,
        servers: Vec<Server>,
    ) -> Result<()> {
        for s in &servers {
            storage::safe_id(&s.id)?;
        }
        self.store.cache_servers(&servers)?;
        self.servers = servers;
        let rows = self
            .servers
            .iter()
            .map(|s| ServerRow {
                id: s.id.clone().into(),
                caption: s.caption.clone().into(),
                online: format!("{} / {} online", s.online_current, s.online_max).into(),
            })
            .collect::<Vec<_>>();
        ui.set_servers(Rc::new(VecModel::from(rows)).into());
        let i = self
            .servers
            .iter()
            .position(|s| s.id == self.config.selected_server)
            .unwrap_or(0);
        ui.set_selected_server(i as i32);
        if !self.servers.is_empty() {
            self.select(ui, i)?;
        } else {
            self.seeder = None;
            ui.set_server_title("Нет доступных серверов".into());
            ui.set_server_description("".into());
            ui.set_server_online("".into());
            ui.set_install_path("".into());
            ui.set_installed(false);
            self.news.clear();
            ui.set_news(Rc::new(VecModel::<NewsRow>::default()).into());
        }
        ui.set_status("Готов к работе".into());
        Ok(())
    }

    pub(super) fn select(&mut self, ui: &LauncherWindow, index: usize) -> Result<()> {
        ensure!(
            !self.busy && self.control.is_none() && self.child.is_none(),
            "Клиент занят"
        );
        self.seeder = None;
        let server = self.servers.get(index).context("Сервер не найден")?.clone();
        self.config.selected_server = server.id.clone();
        self.store.save(&self.config)?;
        ui.set_selected_server(index as i32);
        ui.set_server_title(server.caption.clone().into());
        ui.set_server_description(server.desc.join("\n").into());
        ui.set_server_online(
            format!("{} / {} online", server.online_current, server.online_max).into(),
        );
        let path = self
            .config
            .clients
            .get(&server.id)
            .map(PathBuf::from)
            .unwrap_or_else(|| self.store.root.join("games").join(&server.id));
        ui.set_install_path(path.to_string_lossy().into_owned().into());
        ui.set_installed(self.config.clients.contains_key(&server.id) && path.is_dir());
        ui.set_progress(0.);
        ui.set_progress_label("".into());
        ui.set_error_message("".into());
        ui.set_server_background(ui.get_fallback_background());
        self.news.clear();
        self.news_generation += 1;
        ui.set_news(Rc::new(VecModel::<NewsRow>::default()).into());
        if !server.background.is_empty() {
            let api = self.api()?;
            let tx = self.sender.clone();
            let cache = self.store.root.join("images");
            std::thread::spawn(move || {
                if let Ok(bytes) = api.external_bytes(&server.background, 16 * 1024 * 1024) {
                    let digest = api::java_hex(&sha1::Sha1::digest(&bytes));
                    let path = cache.join(format!("{}-{digest}.png", server.id));
                    if storage::atomic_write(&path, &bytes).is_ok() {
                        let _ = tx.send(Event::Image(server.id, path));
                    }
                }
            });
        }
        Ok(())
    }

    pub(super) fn news(&mut self, ui: &LauncherWindow) -> Result<()> {
        let server = self.selected(ui)?;
        let api = self.api()?;
        let tx = self.sender.clone();
        let id = server.id.clone();
        self.news_generation += 1;
        let generation = self.news_generation;
        std::thread::spawn(move || {
            let _ = tx.send(Event::News(id, generation, api.news(&server)));
        });
        Ok(())
    }

    pub(super) fn start_seed(&mut self, ui: &LauncherWindow) -> Result<()> {
        self.seeder = None;
        if self.config.settings.p2p == 2 {
            return Ok(());
        }
        let server = self.selected(ui)?;
        let bytes = fs::read(self.store.metadata_path(&server.id)?)?;
        let meta = download::Metadata::parse(&bytes)?;
        let root = PathBuf::from(ui.get_install_path().to_string());
        let tx = self.sender.clone();
        self.seeder = Some(seeding::Seeder::start(
            meta,
            root,
            self.config.settings.clone(),
            Arc::new(move |bytes| {
                let _ = tx.send(Event::SeedUploaded(bytes));
            }),
        )?);
        Ok(())
    }

    pub(super) fn begin_download(
        &mut self,
        ui: &LauncherWindow,
        play: bool,
        rewrite: bool,
    ) -> Result<()> {
        ensure!(
            !self.busy && self.control.is_none() && self.child.is_none(),
            "Клиент занят"
        );
        self.seeder = None;
        ensure!(
            !ui.get_license_required(),
            "Сначала примите лицензионное соглашение"
        );
        let server = self.selected(ui)?;
        let api = self.api()?;
        let (login, token) = self.credentials()?;
        let root = PathBuf::from(ui.get_install_path().to_string());
        ensure!(
            root.is_absolute() && root.components().count() > 2,
            "Укажите полный путь к каталогу игры"
        );
        self.config
            .clients
            .insert(server.id.clone(), root.to_string_lossy().into());
        self.store.save(&self.config)?;
        let settings = self.config.settings.clone();
        let store = self.store.clone();
        let legacy = PathBuf::from(&self.config.legacy_root);
        let control = download::Control::default();
        self.control = Some(control.clone());
        let tx = self.sender.clone();
        let progress_tx = tx.clone();
        let throttle = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
        let reporter: download::Reporter = Arc::new(move |p| {
            let mut last = throttle.lock().unwrap();
            if last.elapsed() >= Duration::from_millis(100) || p.done >= p.total {
                let _ = progress_tx.send(Event::Progress(p));
                *last = Instant::now();
            }
        });
        ui.set_downloading(true);
        ui.set_paused(false);
        ui.set_progress(0.);
        ui.set_progress_label("".into());
        ui.set_error_message("".into());
        ui.set_status("Проверка метаданных…".into());
        std::thread::spawn(move || {
            if settings.defender {
                let result = platform::defender_status(&root);
                let _ = tx.send(Event::DefenderStatus(result, false));
            }
            let outcome = (|| {
                let meta = download::metadata(&api, &store, &legacy, &server, &login, &token)?;
                download::update(
                    &api, &store, &server, &meta, &root, &settings, &control, &reporter, rewrite,
                )
            })();
            let _ = tx.send(Event::DownloadDone(outcome, play));
        });
        Ok(())
    }

    pub(super) fn play(&mut self, ui: &LauncherWindow) -> Result<()> {
        ensure!(
            !self.busy && self.control.is_none() && self.child.is_none(),
            "Клиент занят"
        );
        ensure!(
            !ui.get_license_required(),
            "Сначала примите лицензионное соглашение"
        );
        let api = self.api()?;
        let (login, token) = self.credentials()?;
        let server = self.selected(ui)?;
        let root = PathBuf::from(ui.get_install_path().to_string());
        ensure!(
            root.is_absolute() && root.components().count() > 2 && root.is_dir(),
            "Укажите существующий каталог установленной игры"
        );
        self.config
            .clients
            .insert(server.id.clone(), root.to_string_lossy().into_owned());
        self.store.save(&self.config)?;
        let display = ui.get_display_login().to_string();
        let settings = self.config.settings.clone();
        let logs = self.store.root.join("logs").join(&server.id);
        let tx = self.sender.clone();
        let id = server.id.clone();
        let version = server.version.clone();
        self.busy(ui, true);
        ui.set_status("Запуск клиента…".into());
        std::thread::spawn(move || {
            let result = (|| {
                let session = api.join(&login, &token)?;
                launch::start(
                    server
                        .run_command
                        .as_deref()
                        .context("Сервер временно недоступен")?,
                    &root,
                    &login,
                    &display,
                    &session,
                    &settings,
                    &logs,
                )
            })();
            let _ = tx.send(Event::Started(result, id, version));
        });
        Ok(())
    }
}
