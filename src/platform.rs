use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

#[cfg(windows)]
pub fn registry_string(key: &str, name: &str) -> Option<String> {
    use windows::{Win32::System::Registry::*, core::PCWSTR};
    let k: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let n: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_err()
        {
            return None;
        }
        let mut data = vec![0u16; size as usize / 2 + 1];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(k.as_ptr()),
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .is_err()
        {
            return None;
        }
        Some(String::from_utf16_lossy(
            &data[..data.iter().position(|&x| x == 0).unwrap_or(data.len())],
        ))
    }
}
#[cfg(not(windows))]
pub fn registry_string(_: &str, _: &str) -> Option<String> {
    None
}
pub fn legacy_user_id() -> Option<String> {
    registry_string("SOFTWARE", "user_id")
}
pub fn legacy_root() -> Option<PathBuf> {
    if let Some(path) = registry_string("Software\\EXBO", "root") {
        let p = PathBuf::from(path.trim_matches('"'));
        if p.join("bootstrap.jar").exists() {
            return Some(p);
        }
    }
    let p = PathBuf::from(r"C:\EXBO\EXBO");
    p.join("bootstrap.jar").exists().then_some(p)
}

pub fn hwid(data: &Path) -> Result<String> {
    let dir = crate::support::native_dir(data)?;
    let output = hidden(Command::new(dir.join("exbo-hwid.exe")).arg(dir.join("jmt.dll")))
        .output()
        .context("Не удалось запустить встроенный компонент HWID")?;
    ensure!(
        output.status.success() && output.stdout.len() == 40,
        "Нативный провайдер HWID завершился с ошибкой"
    );
    Ok(output.stdout.iter().map(|x| format!("{x:02x}")).collect())
}

#[cfg(windows)]
fn crypt(data: &[u8], decrypt: bool) -> Result<Vec<u8>> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::*,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len().try_into()?,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData.cast()));
        Ok(result)
    }
}
#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Result<Vec<u8>> {
    anyhow::bail!("Сохранение сессии поддерживается на Windows")
}
pub fn protect(session: &str) -> Result<String> {
    Ok(STANDARD.encode(crypt(session.as_bytes(), false)?))
}
pub fn unprotect(session: &str) -> Result<String> {
    Ok(String::from_utf8(crypt(&STANDARD.decode(session)?, true)?)?)
}

pub fn open_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url)?;
    ensure!(
        matches!(parsed.scheme(), "https" | "http"),
        "Разрешены только ссылки HTTP(S)"
    );
    shell_open(url)
}
pub fn open_path(path: &Path) -> Result<()> {
    ensure!(path.exists(), "Каталог ещё не создан");
    shell_open(&path.to_string_lossy())
}
#[cfg(windows)]
fn shell_open(target: &str) -> Result<()> {
    use windows::{
        Win32::{
            Foundation::HWND,
            UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
        },
        core::PCWSTR,
    };
    let value: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            HWND::default(),
            None,
            PCWSTR(value.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    ensure!(result.0 as isize > 32, "Windows не удалось открыть ресурс");
    Ok(())
}
#[cfg(not(windows))]
fn shell_open(target: &str) -> Result<()> {
    Command::new("xdg-open").arg(target).spawn()?;
    Ok(())
}

pub fn heap_size(ram: f64) -> u64 {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::*;
        let mut info = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut info) }.is_ok() {
            let max = if cfg!(target_pointer_width = "32") {
                256
            } else {
                (info.ullTotalPhys / 1024 / 1024)
                    .saturating_sub(1024)
                    .min(3584)
            };
            return 512 + (max as f64 * ram.clamp(0., 1.)) as u64;
        }
    }
    512 + (3584. * ram.clamp(0., 1.)) as u64
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut cmd = Command::new("reg.exe");
    if enabled {
        cmd.args([
            "add",
            key,
            "/v",
            "ExboLauncherNew",
            "/t",
            "REG_SZ",
            "/f",
            "/d",
        ])
        .arg(format!(
            "\"{}\" --autostart",
            std::env::current_exe()?.display()
        ));
    } else {
        cmd.args(["delete", key, "/v", "ExboLauncherNew", "/f"]);
    }
    let output = hidden(&mut cmd).output()?;
    if enabled {
        ensure!(output.status.success(), "Не удалось включить автозапуск");
    }
    Ok(())
}

pub fn metered_connection() -> bool {
    let script = "[void][Windows.Networking.Connectivity.NetworkInformation,Windows,ContentType=WindowsRuntime]; $p=[Windows.Networking.Connectivity.NetworkInformation]::GetInternetConnectionProfile(); if($null -eq $p){'True'}else{$c=$p.GetConnectionCost(); $c.ApproachingDataLimit -or $c.OverDataLimit -or $c.Roaming -or $c.BackgroundDataUsageRestricted -or ($c.NetworkCostType -ne 'Unrestricted')}";
    hidden(Command::new("powershell.exe").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]))
    .output()
    .ok()
    .filter(|x| x.status.success())
    .map(|x| {
        String::from_utf8_lossy(&x.stdout)
            .trim()
            .eq_ignore_ascii_case("True")
    })
    .unwrap_or(true)
}

pub fn defender_status(path: &Path) -> Result<String> {
    let script = "$ErrorActionPreference='Stop'; $s=Get-MpComputerStatus; $p=Get-MpPreference; $target=[IO.Path]::GetFullPath($env:EXBO_TARGET).TrimEnd('\\'); $excluded=@($p.ExclusionPath | Where-Object { $base=[IO.Path]::GetFullPath($_).TrimEnd('\\'); $target.Equals($base,[StringComparison]::OrdinalIgnoreCase) -or $target.StartsWith($base+'\\',[StringComparison]::OrdinalIgnoreCase) }).Count -gt 0; @{enabled=$s.RealTimeProtectionEnabled; excluded=$excluded} | ConvertTo-Json -Compress";
    let output = hidden(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("EXBO_TARGET", path),
    )
    .output()?;
    ensure!(
        output.status.success(),
        "Windows Defender недоступен или управляется политикой системы"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(format!(
        "Windows Defender: {}; каталог {}",
        if value["enabled"] == true {
            "включён"
        } else {
            "выключен"
        },
        if value["excluded"] == true {
            "в исключениях"
        } else {
            "не в исключениях"
        }
    ))
}

pub fn add_defender_exclusion(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.is_dir(),
        "Выберите существующий каталог игры"
    );
    let encoded = STANDARD.encode(path.to_string_lossy().as_bytes());
    let script = format!(
        "$ErrorActionPreference='Stop'; Add-MpPreference -ExclusionPath ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')))"
    );
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let args = format!(
        "-NoProfile -NonInteractive -EncodedCommand {}",
        STANDARD.encode(bytes)
    );
    #[cfg(windows)]
    {
        use windows::{
            Win32::UI::{
                Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
                WindowsAndMessaging::SW_HIDE,
            },
            core::PCWSTR,
        };
        let verb: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
        let exe: Vec<u16> = "powershell.exe".encode_utf16().chain(Some(0)).collect();
        let args: Vec<u16> = args.encode_utf16().chain(Some(0)).collect();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(exe.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn WaitForSingleObject(handle: *mut std::ffi::c_void, millis: u32) -> u32;
            fn GetExitCodeProcess(handle: *mut std::ffi::c_void, code: *mut u32) -> i32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            ShellExecuteExW(&mut info)?;
            let handle = info.hProcess.0;
            let wait = WaitForSingleObject(handle, 120000);
            let mut code = 1;
            GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ensure!(
                wait == 0 && code == 0,
                "Не удалось добавить исключение Defender"
            );
        }
    }
    Ok(())
}

pub fn create_shortcut(path: &Path, id: &str) -> Result<()> {
    crate::storage::safe_id(id)?;
    let exe = std::env::current_exe()?;
    let target = path.join(format!("EXBO-{id}.lnk"));
    let script = "$w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut($env:EXBO_SHORTCUT); $s.TargetPath=$env:EXBO_EXECUTABLE; $s.Arguments=$env:EXBO_ARGUMENTS; $s.WorkingDirectory=[IO.Path]::GetDirectoryName($env:EXBO_EXECUTABLE); $s.IconLocation=$env:EXBO_EXECUTABLE+',0'; $s.Save()";
    let output = hidden(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("EXBO_SHORTCUT", &target)
            .env("EXBO_EXECUTABLE", exe)
            .env("EXBO_ARGUMENTS", format!("--server {id}")),
    )
    .output()?;
    ensure!(
        output.status.success() && target.is_file(),
        "Не удалось создать ярлык"
    );
    Ok(())
}

#[cfg(windows)]
pub struct InstanceGuard(*mut std::ffi::c_void);
#[cfg(windows)]
impl InstanceGuard {
    pub fn acquire(root: &Path) -> Result<Option<Self>> {
        use sha1::{Digest, Sha1};
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateMutexW(
                attr: *const std::ffi::c_void,
                owner: i32,
                name: *const u16,
            ) -> *mut std::ffi::c_void;
            fn GetLastError() -> u32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let name = format!(
            "Local\\ExboLauncherNew-{}",
            crate::api::java_hex(&Sha1::digest(
                root.to_string_lossy().to_lowercase().as_bytes()
            ))
        );
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            ensure!(
                !handle.is_null(),
                "Не удалось создать блокировку экземпляра"
            );
            if GetLastError() == 183 {
                CloseHandle(handle);
                Ok(None)
            } else {
                Ok(Some(Self(handle)))
            }
        }
    }
}
#[cfg(windows)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            CloseHandle(self.0);
        }
    }
}
