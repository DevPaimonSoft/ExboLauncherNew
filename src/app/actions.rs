use super::*;

impl App {
    pub(super) fn action(&mut self, ui: &LauncherWindow, action: &str) -> Result<()> {
        ui.set_error_message("".into());
        let passive = matches!(
            action,
            "register"
                | "restore"
                | "support"
                | "license-open"
                | "discord"
                | "vk"
                | "youtube"
                | "website"
                | "folder"
                | "screenshots"
                | "crashes"
                | "logs"
        ) || action.starts_with("article:");
        ensure!(
            !self.busy || passive,
            "Дождитесь завершения текущей операции"
        );
        if action.starts_with("opt-") {
            return self.optimization_action(ui, action);
        }
        if matches!(
            action,
            "refresh"
                | "logout"
                | "browse"
                | "delete"
                | "repair"
                | "verify"
                | "install"
                | "play"
                | "rust-update"
                | "save-settings"
                | "defender-add"
                | "forget-account"
                | "check-update"
                | "launcher-update"
                | "defender-status"
                | "license"
        ) {
            ensure!(
                self.control.is_none() && self.child.is_none(),
                "Дождитесь завершения загрузки или игры"
            );
        }
        match action {
            "login" => self.auth_request(ui, false)?,
            "account" => self.auth_request(ui, true)?,
            "auth-submit" => self.auth_step(ui, false)?,
            "resend" => self.auth_step(ui, true)?,
            "auth-change-email" => {
                let pending = self.pending.as_ref().context("Нет данных авторизации")?;
                let token = pending.token.clone().context("Нет токена")?;
                let login = ui.get_login().to_string();
                let api = self.api()?;
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let _ = tx.send(Event::auth(api.mail_change(&login, &token)));
                });
            }
            "auth-back" => {
                self.pending = None;
                self.pending_password.clear();
                ui.set_auth_step("password".into());
                ui.set_code("".into());
                ui.set_new_password("".into());
                ui.set_email("".into());
            }
            "steam" => {
                ensure!(!self.busy, "Вход уже выполняется");
                let api = self.api()?;
                let tx = self.sender.clone();
                let port = self.socket_port;
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let _ = tx.send(Event::auth(api.distributor("steam", port, None)));
                });
            }
            "distributor-open" => {
                protocol::register(&self.store.root)?;
                platform::open_url(
                    self.pending
                        .as_ref()
                        .and_then(|r| r.login_url.as_deref())
                        .context("Нет ссылки входа")?,
                )?;
            }
            "register" => platform::open_url("https://exbo.ru/index.php?do=register")?,
            "restore" => platform::open_url("https://exbo.ru/index.php?do=lostpassword")?,
            "support" => platform::open_url("https://support.exbo.net")?,
            "license-open" => platform::open_url("https://exbo.ru/rules.html")?,
            "license" => {
                let api = self.api()?;
                let (login, token) = self.credentials()?;
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let _ = tx.send(Event::License(api.accept_license(&login, &token)));
                });
            }
            "refresh" => self.refresh(ui)?,
            "news" => self.news(ui)?,
            "browse" => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_directory(ui.get_install_path().as_str())
                    .pick_folder()
                {
                    ui.set_install_path(path.to_string_lossy().into_owned().into());
                }
            }
            "install" => {
                self.begin_download(ui, self.config.settings.start_on_downloaded, false)?
            }
            "play" => {
                let server = self.selected(ui)?;
                let installed = self.store.installed_version(&server.id)?;
                if self
                    .config
                    .settings
                    .verify_before_launch(installed.as_deref(), &server.version)
                    || !Path::new(ui.get_install_path().as_str()).is_dir()
                {
                    self.begin_download(ui, true, false)?;
                } else {
                    self.play(ui)?;
                }
            }
            "verify" => self.begin_download(ui, false, false)?,
            "repair" => self.begin_download(ui, false, true)?,
            "pause" => {
                if let Some(control) = &self.control {
                    ensure!(control.state() != 2, "Дождитесь остановки загрузки");
                    let paused = control.state() != 1;
                    control.pause(paused);
                    ui.set_paused(paused);
                    if paused {
                        ui.set_progress_label("Загрузка приостановлена · 0 B/s".into());
                    }
                    ui.set_status(
                        if paused {
                            "На паузе"
                        } else {
                            "Загрузка продолжена"
                        }
                        .into(),
                    );
                }
            }
            "cancel" => {
                if let Some(control) = &self.control {
                    control.cancel();
                    ui.set_status("Остановка загрузки…".into());
                }
            }
            "save-settings" => self.save_settings(ui)?,
            "check-update" => {
                ensure!(!self.busy, "Операция уже выполняется");
                let feed = self.config.settings.update_feed.clone();
                ensure!(
                    !feed.is_empty(),
                    "Укажите HTTPS URL манифеста обновлений Rust-лаунчера и сохраните настройки"
                );
                let api = self.api()?;
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let _ = tx.send(Event::UpdateChecked(updater::check(&api, &feed)));
                });
            }
            "rust-update" => {
                ensure!(
                    !self.busy && self.control.is_none() && self.child.is_none(),
                    "Закройте игру и завершите загрузки перед обновлением"
                );
                let release = self.update.clone().context("Нет доступного обновления")?;
                let api = self.api()?;
                let root = self.store.root.clone();
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let result = updater::stage(&api, &release, &root);
                    let _ = tx.send(Event::UpdateReady(result, release.sha256));
                });
            }
            "defender-status" => {
                let path = PathBuf::from(ui.get_install_path().to_string());
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let _ = tx.send(Event::DefenderStatus(
                        platform::defender_status(&path),
                        true,
                    ));
                });
            }
            "defender-add" => {
                let path = PathBuf::from(ui.get_install_path().to_string());
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let result = platform::add_defender_exclusion(&path)
                        .and_then(|_| platform::defender_status(&path));
                    let _ = tx.send(Event::DefenderStatus(result, true));
                });
            }
            "folder" | "screenshots" | "crashes" => {
                let mut p = PathBuf::from(ui.get_install_path().to_string());
                if action == "screenshots" {
                    p.push("screenshots");
                }
                if action == "crashes" {
                    p.push("crash-reports");
                }
                platform::open_path(&p)?;
            }
            "logs" => {
                let server = self.selected(ui)?;
                let p = self.store.root.join("logs").join(&server.id);
                fs::create_dir_all(&p)?;
                platform::open_path(&p)?;
            }
            "shortcut" => {
                let server = self.selected(ui)?;
                let desktop = rfd::FileDialog::new()
                    .set_title("Каталог для ярлыка")
                    .pick_folder();
                if let Some(p) = desktop {
                    platform::create_shortcut(&p, &server.id)?;
                    ui.set_status("Ярлык создан".into());
                }
            }
            "logout" => {
                self.seeder = None;
                ensure!(
                    self.control.is_none() && self.child.is_none(),
                    "Дождитесь завершения операции"
                );
                self.auth = None;
                self.auth_generation += 1;
                self.polling = false;
                self.news_generation += 1;
                self.pending = None;
                self.pending_password.clear();
                self.servers.clear();
                self.news.clear();
                ui.set_servers(Rc::new(VecModel::<ServerRow>::default()).into());
                ui.set_news(Rc::new(VecModel::<NewsRow>::default()).into());
                ui.set_installed(false);
                ui.set_progress(0.);
                ui.set_progress_label("".into());
                ui.set_page("auth".into());
                ui.set_auth_step("password".into());
                ui.set_password("".into());
                ui.set_login("".into());
                ui.set_license_required(false);
            }
            "forget-account" => {
                let login = ui.get_selected_account().to_string();
                self.config.accounts.retain(|a| a.login != login);
                if self.config.active_account == login {
                    self.config.active_account.clear();
                }
                self.store.save(&self.config)?;
                self.accounts(ui);
            }
            "delete" => {
                self.seeder = None;
                ensure!(
                    !self.busy && self.control.is_none() && self.child.is_none(),
                    "Клиент занят"
                );
                let api = self.api()?;
                let (login, token) = self.credentials()?;
                let server = self.selected(ui)?;
                let root = PathBuf::from(ui.get_install_path().to_string());
                let store = self.store.clone();
                let legacy = PathBuf::from(&self.config.legacy_root);
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let result = (|| {
                        let meta =
                            download::metadata(&api, &store, &legacy, &server, &login, &token)?;
                        download::remove_client(&root, &meta)
                    })();
                    let _ = tx.send(Event::Deleted(result));
                });
            }
            "launcher-update" => {
                let data = self.store.root.clone();
                let tx = self.sender.clone();
                self.busy(ui, true);
                std::thread::spawn(move || {
                    let result = (|| {
                        support::native_dir(&data)?;
                        ensure!(
                            !support::bootstrap_hash().is_empty(),
                            "Нет встроенного bootstrap"
                        );
                        Ok("Встроенные компоненты проверены и восстановлены. Официальная установка EXBO не требуется.".into())
                    })();
                    let _ = tx.send(Event::Message(result));
                });
            }
            "discord" | "vk" | "youtube" | "website" => {
                let server = self.selected(ui)?;
                let info = server.info.context("Ссылки сервера отсутствуют")?;
                let url = match action {
                    "discord" => info.discord,
                    "vk" => info.vk,
                    "youtube" => info.youtube,
                    _ => info.news_link,
                };
                ensure!(!url.is_empty(), "Ссылка отсутствует");
                platform::open_url(&url)?;
            }
            _ if action.starts_with("article:") => {
                let index: usize = action[8..].parse()?;
                let item = self.news.get(index).context("Новость не найдена")?;
                platform::open_url(&item.link)?;
            }
            _ => bail!("Неизвестное действие: {action}"),
        }
        Ok(())
    }
}
