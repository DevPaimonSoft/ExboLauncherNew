use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AuthResult {
    #[serde(rename = "type")]
    pub kind: String,
    pub true_login: Option<String>,
    pub session: Option<String>,
    pub token: Option<String>,
    pub time_left: i64,
    pub mail_abbreviation: Option<String>,
    pub license_accepted: bool,
    pub generic_error: Option<String>,
    pub new_email: Option<String>,
    pub new_email_hash: Option<String>,
    pub until_changing_finish: Option<i64>,
    pub secret_hash: Option<String>,
    #[serde(rename = "loginURL")]
    pub login_url: Option<String>,
    pub display_login: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Server {
    pub id: String,
    pub caption: String,
    pub ping_target: String,
    pub exclusions: Vec<String>,
    pub run_command: Option<String>,
    pub background: String,
    pub icon: String,
    pub desc: Vec<String>,
    pub total_size: u64,
    pub info: Option<ServerInfo>,
    pub version: String,
    pub online_max: u64,
    pub online_current: u64,
    pub http_seeds: Vec<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerInfo {
    pub news_link: String,
    pub news_api: String,
    pub instagram: String,
    pub vk: String,
    pub twitch: String,
    pub discord: String,
    pub youtube: String,
    pub logo: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct News {
    pub title: String,
    pub text: String,
    pub link: String,
    pub image: String,
    #[serde(
        rename = "image_link",
        alias = "imageLink",
        deserialize_with = "null_string"
    )]
    pub image_link: String,
}
fn null_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub optimization: crate::optimization::Settings,
    pub tray: bool,
    pub exit_new: u8,
    pub download: u32,
    pub upload: u32,
    pub start_on_downloaded: bool,
    pub verify_files_on_launch: bool,
    pub run_on_start: bool,
    pub ram: f64,
    pub p2p: u8,
    pub defender: bool,
    pub language: String,
    pub update_feed: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            optimization: crate::optimization::Settings::default(),
            tray: true,
            exit_new: 2,
            download: 0,
            upload: 0,
            start_on_downloaded: true,
            verify_files_on_launch: false,
            run_on_start: false,
            ram: 0.5,
            p2p: 1,
            defender: true,
            language: "ru".into(),
            update_feed: String::new(),
        }
    }
}

impl Settings {
    pub fn verify_before_launch(
        &self,
        installed_version: Option<&str>,
        server_version: &str,
    ) -> bool {
        self.verify_files_on_launch
            || installed_version.is_some_and(|version| version != server_version)
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Account {
    pub login: String,
    pub display_login: String,
    pub protected_session: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub legacy_root: String,
    pub user_id: String,
    pub auto_login: bool,
    pub active_account: String,
    pub accounts: Vec<Account>,
    pub clients: BTreeMap<String, String>,
    pub selected_server: String,
    pub settings: Settings,
    pub confirmed_email_hash: String,
}
