use anyhow::{Context, Result, ensure};
use std::process::{Child, Command};

pub fn validate(selection: Option<&[u16]>) -> Result<()> {
    if let Some(cpus) = selection {
        ensure!(!cpus.is_empty(), "Выберите хотя бы один процессор CPU");
        let mut seen = 0u64;
        for &cpu in cpus {
            ensure!(cpu < 64, "Номер CPU должен быть от 0 до 63");
            let bit = 1u64 << cpu;
            ensure!(seen & bit == 0, "CPU {cpu} указан несколько раз");
            seen |= bit;
        }
    }
    Ok(())
}

fn mask_for(cpus: &[u16], available: u64) -> Result<usize> {
    validate(Some(cpus))?;
    let mask = cpus.iter().fold(0u64, |mask, &cpu| mask | (1u64 << cpu));
    ensure!(
        mask & !available == 0,
        "Выбранные CPU недоступны на этом ПК. Откройте «Оптимизация → Процессор» и измените выбор или отключите ограничение"
    );
    Ok(usize::try_from(mask)?)
}

pub fn available_mask() -> Result<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::{
            GetActiveProcessorGroupCount, GetCurrentProcess, GetProcessAffinityMask,
        };
        ensure!(
            unsafe { GetActiveProcessorGroupCount() } == 1,
            "Выбор CPU пока доступен только на ПК с одной группой процессоров Windows (до 64 логических процессоров)"
        );
        let (mut process, mut system) = (0usize, 0usize);
        unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut process, &mut system) }
            .context("Не удалось получить доступные CPU Windows")?;
        let available = process & system;
        ensure!(available != 0, "Windows не вернула доступные CPU");
        Ok(available as u64)
    }
    #[cfg(not(windows))]
    anyhow::bail!("Выбор CPU поддерживается только в Windows")
}

pub fn validate_current(selection: Option<&[u16]>) -> Result<()> {
    if let Some(cpus) = selection {
        mask_for(cpus, available_mask()?)?;
    }
    Ok(())
}

pub fn spawn(command: &mut Command, selection: Option<&[u16]>) -> Result<Child> {
    let Some(cpus) = selection else {
        return command.spawn().context("Не удалось запустить клиент");
    };
    let mask = mask_for(cpus, available_mask()?)?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows::Win32::System::Threading::{CREATE_NO_WINDOW, CREATE_SUSPENDED};
        command.creation_flags((CREATE_NO_WINDOW | CREATE_SUSPENDED).0);
        let mut child = command.spawn().context("Не удалось запустить клиент")?;
        if let Err(error) = configure_and_resume(&child, mask) {
            if child.kill().is_ok() {
                let _ = child.wait();
            }
            return Err(error.context("Не удалось применить выбор CPU; запуск отменён"));
        }
        Ok(child)
    }
    #[cfg(not(windows))]
    {
        let _ = mask;
        anyhow::bail!("Выбор CPU поддерживается только в Windows")
    }
}

#[cfg(windows)]
fn configure_and_resume(child: &Child, mask: usize) -> Result<()> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows::Win32::{
        Foundation::{ERROR_NO_MORE_FILES, HANDLE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{
                GetProcessIdOfThread, OpenThread, ResumeThread, SetProcessAffinityMask,
                THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
            },
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }?;
    let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot.0) };
    let handle = HANDLE(snapshot.as_raw_handle());
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    unsafe { Thread32First(handle, &mut entry) }?;
    let mut thread_id = None;
    loop {
        if entry.th32OwnerProcessID == child.id() {
            ensure!(
                thread_id.is_none(),
                "У нового процесса больше одного потока"
            );
            thread_id = Some(entry.th32ThreadID);
        }
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        match unsafe { Thread32Next(handle, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
            Err(error) => return Err(error.into()),
        }
    }
    let thread_id = thread_id.context("Не найден начальный поток клиента")?;
    let thread = unsafe {
        OpenThread(
            THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
            false,
            thread_id,
        )
    }?;
    let thread = unsafe { OwnedHandle::from_raw_handle(thread.0) };
    ensure!(
        unsafe { GetProcessIdOfThread(HANDLE(thread.as_raw_handle())) } == child.id(),
        "Начальный поток клиента уже завершён"
    );
    unsafe { SetProcessAffinityMask(HANDLE(child.as_raw_handle()), mask) }?;
    let previous = unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) };
    if previous == u32::MAX {
        return Err(std::io::Error::last_os_error().into());
    }
    ensure!(
        previous == 1,
        "Неожиданное состояние начального потока клиента"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_selection_is_checked_without_truncation() {
        assert!(validate(None).is_ok());
        assert!(validate(Some(&[])).is_err());
        assert!(validate(Some(&[0, 0])).is_err());
        assert!(validate(Some(&[64])).is_err());
        assert_eq!(mask_for(&[1, 3], 0b1111).unwrap(), 0b1010);
        assert!(mask_for(&[4], 0b1111).is_err());
        #[cfg(target_pointer_width = "64")]
        assert_eq!(mask_for(&[63], u64::MAX).unwrap(), 1usize << 63);
    }
}
