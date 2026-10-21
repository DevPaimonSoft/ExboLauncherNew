use super::*;

impl App {
    pub(super) fn accounts(&self, ui: &LauncherWindow) {
        ui.set_accounts(strings(
            self.config.accounts.iter().map(|a| a.login.clone()),
        ));
        ui.set_selected_account(self.config.active_account.clone().into());
    }

    pub(super) fn auth_request(&mut self, ui: &LauncherWindow, account: bool) -> Result<()> {
        ensure!(!self.busy, "Вход уже выполняется");
        self.config.auto_login = ui.get_auto_login();
        self.store.save(&self.config)?;
        let api = self.api()?;
        let tx = self.sender.clone();
        let (login, password, session) = if account {
            self.pending_password.clear();
            let selected = ui.get_selected_account().to_string();
            let a = self
                .config
                .accounts
                .iter()
                .find(|a| a.login == selected)
                .context("Аккаунт не найден")?;
            (
                a.login.clone(),
                String::new(),
                Some(platform::unprotect(&a.protected_session)?),
            )
        } else {
            let login = ui.get_login().trim().to_string();
            let password = ui.get_password().to_string();
            ensure!(
                !login.is_empty() && !password.is_empty(),
                "Введите логин и пароль"
            );
            self.pending_password = password.clone();
            (login, password, None)
        };
        ui.set_login(login.clone().into());
        ui.set_password("".into());
        self.busy(ui, true);
        std::thread::spawn(move || {
            let _ = tx.send(Event::auth(api.auth(
                &login,
                &password,
                session.as_deref(),
                true,
            )));
        });
        Ok(())
    }

    pub(super) fn apply_auth(&mut self, ui: &LauncherWindow, result: AuthResult) -> Result<()> {
        self.pending = Some(result.clone());
        if let Some(login) = &result.true_login {
            ui.set_login(login.clone().into());
        }
        match result.kind.as_str() {
            "OK" => {
                if result.new_email.is_some()
                    && result.new_email_hash.as_deref() != Some(&self.config.confirmed_email_hash)
                {
                    ui.set_auth_step("email-warning".into());
                    ui.set_email_warning(format!("Запрошена смена почты на {}. До завершения: {} дн. Подтвердите, что изменение выполнено вами.",result.new_email.as_deref().unwrap_or(""),result.until_changing_finish.unwrap_or(0)).into());
                    return Ok(());
                }
                let login = result
                    .true_login
                    .clone()
                    .unwrap_or_else(|| ui.get_login().to_string());
                ensure!(
                    result.token.as_ref().is_some_and(|x| !x.is_empty())
                        && result.session.as_ref().is_some_and(|x| !x.is_empty()),
                    "Сервер не выдал токен или сессию"
                );
                let display = result.display_login.clone().unwrap_or(login.clone());
                let account = Account {
                    login: login.clone(),
                    display_login: display.clone(),
                    protected_session: platform::protect(result.session.as_deref().unwrap())?,
                };
                if let Some(old) = self.config.accounts.iter_mut().find(|x| x.login == login) {
                    *old = account;
                } else {
                    self.config.accounts.push(account);
                }
                self.config.active_account = login.clone();
                self.store.save(&self.config)?;
                let mut result = result;
                result.true_login = Some(login);
                self.auth = Some(result.clone());
                self.auth_generation += 1;
                self.polling = false;
                self.pending = None;
                self.pending_password.clear();
                self.accounts(ui);
                ui.set_display_login(display.into());
                ui.set_license_required(!result.license_accepted);
                ui.set_page("main".into());
                ui.set_tab("overview".into());
                ui.set_auth_step("password".into());
                ui.set_code("".into());
                ui.set_new_password("".into());
                self.refresh(ui)?;
            }
            "GOOGLE_AUTH" => {
                ui.set_auth_step("totp".into());
                ui.set_status("Введите код аутентификатора".into());
            }
            "MAIL_AUTH" => {
                ui.set_auth_step("mail".into());
                ui.set_status(
                    format!(
                        "Код отправлен: {}",
                        result
                            .mail_abbreviation
                            .as_deref()
                            .unwrap_or("электронная почта")
                    )
                    .into(),
                );
            }
            "NEW_PLACE" => {
                ui.set_auth_step("password".into());
                bail!("Сессия завершена на сервере. Войдите повторно");
            }
            "EMAIL_CONFIRMATION" => {
                ui.set_auth_step("email".into());
                ui.set_status("Подтвердите почту аккаунта".into());
            }
            "PASSWORD_COMPROMISED" => {
                ui.set_auth_step("password-change".into());
            }
            "DISTRIBUTOR_PENDING" => {
                ui.set_auth_step("distributor".into());
                if let Some(url) = result.login_url {
                    protocol::register(&self.store.root)?;
                    platform::open_url(&url)?;
                }
            }
            "NEUTRAL" => {
                if ui.get_auth_step() == "email" {
                    ui.set_auth_step("email-code".into());
                    ui.set_status("Код подтверждения отправлен".into());
                } else {
                    ui.set_status("Запрос принят".into());
                }
            }
            "CODE_REGENERATED" => {
                ui.set_status("Новый код отправлен на почту".into());
            }
            "WRONG_CREDENTIALS" => bail!("Неверный логин, пароль или сохранённая сессия"),
            "WRONG_CODE" => bail!("Неверный код подтверждения"),
            "INVALID_GOOGLE_SECRET" => bail!("Неверный код аутентификатора"),
            "WRONG_HWID" => bail!("Сервер отклонил идентификатор оборудования"),
            "BANNED" => bail!(
                "Вход временно заблокирован. Осталось {} сек.",
                result.time_left
            ),
            "TOO_OFTEN_CODE_REGENERATION" => bail!(
                "Повторная отправка доступна через {} сек.",
                result.time_left
            ),
            "EMAIL_EXISTS" => bail!("Этот адрес почты уже используется"),
            "BANNED_BOOTSTRAP" | "FORCE_UPDATE" => bail!(
                "Сервер требует обновить компонент совместимости. Установите новую версию Rust-лаунчера и повторите вход"
            ),
            "DISTRIBUTOR_TIMEOUT" => {
                ui.set_auth_step("password".into());
                bail!("Время входа через Steam истекло");
            }
            "GENERIC_ERROR" => bail!(
                "{}",
                result
                    .generic_error
                    .unwrap_or_else(|| "Ошибка сервера авторизации".into())
            ),
            other => bail!("Ответ сервера авторизации: {other}"),
        }
        Ok(())
    }

    pub(super) fn auth_step(&mut self, ui: &LauncherWindow, resend: bool) -> Result<()> {
        if ui.get_auth_step() == "email-warning" {
            let mut result = self.pending.take().context("Нет данных авторизации")?;
            if ui.get_remember_email() {
                self.config.confirmed_email_hash =
                    result.new_email_hash.clone().unwrap_or_default();
                self.store.save(&self.config)?;
            }
            result.new_email = None;
            return self.apply_auth(ui, result);
        }
        ensure!(!self.busy, "Операция уже выполняется");
        let api = self.api()?;
        let tx = self.sender.clone();
        let login = ui.get_login().to_string();
        let step = ui.get_auth_step().to_string();
        let code = ui.get_code().trim().to_string();
        let email = ui.get_email().trim().to_string();
        let new_password = ui.get_new_password().to_string();
        let old_password = self.pending_password.clone();
        let port = self.socket_port.to_string();
        if !resend && matches!(step.as_str(), "mail" | "totp" | "email-code") {
            ensure!(
                code.len() >= 4 && code.len() <= 16 && code.bytes().all(|c| c.is_ascii_digit()),
                "Введите числовой код подтверждения"
            );
        }
        if step == "password-change" {
            ensure!(
                new_password.chars().count() >= 8,
                "Новый пароль должен содержать минимум 8 символов"
            );
        }
        self.busy(ui, true);
        std::thread::spawn(move || {
            let result = api::auth_step_parameters(
                &step,
                &code,
                &email,
                &new_password,
                &old_password,
                &port,
                resend,
            )
            .and_then(|extra| api.auth_step(&login, &extra));
            let _ = tx.send(Event::auth(result));
        });
        Ok(())
    }

    pub(super) fn link(&mut self, ui: &LauncherWindow, link: &str) -> Result<()> {
        let Ok(url) = protocol::callback_url(link) else {
            return Ok(());
        };
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        if params.get("action").map(|x| x.as_ref()) == Some("distributor_auth") {
            ensure!(
                ui.get_auth_step() == "distributor" && !self.busy,
                "Вход через Steam не запрошен"
            );
            let p = self
                .pending
                .as_ref()
                .context("Нет ожидающей сессии Steam")?;
            let distributor = p.token.clone().context("Нет идентификатора входа Steam")?;
            let session = p.session.clone().context("Нет сессии Steam")?;
            let token = params
                .get("verify_token")
                .context("Нет токена подтверждения")?
                .to_string();
            let api = self.api()?;
            let port = self.socket_port;
            let tx = self.sender.clone();
            self.busy(ui, true);
            std::thread::spawn(move || {
                let _ = tx.send(Event::auth(api.distributor(
                    &distributor,
                    port,
                    Some((&session, &token)),
                )));
            });
        }
        Ok(())
    }
}
