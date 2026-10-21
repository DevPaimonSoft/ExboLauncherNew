use anyhow::{Context, Result, ensure};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const BOOTSTRAP: &[u8] = include_bytes!("../assets/support/bootstrap.jar");
const BRIDGE: &[u8] = include_bytes!("../native/exbo-hwid.exe");
const HWID: &[u8] = include_bytes!("../native/jmt.dll");

pub fn bootstrap_hash() -> String {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::api::java_hex(&Sha1::digest(BOOTSTRAP)))
        .clone()
}

struct Resource {
    name: &'static str,
    bytes: &'static [u8],
    digest: [u8; 32],
}

struct NativeBundle {
    directory: String,
    resources: [Resource; 2],
}

fn native_bundle() -> &'static NativeBundle {
    static BUNDLE: OnceLock<NativeBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let mut hash = Sha256::new();
        hash.update(BRIDGE);
        hash.update(HWID);
        NativeBundle {
            directory: format!("native-{:x}", hash.finalize()),
            resources: [("exbo-hwid.exe", BRIDGE), ("jmt.dll", HWID)].map(|(name, bytes)| {
                Resource {
                    name,
                    bytes,
                    digest: Sha256::digest(bytes).into(),
                }
            }),
        }
    })
}

fn matches(path: &Path, resource: &Resource) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == resource.bytes.len() as u64)
        && fs::read(path)
            .is_ok_and(|bytes| <[u8; 32]>::from(Sha256::digest(&bytes)) == resource.digest)
}

pub fn native_dir(data: &Path) -> Result<PathBuf> {
    ensure!(
        data.is_absolute(),
        "Каталог данных лаунчера должен быть абсолютным"
    );
    let bundle = native_bundle();
    let dir = data.join("support").join(&bundle.directory);
    for resource in &bundle.resources {
        let path = dir.join(resource.name);
        if !matches(&path, resource) {
            crate::storage::atomic_write(&path, resource.bytes).with_context(|| {
                let name = resource.name;
                format!("Не удалось подготовить встроенный компонент {name}")
            })?;
            ensure!(
                matches(&path, resource),
                "Не удалось проверить компонент {}",
                resource.name
            );
        }
    }
    Ok(dir)
}
