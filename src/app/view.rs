use super::*;

pub(super) fn strings(values: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    Rc::new(VecModel::from(
        values.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
    .into()
}
pub(super) fn error(ui: &LauncherWindow, e: impl std::fmt::Display) {
    ui.set_error_message(e.to_string().into());
}
pub(super) fn image(path: &Path) -> slint::Image {
    slint::Image::load_from_path(path).unwrap_or_default()
}
pub(crate) fn restores_from_tray(event: &tray_icon::TrayIconEvent) -> bool {
    matches!(
        event,
        tray_icon::TrayIconEvent::DoubleClick {
            button: tray_icon::MouseButton::Left,
            ..
        }
    )
}
pub(super) fn restore_window(ui: &LauncherWindow) {
    let _ = ui.show();
    ui.window().set_minimized(false);
    ui.window()
        .with_winit_window(|window| window.focus_window());
}
pub(super) fn set_window_size(ui: &LauncherWindow, width: f32, height: f32) {
    let scale = ui.window().scale_factor();
    ui.window().set_size(slint::PhysicalSize::new(
        (width * scale).round() as u32,
        (height * scale).round() as u32,
    ));
}
pub(crate) fn size_label(bytes: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit + 1 < units.len() {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", units[unit])
    }
}
pub(crate) fn progress_label(p: &download::Progress, paused: bool) -> String {
    let amount = format!(
        "{} / {}",
        size_label(p.done.min(p.total)),
        size_label(p.total)
    );
    match p.phase {
        download::Phase::Checking => format!("Проверено: {amount}"),
        download::Phase::Downloading => format!(
            "Загрузка: {amount} · {}/s",
            size_label(if paused { 0 } else { p.rate })
        ),
        download::Phase::Ready => {
            if p.total == 0 {
                "Все файлы проверены".into()
            } else {
                format!("Восстановлено: {}", size_label(p.total))
            }
        }
    }
}

pub(super) fn tray() -> Result<(
    tray_icon::TrayIcon,
    tray_icon::menu::MenuId,
    tray_icon::menu::MenuId,
)> {
    use tray_icon::{
        Icon, TrayIconBuilder,
        menu::{Menu, MenuItem},
    };
    let menu = Menu::new();
    let show = MenuItem::new("Открыть EXBO Launcher", true, None);
    let quit = MenuItem::new("Выход", true, None);
    menu.append(&show)?;
    menu.append(&quit)?;
    let img = ::image::load_from_memory(include_bytes!("../../assets/images/tray-icon.png"))?
        .into_rgba8();
    let (w, h) = img.dimensions();
    let icon = Icon::from_rgba(img.into_raw(), w, h)?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("EXBO Launcher New")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok((tray, show.id().clone(), quit.id().clone()))
}
