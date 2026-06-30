use serde::Serialize;
use tauri::{Emitter, Manager};
use crate::log::log_debug;

/// Fixed capsule window size for the whole recording cycle. Wide enough for
/// realtime text so we never resize at runtime (resizing makes the centered
/// capsule jump → flicker). All position math MUST use these consistently so
/// the capsule stays where the user placed it.
pub const CAPSULE_WIN_W: f64 = 520.0;
pub const CAPSULE_WIN_H: f64 = 80.0;

fn load_config() -> crate::config::AppConfig {
    crate::config::AppConfig::load(&crate::config::config_path())
}

fn config_path() -> std::path::PathBuf {
    crate::config::config_path()
}

#[derive(Debug, Clone, Serialize)]
pub struct CapsuleState {
    pub phase: String,
    pub rms: f32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

#[tauri::command]
pub fn get_capsule_state(
    state: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>,
    buffer: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::audio::AudioBuffer>>>,
) -> CapsuleState {
    let s = state.lock().unwrap();
    let buf = buffer.lock().unwrap();
    let rms = crate::audio::calculate_rms(&buf.as_i16_slice());
    match &s.recording {
        crate::state::RecordingState::Idle => CapsuleState {
            phase: "idle".into(), rms: 0.0, elapsed_ms: 0, error: None,
        },
        crate::state::RecordingState::Recording { started_at } => CapsuleState {
            phase: "recording".into(), rms,
            elapsed_ms: started_at.elapsed().as_millis() as u64, error: None,
        },
        crate::state::RecordingState::RealtimeRecording { started_at, .. } => CapsuleState {
            phase: "recording".into(), rms,
            elapsed_ms: started_at.elapsed().as_millis() as u64, error: None,
        },
        crate::state::RecordingState::Processing => CapsuleState {
            phase: "loading".into(), rms, elapsed_ms: 0, error: None,
        },
        crate::state::RecordingState::LlmProcessing { .. } => CapsuleState {
            phase: "llm_processing".into(), rms, elapsed_ms: 0, error: None,
        },
        crate::state::RecordingState::ModeSelection { .. } => CapsuleState {
            phase: "mode_selection".into(), rms, elapsed_ms: 0, error: None,
        },
        crate::state::RecordingState::Done => CapsuleState {
            phase: "done".into(), rms, elapsed_ms: 0, error: None,
        },
        crate::state::RecordingState::Error(msg) => CapsuleState {
            phase: "error".into(), rms: 0.0, elapsed_ms: 0, error: Some(msg.clone()),
        },
    }
}

#[tauri::command]
pub fn show_capsule_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("capsule") {
        win.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn close_capsule_window_cmd(
    app: tauri::AppHandle,
    state: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>,
) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("capsule") {
        win.hide().map_err(|e| e.to_string())?;
    }
    let mut s = state.lock().unwrap();
    let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Idle);
    Ok(())
}

#[tauri::command]
pub fn resize_capsule_window(app: tauri::AppHandle, width: f64, height: f64, follow_capsule: Option<bool>) -> Result<(), String> {
    // Extra padding for box-shadow effects
    const SHADOW_PAD: f64 = 20.0;

    if let Some(win) = app.get_webview_window("capsule") {
        let scale = win.scale_factor().unwrap_or(1.0);
        let physical_w = ((width + SHADOW_PAD * 2.0) * scale) as u32;
        let physical_h = ((height + SHADOW_PAD * 2.0) * scale) as u32;

        // Get current position and size to keep center stable
        let old_pos = win.outer_position().ok();
        let old_size = win.outer_size().ok();

        win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(physical_w, physical_h)))
            .map_err(|e| e.to_string())?;

        // Adjust position to keep center stable
        if let (Some(pos), Some(old)) = (old_pos, old_size) {
            let dx = (old.width as i32 - physical_w as i32) / 2;
            let dy = (old.height as i32 - physical_h as i32) / 2;
            let _ = win.set_position(tauri::Position::Physical(
                tauri::PhysicalPosition::new(pos.x + dx, pos.y + dy)
            ));
        } else if let Ok(Some(monitor)) = win.primary_monitor() {
            // Fallback: use config position
            let screen_w = monitor.size().width as f64;
            let screen_h = monitor.size().height as f64;
            let cfg = load_config();
            let (x, y) = cfg.capsule_position(
                screen_w, screen_h,
                physical_w as f64, physical_h as f64, scale,
            );
            let _ = win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn get_config() -> serde_json::Value {
    let cfg = load_config();
    serde_json::to_value(cfg).unwrap_or_default()
}

#[tauri::command]
pub fn update_hotwords(hotwords: Vec<String>) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.hotwords = hotwords;
    cfg.save(&path);
    Ok(())
}

#[tauri::command]
pub fn save_config(
    enabled: Option<bool>,
    hotkey: Option<String>,
    language: Option<String>,
    model: Option<String>,
    hotwords: Option<Vec<String>>,
    backend_url: Option<String>,
    asr_api_key: Option<String>,
    realtime_asr: Option<bool>,
    hotkey_config: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::hotkey::HotkeyConfig>>>,
) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    if let Some(v) = enabled { cfg.enabled = v; }
    if let Some(ref v) = hotkey {
        cfg.hotkey = v.clone();
        if let Ok(new_hc) = crate::hotkey::HotkeyConfig::from_string(v) {
            let mut hc = hotkey_config.lock().unwrap();
            let max_hold = hc.max_hold_secs;
            *hc = new_hc;
            hc.max_hold_secs = max_hold;
        }
    }
    cfg.language = language;
    if let Some(v) = model { cfg.model = v; }
    if let Some(v) = hotwords { cfg.hotwords = v; }
    if let Some(v) = backend_url { cfg.backend_url = v; }
    if let Some(v) = asr_api_key { cfg.asr_api_key = v; }
    if let Some(v) = realtime_asr { cfg.realtime_asr = v; }
    cfg.save(&path);
    Ok(())
}

#[tauri::command]
pub async fn fetch_asr_model_list() -> serde_json::Value {
    tokio::task::spawn_blocking(|| {
        let cfg = load_config();
        let client = crate::asr::AsrClient::new(&cfg.backend_url, &cfg.asr_api_key);
        match client.list_models() {
            Ok(models) => serde_json::json!({"success": true, "models": models}),
            Err(e) => serde_json::json!({"success": false, "error": e.to_string()}),
        }
    }).await.unwrap_or_else(|_| serde_json::json!({"success": false, "error": "任务异常"}))
}

#[tauri::command]
pub fn enter_position_mode(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("capsule") {
        let cfg = load_config();
        if let Ok(Some(monitor)) = win.primary_monitor() {
            let scale = monitor.scale_factor();
            // MUST set_size before set_position: the window may have been
            // resized to a small size (e.g. showDone → 60x60) after the last
            // recording. Computing the position with 520x80 while the window
            // is actually 60x60 causes the position-mode capsule to misalign
            // with the recording capsule.
            let phys_w = (CAPSULE_WIN_W * scale) as u32;
            let phys_h = (CAPSULE_WIN_H * scale) as u32;
            let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(phys_w, phys_h)));
            let (x, y) = cfg.capsule_position(
                monitor.size().width as f64, monitor.size().height as f64,
                CAPSULE_WIN_W * scale, CAPSULE_WIN_H * scale, scale,
            );
            win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)))
                .map_err(|e| e.to_string())?;
        }
        win.show().map_err(|e| e.to_string())?;
        win.emit("position-mode", true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn save_capsule_position(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("capsule") {
        let pos = win.outer_position().map_err(|e| e.to_string())?;
        let path = config_path();
        let mut cfg = crate::config::AppConfig::load(&path);
        cfg.capsule_x = Some(pos.x);
        cfg.capsule_y = Some(pos.y);
        cfg.save(&path);
        win.emit("position-mode", false).map_err(|e| e.to_string())?;
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn reset_capsule_position(app: tauri::AppHandle) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.capsule_x = None;
    cfg.capsule_y = None;
    cfg.save(&path);
    if let Some(win) = app.get_webview_window("capsule") {
        if let Ok(Some(monitor)) = win.primary_monitor() {
            let scale = monitor.scale_factor();
            let phys_w = (CAPSULE_WIN_W * scale) as u32;
            let phys_h = (CAPSULE_WIN_H * scale) as u32;
            let _ = win.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(phys_w, phys_h)));
            let (x, y) = cfg.capsule_position(
                monitor.size().width as f64, monitor.size().height as f64,
                CAPSULE_WIN_W * scale, CAPSULE_WIN_H * scale, scale,
            );
            win.set_position(tauri::Position::Physical(tauri::PhysicalPosition::new(x, y)))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

// ── LLM config commands ──

#[tauri::command]
pub fn get_llm_config() -> serde_json::Value {
    let cfg = load_config();
    serde_json::to_value(&cfg.llm).unwrap_or_default()
}

#[tauri::command]
pub fn save_llm_config(
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    extra_params: Option<String>,
) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    if let Some(v) = base_url { cfg.llm.base_url = v; }
    if let Some(v) = api_key { cfg.llm.api_key = v; }
    if let Some(v) = model { cfg.llm.model = v; }
    if let Some(v) = temperature { cfg.llm.temperature = v; }
    if let Some(v) = max_tokens { cfg.llm.max_tokens = v; }
    if extra_params.is_some() { cfg.llm.extra_params = extra_params; }
    cfg.save(&path);
    Ok(())
}

#[tauri::command]
pub async fn test_llm_connection() -> serde_json::Value {
    tokio::task::spawn_blocking(|| {
        let cfg = load_config();
        let client = crate::llm::LlmClient::new(cfg.llm);
        match client.health() {
            Ok(true) => serde_json::json!({"connected": true}),
            Ok(false) => serde_json::json!({"connected": false, "error": "API 返回错误"}),
            Err(e) => serde_json::json!({"connected": false, "error": e.to_string()}),
        }
    }).await.unwrap_or_else(|_| serde_json::json!({"connected": false, "error": "任务异常"}))
}

#[tauri::command]
pub async fn fetch_model_list() -> serde_json::Value {
    tokio::task::spawn_blocking(|| {
        let cfg = load_config();
        let client = crate::llm::LlmClient::new(cfg.llm);
        match client.list_models() {
            Ok(models) => serde_json::json!({"success": true, "models": models}),
            Err(e) => serde_json::json!({"success": false, "error": e.to_string()}),
        }
    }).await.unwrap_or_else(|_| serde_json::json!({"success": false, "error": "任务异常"}))
}

// ── Processing mode commands ──

#[tauri::command]
pub fn get_processing_modes() -> serde_json::Value {
    let cfg = load_config();
    serde_json::to_value(&cfg.processing_modes).unwrap_or_default()
}

#[tauri::command]
pub fn save_processing_mode(mode: crate::config::ProcessingMode) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    if let Some(existing) = cfg.processing_modes.iter_mut().find(|m| m.id == mode.id) {
        *existing = mode;
    } else {
        cfg.processing_modes.push(mode);
    }
    cfg.save(&path);
    Ok(())
}

#[tauri::command]
pub fn delete_processing_mode(mode_id: String) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.processing_modes.retain(|m| m.id != mode_id);
    cfg.save(&path);
    Ok(())
}

#[tauri::command]
pub fn get_default_processing_mode() -> String {
    let cfg = load_config();
    cfg.default_processing_mode
}

#[tauri::command]
pub fn save_default_processing_mode(mode_id: String) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.default_processing_mode = mode_id;
    cfg.save(&path);
    Ok(())
}

// ── Secondary hotkey commands ──

#[tauri::command]
pub fn get_secondary_hotkey() -> Option<String> {
    let cfg = load_config();
    cfg.hotkey_secondary
}

#[tauri::command]
pub fn save_secondary_hotkey(
    hotkey: Option<String>,
    secondary_hotkey_config: tauri::State<'_, std::sync::Arc<std::sync::Mutex<Option<crate::hotkey::HotkeyConfig>>>>,
) -> Result<(), String> {
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.hotkey_secondary = hotkey.clone();
    cfg.save(&path);

    let mut hc = secondary_hotkey_config.lock().unwrap();
    *hc = hotkey.and_then(|s| crate::hotkey::HotkeyConfig::from_string(&s).ok());

    Ok(())
}

// ── Capsule position commands ──

#[tauri::command]
pub fn save_capsule_default_position(position: String, offset: u32) -> Result<(), String> {
    if position != "bottom" && position != "top" {
        return Err("position must be 'bottom' or 'top'".into());
    }
    let path = config_path();
    let mut cfg = crate::config::AppConfig::load(&path);
    cfg.capsule_default_position = position;
    cfg.capsule_default_offset = offset;
    cfg.save(&path);
    Ok(())
}

// ── Cancel LLM processing ──

#[tauri::command]
pub fn cancel_llm_processing(
    state: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>,
) -> Result<(), String> {
    let mut s = state.lock().unwrap();
    match &s.recording {
        crate::state::RecordingState::LlmProcessing { .. } => {
            log_debug("[cancel_llm] ESC during LLM → Idle (no inject)");
            crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Idle).map(|_| ()).map_err(|e| e)
        }
        _ => Err("Not in LLM processing state".into()),
    }
}

// ── Processing mode selection command ──

#[tauri::command]
pub fn select_processing_mode(
    mode_id: String,
    state: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::state::AppState>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    log_debug(&format!("[select_mode] Called with mode_id={}", mode_id));
    let mut s = state.lock().unwrap();
    let text = match &s.recording {
        crate::state::RecordingState::ModeSelection { text } => text.clone(),
        other => {
            log_debug(&format!("[select_mode] Wrong state: {:?}", other));
            return Err("Not in mode selection state".into());
        }
    };
    let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::LlmProcessing { text: text.clone() });
    drop(s);
    log_debug(&format!("[select_mode] Text='{}', transitioned to LlmProcessing", text));

    let cfg = load_config();
    let mode = cfg.processing_modes.iter()
        .find(|m| m.id == mode_id)
        .cloned()
        .ok_or_else(|| "Mode not found".to_string())?;
    log_debug(&format!("[select_mode] Mode: id={}", mode.id));

    let state_clone = state.inner().clone();
    let app_clone = app.clone();
    std::thread::spawn(move || {
        match crate::util::run_llm_with_fallback(&text, &cfg, &mode) {
            Ok(result_text) => {
                let mut s = state_clone.lock().unwrap();
                let still_active = matches!(s.recording, crate::state::RecordingState::LlmProcessing { .. });
                if still_active {
                    let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Done);
                    drop(s);
                    let _ = crate::inject::inject_text(&result_text);
                } else {
                    log_debug(&format!("[select_mode] Cancelled before injection, discarding"));
                    if !matches!(s.recording, crate::state::RecordingState::Idle) {
                        let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Idle);
                    }
                }
            }
            Err(e) => {
                let mut s = state_clone.lock().unwrap();
                let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Error(e));
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(2000));
        let mut s = state_clone.lock().unwrap();
        if matches!(s.recording, crate::state::RecordingState::Done | crate::state::RecordingState::Error(_)) {
            let _ = crate::state::transition_to(&mut s.recording, crate::state::RecordingState::Idle);
            drop(s);
            if let Some(win) = app_clone.get_webview_window("capsule") {
                let _ = win.hide();
            }
        }
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capsule_state_serialization() {
        let state = CapsuleState {
            phase: "recording".into(), rms: 0.5, elapsed_ms: 1500, error: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("recording"));
        assert!(json.contains("1500"));
    }

    #[test]
    fn test_capsule_state_error_variant() {
        let state = CapsuleState {
            phase: "error".into(), rms: 0.0, elapsed_ms: 0,
            error: Some("后端未连接".into()),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("后端未连接"));
    }

    #[test]
    fn test_capsule_state_all_phases() {
        for phase in &["idle", "recording", "loading", "done", "error"] {
            let state = CapsuleState {
                phase: phase.to_string(), rms: 0.0, elapsed_ms: 0, error: None,
            };
            let json = serde_json::to_string(&state).unwrap();
            assert!(json.contains(phase));
        }
    }
}
