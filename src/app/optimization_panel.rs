use super::*;
use slint::Model;

pub fn fill(ui: &LauncherWindow, settings: &optimization::Settings, hw: optimization::Hardware) {
    let available = optimization::cpu::available_mask();
    ui.set_opt_cpu_supported(available.is_ok());
    ui.set_opt_cpu_status(
        available
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default()
            .into(),
    );
    let mask = available.unwrap_or(0);
    let selected = settings.cpu_affinity.as_deref();
    let cpus: Vec<_> = (0..64u16)
        .filter(|cpu| mask & (1u64 << cpu) != 0 || selected.is_some_and(|v| v.contains(cpu)))
        .map(|cpu| CpuRow {
            id: i32::from(cpu),
            available: mask & (1u64 << cpu) != 0,
            checked: selected.is_none_or(|v| v.contains(&cpu)),
        })
        .collect();
    ui.set_opt_cpus(Rc::new(VecModel::from(cpus)).into());
    ui.set_opt_cpu_active(selected.is_some());
    let values = serde_json::to_value(settings.profile_for(hw)).expect("typed profile");
    let mut rows: Vec<_> = optimization::parameters()
        .iter()
        .map(|p| OptimizationRow {
            key: p.key.clone().into(),
            label_ru: p.label_ru.clone().into(),
            label_en: p.label_en.clone().into(),
            help_ru: p.help_ru.clone().into(),
            help_en: p.help_en.clone().into(),
            category: p.category,
            value: if p.boolean {
                String::new()
            } else {
                values[&p.key].to_string()
            }
            .into(),
            boolean: p.boolean,
            checked: values[&p.key].as_bool().unwrap_or(false),
            range: if p.boolean {
                String::new()
            } else {
                format!("{}…{} {}", p.minimum, p.maximum, p.unit)
            }
            .into(),
            error: Default::default(),
        })
        .collect();
    for region in optimization::REGIONS {
        rows.push(OptimizationRow {key:format!("tunnel:{region}").into(),label_ru:format!("Туннель {}",region.to_uppercase()).into(),label_en:format!("{} tunnel",region.to_uppercase()).into(),help_ru:"Адрес host:port, например node.example.org:2000. Пустое поле возвращает выбор игры для этого региона. Адрес применяется при следующем запуске, даже если JVM-профиль выключен.".into(),help_en:"A host:port address, e.g. node.example.org:2000. Empty lets the game choose for this region. Applied on next start even if JVM tuning is off.".into(),category:3,value:settings.tunnels.get(region).cloned().unwrap_or_default().into(),..Default::default()});
        rows.push(OptimizationRow {key:format!("exclude:{region}").into(),label_ru:format!("Исключённые группы {}",region.to_uppercase()).into(),label_en:format!("Excluded {} pools",region.to_uppercase()).into(),help_ru:"Названия групп через точку с запятой. Поиск пропустит эти группы. Ручной адрес туннеля и другие регионы не меняются. Пусто — искать во всех группах.".into(),help_en:"Pool names separated by semicolons. Search skips these pools. Manual overrides and other regions are unchanged. Empty searches every pool.".into(),category:3,value:settings.excluded_pools.get(region).map(|p|p.join("; ")).unwrap_or_default().into(),..Default::default()});
    }
    ui.set_opt_rows(Rc::new(VecModel::from(rows)).into());
    ui.set_opt_active(settings.enabled);
    ui.set_opt_name(if settings.name.is_empty() {
        "Auto v1.1.2".into()
    } else {
        settings.name.clone().into()
    });
    ui.set_opt_search_mode(settings.search_mode as i32);
    ui.set_opt_hardware_ru(hw.description(false).into());
    ui.set_opt_hardware_en(hw.description(true).into());
    ui.set_opt_preview("".into());
    ui.set_opt_dirty(false);
}
pub fn edit(ui: &LauncherWindow, index: usize, value: SharedString, checked: bool) {
    let model = ui.get_opt_rows();
    let Some(mut row) = model.row_data(index) else {
        return;
    };
    row.value = value;
    row.checked = checked;
    row.error = "".into();
    if row.key.starts_with("tunnel:") && !row.value.trim().is_empty() {
        if let Err(e) = tunnel::parse_address(row.value.trim()) {
            row.error = e.to_string().into();
        }
    } else if let Some(p) = optimization::parameters()
        .iter()
        .find(|p| p.key == row.key.as_str())
        && !p.boolean
    {
        let value = row.value.trim().replace(',', ".");
        let parsed = value.parse::<f64>();
        if !parsed.is_ok_and(|v| {
            v.is_finite() && v >= p.minimum && v <= p.maximum && (p.decimal || v.fract() == 0.)
        }) {
            row.error = format!(
                "{}…{}{}",
                p.minimum,
                p.maximum,
                if p.decimal {
                    ""
                } else {
                    " · целое число / integer"
                }
            )
            .into();
        }
    }
    if let Some(model) = model.as_any().downcast_ref::<VecModel<OptimizationRow>>() {
        model.set_row_data(index, row);
    }
    ui.set_opt_dirty(true);
    ui.set_opt_preview("".into());
}
fn envelope(ui: &LauncherWindow) -> optimization::Settings {
    let mut settings = optimization::Settings {
        enabled: ui.get_opt_active(),
        name: ui.get_opt_name().trim().into(),
        search_mode: ui.get_opt_search_mode().clamp(0, 4) as u8,
        cpu_affinity: ui.get_opt_cpu_active().then(|| {
            ui.get_opt_cpus()
                .iter()
                .filter(|cpu| cpu.checked)
                .map(|cpu| cpu.id as u16)
                .collect()
        }),
        ..Default::default()
    };
    for row in ui.get_opt_rows().iter() {
        if let Some(region) = row.key.strip_prefix("tunnel:") {
            if !row.value.trim().is_empty() {
                settings
                    .tunnels
                    .insert(region.into(), row.value.trim().into());
            }
        } else if let Some(region) = row.key.strip_prefix("exclude:") {
            let mut pools: Vec<String> = row
                .value
                .split(';')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect();
            pools.sort();
            pools.dedup();
            if !pools.is_empty() {
                settings.excluded_pools.insert(region.into(), pools);
            }
        }
    }
    settings
}
pub fn read(ui: &LauncherWindow) -> Result<optimization::Settings> {
    let mut values = serde_json::Map::new();
    for (index, row) in ui.get_opt_rows().iter().enumerate() {
        let Some(p) = optimization::parameters()
            .iter()
            .find(|p| p.key == row.key.as_str())
        else {
            continue;
        };
        let value = if p.boolean {
            serde_json::json!(row.checked)
        } else {
            let text = row.value.trim().replace(',', ".");
            if p.decimal {
                serde_json::json!(
                    text.parse::<f64>()
                        .with_context(|| format!("{}: введите число", p.label_ru))?
                )
            } else {
                serde_json::json!(
                    text.parse::<i64>()
                        .with_context(|| format!("{}: введите целое число", p.label_ru))?
                )
            }
        };
        if !row.error.is_empty() {
            ui.set_opt_category(row.category);
            anyhow::bail!("{}: {} (поле {})", p.label_ru, row.error, index + 1);
        }
        values.insert(p.key.clone(), value);
    }
    let mut settings = envelope(ui);
    settings.profile = Some(serde_json::from_value(serde_json::Value::Object(values))?);
    if let Err(error) = optimization::cpu::validate(settings.cpu_affinity.as_deref()) {
        ui.set_opt_category(5);
        return Err(error);
    }
    settings.validate()?;
    Ok(settings)
}

impl App {
    pub fn optimization_action(&mut self, ui: &LauncherWindow, action: &str) -> Result<()> {
        ensure!(
            !ui.get_opt_network_busy()
                || !matches!(
                    action,
                    "opt-auto" | "opt-aikar" | "opt-import" | "opt-revert" | "opt-reset"
                ),
            "Дождитесь окончания измерения туннелей"
        );
        match action {
            "opt-cpu-all" | "opt-cpu-clear" => {
                let cpus = ui.get_opt_cpus();
                if let Some(model) = cpus.as_any().downcast_ref::<VecModel<CpuRow>>() {
                    for (i, mut cpu) in cpus.iter().enumerate() {
                        cpu.checked = action == "opt-cpu-all" && cpu.available;
                        model.set_row_data(i, cpu);
                    }
                }
                ui.set_opt_dirty(true);
                ui.set_opt_preview("".into());
            }
            action if action.starts_with("opt-cpu:") => {
                let index: usize = action[8..].parse()?;
                let cpus = ui.get_opt_cpus();
                let mut cpu = cpus.row_data(index).context("CPU не найден")?;
                ensure!(
                    ui.get_opt_cpu_active() && cpu.available,
                    "CPU недоступен для выбора"
                );
                cpu.checked = !cpu.checked;
                if let Some(model) = cpus.as_any().downcast_ref::<VecModel<CpuRow>>() {
                    model.set_row_data(index, cpu);
                }
                ui.set_opt_dirty(true);
                ui.set_opt_preview("".into());
            }
            "opt-auto" | "opt-aikar" => {
                let mut settings = envelope(ui);
                let auto = optimization::automatic(settings.profile_hardware(self.hardware));
                let mut profile = if action == "opt-aikar" {
                    optimization::aikar()
                } else {
                    auto.clone()
                };
                if action == "opt-aikar" {
                    profile.heap_size_gb = profile.heap_size_gb.min(auto.heap_size_gb);
                    profile.pre_touch = auto.pre_touch;
                    profile.parallel_gc_threads = auto.parallel_gc_threads;
                    profile.conc_gc_threads = auto.conc_gc_threads;
                    profile.g1_heap_region_size_mb = auto.g1_heap_region_size_mb;
                }
                settings.profile = Some(profile);
                settings.name = if action == "opt-aikar" {
                    "Aikar"
                } else {
                    "Auto v1.1.2"
                }
                .into();
                fill(ui, &settings, self.hardware);
                ui.set_opt_dirty(true);
            }
            "opt-reset" => {
                fill(ui, &optimization::Settings::default(), self.hardware);
                self.invalidate_tunnel_results(ui);
                ui.set_opt_dirty(true);
            }
            "opt-revert" => {
                fill(ui, &self.config.settings.optimization, self.hardware);
                self.invalidate_tunnel_results(ui);
            }
            "opt-import" => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Optimization JSON", &["json"])
                    .pick_file()
                {
                    let settings = optimization::Settings::read_import(&path, &envelope(ui))?;
                    fill(ui, &settings, self.hardware);
                    self.invalidate_tunnel_results(ui);
                    ui.set_opt_dirty(true);
                    ui.set_status(
                        "Профиль импортирован для просмотра; нажмите «Сохранить и применить»"
                            .into(),
                    );
                }
            }
            "opt-export" | "opt-export-jvm" => {
                let settings = read(ui)?;
                let bytes = settings.export(action == "opt-export-jvm", self.hardware)?;
                let filename = if action == "opt-export-jvm" {
                    "jvm-profile.json"
                } else {
                    "optimization-profile.json"
                };
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Optimization JSON", &["json"])
                    .set_file_name(filename)
                    .save_file()
                {
                    storage::atomic_write(&path, &bytes)?;
                    ui.set_status("Профиль экспортирован".into());
                }
            }
            "opt-save" => {
                let settings = read(ui)?;
                if let Err(error) =
                    optimization::cpu::validate_current(settings.cpu_affinity.as_deref())
                {
                    ui.set_opt_category(5);
                    return Err(error);
                }
                if settings.enabled {
                    settings
                        .profile_for(self.hardware)
                        .validate_hardware(self.hardware)?;
                }
                let mut config = self.config.clone();
                config.settings.optimization = settings;
                self.store.save(&config)?;
                self.config = config;
                ui.set_opt_applied_active(self.config.settings.optimization.enabled);
                ui.set_opt_dirty(false);
                ui.set_status(
                    "Оптимизация сохранена; применяется со следующего запуска игры".into(),
                );
            }
            "opt-preview" => {
                let settings = read(ui)?;
                let flags = settings.flags(self.hardware);
                let mut preview = if flags.is_empty() {
                    "JVM-профиль выключен; переопределений туннелей нет. / JVM profile off; no tunnel overrides.".to_owned()
                } else {
                    flags.join("\n")
                };
                preview.push_str(&match settings.cpu_affinity {
                    Some(cpus) => format!(
                        "\nCPU: {}",
                        cpus.iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    None => "\nCPU: без ограничения / Windows default".into(),
                });
                ui.set_opt_preview(preview.into());
            }
            "opt-source" => platform::open_url(optimization::SOURCE)?,
            "opt-legacy-export" => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("PowerShell", &["ps1"])
                    .set_file_name("recover-legacy-optimizer.ps1")
                    .save_file()
                {
                    storage::atomic_write(
                        &path,
                        include_bytes!("../../scripts/recover-legacy-optimizer.ps1"),
                    )?;
                    ui.set_status("Скрипт сохранён. По умолчанию — только просмотр; удаление требует -Apply, подтверждения записей и UAC. / Script exported; default is preview only.".into());
                }
            }
            "opt-region" => {
                self.cancel_tunnel_search(ui);
                self.opt_targets.clear();
                self.opt_result_region.clear();
                self.render_tunnels(ui);
                ui.set_opt_network_status("".into());
            }
            "opt-cancel" => {
                self.cancel_tunnel_search(ui);
                self.render_tunnels(ui);
                ui.set_opt_network_status("Измерение отменено; предварительные результаты не применены. / Cancelled; provisional results were not applied.".into());
            }
            "opt-mode" => {
                if self.opt_search.is_some() {
                    self.cancel_tunnel_search(ui);
                    self.opt_targets.clear();
                    ui.set_opt_network_status(
                        "Режим изменён; повторите измерение. / Mode changed; measure again.".into(),
                    );
                } else {
                    self.opt_targets = self
                        .opt_targets
                        .iter()
                        .map(|r| r.for_mode(ui.get_opt_search_mode().clamp(0, 4) as u8))
                        .collect();
                }
                self.render_tunnels(ui);
            }
            "opt-measure" => {
                let settings = envelope(ui);
                settings.validate()?;
                self.cancel_tunnel_search(ui);
                let region =
                    optimization::REGIONS[ui.get_opt_region().clamp(0, 4) as usize].to_owned();
                let excluded = settings
                    .excluded_pools
                    .get(&region)
                    .cloned()
                    .unwrap_or_default();
                let cache =
                    tunnel::Cache::read(&self.store.root.join("optimization-tunnel-cache.json"));
                let control = tunnel::Control::default();
                self.opt_search = Some(control.clone());
                let generation = self.opt_generation;
                let tx = self.sender.clone();
                let progress_tx = tx.clone();
                ui.set_opt_network_busy(true);
                ui.set_opt_network_status("Получение списка / Fetching nodes…".into());
                self.opt_targets.clear();
                self.opt_result_region = region.clone();
                self.render_tunnels(ui);
                std::thread::spawn(move || {
                    let result = tunnel::measure(
                        region,
                        settings.search_mode,
                        excluded,
                        cache,
                        control,
                        move |progress| {
                            let _ = progress_tx
                                .send(Event::TunnelProgress(generation, Box::new(progress)));
                        },
                    );
                    let _ = tx.send(Event::Tunnels(generation, Box::new(result)));
                });
            }
            "opt-tunnel-best" => {
                ensure!(
                    self.opt_round_complete && !ui.get_opt_network_busy(),
                    "Дождитесь полного сравнения туннелей"
                );
                let index = tunnel::best_index(&self.opt_targets)
                    .context("Нет узлов с минимум 5 успешными ответами; повторите измерение")?;
                self.choose_tunnel(ui, index)?;
            }
            _ if action.starts_with("opt-tunnel:") => {
                self.choose_tunnel(ui, action[11..].parse()?)?
            }
            _ => anyhow::bail!("Неизвестное действие оптимизации"),
        }
        Ok(())
    }
    fn cancel_tunnel_search(&mut self, ui: &LauncherWindow) {
        if let Some(control) = self.opt_search.take() {
            control.cancel();
        }
        self.opt_generation = self.opt_generation.wrapping_add(1);
        self.opt_round_complete = false;
        ui.set_opt_network_busy(false);
        ui.set_opt_network_best(false);
    }
    fn invalidate_tunnel_results(&mut self, ui: &LauncherWindow) {
        self.cancel_tunnel_search(ui);
        self.opt_targets.clear();
        self.opt_result_region.clear();
        self.render_tunnels(ui);
        ui.set_opt_network_status("".into());
    }
    fn choose_tunnel(&mut self, ui: &LauncherWindow, index: usize) -> Result<()> {
        ensure!(
            self.opt_round_complete && !ui.get_opt_network_busy(),
            "Дождитесь полного сравнения туннелей"
        );
        let target = self
            .opt_targets
            .get(index)
            .context("Повторите поиск туннелей")?;
        ensure!(
            target.available,
            "Туннель недоступен или достигнут лимит подключений"
        );
        let region = optimization::REGIONS[ui.get_opt_region().clamp(0, 4) as usize];
        ensure!(
            self.opt_result_region == region,
            "Регион изменён; повторите поиск"
        );
        ensure!(
            !envelope(ui)
                .excluded_pools
                .get(region)
                .is_some_and(|p| p.contains(&target.target.pool)),
            "Группа туннеля исключена; повторите поиск"
        );
        let model = ui.get_opt_rows();
        if let Some(model) = model.as_any().downcast_ref::<VecModel<OptimizationRow>>() {
            let index = model
                .iter()
                .position(|row| row.key == format!("tunnel:{region}"))
                .context("Нет поля региона")?;
            let mut row = model.row_data(index).unwrap();
            row.value = target.target.endpoint.address.clone().into();
            row.error = "".into();
            model.set_row_data(index, row);
        }
        ui.set_opt_dirty(true);
        ui.set_status("Туннель выбран для профиля; нажмите «Сохранить и применить»".into());
        Ok(())
    }
    pub(super) fn render_tunnels(&mut self, ui: &LauncherWindow) {
        tunnel::sort(&mut self.opt_targets);
        let complete = self.opt_round_complete;
        let rows:Vec<_>=self.opt_targets.iter().map(|r|{
            let diagnostic=format!("{} · median {:.1} ms · P90 {:.1} ms · max {:.1} ms · replies {}/{} · loss {:.0}% · last {:.1} + {:.1} ms · avg {:.1} + {:.1} ms · jitter {:.1} ms",r.target.endpoint.address,r.median,r.p90,r.max,r.successes,r.samples,r.loss*100.,r.last_client,r.last_server,r.client,r.server,r.jitter);
            let state=if r.limited {"Лимит подключений / Connection limit".to_owned()}else if !r.available {"Нет ответа / No reply".to_owned()}else{format!("Оценка / Score {:.1}",r.score)};
            TunnelRow{name:format!("{} · {}",r.target.pool,r.target.endpoint.name).into(),details:format!("{state} · {diagnostic}{}",if !complete || r.successes<tunnel::MIN_REPLIES {" · Предварительно / Provisional"}else{""}).into(),available:complete && r.available}
        }).collect();
        ui.set_opt_tunnels(Rc::new(VecModel::from(rows)).into());
        ui.set_opt_network_best(complete && tunnel::best_index(&self.opt_targets).is_some());
    }
    pub fn tunnel_results(
        &mut self,
        ui: &LauncherWindow,
        result: Result<tunnel::Results>,
    ) -> Result<()> {
        self.opt_search = None;
        ui.set_opt_network_busy(false);
        let result = match result {
            Ok(result) => result,
            Err(e) => {
                self.opt_round_complete = false;
                self.render_tunnels(ui);
                ui.set_opt_network_status(e.to_string().into());
                return Err(e);
            }
        };
        self.opt_result_region = result.region;
        self.opt_targets = result.entries;
        self.opt_round_complete = true;
        self.render_tunnels(ui);
        ui.set_opt_network_status(format!("{}: {} узлов / nodes · {} проб на узел. Выбор действует после сохранения. / Save to apply.",self.opt_result_region.to_uppercase(),self.opt_targets.len(),tunnel::PROBE_COUNT).into());
        result
            .cache
            .save(&self.store.root.join("optimization-tunnel-cache.json"))?;
        Ok(())
    }
}
