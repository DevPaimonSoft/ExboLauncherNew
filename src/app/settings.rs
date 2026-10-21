use super::*;

impl App {
    pub(super) fn save_settings(&mut self, ui: &LauncherWindow) -> Result<()> {
        let old_autostart = platform::registry_string(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "ExboLauncherNew",
        )
        .is_some();
        let mut config = self.config.clone();
        config.settings.ram = (ui.get_ram() as f64).clamp(0., 1.);
        config.settings.tray = ui.get_tray();
        config.settings.run_on_start = ui.get_autostart();
        config.settings.start_on_downloaded = ui.get_start_on_downloaded();
        config.settings.verify_files_on_launch = ui.get_verify_files_on_launch();
        config.settings.defender = ui.get_defender();
        config.settings.p2p = ui.get_p2p().clamp(0, 2) as u8;
        config.settings.exit_new = ui.get_exit_mode().clamp(0, 2) as u8;
        config.settings.download = ui
            .get_download_limit()
            .trim()
            .parse()
            .context("Лимит загрузки должен быть целым числом")?;
        config.settings.upload = ui
            .get_upload_limit()
            .trim()
            .parse()
            .context("Лимит отдачи должен быть целым числом")?;
        config.settings.language = if ui.get_en() { "en" } else { "ru" }.into();
        config.legacy_root = ui.get_legacy_root().to_string();
        config.settings.update_feed = ui.get_update_feed().trim().to_string();
        if !config.settings.update_feed.is_empty() {
            ensure!(
                url::Url::parse(&config.settings.update_feed)?.scheme() == "https",
                "Источник обновлений должен использовать HTTPS"
            );
        }
        let api = api::Api::new(&config, &self.store.root)?;
        if old_autostart != config.settings.run_on_start {
            platform::set_autostart(config.settings.run_on_start)?;
        }
        self.store.save(&config)?;
        self.config = config;
        self.api = Some(api);
        if self.seeder.is_some() {
            self.seeder = None;
            let _ = self.start_seed(ui);
        }
        ui.set_status("Настройки сохранены".into());
        Ok(())
    }
}
