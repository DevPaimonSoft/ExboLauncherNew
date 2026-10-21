use crate::{model::Settings, platform};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn parse_args(text: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut active = false;
    for c in text.chars() {
        if c == '"' {
            quoted = !quoted;
            active = true;
        } else if c.is_whitespace() && !quoted {
            if active {
                args.push(std::mem::take(&mut word));
                active = false;
            }
        } else {
            word.push(c);
            active = true;
        }
    }
    ensure!(!quoted, "Незакрытая кавычка в команде запуска");
    if active {
        args.push(word);
    }
    ensure!(!args.is_empty(), "Пустая команда запуска");
    Ok(args)
}
pub fn bake(
    command: &str,
    root: &Path,
    user: &str,
    display: &str,
    session: &str,
    memory: u64,
) -> Result<Vec<String>> {
    let java = root
        .join(if cfg!(target_pointer_width = "64") {
            "win64"
        } else {
            "win32"
        })
        .to_string_lossy()
        .into_owned();
    let mut utf16 = vec![0xfe, 0xff];
    for unit in display.encode_utf16() {
        utf16.extend(unit.to_be_bytes());
    }
    let display = STANDARD.encode(utf16);
    let cd = root.to_string_lossy();
    let memory = memory.to_string();
    Ok(parse_args(command)?
        .into_iter()
        .map(|arg| {
            arg.replace("<java>", &java)
                .replace("<cd>", &cd)
                .replace("<usr>", user)
                .replace("<sess>", session)
                .replace("<mem>", &memory)
                .replace('/', "\\")
                .replace("<dlogin>", &display)
        })
        .collect())
}
pub fn start(
    command: &str,
    root: &Path,
    user: &str,
    display: &str,
    session: &str,
    settings: &Settings,
    logs: &Path,
) -> Result<Child> {
    ensure!(root.is_dir(), "Клиент не установлен");
    let mut args = bake(
        command,
        root,
        user,
        display,
        session,
        platform::heap_size(settings.ram),
    )?;
    fs::create_dir_all(logs)?;
    let log = fs::File::create(logs.join("game.log"))?;
    let executable = std::path::PathBuf::from(&args[0]);
    let executable = if executable.is_absolute() {
        executable
    } else {
        root.join(executable)
    };
    ensure!(executable.is_file(), "Не найден исполняемый файл клиента");
    let optimization = &settings.optimization;
    if optimization.enabled || !optimization.tunnels.is_empty() {
        optimization.validate()?;
        crate::optimization::validate_interceptor(&executable)?;
        let target = crate::optimization::jvm_target_index(&args)?;
        let java_executable = std::path::PathBuf::from(&args[target]);
        let java_executable = if java_executable.is_absolute() {
            java_executable
        } else {
            root.join(java_executable)
        };
        ensure!(
            java_executable.is_file(),
            "Не найдена Java-программа клиента; восстановите файлы игры"
        );
        if target != 0 {
            crate::optimization::validate_interceptor(&java_executable)?;
        }
        let hardware = crate::optimization::Hardware::detect();
        if optimization.enabled {
            optimization
                .profile_for(hardware)
                .validate_hardware(hardware)?;
            crate::optimization::validate_runtime(&java_executable)?;
        }
        args = crate::optimization::inject(
            &args,
            &optimization.flags(hardware),
            optimization.enabled,
        )?;
    }
    let mut command = Command::new(executable);
    command
        .args(&args[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    crate::optimization::cpu::spawn(
        platform::hidden(&mut command),
        optimization.cpu_affinity.as_deref(),
    )
}

pub fn plain_news(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        if c == '<' {
            tag = true;
            out.push(' ');
        } else if c == '>' {
            tag = false;
        } else if !tag {
            out.push(c);
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
