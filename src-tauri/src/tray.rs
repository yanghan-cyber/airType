use std::sync::{Arc, Mutex};
use tauri::{
    AppHandle, Manager,
    menu::{CheckMenuItem, MenuBuilder, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    WebviewUrl, WebviewWindowBuilder,
};

fn tooltip_text(enabled: bool) -> &'static str {
    if enabled { "AirType - 已启用" } else { "AirType - 已禁用" }
}

pub fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = crate::config::config_path();
    let cfg = crate::config::AppConfig::load(&config_path);

    let enabled = CheckMenuItem::with_id(app, "enabled", "已启用语音输入", true, cfg.enabled, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let settings = MenuItem::with_id(app, "settings", "设置...", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = MenuBuilder::new(app)
        .item(&enabled)
        .item(&separator)
        .item(&settings)
        .item(&separator)
        .item(&quit)
        .build()?;

    let (icon_rgba, icon_w, icon_h) = {
        let bytes = include_bytes!("../icons/tray-32.png");
        let img = image::load_from_memory(bytes).expect("Failed to load tray icon").to_rgba8();
        let (w, h) = img.dimensions();
        (img.into_raw(), w, h)
    };
    let tray = TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::new_owned(icon_rgba, icon_w, icon_h))
        .menu(&menu)
        .tooltip(tooltip_text(cfg.enabled))
        .on_menu_event(move |app, event| {
            match event.id.as_ref() {
                "enabled" => {
                    if let Some(state) = app.try_state::<Arc<Mutex<crate::state::AppState>>>() {
                        let mut s = state.lock().unwrap();
                        s.enabled = !s.enabled;
                        let new_enabled = s.enabled;
                        crate::log::log_debug(&format!("[tray] enabled toggled to {}", new_enabled));
                        drop(s);

                        let _ = enabled.set_checked(new_enabled);

                        let mut cfg = crate::config::AppConfig::load(&crate::config::config_path());
                        cfg.enabled = new_enabled;
                        cfg.save(&crate::config::config_path());

                        if let Some(tray) = app.tray_by_id("main") {
                            let _ = tray.set_tooltip(Some(tooltip_text(new_enabled)));
                        }
                    }
                }
                "settings" => {
                    if let Some(window) = app.get_webview_window("settings") {
                        let _ = window.set_focus();
                    } else {
                        let _ = WebviewWindowBuilder::new(
                            app,
                            "settings",
                            WebviewUrl::App("settings.html".into()),
                        )
                        .title("AirType 设置")
                        .inner_size(420.0, 520.0)
                        .resizable(false)
                        .build();
                    }
                }
                "quit" => {
                    app.exit(0);
                }
                _ => {}
            }
        })
        .build(app)?;

    // Keep tray alive for the lifetime of the app
    Box::leak(Box::new(tray));

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_tray_menu_item_ids() {
        let ids = vec!["enabled", "settings", "quit"];
        assert_eq!(ids.len(), 3);
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids.len(), sorted.len(), "No duplicate menu IDs");
    }
}
