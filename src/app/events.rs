use super::*;

impl App {
    pub(super) fn event(&mut self, ui: &LauncherWindow, event: Event) -> Result<()> {
        match event {
            Event::Auth(result) => {
                self.busy(ui, false);
                self.apply_auth(ui, (*result)?)?;
            }
            Event::Servers(result) => {
                self.busy(ui, false);
                match result {
                    Ok(servers) => self.apply_servers(ui, servers)?,
                    Err(e) => {
                        let cached = self.store.cached_servers();
                        if !cached.is_empty() {
                            self.apply_servers(ui, cached)?;
                            ui.set_status("Нет соединения · показан сохранённый список".into());
                        }
                        return Err(e);
                    }
                }
            }
            Event::ServerMeta(generation, result) => {
                if generation != self.auth_generation {
                    return Ok(());
                }
                self.polling = false;
                if self.auth.is_some() {
                    match result {
                        Ok(entries) => {
                            for meta in entries {
                                if let Some(server) =
                                    self.servers.iter_mut().find(|s| s.id == meta.id)
                                {
                                    if !self.busy && self.control.is_none() && self.child.is_none()
                                    {
                                        server.version = meta.version;
                                        server.run_command = meta.run_command;
                                    }
                                    server.online_current = meta.online_current;
                                    server.online_max = meta.online_max;
                                }
                            }
                            let rows = self
                                .servers
                                .iter()
                                .map(|s| ServerRow {
                                    id: s.id.clone().into(),
                                    caption: s.caption.clone().into(),
                                    online: format!(
                                        "{} / {} online",
                                        s.online_current, s.online_max
                                    )
                                    .into(),
                                })
                                .collect::<Vec<_>>();
                            ui.set_servers(Rc::new(VecModel::from(rows)).into());
                            if let Ok(s) = self.selected(ui) {
                                ui.set_server_online(
                                    format!("{} / {} online", s.online_current, s.online_max)
                                        .into(),
                                );
                            }
                        }
                        Err(e) => {
                            if e.to_string().contains("Сессия истекла") {
                                if self.control.is_some() || self.child.is_some() {
                                    return Err(e);
                                }
                                self.seeder = None;
                                self.auth = None;
                                self.auth_generation += 1;
                                self.pending = None;
                                self.pending_password.clear();
                                ui.set_downloading(false);
                                ui.set_page("auth".into());
                                ui.set_auth_step("password".into());
                                return Err(e);
                            }
                        }
                    }
                }
            }
            Event::News(id, generation, result) => {
                if self.config.selected_server == id
                    && self.news_generation == generation
                    && self.auth.is_some()
                {
                    self.news = result?;
                    let rows = self
                        .news
                        .iter()
                        .map(|n| NewsRow {
                            title: n.title.clone().into(),
                            body: launch::plain_news(&n.text).into(),
                            image: ui.get_news_fallback(),
                        })
                        .collect::<Vec<_>>();
                    ui.set_news(Rc::new(VecModel::from(rows)).into());
                    let api = self.api()?;
                    let tx = self.sender.clone();
                    let cache = self.store.root.join("images");
                    let news = self.news.clone();
                    std::thread::spawn(move || {
                        for (i, n) in news.iter().enumerate() {
                            let url = if n.image_link.is_empty() {
                                &n.image
                            } else {
                                &n.image_link
                            };
                            if let Ok(bytes) = api.external_bytes(url, 8 * 1024 * 1024) {
                                let digest = api::java_hex(&sha1::Sha1::digest(&bytes));
                                let p = cache.join(format!("{id}-news-{digest}.png"));
                                if storage::atomic_write(&p, &bytes).is_ok() {
                                    let _ = tx.send(Event::NewsImage(id.clone(), generation, i, p));
                                }
                            }
                        }
                    });
                }
            }
            Event::Image(id, path) => {
                if self.config.selected_server == id {
                    ui.set_server_background(image(&path));
                }
            }
            Event::NewsImage(id, generation, i, path) => {
                if self.config.selected_server == id
                    && self.news_generation == generation
                    && self.auth.is_some()
                {
                    use slint::Model;
                    let model = ui.get_news();
                    if let Some(mut row) = model.row_data(i) {
                        row.image = image(&path);
                        model.set_row_data(i, row);
                    }
                }
            }
            Event::Progress(p) => {
                if self.control.is_none() {
                    return Ok(());
                }
                let label = progress_label(&p, ui.get_paused());
                ui.set_progress(if p.total > 0 {
                    (p.done as f64 / p.total as f64).clamp(0., 1.) as f32
                } else {
                    0.
                });
                if !ui.get_paused() {
                    ui.set_status(p.message.into());
                }
                ui.set_progress_label(label.into());
            }
            Event::DownloadDone(result, play) => {
                self.control = None;
                ui.set_downloading(false);
                ui.set_paused(false);
                if let Err(e) = result {
                    ui.set_status("Операция остановлена".into());
                    return Err(e);
                }
                ui.set_installed(true);
                ui.set_progress(1.);
                ui.set_status("Клиент готов".into());
                let _ = self.start_seed(ui);
                if play {
                    self.play(ui)?;
                }
            }
            Event::Started(result, id, version) => {
                self.busy(ui, false);
                self.child = Some(result?);
                ui.set_running(true);
                ui.set_status("Игра запущена".into());
                if let Err(e) = self.store.record_installed_version(&id, &version) {
                    error(ui, e);
                }
                match self.config.settings.exit_new {
                    0 => {
                        self.quit_requested = true;
                    }
                    1 => {
                        ui.window().set_minimized(true);
                    }
                    _ => {}
                }
            }
            Event::Deleted(result) => {
                self.busy(ui, false);
                let count = result?;
                let id = self.selected(ui)?.id;
                self.config.clients.remove(&id);
                self.store.save(&self.config)?;
                ui.set_installed(false);
                ui.set_status(format!("Удалено файлов: {count}").into());
            }
            Event::License(result) => {
                self.busy(ui, false);
                result?;
                ui.set_license_required(false);
                if let Some(a) = &mut self.auth {
                    a.license_accepted = true;
                }
            }
            Event::Message(result) => {
                self.busy(ui, false);
                ui.set_status(result?.into());
            }
            Event::UpdateChecked(result) => {
                self.busy(ui, false);
                if let Some(release) = result? {
                    ui.set_confirm_title(
                        format!("Доступен EXBO Launcher New {}", release.version).into(),
                    );
                    ui.set_confirm_body(
                        "Установить обновление Rust-лаунчера и перезапустить приложение?".into(),
                    );
                    ui.set_confirm_action("rust-update".into());
                    self.update = Some(release);
                } else {
                    ui.set_status("Установлена последняя версия Rust-лаунчера".into());
                }
            }
            Event::UpdateReady(result, hash) => {
                self.busy(ui, false);
                updater::schedule(&result?, &hash, &self.store.root)?;
                self.quit_requested = true;
            }
            Event::Tunnels(generation, result) => {
                if generation == self.opt_generation {
                    self.tunnel_results(ui, *result)?;
                }
            }
            Event::TunnelProgress(generation, progress) => {
                if generation == self.opt_generation && self.opt_search.is_some() {
                    if let Some(entry) = progress.entry {
                        self.opt_targets.push(entry);
                        self.render_tunnels(ui);
                    }
                    ui.set_opt_network_status(format!("Предварительно / Provisional: {} / {} узлов · {} проб на узел / probes per node",progress.done,progress.total,tunnel::PROBE_COUNT).into());
                }
            }
            Event::SeedUploaded(bytes) => {
                if self.control.is_none() && self.seeder.is_some() {
                    ui.set_progress_label(
                        format!("P2P · отдано {:.1} MiB", bytes as f64 / 1048576.).into(),
                    );
                }
            }
            Event::DefenderStatus(result, manual) => {
                if manual {
                    self.busy(ui, false);
                }
                match result {
                    Ok(text) => {
                        ui.set_defender_status(text.into());
                    }
                    Err(e) => {
                        ui.set_defender_status("Проверка Defender недоступна".into());
                        if manual {
                            return Err(e);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
