use crate::model::{Account, Config, Server, Settings};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        let root = std::path::absolute(root)?;
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }
    pub fn load(&self) -> Result<Config> {
        self.load_with_import(true)
    }
    pub fn load_with_import(&self, import_legacy: bool) -> Result<Config> {
        let path = self.root.join("config.json");
        if path.exists() {
            return serde_json::from_slice(&fs::read(path)?)
                .context("Повреждён config.json; исходный файл сохранён");
        }
        let mut config = Config {
            user_id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        };
        if import_legacy {
            config.user_id = crate::platform::legacy_user_id().unwrap_or(config.user_id);
            if let Some(root) = crate::platform::legacy_root() {
                config.legacy_root = root.to_string_lossy().into();
                let runtime = root.join("runtime");
                if let Ok(data) = fs::read(runtime.join("settings.json")) {
                    config.settings = serde_json::from_slice::<Settings>(&data)
                        .context("Не удалось импортировать настройки EXBO")?;
                }
                if let Ok(data) = fs::read(runtime.join("clients")) {
                    let mut reader = JavaReader::new(&data);
                    let count = reader.count(10_000)?;
                    for _ in 0..count {
                        let id = reader.utf()?;
                        let path = reader.utf()?;
                        safe_id(&id)?;
                        config.clients.insert(id, path);
                    }
                }
            }
            if let (Some(login), Some(session)) = (
                crate::platform::registry_string("Software\\EXBO", "EXBO_LOGIN"),
                crate::platform::registry_string("Software\\EXBO", "EXBO_SESSION"),
            ) && !login.is_empty()
                && !session.is_empty()
            {
                config.accounts.push(Account {
                    login: login.clone(),
                    display_login: login.clone(),
                    protected_session: crate::platform::protect(&session)?,
                });
                config.active_account = login;
                config.auto_login =
                    crate::platform::registry_string("Software\\EXBO", "EXBO_AUTO_AUTH").as_deref()
                        == Some("true");
            }
        }
        self.save(&config)?;
        Ok(config)
    }
    pub fn save(&self, config: &Config) -> Result<()> {
        atomic_write(
            &self.root.join("config.json"),
            &serde_json::to_vec_pretty(config)?,
        )
    }
    pub fn cache_servers(&self, servers: &[Server]) -> Result<()> {
        atomic_write(
            &self.root.join("servers.json"),
            &serde_json::to_vec(servers)?,
        )
    }
    pub fn cached_servers(&self) -> Vec<Server> {
        fs::read(self.root.join("servers.json"))
            .ok()
            .and_then(|x| serde_json::from_slice(&x).ok())
            .unwrap_or_default()
    }
    pub fn metadata_path(&self, id: &str) -> Result<PathBuf> {
        safe_id(id)?;
        Ok(self.root.join(format!("{id}.torrent.bin")))
    }
    pub fn installed_version(&self, id: &str) -> Result<Option<String>> {
        safe_id(id)?;
        match fs::read_to_string(self.root.join(format!("{id}.installed"))) {
            Ok(version) => Ok(Some(version)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    pub fn record_installed_version(&self, id: &str, version: &str) -> Result<()> {
        safe_id(id)?;
        atomic_write(
            &self.root.join(format!("{id}.installed")),
            version.as_bytes(),
        )
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    {
        use std::io::Write;
        let mut f = fs::File::create(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if path.exists() {
        fs::copy(path, path.with_extension("bak"))?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
        }
        let a: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0x1 | 0x8) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(windows))]
    fs::rename(temp, path)?;
    Ok(())
}

pub fn safe_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() < 128
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-'),
        "Недопустимый ID клиента"
    );
    Ok(())
}

pub fn relative_path(name: &str) -> Result<PathBuf> {
    ensure!(
        !name.is_empty() && !name.contains('\\') && !name.contains(':') && !name.contains('\0'),
        "Недопустимый путь в метаданных"
    );
    let path = Path::new(name);
    ensure!(
        path.components().all(|p| matches!(p, Component::Normal(_))),
        "Путь выходит за каталог клиента"
    );
    for segment in name.split('/') {
        let base = segment.split('.').next().unwrap_or("").to_uppercase();
        ensure!(
            !segment.is_empty()
                && !segment
                    .chars()
                    .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
                && !segment.ends_with([' ', '.'])
                && !matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !(base.len() == 4
                    && (base.starts_with("COM") || base.starts_with("LPT"))
                    && base.as_bytes()[3].is_ascii_digit()),
            "Недопустимое имя файла Windows"
        );
    }
    Ok(path.to_path_buf())
}

pub fn guarded_path(root: &Path, name: &str) -> Result<PathBuf> {
    let rel = relative_path(name)?;
    let canonical = root.canonicalize()?;
    let mut path = root.to_path_buf();
    for segment in rel.components() {
        path.push(segment);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            ensure!(
                !meta.file_type().is_symlink(),
                "Символьная ссылка внутри клиента"
            );
            ensure!(
                path.canonicalize()?.starts_with(&canonical),
                "Junction ведёт за каталог клиента"
            );
        }
    }
    Ok(path)
}

pub struct JavaReader<'a> {
    cursor: Cursor<&'a [u8]>,
}
impl<'a> JavaReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            cursor: Cursor::new(data),
        }
    }
    pub fn bytes<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut b = [0; N];
        self.cursor.read_exact(&mut b)?;
        Ok(b)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.bytes()?))
    }
    pub fn count(&mut self, limit: u32) -> Result<u32> {
        let n = self.u32()?;
        ensure!(n <= limit, "Слишком большой счётчик в метаданных");
        Ok(n)
    }
    pub fn remaining(&self) -> usize {
        self.cursor
            .get_ref()
            .len()
            .saturating_sub(self.cursor.position() as usize)
    }
    pub fn utf(&mut self) -> Result<String> {
        let length = u16::from_be_bytes(self.bytes()?) as usize;
        let mut data = vec![0; length];
        self.cursor.read_exact(&mut data)?;
        let mut units = Vec::new();
        let mut i = 0;
        while i < data.len() {
            let a = data[i];
            i += 1;
            let unit = if a < 0x80 {
                a as u16
            } else if a & 0xe0 == 0xc0 {
                let b = *data.get(i).context("Обрезанный modified UTF-8")?;
                i += 1;
                ensure!(b & 0xc0 == 0x80, "Некорректный UTF-8");
                ((a as u16 & 31) << 6) | (b as u16 & 63)
            } else if a & 0xf0 == 0xe0 {
                let b = *data.get(i).context("Обрезанный modified UTF-8")?;
                let c = *data.get(i + 1).context("Обрезанный modified UTF-8")?;
                i += 2;
                ensure!(b & 0xc0 == 0x80 && c & 0xc0 == 0x80, "Некорректный UTF-8");
                ((a as u16 & 15) << 12) | ((b as u16 & 63) << 6) | (c as u16 & 63)
            } else {
                bail!("Некорректный modified UTF-8")
            };
            units.push(unit);
        }
        String::from_utf16(&units).context("Некорректный UTF-16")
    }
}
