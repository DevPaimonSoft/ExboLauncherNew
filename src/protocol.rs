use anyhow::{Context, Result, ensure};
use std::path::Path;

pub fn callback_url(link: &str) -> Result<url::Url> {
    ensure!(
        link.len() < 8192 && !link.contains(['\0', '\r', '\n']),
        "Некорректная ссылка возврата EXBO"
    );
    let url = url::Url::parse(link)?;
    ensure!(url.scheme() == "exbo", "Ожидалась ссылка exbo://");
    Ok(url)
}

pub fn link_argument(args: &[String]) -> Result<Option<String>> {
    let link = if let Some(index) = args.iter().position(|arg| arg == "--link") {
        Some(
            args.get(index + 1)
                .context("После --link нужна ссылка EXBO")?,
        )
    } else {
        args.iter().skip(1).find(|arg| {
            arg.get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("exbo:"))
        })
    };
    link.map(|link| callback_url(link).map(|_| link.clone()))
        .transpose()
}

fn quote_argument(value: &str) -> Result<String> {
    ensure!(!value.contains('\0'), "Недопустимый символ в пути EXBO");
    let mut result = String::from("\"");
    let mut slashes = 0;
    for c in value.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        result.extend(std::iter::repeat_n(
            '\\',
            slashes * if c == '"' { 2 } else { 1 },
        ));
        if c == '"' {
            result.push('\\');
        }
        result.push(c);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    Ok(result)
}

fn handler_command(exe: &Path, profile: &Path) -> Result<String> {
    Ok(format!(
        "{} --data-dir {} --link \"%1\"",
        quote_argument(exe.to_str().context("Некорректный путь EXE")?)?,
        quote_argument(profile.to_str().context("Некорректный путь профиля")?)?
    ))
}

pub fn register(profile: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let profile = std::path::absolute(profile)?;
    let command = handler_command(&exe, &profile)?;
    #[cfg(windows)]
    {
        use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        let key = r"Software\Classes\exbo";
        set_string(&format!(r"{key}\shell\open\command"), "", &command)?;
        set_string(key, "", "URL:EXBO Launcher New")?;
        set_string(key, "URL Protocol", "")?;
        set_string(
            &format!(r"{key}\Application"),
            "ApplicationName",
            "EXBO Launcher New",
        )?;
        set_string(
            &format!(r"{key}\DefaultIcon"),
            "",
            &format!("\"{}\",0", exe.display()),
        )?;
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
    #[cfg(not(windows))]
    {
        let _ = command;
        anyhow::bail!("Регистрация ссылок EXBO поддерживается на Windows");
    }
    Ok(())
}

#[cfg(windows)]
fn set_string(key: &str, name: &str, value: &str) -> Result<()> {
    use windows::{Win32::System::Registry::*, core::PCWSTR};
    let key: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let data: Vec<u8> = value
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut handle = HKEY::default();
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut handle,
            None,
        )
        .ok()
        .context("Не удалось зарегистрировать обработчик ссылок EXBO")?;
        let result = RegSetValueExW(handle, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(&data)).ok();
        let _ = RegCloseKey(handle);
        result.context("Не удалось записать обработчик ссылок EXBO")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_callback_accepts_registered_and_raw_argument_forms() {
        let link = "exbo://127.0.0.1:43210/?action=distributor_auth&verify_token=a%2Bb%26c";
        for args in [vec!["launcher", "--link", link], vec!["launcher", link]] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            assert_eq!(link_argument(&args).unwrap().as_deref(), Some(link));
        }
        let parsed = callback_url(link).unwrap();
        assert_eq!(
            parsed
                .query_pairs()
                .find(|(key, _)| key == "verify_token")
                .unwrap()
                .1,
            "a+b&c"
        );
        assert!(link_argument(&["launcher".into(), "--link".into()]).is_err());
        assert_eq!(link_argument(&["launcher".into()]).unwrap(), None);
    }

    #[test]
    fn browser_callback_rejects_other_schemes_and_ipc_injection() {
        for link in [
            "https://example.org/?action=distributor_auth",
            "exbo://auth?verify_token=a\nexbo://wake",
            "exbo://auth\0",
            "exbo://auth\r",
        ] {
            assert!(callback_url(link).is_err());
        }
        assert!(callback_url(&format!("exbo://auth?token={}", "x".repeat(8192))).is_err());
    }

    #[test]
    #[cfg(windows)]
    fn registered_command_preserves_executable_profile_and_complete_callback() {
        use windows::{
            Win32::{
                Foundation::{HLOCAL, LocalFree},
                UI::Shell::CommandLineToArgvW,
            },
            core::PCWSTR,
        };
        let exe = r"D:\PC Backup 7\Игры\ExboLauncherNew.exe";
        let profile = "C:\\Тестовый профиль\\";
        let link = "exbo://127.0.0.1:43210/?action=distributor_auth&verify_token=a%2Bb&port=43210";
        let command = handler_command(Path::new(exe), Path::new(profile)).unwrap();
        assert!(!command.contains("ExboLink.exe"));
        let command: Vec<u16> = command
            .replace("%1", link)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let mut count = 0;
            let argv = CommandLineToArgvW(PCWSTR(command.as_ptr()), &mut count);
            assert!(!argv.is_null());
            let args: Vec<String> = std::slice::from_raw_parts(argv, count as usize)
                .iter()
                .map(|arg| arg.to_string().unwrap())
                .collect();
            let _ = LocalFree(HLOCAL(argv.cast()));
            assert_eq!(args, [exe, "--data-dir", profile, "--link", link]);
        }
    }
}
