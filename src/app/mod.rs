mod actions;
mod auth;
mod events;
mod game;
mod optimization_panel;
mod settings;
mod startup;
mod ui_preview;
pub(crate) mod view;
pub(crate) use startup::run;
use view::*;

use crate::model::{Account, AuthResult, Config, News, Server};
use crate::{
    CpuRow, LauncherWindow, NewsRow, OptimizationRow, ServerRow, TunnelRow, api, download, ipc,
    launch, optimization, platform, protocol, seeding, storage, support, updater,
};
use anyhow::{Context, Result, bail, ensure};
use i_slint_backend_winit::WinitWindowAccessor;
use optimization::tunnel;
use sha1::Digest;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Child,
    rc::Rc,
    sync::{
        Arc,
        mpsc::{self, Sender},
    },
    time::{Duration, Instant},
};
enum Event {
    Tunnels(u64, Box<Result<tunnel::Results>>),
    TunnelProgress(u64, Box<tunnel::Progress>),
    Auth(Box<Result<AuthResult>>),
    Servers(Result<Vec<Server>>),
    ServerMeta(u64, Result<Vec<Server>>),
    News(String, u64, Result<Vec<News>>),
    Image(String, PathBuf),
    NewsImage(String, u64, usize, PathBuf),
    Progress(download::Progress),
    DownloadDone(Result<()>, bool),
    Started(Result<Child>, String, String),
    Deleted(Result<u64>),
    License(Result<()>),
    Message(Result<String>),
    UpdateChecked(Result<Option<updater::Release>>),
    UpdateReady(Result<PathBuf>, String),
    SeedUploaded(u64),
    DefenderStatus(Result<String>, bool),
}

impl Event {
    fn auth(result: Result<AuthResult>) -> Self {
        Self::Auth(Box::new(result))
    }
}

struct App {
    hardware: optimization::Hardware,
    opt_targets: Vec<tunnel::Measured>,
    opt_result_region: String,
    opt_search: Option<tunnel::Control>,
    opt_generation: u64,
    opt_round_complete: bool,
    store: storage::Store,
    config: Config,
    api: Option<api::Api>,
    auth: Option<AuthResult>,
    pending: Option<AuthResult>,
    pending_password: String,
    servers: Vec<Server>,
    news: Vec<News>,
    sender: Sender<Event>,
    control: Option<download::Control>,
    child: Option<Child>,
    socket_port: u16,
    busy: bool,
    tray_available: bool,
    update: Option<updater::Release>,
    polling: bool,
    seeder: Option<seeding::Seeder>,
    auth_generation: u64,
    news_generation: u64,
    quit_requested: bool,
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(control) = &self.opt_search {
            control.cancel();
        }
    }
}

impl App {
    fn api(&self) -> Result<api::Api> {
        self.api
            .clone()
            .context("API не готов: проверьте встроенные компоненты лаунчера")
    }

    fn credentials(&self) -> Result<(String, String)> {
        let a = self.auth.as_ref().context("Сначала войдите в аккаунт")?;
        Ok((
            a.true_login.clone().context("Нет логина сессии")?,
            a.token.clone().context("Нет токена сессии")?,
        ))
    }

    fn selected(&self, ui: &LauncherWindow) -> Result<Server> {
        self.servers
            .get(ui.get_selected_server().max(0) as usize)
            .cloned()
            .context("Выберите сервер")
    }

    fn busy(&mut self, ui: &LauncherWindow, value: bool) {
        self.busy = value;
        ui.set_busy(value);
    }
}
