use crate::{
    model::{AuthResult, Config, News, Server},
    storage::JavaReader,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use sha1::{Digest, Sha1};
use std::{io::Read, time::Duration};

pub const ENDPOINTS: [&str; 3] = [
    "https://launcher.stalcraft.net:443",
    "https://launcher.stalcraftx.ru:443",
    "https://launcher.stalcraft-tech.net:443",
];
#[derive(Clone)]
pub struct Api {
    client: Client,
    endpoints: Vec<String>,
    pub user_id: String,
    bootstrap: String,
    hwid: String,
}
impl Api {
    pub fn new(config: &Config, data: &std::path::Path) -> Result<Self> {
        let bootstrap = crate::support::bootstrap_hash();
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt
                        .previous()
                        .last()
                        .is_some_and(|url| url.scheme() == "https")
                        && attempt.url().scheme() != "https"
                    {
                        attempt.error("HTTPS downgrade is not allowed")
                    } else if attempt.previous().len() >= 10 {
                        attempt.error("Too many redirects")
                    } else {
                        attempt.follow()
                    }
                }))
                .user_agent(concat!("EXBO-Rust/", env!("CARGO_PKG_VERSION")))
                .build()?,
            endpoints: ENDPOINTS.into_iter().map(str::to_string).collect(),
            user_id: config.user_id.clone(),
            bootstrap,
            hwid: crate::platform::hwid(data).context("Не удалось определить HWID")?,
        })
    }
    #[cfg(test)]
    pub fn testing(endpoint: String) -> Self {
        Self {
            client: Client::new(),
            endpoints: vec![endpoint],
            user_id: "test".into(),
            bootstrap: "test".into(),
            hwid: String::new(),
        }
    }
    #[cfg(test)]
    pub fn testing_autonomous(
        endpoint: String,
        config: &Config,
        data: &std::path::Path,
    ) -> Result<Self> {
        let mut api = Self::new(config, data)?;
        api.endpoints = vec![endpoint];
        Ok(api)
    }
    pub fn get(&self, path: &str, query: &[(String, String)]) -> Result<Response> {
        let mut last = String::new();
        for endpoint in &self.endpoints {
            let response = self
                .client
                .get(format!("{endpoint}{path}"))
                .query(query)
                .send();
            match response {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) if r.status().is_server_error() => {
                    last = format!("{}: HTTP {}", endpoint, r.status())
                }
                Ok(r) => bail!("{}: HTTP {}", path, r.status()),
                Err(_) => last = format!("Нет соединения с {}", endpoint),
            }
        }
        bail!("{last}")
    }
    pub fn json<T: DeserializeOwned>(&self, path: &str, query: &[(String, String)]) -> Result<T> {
        let mut bytes = Vec::new();
        self.get(path, query)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 16 * 1024 * 1024, "Слишком большой ответ API");
        serde_json::from_slice(&bytes).with_context(|| format!("Некорректный JSON {path}"))
    }
    fn identity(&self) -> Vec<(String, String)> {
        pairs(&[
            ("bootstrap", &self.bootstrap),
            ("hwid", &self.hwid),
            ("id", &self.user_id),
            ("protocol_version", "3"),
        ])
    }
    pub fn auth(
        &self,
        login: &str,
        password: &str,
        session: Option<&str>,
        start: bool,
    ) -> Result<AuthResult> {
        ensure!(
            !self.bootstrap.is_empty(),
            "Не удалось подготовить встроенный компонент совместимости"
        );
        let mut q = self.identity();
        q.extend(pairs(&[
            ("login", login),
            ("state", if session.is_some() { "false" } else { "true" }),
        ]));
        if let Some(session) = session {
            q.extend(pairs(&[("session", session)]));
        } else {
            q.extend(pairs(&[("pass", password)]));
        }
        if start {
            q.extend(pairs(&[("start", "true")]));
        }
        let mut initial = q.clone();
        initial.retain(|(key, _)| key != "start");
        let mut result: AuthResult = self.json("/auth", &q)?;
        for _ in 0..4 {
            match result.kind.as_str() {
                "NEED_SECRET" => {
                    let answer =
                        reverse_sha1(result.secret_hash.as_deref().context("Нет secretHash")?)?;
                    q = self.identity();
                    q.extend(pairs(&[
                        ("state", "true"),
                        ("login", login),
                        ("answer", &answer),
                        ("new_protocol", "true"),
                    ]));
                    result = self.json("/auth", &q)?;
                }
                "POW_BCRYPT" => {
                    use base64::Engine;
                    let salt = bcrypt::BASE_64
                        .decode(result.secret_hash.as_deref().context("Нет соли BCrypt")?)
                        .context("Некорректная соль BCrypt")?;
                    let salt: [u8; 16] = salt
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Неверная длина соли"))?;
                    let first = format!("{:x}", md5::Md5::digest(password.as_bytes()));
                    let password_hash = format!("{:x}", md5::Md5::digest(first.as_bytes()));
                    let hash = bcrypt::hash_with_salt(password_hash, 10, salt)?
                        .format_for_version(bcrypt::Version::TwoY);
                    q = self.identity();
                    q.extend(pairs(&[
                        ("state", "true"),
                        ("login", login),
                        ("pass", password),
                        ("passhash", &hash),
                    ]));
                    result = self.json("/auth", &q)?;
                }
                "NICE_SECRET" => {
                    result = self.json("/auth", &initial)?;
                }
                _ => return Ok(result),
            }
        }
        bail!("Сервер повторяет проверку входа")
    }
    pub fn auth_step(&self, login: &str, extra: &[(&str, &str)]) -> Result<AuthResult> {
        let mut q = pairs(&[
            ("login", login),
            ("id", &self.user_id),
            ("protocol_version", "3"),
        ]);
        q.extend(pairs(extra));
        self.json("/auth", &q)
    }
    pub fn mail_change(&self, login: &str, token: &str) -> Result<AuthResult> {
        let mut q = self.identity();
        q.extend(pairs(&[
            ("state", "true"),
            ("login", login),
            ("token", token),
            ("mc", "true"),
            ("start", "true"),
        ]));
        self.json("/auth", &q)
    }
    pub fn distributor(
        &self,
        name: &str,
        port: u16,
        pending: Option<(&str, &str)>,
    ) -> Result<AuthResult> {
        let mut q = self.identity();
        q.extend(pairs(&[
            ("state", "true"),
            ("distributor", name),
            ("socket_port", &port.to_string()),
        ]));
        if let Some((session, token)) = pending {
            q.extend(pairs(&[
                ("distributor_session", session),
                ("verify_token", token),
            ]));
        }
        self.json("/distributor", &q)
    }
    pub fn servers(&self, login: &str, token: &str) -> Result<Vec<Server>> {
        let data: serde_json::Value = self.json(
            "/listServers",
            &pairs(&[("login", login), ("token", token)]),
        )?;
        let value = unwrap_token(data)?;
        serde_json::from_value(value).context("Некорректный список серверов")
    }
    pub fn server_meta(&self, login: &str, token: &str) -> Result<Vec<Server>> {
        let data = self.json(
            "/listServers",
            &pairs(&[("login", login), ("token", token), ("full", "false")]),
        )?;
        Ok(serde_json::from_value(unwrap_token(data)?)?)
    }
    pub fn join(&self, login: &str, token: &str) -> Result<String> {
        let result = self
            .get(
                "/joinGame",
                &pairs(&[
                    ("login", login),
                    ("token", token),
                    ("protocol_version", "3"),
                ]),
            )?
            .text()?;
        ensure!(
            !result.is_empty() && result.len() < 16384 && !result.trim_start().starts_with('<'),
            "Сервер не выдал игровую сессию"
        );
        Ok(result)
    }
    pub fn accept_license(&self, login: &str, token: &str) -> Result<()> {
        self.get(
            "/acceptLicense",
            &pairs(&[("login", login), ("token", token)]),
        )?
        .text()?;
        Ok(())
    }
    pub fn torrent_data(&self, login: &str, token: &str, id: &str) -> Result<Vec<u8>> {
        let mut r = self.get(
            "/torrentData",
            &pairs(&[("login", login), ("token", token), ("id", id)]),
        )?;
        let mut len = [0; 4];
        r.read_exact(&mut len)
            .context("Неполный заголовок torrentData")?;
        let length = u32::from_le_bytes(len) as usize;
        if length == 0 || length > 128 * 1024 * 1024 {
            if std::str::from_utf8(&len).is_ok_and(|s| s.trim_start().starts_with('{')) {
                let mut json = len.to_vec();
                r.take(16 * 1024).read_to_end(&mut json)?;
                let value: serde_json::Value = serde_json::from_slice(&json)
                    .context("Сервер вернул ошибку вместо torrent-метаданных")?;
                unwrap_token(value)?;
                bail!("Сервер вернул JSON вместо torrent-метаданных");
            }
            bail!("Недопустимый размер torrent-метаданных: {length} байт");
        }
        let mut bytes = Vec::new();
        flate2::read::GzDecoder::new(r)
            .take(length as u64 + 1)
            .read_to_end(&mut bytes)
            .context("Повреждённый или неполный GZIP torrentData")?;
        ensure!(bytes.len() == length, "Обрезанные torrent-метаданные");
        Ok(bytes)
    }
    pub fn external_bytes(&self, url: &str, max: u64) -> Result<Vec<u8>> {
        let url = url::Url::parse(url)?;
        ensure!(matches!(url.scheme(), "https" | "http"), "Недопустимый URL");
        let mut bytes = Vec::new();
        self.client
            .get(url)
            .send()
            .map_err(|_| anyhow::anyhow!("Ошибка загрузки ресурса"))?
            .error_for_status()
            .map_err(|_| anyhow::anyhow!("Ресурс недоступен"))?
            .take(max + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= max, "Ресурс слишком большой");
        Ok(bytes)
    }
    pub fn news(&self, server: &Server) -> Result<Vec<News>> {
        let Some(info) = &server.info else {
            return Ok(Vec::new());
        };
        if info.news_api.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_slice(&self.external_bytes(&info.news_api, 8 * 1024 * 1024)?)
            .context("Некорректная лента новостей")
    }
    pub fn range(&self, url: &str, start: u64, size: u64) -> Result<Response> {
        ensure!(
            size > 0 && start.checked_add(size).is_some(),
            "Недопустимый диапазон загрузки"
        );
        self.client
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(
                reqwest::header::RANGE,
                format!("bytes={start}-{}", start + size - 1),
            )
            .send()
            .map_err(|_| anyhow::anyhow!("HTTP seed недоступен"))?
            .error_for_status()
            .map_err(|_| anyhow::anyhow!("HTTP seed вернул ошибку"))
    }
}
pub fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
pub fn auth_step_parameters<'a>(
    step: &str,
    code: &'a str,
    email: &'a str,
    new_password: &'a str,
    old_password: &'a str,
    port: &'a str,
    resend: bool,
) -> Result<Vec<(&'static str, &'a str)>> {
    if resend {
        return match step {
            "mail" => Ok(vec![("regenerate", "true")]),
            "email-code" if email.is_empty() => Ok(vec![("action", "confirm_existed")]),
            "email-code" => Ok(vec![
                ("action", "confirm_new_mail_initial"),
                ("mail", email),
                ("socket_port", port),
            ]),
            _ => bail!("Повторная отправка недоступна на этом шаге"),
        };
    }
    Ok(match step {
        "mail" => vec![("code", code)],
        "totp" => vec![("secret", code)],
        "email" if email.is_empty() => vec![("action", "confirm_existed")],
        "email" => vec![
            ("action", "confirm_new_mail_initial"),
            ("mail", email),
            ("socket_port", port),
        ],
        "email-code" => vec![
            ("action", "confirm_new_mail_code"),
            ("mail", if email.is_empty() { "null" } else { email }),
            ("code", code),
            ("socket_port", port),
        ],
        "password-change" => vec![("pass", new_password), ("old_pass", old_password)],
        _ => bail!("Неизвестный шаг авторизации"),
    })
}
pub fn java_hex(data: &[u8]) -> String {
    let hex = data.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".into()
    } else {
        trimmed.into()
    }
}
pub fn reverse_sha1(hash: &str) -> Result<String> {
    for n in 0..100_000 {
        let text = format!("{n:05}").chars().rev().collect::<String>();
        if java_hex(&Sha1::digest(text.as_bytes())) == hash {
            return Ok(text);
        }
    }
    bail!("Не удалось решить проверку входа")
}
pub fn unwrap_token(mut value: serde_json::Value) -> Result<serde_json::Value> {
    if value.get("success").and_then(|v| v.as_bool()) != Some(true) {
        bail!("Сессия истекла: войдите повторно")
    }
    let data = value.get_mut("data").context("Отсутствует data")?.take();
    if let Some(text) = data.as_str() {
        Ok(serde_json::from_str(text)?)
    } else {
        Ok(data)
    }
}

#[allow(dead_code)]
pub fn legacy_clients(bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let mut r = JavaReader::new(bytes);
    let n = r.count(10000)?;
    (0..n).map(|_| Ok((r.utf()?, r.utf()?))).collect()
}
