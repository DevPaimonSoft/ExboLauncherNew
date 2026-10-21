use crate::{
    api::{Api, java_hex},
    model::{Server, Settings},
    storage::{JavaReader, Store, atomic_write, guarded_path, relative_path},
};
use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub hash: [u8; 20],
}
#[derive(Clone, Debug)]
pub struct Metadata {
    pub total: u64,
    pub comment: String,
    pub trackers: Vec<String>,
    pub piece_length: u32,
    pub name: String,
    pub files: Vec<FileEntry>,
    pub pieces: Vec<[u8; 20]>,
}
impl Metadata {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = JavaReader::new(bytes);
        let total = r.u64()?;
        let comment = r.utf()?;
        let n = r.count(1000)?;
        let mut trackers = Vec::new();
        for _ in 0..n {
            trackers.push(r.utf()?);
        }
        r.utf()?;
        let piece_length = r.u32()?;
        ensure!(
            (1..=64 * 1024 * 1024).contains(&piece_length),
            "Недопустимый размер torrent-части"
        );
        let name = r.utf()?;
        crate::storage::safe_id(&name)?;
        let n = r.count(1_000_000)?;
        let mut files = Vec::new();
        let mut names = HashSet::new();
        let mut content_sizes = HashMap::new();
        let mut sum = 0u64;
        for _ in 0..n {
            let path = r.utf()?;
            relative_path(&path)?;
            ensure!(
                names.insert(path.to_lowercase()),
                "Повторяющийся путь в метаданных"
            );
            let size = r.u64()?;
            sum = sum.checked_add(size).context("Переполнение размера")?;
            ensure!(size <= i64::MAX as u64, "Недопустимый размер файла");
            let hash = r.bytes()?;
            ensure!(
                content_sizes
                    .insert(hash, size)
                    .is_none_or(|old| old == size),
                "Один SHA-1 указан для файлов разного размера"
            );
            files.push(FileEntry { path, size, hash });
        }
        for name in &names {
            for (i, _) in name.match_indices('/') {
                ensure!(
                    !names.contains(&name[..i]),
                    "Файл одновременно используется как каталог"
                );
            }
        }
        ensure!(sum == total, "Не совпадает суммарный размер torrent");
        let n = r.count(10_000_000)?;
        ensure!(
            n as u64 == total.div_ceil(piece_length as u64),
            "Число частей не совпадает с размером torrent"
        );
        let mut pieces = Vec::new();
        for _ in 0..n {
            pieces.push(r.bytes()?);
        }
        ensure!(r.remaining() == 0, "Лишние данные в torrent-метаданных");
        Ok(Self {
            total,
            comment,
            trackers,
            piece_length,
            name,
            files,
            pieces,
        })
    }
    pub fn torrent(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"d");
        ben(&mut b, b"encoding");
        ben(&mut b, b"UTF-8");
        ben(&mut b, b"publisher");
        ben(&mut b, b"EXBO");
        ben(&mut b, b"comment");
        ben(&mut b, self.comment.as_bytes());
        ben(&mut b, b"announce-list");
        b.extend(b"ll");
        for t in &self.trackers {
            ben(&mut b, t.as_bytes());
        }
        b.extend(b"ee");
        if let Some(t) = self.trackers.first() {
            ben(&mut b, b"announce");
            ben(&mut b, t.as_bytes());
        }
        ben(&mut b, b"created by");
        ben(&mut b, b"JustAGod");
        ben(&mut b, b"publisher-url");
        ben(&mut b, b"https://stalcraft.ru");
        ben(&mut b, b"info");
        b.push(b'd');
        ben(&mut b, b"files");
        b.push(b'l');
        for f in &self.files {
            b.push(b'd');
            ben(&mut b, b"length");
            integer(&mut b, f.size);
            ben(&mut b, b"path");
            b.push(b'l');
            ben(&mut b, java_hex(&f.hash).as_bytes());
            b.push(b'e');
            ben(&mut b, b"true_path");
            b.push(b'l');
            for p in f.path.split('/') {
                ben(&mut b, p.as_bytes());
            }
            b.extend(b"ee");
        }
        b.push(b'e');
        ben(&mut b, b"pieces");
        let hashes: Vec<u8> = self.pieces.iter().flatten().copied().collect();
        ben(&mut b, &hashes);
        ben(&mut b, b"piece length");
        integer(&mut b, self.piece_length as u64);
        ben(&mut b, b"name");
        ben(&mut b, self.name.as_bytes());
        b.extend(b"ee");
        b
    }
}
fn ben(b: &mut Vec<u8>, v: &[u8]) {
    b.extend(format!("{}:", v.len()).as_bytes());
    b.extend(v);
}
fn integer(b: &mut Vec<u8>, v: u64) {
    b.extend(format!("i{v}e").as_bytes());
}

#[derive(Clone, Default)]
pub struct Control(pub Arc<AtomicU8>);
impl Control {
    pub fn pause(&self, paused: bool) {
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                (state != 2).then_some(u8::from(paused))
            });
    }
    pub fn cancel(&self) {
        self.0.store(2, Ordering::SeqCst);
    }
    pub fn state(&self) -> u8 {
        self.0.load(Ordering::SeqCst)
    }
    pub fn checkpoint(&self) -> Result<()> {
        while self.state() == 1 {
            thread::sleep(Duration::from_millis(100));
        }
        ensure!(self.state() != 2, "Операция отменена");
        Ok(())
    }
}
#[derive(Clone)]
pub enum Phase {
    Checking,
    Downloading,
    Ready,
}
#[derive(Clone)]
pub struct Progress {
    pub phase: Phase,
    pub done: u64,
    pub total: u64,
    pub rate: u64,
    pub message: String,
}
pub type Reporter = Arc<dyn Fn(Progress) + Send + Sync>;

pub fn metadata(
    api: &Api,
    store: &Store,
    legacy: &Path,
    server: &Server,
    login: &str,
    token: &str,
) -> Result<Metadata> {
    let cache = store.metadata_path(&server.id)?;
    for path in [
        &cache,
        &legacy
            .join("runtime")
            .join(format!("{}.torrent.bin", server.id)),
    ] {
        if let Ok(bytes) = fs::read(path)
            && java_hex(&Sha1::digest(&bytes)) == server.version
        {
            let meta = Metadata::parse(&bytes)?;
            if *path != cache {
                atomic_write(&cache, &bytes)?;
            }
            return Ok(meta);
        }
    }
    let bytes = api.torrent_data(login, token, &server.id)?;
    ensure!(
        java_hex(&Sha1::digest(&bytes)) == server.version,
        "Хеш метаданных не совпадает с версией сервера"
    );
    let metadata = Metadata::parse(&bytes)?;
    atomic_write(&cache, &bytes)?;
    Ok(metadata)
}

pub fn file_hash(path: &Path) -> Result<[u8; 20]> {
    checked_hash(path, &Control::default(), &|_| {})
}
fn checked_hash(path: &Path, control: &Control, progress: &dyn Fn(u64)) -> Result<[u8; 20]> {
    let mut f = File::open(path)?;
    let mut hasher = Sha1::new();
    let mut block = [0; 128 * 1024];
    let mut read = 0;
    loop {
        control.checkpoint()?;
        let n = f.read(&mut block)?;
        if n == 0 {
            break;
        }
        hasher.update(&block[..n]);
        read += n as u64;
        progress(read);
    }
    Ok(hasher.finalize().into())
}
fn excluded(file: &FileEntry, server: &Server, rewrite: bool) -> bool {
    hard_excluded(file)
        || (!rewrite
            && server.exclusions.iter().any(|x| {
                file.path.eq_ignore_ascii_case(x)
                    || file
                        .path
                        .to_lowercase()
                        .starts_with(&format!("{}/", x.trim_end_matches('/').to_lowercase()))
            }))
}
fn hard_excluded(file: &FileEntry) -> bool {
    let arch = if cfg!(target_pointer_width = "64") {
        "win32/"
    } else {
        "win64/"
    };
    file.path.to_lowercase().starts_with(arch)
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    api: &Api,
    store: &Store,
    server: &Server,
    meta: &Metadata,
    root: &Path,
    settings: &Settings,
    control: &Control,
    report: &Reporter,
    rewrite: bool,
) -> Result<()> {
    fs::create_dir_all(root)?;
    let stage = store.root.join("downloads").join(&server.id);
    fs::create_dir_all(&stage)?;
    let mut missing = Vec::new();
    let checking_total = meta
        .files
        .iter()
        .filter(|f| !hard_excluded(f))
        .map(|f| f.size)
        .sum();
    let mut checked = 0;
    report(Progress {
        phase: Phase::Checking,
        done: 0,
        total: checking_total,
        rate: 0,
        message: "Проверка файлов…".into(),
    });
    for (i, f) in meta.files.iter().enumerate() {
        control.checkpoint()?;
        if hard_excluded(f) {
            continue;
        }
        let path = guarded_path(root, &f.path)?;
        let exists = path.is_file();
        let skip = excluded(f, server, rewrite);
        let message = format!(
            "Проверка файлов: {} / {} · {}",
            i + 1,
            meta.files.len(),
            f.path
        );
        let valid = skip && exists
            || exists
                && path.metadata()?.len() == f.size
                && checked_hash(&path, control, &|bytes| {
                    report(Progress {
                        phase: Phase::Checking,
                        done: checked + bytes.min(f.size),
                        total: checking_total,
                        rate: 0,
                        message: message.clone(),
                    });
                })? == f.hash;
        if !valid && f.size == 0 && f.hash == <[u8; 20]>::from(Sha1::digest([])) {
            let empty = stage.join("empty.part");
            File::create(&empty)?;
            install(&empty, root, f)?;
            fs::remove_file(empty)?;
        } else if !valid {
            missing.push(i);
        }
        checked += f.size;
        if i % 80 == 0 || i + 1 == meta.files.len() {
            report(Progress {
                phase: Phase::Checking,
                done: checked,
                total: checking_total,
                rate: 0,
                message: format!("Проверка файлов: {} / {}", i + 1, meta.files.len()),
            });
        }
    }
    let total = missing.iter().map(|&i| meta.files[i].size).sum();
    let done = Arc::new(AtomicU64::new(0));
    let p2p_allowed =
        || settings.p2p == 0 || (settings.p2p == 1 && !crate::platform::metered_connection());
    if missing.is_empty() {
    } else if server.http_seeds.is_empty() {
        ensure!(
            p2p_allowed(),
            "Нет HTTP seeds, а P2P отключён настройками подключения"
        );
        p2p(meta, &stage, root, settings, control, report, &missing)?;
    } else {
        let mut grouped = std::collections::BTreeMap::<[u8; 20], Vec<usize>>::new();
        for index in missing {
            grouped
                .entry(meta.files[index].hash)
                .or_default()
                .push(index);
        }
        let missing: Vec<Vec<usize>> = grouped.into_values().collect();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let errors = Mutex::new(Vec::new());
        let start = Instant::now();
        let transferred = AtomicU64::new(0);
        thread::scope(|scope| {
            let workers = missing.len().min(4);
            for _ in 0..workers {
                let errors = &errors;
                let cursor = &cursor;
                let missing = &missing;
                let done = &done;
                let stage = &stage;
                let transferred = &transferred;
                scope.spawn(move || {
                    loop {
                        let n = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(indices) = missing.get(n) else { break };
                        let index = indices[0];
                        let f = &meta.files[index];
                        let accounted = AtomicU64::new(0);
                        let emit = |position: u64, received: u64| {
                            let previous = accounted.swap(position, Ordering::Relaxed);
                            let now = if position >= previous {
                                done.fetch_add(position - previous, Ordering::Relaxed) + position
                                    - previous
                            } else {
                                done.fetch_sub(previous - position, Ordering::Relaxed)
                                    - (previous - position)
                            };
                            let network =
                                transferred.fetch_add(received, Ordering::Relaxed) + received;
                            report(Progress {
                                phase: Phase::Downloading,
                                done: now,
                                total,
                                rate: (network as f64 / start.elapsed().as_secs_f64().max(0.01))
                                    as u64,
                                message: format!("Загрузка: {}", f.path),
                            });
                        };
                        let result = download_file(
                            api, server, meta, f, stage, root, settings, control, workers, &emit,
                        );
                        let result = result.and_then(|()| {
                            let cached = stage.join(format!("{}.part", java_hex(&f.hash)));
                            for &other in &indices[1..] {
                                control.checkpoint()?;
                                install(&cached, root, &meta.files[other])?;
                                done.fetch_add(meta.files[other].size, Ordering::Relaxed);
                            }
                            if cached.exists() {
                                fs::remove_file(cached)?;
                            }
                            Ok(())
                        });
                        if let Err(e) = result {
                            emit(0, 0);
                            let text = e.to_string();
                            for &i in indices {
                                errors.lock().unwrap().push((i, text.clone()));
                            }
                            if control.state() == 2 {
                                break;
                            }
                        }
                    }
                });
            }
        });
        let errors = errors.into_inner().unwrap();
        if !errors.is_empty() {
            control.checkpoint()?;
            if !p2p_allowed() {
                bail!(
                    "Не удалось загрузить {} файлов: {}",
                    errors.len(),
                    errors[0].1
                )
            }
            report(Progress {
                phase: Phase::Downloading,
                done: done.load(Ordering::Relaxed),
                total,
                rate: 0,
                message: "HTTP недоступен; подключение к P2P".into(),
            });
            p2p(
                meta,
                &stage,
                root,
                settings,
                control,
                report,
                &errors.into_iter().map(|(i, _)| i).collect::<Vec<_>>(),
            )?;
        }
    }
    control.checkpoint()?;
    atomic_write(
        &store.root.join(format!("{}.installed", server.id)),
        server.version.as_bytes(),
    )?;
    report(Progress {
        phase: Phase::Ready,
        done: total,
        total,
        rate: 0,
        message: "Клиент готов к запуску".into(),
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn download_file(
    api: &Api,
    server: &Server,
    meta: &Metadata,
    f: &FileEntry,
    stage: &Path,
    root: &Path,
    settings: &Settings,
    control: &Control,
    workers: usize,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    let hash = java_hex(&f.hash);
    let temp = stage.join(format!("{hash}.part"));
    if f.size == 0 {
        File::create(&temp)?;
    }
    let mut success = false;
    for _ in 0..2 {
        for seed in &server.http_seeds {
            control.checkpoint()?;
            let result = (|| -> Result<()> {
                let mut offset = temp.metadata().map(|x| x.len()).unwrap_or(0);
                if offset > f.size {
                    File::create(&temp)?;
                    offset = 0;
                }
                progress(offset, 0);
                if offset < f.size {
                    let url = format!("{}/{}/{}", seed.trim_end_matches('/'), meta.name, hash);
                    let mut r = api.range(&url, offset, f.size - offset)?;
                    let partial = r.status() == reqwest::StatusCode::PARTIAL_CONTENT;
                    if partial {
                        let range = r
                            .headers()
                            .get(reqwest::header::CONTENT_RANGE)
                            .and_then(|x| x.to_str().ok())
                            .context("Seed не указал Content-Range")?;
                        ensure!(
                            range == format!("bytes {offset}-{}/{}", f.size - 1, f.size),
                            "Неверный размер или смещение Content-Range"
                        );
                    } else if offset > 0 {
                        offset = 0;
                        File::create(&temp)?;
                        progress(0, 0);
                    }
                    let mut file = OpenOptions::new().create(true).append(true).open(&temp)?;
                    let started = Instant::now();
                    let mut received = 0u64;
                    let mut block = [0; 64 * 1024];
                    loop {
                        control.checkpoint()?;
                        let n = r.read(&mut block)?;
                        if n == 0 {
                            break;
                        }
                        ensure!(
                            offset + received + n as u64 <= f.size,
                            "Seed превысил размер файла"
                        );
                        file.write_all(&block[..n])?;
                        received += n as u64;
                        progress(offset + received, n as u64);
                        if settings.download > 0 {
                            let expected = Duration::from_secs_f64(
                                received as f64
                                    / (settings.download as f64 * 1024. / workers as f64),
                            );
                            if let Some(wait) = expected.checked_sub(started.elapsed()) {
                                let deadline = Instant::now() + wait;
                                while Instant::now() < deadline {
                                    control.checkpoint()?;
                                    thread::sleep(Duration::from_millis(20));
                                }
                            }
                        }
                    }
                    file.sync_all()?;
                }
                ensure!(
                    temp.metadata()?.len() == f.size
                        && checked_hash(&temp, control, &|_| {})? == f.hash,
                    "Файл не прошёл проверку SHA-1"
                );
                Ok(())
            })();
            if result.is_ok() {
                success = true;
                break;
            }
            if temp.metadata().is_ok_and(|m| m.len() >= f.size) {
                File::create(&temp)?;
                progress(0, 0);
            }
        }
        if success {
            break;
        }
    }
    ensure!(success, "HTTP seeds не отдали проверенный файл {}", f.path);
    install(&temp, root, f)?;
    Ok(())
}

fn install(source: &Path, root: &Path, f: &FileEntry) -> Result<()> {
    let target = guarded_path(root, &f.path)?;
    fs::create_dir_all(target.parent().context("Нет родительского каталога")?)?;
    let pending = target.with_extension(format!("{}.exbo-new", uuid::Uuid::new_v4()));
    fs::copy(source, &pending)?;
    ensure!(
        file_hash(&pending)? == f.hash,
        "Ошибка проверки перед установкой"
    );
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        unsafe extern "system" {
            fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
        }
        let a: Vec<u16> = pending.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 1 | 8) } != 0,
            "Не удалось заменить {}: {}",
            f.path,
            std::io::Error::last_os_error()
        );
    }
    #[cfg(not(windows))]
    fs::rename(pending, target)?;
    Ok(())
}

fn p2p(
    meta: &Metadata,
    stage: &Path,
    root: &Path,
    settings: &Settings,
    control: &Control,
    report: &Reporter,
    missing: &[usize],
) -> Result<()> {
    use librqbit::{AddTorrent, AddTorrentOptions, Session, SessionOptions, limits::LimitsConfig};
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let session = Session::new_with_opts(
            stage.into(),
            SessionOptions {
                dht: None,
                disable_local_service_discovery: true,
                ratelimits: LimitsConfig {
                    download_bps: std::num::NonZeroU32::new(settings.download.saturating_mul(1024)),
                    upload_bps: std::num::NonZeroU32::new(settings.upload.saturating_mul(1024)),
                },
                ..Default::default()
            },
        )
        .await?;
        let outcome = async {
            let torrent = meta.torrent();
            let handle = session
                .add_torrent(
                    AddTorrent::from_bytes(torrent),
                    Some(AddTorrentOptions {
                        overwrite: true,
                        output_folder: Some(stage.to_string_lossy().into()),
                        only_files: Some(missing.to_vec()),
                        ..Default::default()
                    }),
                )
                .await?
                .into_handle()
                .context("Torrent не добавлен")?;
            let mut paused = false;
            let mut last_progress = (0, Instant::now());
            loop {
                if control.state() == 2 {
                    bail!("Операция отменена")
                }
                if control.state() == 1 && !paused {
                    session.pause(&handle).await?;
                    paused = true;
                }
                if control.state() == 0 && paused {
                    session.unpause(&handle).await?;
                    paused = false;
                }
                let stats = handle.stats();
                if let Some(error) = stats.error {
                    bail!("P2P: {error}")
                }
                if paused || stats.progress_bytes != last_progress.0 {
                    last_progress = (stats.progress_bytes, Instant::now());
                }
                ensure!(last_progress.1.elapsed() < Duration::from_secs(120),
                    "P2P не получает данные более двух минут. Проверьте соединение или повторите загрузку позже");
                report(Progress {
                    phase: if matches!(stats.state, librqbit::TorrentStatsState::Initializing { .. }) { Phase::Checking } else { Phase::Downloading },
                    done: if matches!(stats.state, librqbit::TorrentStatsState::Initializing { .. }) { stats.progress_bytes } else {
                        missing.iter().map(|&i| stats.file_progress.get(i).copied().unwrap_or(0).min(meta.files[i].size)).sum()
                    },
                    total: if matches!(stats.state, librqbit::TorrentStatsState::Initializing { .. }) { stats.total_bytes } else { missing.iter().map(|&i| meta.files[i].size).sum() },
                    rate: stats.live.as_ref().map(|s| (s.download_speed.mbps * 1024. * 1024.) as u64).unwrap_or(0),
                    message: if paused {
                        "P2P на паузе"
                    } else {
                        "Загрузка P2P"
                    }
                    .into(),
                });
                if stats.finished {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if !paused { session.pause(&handle).await?; }
            for &i in missing {
                control.checkpoint()?;
                let f = &meta.files[i];
                let source = guarded_path(stage, &f.path)?;
                ensure!(
                    file_hash(&source)? == f.hash,
                    "P2P: неверный хеш {}",
                    f.path
                );
                install(&source, root, f)?;
            }
            Ok(())
        }
        .await;
        session.stop().await;
        outcome
    })
}

pub fn remove_client(root: &Path, meta: &Metadata) -> Result<u64> {
    ensure!(
        root.is_absolute() && root.components().count() > 2,
        "Небезопасный каталог удаления"
    );
    let mut deleted = 0;
    for f in &meta.files {
        let path = guarded_path(root, &f.path)?;
        if path.is_file() {
            fs::remove_file(path)?;
            deleted += 1;
        }
    }
    Ok(deleted)
}
