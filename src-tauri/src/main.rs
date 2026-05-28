#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod asr;
mod audio;
mod commands;
mod config;
mod hotkey;
mod inject;
mod llm;
mod log;
mod state;
mod toast;
mod tray;
mod util;

use state::{AppState, RecordingState, transition_to};
use hotkey::{HotkeyConfig, DualHotkeyTracker, HotkeySource, HotkeyTransition};
use audio::AudioBuffer;
use log::log_debug;
use cpal::traits::{HostTrait, StreamTrait, DeviceTrait};
use rdev::{Event, grab};
use tauri::{Manager, Emitter};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Wrapper for on-demand audio stream.
/// cpal::Stream is !Send due to `NotSendSyncAcrossAllPlatforms`, but it's only ever
/// created and dropped from the rdev::grab callback thread, so this is safe.
struct AudioStreamHolder(Mutex<Option<cpal::Stream>>);
unsafe impl Send for AudioStreamHolder {}
unsafe impl Sync for AudioStreamHolder {}

/// Shared context for the hotkey callback thread
struct PipelineCtx {
    state: Arc<Mutex<AppState>>,
    buffer: Arc<Mutex<AudioBuffer>>,
    sample_rate: Arc<Mutex<u32>>,
    stream: Arc<AudioStreamHolder>,
    app_handle: tauri::AppHandle,
    hotkey_config: Arc<Mutex<HotkeyConfig>>,
    secondary_hotkey_config: Arc<Mutex<Option<HotkeyConfig>>>,
}

fn main() {
    let config_path = config::config_path();
    let cfg = config::AppConfig::load(&config_path);

    let app_state = AppState::new();
    let audio_buffer: Arc<Mutex<AudioBuffer>> = Arc::new(Mutex::new(AudioBuffer::new(16000)));
    let audio_sample_rate: Arc<Mutex<u32>> = Arc::new(Mutex::new(16000));

    let hotkey_config = Arc::new(Mutex::new(
        HotkeyConfig::from_string(&cfg.hotkey)
            .unwrap_or_else(|_| HotkeyConfig::default_ctrl_win())
    ));
    {
        let mut hc = hotkey_config.lock().unwrap();
        hc.max_hold_secs = 180;
    }
    let secondary_hotkey_config: Arc<Mutex<Option<HotkeyConfig>>> = Arc::new(Mutex::new(
        cfg.hotkey_secondary.as_ref().and_then(|s| HotkeyConfig::from_string(s).ok())
    ));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(app_state.clone())
        .manage(audio_buffer.clone())
        .manage(hotkey_config.clone())
        .manage(secondary_hotkey_config.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_capsule_state,
            commands::show_capsule_window,
            commands::close_capsule_window_cmd,
            commands::resize_capsule_window,
            commands::get_config,
            commands::update_hotwords,
            commands::save_config,
            commands::fetch_asr_model_list,
            commands::enter_position_mode,
            commands::save_capsule_position,
            commands::reset_capsule_position,
            commands::save_capsule_default_position,
            commands::get_llm_config,
            commands::save_llm_config,
            commands::test_llm_connection,
            commands::fetch_model_list,
            commands::get_processing_modes,
            commands::save_processing_mode,
            commands::delete_processing_mode,
            commands::get_default_processing_mode,
            commands::save_default_processing_mode,
            commands::get_secondary_hotkey,
            commands::save_secondary_hotkey,
            commands::select_processing_mode,
            commands::cancel_llm_processing,
            toast::close_toast_window,
        ])
        .setup(move |app| {
            // Hidden main window to keep app alive when capsule closes
            {
                use tauri::{WebviewUrl, WebviewWindowBuilder};
                let main_win = WebviewWindowBuilder::new(
                    app.handle(),
                    "main",
                    WebviewUrl::App("blank.html".into()),
                )
                .visible(false)
                .build()?;
                Box::leak(Box::new(main_win));
            }
            tray::setup_tray(app.handle())?;
            open_capsule_window(app.handle());

            // Show startup toast after short delay (tray icon needs time to initialize)
            let toast_app = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                toast::show_toast(&toast_app, "AirType 已启动");
            });

            let audio_stream: Arc<AudioStreamHolder> = Arc::new(AudioStreamHolder(Mutex::new(None)));
            let pipeline = Arc::new(Mutex::new(PipelineCtx {
                state: app_state.clone(),
                buffer: audio_buffer.clone(),
                sample_rate: audio_sample_rate.clone(),
                stream: audio_stream.clone(),
                app_handle: app.handle().clone(),
                hotkey_config: hotkey_config.clone(),
                secondary_hotkey_config: secondary_hotkey_config.clone(),
            }));

            // Hotkey listener thread
            {
                let pipeline_cb = pipeline.clone();
                let hotkey_cfg = hotkey_config.clone();
                let secondary_cfg = secondary_hotkey_config.clone();
                std::thread::spawn(move || {
                    let initial_cfg = hotkey_cfg.lock().unwrap().clone();
                    let initial_secondary = secondary_cfg.lock().unwrap().clone();
                    let tracker = std::cell::RefCell::new(DualHotkeyTracker::new(initial_cfg, initial_secondary));
                    log_debug("[hotkey] Listener starting");

                    let callback = move |event: Event| -> Option<Event> {
                        {
                            let new_cfg = hotkey_cfg.lock().unwrap().clone();
                            let new_secondary = secondary_cfg.lock().unwrap().clone();
                            tracker.borrow_mut().update_primary(new_cfg);
                            tracker.borrow_mut().update_secondary(new_secondary);
                        }
                        if let Some((source, transition)) = tracker.borrow_mut().process_event(&event.event_type) {
                            log_debug(&format!("[hotkey] {:?} transition: {:?}", source, transition));
                            handle_hotkey_transition(source, transition, &pipeline_cb);
                        }
                        if let Some((source, transition)) = tracker.borrow_mut().tick() {
                            log_debug(&format!("[hotkey] {:?} tick transition: {:?}", source, transition));
                            handle_hotkey_transition(source, transition, &pipeline_cb);
                        }
                        // ESC to cancel during processing (suppress the key)
                        if let rdev::EventType::KeyPress(rdev::Key::Escape) = event.event_type {
                            if handle_esc_cancel(&pipeline_cb) {
                                return None;
                            }
                        }
                        Some(event)
                    };
                    log_debug("[hotkey] Calling rdev::grab...");
                    if let Err(e) = rdev::grab(callback) {
                        log_debug(&format!("[hotkey] grab ERROR: {:?}", e));
                    }
                });
            }


            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Position the capsule window based on config (saved position or default).
pub fn position_capsule(win: &tauri::WebviewWindow) {
    let config_path = config::config_path();
    let cfg = config::AppConfig::load(&config_path);

    if let Ok(monitor) = win.primary_monitor() {
        if let Some(m) = monitor {
            let scale = m.scale_factor();
            let win_w = 150.0 * scale;
            let win_h = 40.0 * scale;

            // Reset size in case it was left at popup dimensions
            let size = tauri::PhysicalSize::new(win_w as u32, win_h as u32);
            let _ = win.set_size(tauri::Size::Physical(size));

            let (x, y) = cfg.capsule_position(
                m.size().width as f64, m.size().height as f64,
                win_w, win_h, scale,
            );
            let pos = tauri::PhysicalPosition::new(x, y);
            if let Err(e) = win.set_position(tauri::Position::Physical(pos)) {
                log_debug(&format!("[capsule] set_position error: {}", e));
            }
        }
    }
}

fn open_capsule_window(app_handle: &tauri::AppHandle) {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    if let Some(win) = app_handle.get_webview_window("capsule") {
        position_capsule(&win);
        return;
    }
    let win = WebviewWindowBuilder::new(
        app_handle,
        "capsule",
        WebviewUrl::App("capsule.html".into()),
    )
    .title("AirType")
    .inner_size(150.0, 40.0)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .focusable(false)
    .visible(false)
    .build();

    match win {
        Ok(win) => {
            remove_window_shadow(&win);
            log_debug("[capsule] Window created");
        }
        Err(e) => {
            log_debug(&format!("[capsule] Failed to create window: {}", e));
        }
    }
}

#[cfg(target_os = "windows")]
fn remove_window_shadow(win: &tauri::WebviewWindow) {
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_STYLE,
        WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    let hwnd = win.hwnd();
    if let Ok(hwnd) = hwnd {
        let h = windows::Win32::Foundation::HWND(hwnd.0);
        unsafe {
            let style = GetWindowLongPtrW(h, GWL_STYLE);
            SetWindowLongPtrW(h, GWL_STYLE, (style & !(
                WS_CAPTION.0 as isize
                    | WS_THICKFRAME.0 as isize
                    | WS_SYSMENU.0 as isize
                    | WS_MINIMIZEBOX.0 as isize
                    | WS_MAXIMIZEBOX.0 as isize
            )) | WS_POPUP.0 as isize);
            let disabled: i32 = 1;
            let _ = DwmSetWindowAttribute(
                h,
                DWMWA_TRANSITIONS_FORCEDISABLED,
                &disabled as *const _ as *const _,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn remove_window_shadow(_win: &tauri::WebviewWindow) {}


fn close_capsule_window(app_handle: &tauri::AppHandle) {
    if let Some(win) = app_handle.get_webview_window("capsule") {
        let _ = win.hide();
        log_debug("[capsule] Window hidden");
    }
}

fn handle_hotkey_transition(
    source: HotkeySource,
    transition: HotkeyTransition,
    ctx: &Arc<Mutex<PipelineCtx>>,
) {
    match transition {
        HotkeyTransition::Confirmed => {
            let state_arc;
            let buffer_arc;
            let stream_arc;
            let sample_rate_arc;
            let app_handle;
            {
                let p = ctx.lock().unwrap();
                state_arc = p.state.clone();
                buffer_arc = p.buffer.clone();
                stream_arc = p.stream.clone();
                sample_rate_arc = p.sample_rate.clone();
                app_handle = p.app_handle.clone();
            };

            let mut s = state_arc.lock().unwrap();
            log_debug(&format!("[pipeline] {:?} Confirmed! enabled={}", source, s.enabled));
            if !s.enabled {
                let warning_msg = "AirType 已禁用";
                drop(s);
                // Show warning in capsule window
                open_capsule_window(&app_handle);
                if let Some(win) = app_handle.get_webview_window("capsule") {
                    let _ = win.show();
                    let _ = win.emit("capsule-state", serde_json::json!({
                        "phase": "warning",
                        "rms": 0.0,
                        "elapsed_ms": 0,
                        "error": warning_msg
                    }));
                }
                // Auto-close after 2 seconds
                let app_handle_clone = app_handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(2000));
                    close_capsule_window(&app_handle_clone);
                });
                return;
            }
            // Only start recording if currently Idle
            if !matches!(s.recording, RecordingState::Idle) {
                log_debug(&format!("[pipeline] Not idle ({:?}), skipping", s.recording));
                return;
            }
            let _ = transition_to(&mut s.recording, RecordingState::Recording {
                started_at: std::time::Instant::now(),
            });
            drop(s);

            // Create audio stream on demand
            {
                let host = cpal::default_host();
                let device = match host.default_input_device() {
                    Some(d) => d,
                    None => {
                        log_debug("[audio] No input device found!");
                        let mut s = state_arc.lock().unwrap();
                        let _ = transition_to(&mut s.recording, RecordingState::Error("未找到麦克风".into()));
                        drop(s);
                        return;
                    }
                };
                let config = match device.default_input_config() {
                    Ok(c) => c,
                    Err(e) => {
                        log_debug(&format!("[audio] Failed to get device config: {}", e));
                        let mut s = state_arc.lock().unwrap();
                        let _ = transition_to(&mut s.recording, RecordingState::Error(format!("音频配置错误: {}", e)));
                        drop(s);
                        return;
                    }
                };
                let channels = config.channels() as usize;
                let sample_rate = config.sample_rate().0;
                log_debug(&format!("[audio] On-demand stream: sr={}, ch={}", sample_rate, channels));
                *sample_rate_arc.lock().unwrap() = sample_rate;

                let buf_for_cb = buffer_arc.clone();
                let stream = device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mono: Vec<f32> = if channels > 1 {
                            data.chunks(channels)
                                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                                .collect()
                        } else {
                            data.to_vec()
                        };
                        let mut buf = buf_for_cb.lock().unwrap();
                        if buf.is_capturing() {
                            buf.push_f32(&mono);
                        }
                    },
                    |err| log_debug(&format!("[audio] Stream error: {}", err)),
                    None,
                );
                match stream {
                    Ok(s) => {
                        if let Err(e) = s.play() {
                            log_debug(&format!("[audio] Failed to play stream: {}", e));
                            let mut st = state_arc.lock().unwrap();
                            let _ = transition_to(&mut st.recording, RecordingState::Error(format!("音频播放失败: {}", e)));
                        } else {
                            log_debug("[audio] On-demand stream playing");
                            *stream_arc.0.lock().unwrap() = Some(s);
                        }
                    }
                    Err(e) => {
                        log_debug(&format!("[audio] Failed to build stream: {}", e));
                        let mut st = state_arc.lock().unwrap();
                        let _ = transition_to(&mut st.recording, RecordingState::Error(format!("音频流创建失败: {}", e)));
                    }
                }
            }

            let mut buf = buffer_arc.lock().unwrap();
            buf.clear();
            buf.start_capture();
            drop(buf);
            open_capsule_window(&app_handle);
            if let Some(win) = app_handle.get_webview_window("capsule") {
                let _ = win.show();
                let _ = win.emit("capsule-state", serde_json::json!({"phase": "recording", "rms": 0.0, "elapsed_ms": 0, "error": null}));
            }
            log_debug("[pipeline] Capture started");
        }
        HotkeyTransition::Released | HotkeyTransition::Timeout => {
            log_debug(&format!("[pipeline] {:?} Released/Timeout triggered", source));

            let state_arc;
            let buffer_arc;
            let stream_arc;
            let sample_rate_val;
            let app_handle;
            {
                let p = ctx.lock().unwrap();
                state_arc = p.state.clone();
                buffer_arc = p.buffer.clone();
                stream_arc = p.stream.clone();
                sample_rate_val = *p.sample_rate.lock().unwrap();
                app_handle = p.app_handle.clone();
            };

            let mut s = state_arc.lock().unwrap();
            if !matches!(s.recording, RecordingState::Recording { .. }) {
                log_debug(&format!("[pipeline] Not recording ({:?}), skipping release", s.recording));
                return;
            }
            let _ = transition_to(&mut s.recording, RecordingState::Processing);
            drop(s);
            if let Some(win) = app_handle.get_webview_window("capsule") {
                let _ = win.emit("capsule-state", serde_json::json!({"phase": "loading", "rms": 0.0, "elapsed_ms": 0, "error": null}));
            }

            let mut buf = buffer_arc.lock().unwrap();
            buf.stop_capture();
            let pcm = buf.take_pcm_bytes();
            drop(buf);
            // Drop audio stream to release microphone
            {
                let mut stream_guard = stream_arc.0.lock().unwrap();
                *stream_guard = None;
            }
            log_debug("[audio] Stream dropped, microphone released");
            log_debug(&format!("[pipeline] PCM bytes captured: {}, sample_rate: {}", pcm.len(), sample_rate_val));

            let state_clone = state_arc.clone();
            let source_clone = source;
            std::thread::spawn(move || {
                log_debug("[pipeline] ASR thread started");
                let cfg = config::AppConfig::load(&config::config_path());
                let client = asr::AsrClient::new(&cfg.backend_url, &cfg.asr_api_key);

                let prompt = {
                    let ctx = cfg.context_string();
                    if ctx.is_empty() { None } else { Some(ctx) }
                };
                match client.transcribe(&pcm, sample_rate_val, &cfg.model, cfg.language.as_deref(), prompt.as_deref()) {
                    Ok(result) => {
                        log_debug(&format!("[pipeline] ASR result: '{}' (empty={})", result.text, result.text.is_empty()));
                        // Check if cancelled during ASR
                        let cancelled = {
                            let s = state_clone.lock().unwrap();
                            !matches!(s.recording, RecordingState::Processing)
                        };
                        if cancelled {
                            log_debug("[pipeline] Cancelled during ASR, discarding result");
                        } else if result.text.is_empty() {
                            let mut s = state_clone.lock().unwrap();
                            let _ = transition_to(&mut s.recording, RecordingState::Error("ASR 返回空文本".into()));
                        } else {
                            // Handle based on hotkey source
                            match source_clone {
                                HotkeySource::Primary => {
                                    // Default hotkey: use default processing mode
                                    handle_default_mode(result.text, &cfg, &state_clone, &app_handle);
                                }
                                HotkeySource::Secondary => {
                                    // Secondary hotkey: show mode selection menu
                                    log_debug("[pipeline] Secondary hotkey: showing mode selection");
                                    let mut s = state_clone.lock().unwrap();
                                    let _ = transition_to(&mut s.recording, RecordingState::ModeSelection {
                                        text: result.text.clone()
                                    });
                                    drop(s);

                                    if let Some(win) = app_handle.get_webview_window("capsule") {
                                        let _ = win.emit("mode-selection", serde_json::json!({
                                            "text": result.text,
                                            "modes": cfg.processing_modes
                                        }));
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log_debug(&format!("[pipeline] ASR error: {}", e));
                        let mut s = state_clone.lock().unwrap();
                        let _ = transition_to(&mut s.recording, RecordingState::Error(e.to_string()));
                    }
                }
                // Fallback close in case JS doesn't call close_capsule_window_cmd
                std::thread::sleep(Duration::from_millis(2000));
                {
                    let mut s = state_clone.lock().unwrap();
                    if matches!(s.recording, RecordingState::Done | RecordingState::Error(_)) {
                        let _ = transition_to(&mut s.recording, RecordingState::Idle);
                        drop(s);
                        close_capsule_window(&app_handle);
                    }
                }
            });
        }
        HotkeyTransition::Debounced | HotkeyTransition::ReleasedTooEarly | HotkeyTransition::Pressed => {}
    }
}

fn handle_default_mode(
    text: String,
    cfg: &config::AppConfig,
    state: &Arc<Mutex<AppState>>,
    app_handle: &tauri::AppHandle,
) {
    // Find the selected mode
    let mode = cfg.processing_modes.iter()
        .find(|m| m.id == cfg.default_processing_mode)
        .cloned()
        .unwrap_or_else(|| config::ProcessingMode::direct());

    // If not "direct", transition to LlmProcessing state and emit the event
    if mode.id != "direct" {
        log_debug(&format!("[pipeline] LLM processing (mode={})", mode.id));
        let mut s = state.lock().unwrap();
        let _ = transition_to(&mut s.recording, RecordingState::LlmProcessing { text: text.clone() });
        drop(s);

        if let Some(win) = app_handle.get_webview_window("capsule") {
            let _ = win.emit("capsule-state", serde_json::json!({
                "phase": "llm_processing",
                "rms": 0.0,
                "elapsed_ms": 0,
                "error": null
            }));
        }
    }

    // Run LLM with fallback, then check if cancelled before injecting
    match util::run_llm_with_fallback(&text, cfg, &mode) {
        Ok(result_text) => {
            let mut s = state.lock().unwrap();
            let still_active = matches!(s.recording, RecordingState::LlmProcessing { .. } | RecordingState::Processing);
            if still_active {
                let _ = transition_to(&mut s.recording, RecordingState::Done);
                drop(s);
                let _ = inject::inject_text(&result_text);
            } else {
                log_debug("[pipeline] Cancelled before injection, discarding result");
                if !matches!(s.recording, RecordingState::Idle) {
                    let _ = transition_to(&mut s.recording, RecordingState::Idle);
                }
            }
        }
        Err(e) => {
            let mut s = state.lock().unwrap();
            let _ = transition_to(&mut s.recording, RecordingState::Error(e));
        }
    }

    // Close capsule window
    std::thread::sleep(Duration::from_millis(2000));
    {
        let mut s = state.lock().unwrap();
        if matches!(s.recording, RecordingState::Done | RecordingState::Error(_)) {
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            close_capsule_window(app_handle);
        }
    }
}

fn handle_esc_cancel(ctx: &Arc<Mutex<PipelineCtx>>) -> bool {
    let state_arc;
    let buffer_arc;
    let stream_arc;
    let app_handle;
    {
        let p = ctx.lock().unwrap();
        state_arc = p.state.clone();
        buffer_arc = p.buffer.clone();
        stream_arc = p.stream.clone();
        app_handle = p.app_handle.clone();
    };

    let mut s = state_arc.lock().unwrap();
    match &s.recording {
        RecordingState::Recording { .. } => {
            log_debug("[cancel] ESC during Recording → Idle");
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            let mut buf = buffer_arc.lock().unwrap();
            buf.stop_capture();
            buf.clear();
            drop(buf);
            {
                let mut stream_guard = stream_arc.0.lock().unwrap();
                *stream_guard = None;
            }
            close_capsule_window(&app_handle);
            true
        }
        RecordingState::Processing => {
            log_debug("[cancel] ESC during ASR processing → Idle");
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            close_capsule_window(&app_handle);
            true
        }
        RecordingState::LlmProcessing { .. } => {
            log_debug("[cancel] ESC during LLM → Idle (no inject)");
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            close_capsule_window(&app_handle);
            true
        }
        RecordingState::ModeSelection { .. } => {
            log_debug("[cancel] ESC during mode selection → Idle (no inject)");
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            close_capsule_window(&app_handle);
            true
        }
        _ => false,
    }
}
