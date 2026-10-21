use crate::{
    download::{Control, Metadata},
    model::Settings,
    storage::guarded_path,
};
use anyhow::{Context, Result, bail, ensure};
use librqbit::{
    ManagedTorrentShared, TorrentMetadata,
    storage::{BoxStorageFactory, StorageFactory, StorageFactoryExt, TorrentStorage},
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
pub struct ReadOnlyFactory {
    pub root: PathBuf,
    pub paths: Arc<Vec<String>>,
    pub control: Control,
}
impl StorageFactory for ReadOnlyFactory {
    type Storage = ReadOnlyStorage;
    fn create(&self, _: &ManagedTorrentShared, _: &TorrentMetadata) -> Result<Self::Storage> {
        let versions = self
            .paths
            .iter()
            .map(|name| {
                let path = guarded_path(&self.root, name).ok()?;
                let metadata = std::fs::metadata(path).ok()?;
                Some((metadata.len(), metadata.modified().ok()?))
            })
            .collect();
        Ok(ReadOnlyStorage {
            source: self.clone(),
            versions: Arc::new(versions),
        })
    }
    fn clone_box(&self) -> BoxStorageFactory {
        self.clone().boxed()
    }
}
#[derive(Clone)]
pub struct ReadOnlyStorage {
    source: ReadOnlyFactory,
    versions: Arc<Vec<Option<(u64, std::time::SystemTime)>>>,
}
impl TorrentStorage for ReadOnlyStorage {
    fn init(&mut self, _: &ManagedTorrentShared, _: &TorrentMetadata) -> Result<()> {
        Ok(())
    }
    fn pread_exact(&self, id: usize, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.source.control.checkpoint()?;
        let path = self
            .source
            .paths
            .get(id)
            .context("Неизвестный файл раздачи")?;
        let path = guarded_path(&self.source.root, path)?;
        let mut file = File::open(path)?;
        let expected = self
            .versions
            .get(id)
            .and_then(|v| *v)
            .context("Файл не был доступен при запуске раздачи")?;
        let before = file.metadata()?;
        ensure!(
            (before.len(), before.modified()?) == expected,
            "Файл изменён после проверки; раздача этой части запрещена"
        );
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buf)?;
        let after = file.metadata()?;
        ensure!(
            (after.len(), after.modified()?) == expected,
            "Файл изменён во время чтения"
        );
        Ok(())
    }
    fn pwrite_all(&self, _: usize, _: u64, _: &[u8]) -> Result<()> {
        bail!("Раздача работает только для чтения")
    }
    fn remove_file(&self, _: usize, _: &Path) -> Result<()> {
        bail!("Раздача не удаляет файлы")
    }
    fn remove_directory_if_empty(&self, _: &Path) -> Result<()> {
        bail!("Раздача не удаляет каталоги")
    }
    fn ensure_file_length(&self, _: usize, _: u64) -> Result<()> {
        Ok(())
    }
    fn take(&self) -> Result<Box<dyn TorrentStorage>> {
        Ok(Box::new(self.clone()))
    }
}
pub struct Seeder {
    control: Control,
}
impl Seeder {
    pub fn start(
        meta: Metadata,
        root: PathBuf,
        settings: Settings,
        report: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Result<Self> {
        ensure!(settings.p2p != 2, "P2P отключён");
        let control = Control::default();
        let c = control.clone();
        std::thread::spawn(move || {
            if settings.p2p == 1 && crate::platform::metered_connection() {
                return;
            }
            let Ok(rt) = tokio::runtime::Runtime::new() else {
                return;
            };
            rt.block_on(async {
                let factory = ReadOnlyFactory {
                    root: root.clone(),
                    paths: Arc::new(meta.files.iter().map(|f| f.path.clone()).collect()),
                    control: c.clone(),
                };
                let options = librqbit::SessionOptions {
                    dht: None,
                    disable_local_service_discovery: true,
                    listen: Some(librqbit::ListenerOptions::default()),
                    ratelimits: librqbit::limits::LimitsConfig {
                        download_bps: None,
                        upload_bps: std::num::NonZeroU32::new(settings.upload.saturating_mul(1024)),
                    },
                    ..Default::default()
                };
                let Ok(session) = librqbit::Session::new_with_opts(root.clone(), options).await
                else {
                    return;
                };
                let result = session
                    .add_torrent(
                        librqbit::AddTorrent::from_bytes(meta.torrent()),
                        Some(librqbit::AddTorrentOptions {
                            output_folder: Some(root.to_string_lossy().into()),
                            overwrite: true,
                            only_files: Some(Vec::new()),
                            storage_factory: Some(factory.boxed()),
                            ..Default::default()
                        }),
                    )
                    .await;
                if let Ok(result) = result
                    && let Some(handle) = result.into_handle()
                {
                    while c.state() != 2 {
                        let stats = handle.stats();
                        if stats.error.is_some() {
                            break;
                        }
                        report(stats.uploaded_bytes);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
                session.stop().await;
            });
        });
        Ok(Self { control })
    }
}
impl Drop for Seeder {
    fn drop(&mut self) {
        self.control.cancel();
    }
}
