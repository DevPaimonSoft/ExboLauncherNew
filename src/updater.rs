use crate::{api::Api, platform, storage::atomic_write};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

#[derive(Clone, Deserialize)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub sha256: String,
}
pub fn check(api: &Api, feed: &str) -> Result<Option<Release>> {
    ensure!(
        url::Url::parse(feed)?.scheme() == "https",
        "Манифест обновлений должен использовать HTTPS"
    );
    let release: Release = serde_json::from_slice(&api.external_bytes(feed, 64 * 1024)?)?;
    ensure!(
        release.sha256.len() == 64 && release.sha256.bytes().all(|c| c.is_ascii_hexdigit()),
        "Некорректный SHA-256 обновления"
    );
    ensure!(
        url::Url::parse(&release.url)?.scheme() == "https",
        "Обновление должно загружаться по HTTPS"
    );
    if semver::Version::parse(&release.version)?
        > semver::Version::parse(env!("CARGO_PKG_VERSION"))?
    {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut b = [0; 128 * 1024];
    loop {
        let n = f.read(&mut b)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn stage(api: &Api, release: &Release, root: &Path) -> Result<PathBuf> {
    let bytes = api.external_bytes(&release.url, 256 * 1024 * 1024)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)).eq_ignore_ascii_case(&release.sha256),
        "SHA-256 обновления не совпадает"
    );
    ensure!(
        bytes.starts_with(b"MZ"),
        "Обновление не является Windows EXE"
    );
    let path = root.join("updates/ExboLauncherNew.new.exe");
    atomic_write(&path, &bytes)?;
    Ok(path)
}
pub fn schedule(staged: &Path, expected: &str, root: &Path) -> Result<()> {
    ensure!(
        sha256_file(staged)?.eq_ignore_ascii_case(expected),
        "Обновление повреждено"
    );
    let current = std::env::current_exe()?;
    let helper = root.join("updates/exbo-update-helper.exe");
    fs::copy(&current, &helper)?;
    platform::hidden(
        Command::new(helper)
            .arg("--apply-update")
            .arg(&current)
            .arg(staged)
            .arg(expected)
            .arg(std::process::id().to_string())
            .arg(root),
    )
    .spawn()?;
    Ok(())
}

pub fn apply(args: &[String]) -> Result<()> {
    ensure!(args.len() == 7, "Некорректные аргументы обновления");
    let target = PathBuf::from(&args[2]);
    let staged = PathBuf::from(&args[3]);
    let expected = &args[4];
    let pid: u32 = args[5].parse()?;
    ensure!(
        target.is_absolute() && staged.is_absolute(),
        "Требуются абсолютные пути обновления"
    );
    ensure!(
        target.file_name().is_some_and(|n| n
            .to_string_lossy()
            .eq_ignore_ascii_case("ExboLauncherNew.exe")),
        "Неожиданный файл назначения"
    );
    ensure!(
        staged
            .file_name()
            .is_some_and(|n| n == "ExboLauncherNew.new.exe"),
        "Неожиданный файл обновления"
    );
    ensure!(
        sha256_file(&staged)?.eq_ignore_ascii_case(expected),
        "Обновление повреждено"
    );
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn WaitForSingleObject(handle: *mut std::ffi::c_void, millis: u32) -> u32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            let handle = OpenProcess(0x100000, 0, pid);
            if !handle.is_null() {
                let wait = WaitForSingleObject(handle, 60000);
                CloseHandle(handle);
                ensure!(
                    wait == 0,
                    "Лаунчер не завершил работу; обновление сохранено"
                );
            }
        }
    }
    let backup = target.with_extension("exe.old");
    fs::copy(&target, &backup)?;
    let pending = target.with_extension("exe.new");
    fs::copy(&staged, &pending)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let result = replace_file(&pending, &target);
        match result {
            Ok(()) => break,
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    ensure!(
        sha256_file(&target)?.eq_ignore_ascii_case(expected),
        "Проверка установленного обновления не прошла; резервная копия сохранена"
    );
    Command::new(&target)
        .arg("--data-dir")
        .arg(&args[6])
        .current_dir(target.parent().context("Нет каталога лаунчера")?)
        .spawn()?;
    Ok(())
}
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
        }
        let a: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let b: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        ensure!(
            unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 1 | 8) } != 0,
            "Не удалось заменить EXE: {}",
            std::io::Error::last_os_error()
        );
    }
    #[cfg(not(windows))]
    fs::rename(source, target)?;
    Ok(())
}
