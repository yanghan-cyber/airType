# 流式实时转录 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 AirType 添加实时流式语音转录功能，用户录音时可实时看到转录文字，松开按键后直接注入文本。

**Architecture:** 基于 OpenAI Realtime Transcription API 协议（WebSocket），三级 fallback（WS → HTTP → Error）。独立 `tokio::Runtime` 管理每个 WS 会话生命周期，音频发送线程通过 `runtime.spawn()` 发送 chunk，WS reader task 累积 delta 文本并推送到前端。

**Tech Stack:** Rust (tokio-tungstenite, base64, futures-util), Tauri 2 event system, HTML/CSS/JS

---

## File Structure

| 文件 | 操作 | 职责 |
|------|------|------|
| `src-tauri/Cargo.toml` | 修改 | 添加 `tokio-tungstenite`, `base64`, `futures-util` |
| `src-tauri/src/config.rs` | 修改 | 新增 `realtime_asr: bool` |
| `src-tauri/src/asr_realtime.rs` | 新建 | WS 连接 + 事件读取 |
| `src-tauri/src/state.rs` | 修改 | 新增 `RealtimeRecording` 变体 |
| `src-tauri/src/audio.rs` | 修改 | 新增 `take_recent_pcm_bytes` |
| `src-tauri/src/commands.rs` | 修改 | 新增 `realtime_recording` phase + `realtime_asr` 参数 |
| `src-tauri/src/main.rs` | 修改 | WS 会话管理 + 音频发送线程 |
| `ui/capsule.html` | 修改 | 实时文字显示 + 事件监听 |
| `ui/settings.html` | 修改 | 高级标签页 toggle 开关 |

---

### Task 1: 添加 Cargo 依赖

**Files:**
- Modify: `src-tauri/Cargo.toml:14-24`

- [ ] **Step 1: 添加依赖**

在 `[dependencies]` 中 `tokio` 行之后添加：

```toml
tokio-tungstenite = { version = "0.26", features = ["native-tls"] }
base64 = "0.22"
futures-util = "0.3"
```

> 不需要单独添加 `tungstenite` — 通过 `tokio_tungstenite::tungstenite::Message` 访问即可。

- [ ] **Step 2: 验证编译**

Run: `cd D:/workspace/airType/src-tauri && cargo check 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 3: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "chore: add tokio-tungstenite, base64, futures-util for realtime transcription"
```

---

### Task 2: 配置层 — `realtime_asr` 字段

**Files:**
- Modify: `src-tauri/src/config.rs`

- [ ] **Step 1: 在 AppConfig struct 中 `processing_modes` 后添加**

```rust
    pub realtime_asr: bool,
```

- [ ] **Step 2: 在 Default impl 的 `processing_modes` 向量后添加**

```rust
            realtime_asr: false,
```

- [ ] **Step 3: 在 tests 模块末尾添加**

```rust
    #[test]
    fn test_realtime_asr_default_false() {
        let cfg = AppConfig::default();
        assert!(!cfg.realtime_asr);
    }

    #[test]
    fn test_realtime_asr_backward_compat() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"hotkey":"Ctrl+Win"}"#).unwrap();
        let cfg = AppConfig::load(&path);
        assert!(!cfg.realtime_asr);
    }

    #[test]
    fn test_realtime_asr_save_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = AppConfig::default();
        cfg.realtime_asr = true;
        cfg.save(&path);
        let loaded = AppConfig::load(&path);
        assert!(loaded.realtime_asr);
    }
```

- [ ] **Step 4: 运行测试**

Run: `cd D:/workspace/airType/src-tauri && cargo test --lib config -- 2>&1 | tail -15`
Expected: 所有测试通过

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/config.rs
git commit -m "feat(config): add realtime_asr toggle with backward compatibility"
```

---

### Task 3: 状态机 — `RealtimeRecording` 变体

**Files:**
- Modify: `src-tauri/src/state.rs`

- [ ] **Step 1: 在 `Recording` 变体后添加**

```rust
    RealtimeRecording { started_at: std::time::Instant, accumulated_text: String },
```

- [ ] **Step 2: 在 `transition_to` 的 match 中，`(Idle, Recording)` 行之后添加**

```rust
        (RecordingState::Idle, RecordingState::RealtimeRecording { .. }) => {}
        (RecordingState::RealtimeRecording { .. }, RecordingState::Done) => {}
        (RecordingState::RealtimeRecording { .. }, RecordingState::Error(_)) => {}
        (RecordingState::RealtimeRecording { .. }, RecordingState::Idle) => {}
```

- [ ] **Step 3: 在 tests 模块末尾添加**

```rust
    #[test]
    fn test_realtime_recording_basic_flow() {
        let mut state = RecordingState::Idle;
        assert!(transition_to(&mut state, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            accumulated_text: String::new(),
        }).is_ok());
        assert!(transition_to(&mut state, RecordingState::Done).is_ok());
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_realtime_recording_cancel() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            accumulated_text: "partial".into(),
        }).unwrap();
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_realtime_recording_invalid_transitions() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            accumulated_text: String::new(),
        }).unwrap();
        assert!(transition_to(&mut state, RecordingState::Processing).is_err());
        assert!(transition_to(&mut state, RecordingState::Recording {
            started_at: std::time::Instant::now(),
        }).is_err());
    }
```

- [ ] **Step 4: 运行测试**

Run: `cd D:/workspace/airType/src-tauri && cargo test --lib state -- 2>&1 | tail -15`
Expected: 所有测试通过

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/state.rs
git commit -m "feat(state): add RealtimeRecording state variant"
```

---

### Task 4: 音频缓冲区 — `take_recent_pcm_bytes`

**Files:**
- Modify: `src-tauri/src/audio.rs`

- [ ] **Step 1: 在 `AudioBuffer` impl 中 `stop_capture` 之后添加**

```rust
    /// Take all PCM bytes accumulated since last call, keeping capture active.
    pub fn take_recent_pcm_bytes(&mut self) -> Vec<u8> {
        let bytes: Vec<u8> = self.data.iter()
            .flat_map(|&s| s.to_le_bytes())
            .collect();
        self.data.clear();
        bytes
    }
```

- [ ] **Step 2: 在 tests 模块末尾添加**

```rust
    #[test]
    fn test_take_recent_pcm_while_capturing() {
        let mut buf = AudioBuffer::new(16000);
        buf.start_capture();
        buf.push_i16(&[100, 200, 300]);
        assert!(buf.is_capturing());
        let bytes = buf.take_recent_pcm_bytes();
        assert_eq!(bytes.len(), 6);
        assert_eq!(buf.len(), 0);
        assert!(buf.is_capturing());
        buf.push_i16(&[400, 500]);
        let bytes2 = buf.take_recent_pcm_bytes();
        assert_eq!(bytes2.len(), 4);
    }
```

- [ ] **Step 3: 运行测试**

Run: `cd D:/workspace/airType/src-tauri && cargo test --lib audio -- 2>&1 | tail -15`
Expected: 所有测试通过

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/audio.rs
git commit -m "feat(audio): add take_recent_pcm_bytes for incremental extraction"
```

---

### Task 5: 新建 `asr_realtime.rs` — WebSocket Realtime Transcription 客户端

**Files:**
- Create: `src-tauri/src/asr_realtime.rs`
- Modify: `src-tauri/src/main.rs:3` (注册模块)

此模块职责单一：**建立连接 + 等待 session.created/updated + 返回拆分后的组件**。不负责后台任务管理（由 main.rs 调用者负责）。

- [ ] **Step 1: 创建 `src-tauri/src/asr_realtime.rs`**

```rust
use crate::log::log_debug;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use tauri::Emitter;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

// ── Type aliases for split WebSocket stream ──

type WsStream = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

pub type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
pub type WsStreamRx = futures_util::SplitStream<WsStream>;

// ── Public API ──

/// Build WebSocket URL from backend_url and model name.
pub fn build_ws_url(backend_url: &str, model: &str) -> String {
    let ws_base = backend_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    format!("{}/realtime?model={}", ws_base, model)
}

/// Connect to the Realtime Transcription WebSocket.
///
/// Returns `(ws_tx, ws_rx, accumulated_text)`:
/// - `ws_tx`: for the caller to send audio chunks
/// - `ws_rx`: for the caller to spawn a reader task
/// - `accumulated_text`: shared text buffer that the reader task appends to
///
/// The caller is responsible for:
/// 1. Spawning a reader task with `ws_rx` and `accumulated_text`
/// 2. Sending audio chunks via `ws_tx`
/// 3. Closing `ws_tx` when done
pub async fn connect_realtime(
    backend_url: &str,
    model: &str,
    api_key: &str,
    language: Option<&str>,
) -> Result<(WsSink, Arc<Mutex<String>>), String> {
    let url = build_ws_url(backend_url, model);
    log_debug(&format!("[realtime] Connecting to {}", url));

    // Build request with optional auth header
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("Invalid WS URL: {}", e))?;
    if !api_key.is_empty() {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", api_key)
                .parse()
                .map_err(|e| format!("Invalid auth header: {}", e))?,
        );
    }

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("WS connect error: {}", e))?;
    log_debug("[realtime] TCP connected");

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // ── Wait for session.created ──
    wait_event(&mut ws_rx, "session.created").await?;

    // ── Send session.update ──
    let mut session_update = serde_json::json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": {
                        "type": "audio/pcm",
                        "rate": 16000
                    },
                    "transcription": {
                        "model": model,
                    },
                    "turn_detection": {
                        "type": "server_vad",
                        "threshold": 0.5,
                        "prefix_padding_ms": 300,
                        "silence_duration_ms": 500
                    }
                }
            }
        }
    });
    if let Some(lang) = language {
        session_update["session"]["audio"]["input"]["transcription"]["language"] =
            serde_json::json!(lang);
    }

    ws_tx
        .send(Message::Text(session_update.to_string()))
        .await
        .map_err(|e| format!("Send session.update error: {}", e))?;
    log_debug("[realtime] session.update sent");

    // ── Wait for session.updated ──
    wait_event(&mut ws_rx, "session.updated").await?;
    log_debug("[realtime] Ready for audio");

    // Shared accumulated text
    let accumulated_text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));

    Ok((ws_tx, accumulated_text))
}

/// Spawn the WS reader task on the given runtime.
/// Reads delta/completed/error events, appends to accumulated_text, emits to frontend.
/// Returns a JoinHandle for the task.
pub fn spawn_reader_task(
    ws_rx: WsStreamRx,
    accumulated_text: Arc<Mutex<String>>,
    app_handle: tauri::AppHandle,
    state: Arc<Mutex<crate::state::AppState>>,
    rt: &tokio::runtime::Runtime,
) {
    rt.spawn(async move {
        let mut ws_rx = ws_rx;
        while let Some(msg_result) = ws_rx.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(&text) {
                        let event_type = event["type"].as_str().unwrap_or("");
                        match event_type {
                            "conversation.item.input_audio_transcription.delta" => {
                                if let Some(delta) = event["delta"].as_str() {
                                    let mut acc = accumulated_text.lock().unwrap();
                                    acc.push_str(delta);
                                    let full_text = acc.clone();
                                    drop(acc);
                                    let _ = app_handle.emit(
                                        "realtime-delta",
                                        serde_json::json!({ "text": full_text }),
                                    );
                                }
                            }
                            "conversation.item.input_audio_transcription.completed" => {
                                // Informational: deltas already accumulated the text
                            }
                            "error" => {
                                let err_msg = event["error"]["message"]
                                    .as_str()
                                    .unwrap_or("Unknown error");
                                log_debug(&format!("[realtime] Server error: {}", err_msg));
                                // Transition to Error state
                                let mut s = state.lock().unwrap();
                                if matches!(
                                    s.recording,
                                    crate::state::RecordingState::RealtimeRecording { .. }
                                ) {
                                    let _ = crate::state::transition_to(
                                        &mut s.recording,
                                        crate::state::RecordingState::Error(err_msg.into()),
                                    );
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    log_debug("[realtime] WS closed by server");
                    break;
                }
                Err(e) => {
                    log_debug(&format!("[realtime] WS recv error: {}", e));
                    // Mid-session disconnect: fallback to error
                    let mut s = state.lock().unwrap();
                    if matches!(
                        s.recording,
                        crate::state::RecordingState::RealtimeRecording { .. }
                    ) {
                        let _ = crate::state::transition_to(
                            &mut s.recording,
                            crate::state::RecordingState::Error("WebSocket 断开连接".into()),
                        );
                    }
                    break;
                }
                _ => {}
            }
        }
        log_debug("[realtime] Reader task ended");
    });
}

// ── Helpers ──

/// Wait for a specific event type from the WebSocket.
async fn wait_event(
    ws_rx: &mut WsStreamRx,
    expected_type: &str,
) -> Result<serde_json::Value, String> {
    let msg = ws_rx
        .next()
        .await
        .ok_or_else(|| format!("WS closed before {}", expected_type))?
        .map_err(|e| format!("WS recv error: {}", e))?;

    let event: serde_json::Value = match &msg {
        Message::Text(t) => serde_json::from_str(t).map_err(|e| format!("JSON parse: {}", e))?,
        _ => return Err(format!("Expected text message for {}", expected_type)),
    };

    if event["type"].as_str() != Some(expected_type) {
        return Err(format!(
            "Expected {}, got: {}",
            expected_type,
            event["type"].as_str().unwrap_or("unknown")
        ));
    }
    Ok(event)
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ws_url_http() {
        assert_eq!(
            build_ws_url("http://localhost:8080/v1", "test-model"),
            "ws://localhost:8080/v1/realtime?model=test-model"
        );
    }

    #[test]
    fn test_build_ws_url_https() {
        assert_eq!(
            build_ws_url("https://api.example.com/v1", "gpt-realtime-whisper"),
            "wss://api.example.com/v1/realtime?model=gpt-realtime-whisper"
        );
    }

    #[test]
    fn test_accumulated_text_shared() {
        let text: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        text.lock().unwrap().push_str("hello ");
        text.lock().unwrap().push_str("world");
        assert_eq!(text.lock().unwrap().as_str(), "hello world");
    }
}
```

- [ ] **Step 2: 在 `src-tauri/src/main.rs` 的 `mod asr;` 之后添加**

```rust
mod asr_realtime;
```

- [ ] **Step 3: 验证编译**

Run: `cd D:/workspace/airType/src-tauri && cargo check 2>&1 | tail -10`
Expected: 编译成功

- [ ] **Step 4: 运行测试**

Run: `cd D:/workspace/airType/src-tauri && cargo test --lib asr_realtime -- 2>&1 | tail -10`
Expected: 3 个测试通过

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/asr_realtime.rs src-tauri/src/main.rs
git commit -m "feat(realtime): add WS Realtime Transcription client with auth headers"
```

---

### Task 6: `commands.rs` — `realtime_recording` phase + `realtime_asr` 参数

**Files:**
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: 在 `get_capsule_state` 的 `Recording` 分支之后添加**

```rust
        crate::state::RecordingState::RealtimeRecording { started_at, .. } => CapsuleState {
            phase: "realtime_recording".into(), rms,
            elapsed_ms: started_at.elapsed().as_millis() as u64, error: None,
        },
```

- [ ] **Step 2: 在 `save_config` 函数签名中 `asr_api_key` 参数后添加 `realtime_asr` 参数**

```rust
    realtime_asr: Option<bool>,
```

在函数体中 `if let Some(v) = asr_api_key` 行之后添加：

```rust
    if let Some(v) = realtime_asr { cfg.realtime_asr = v; }
```

- [ ] **Step 3: 验证编译**

Run: `cd D:/workspace/airType/src-tauri && cargo check 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands.rs
git commit -m "feat(commands): add realtime_recording phase and realtime_asr param"
```

---

### Task 7: `main.rs` — WS 会话管理 + 音频发送线程

**Files:**
- Modify: `src-tauri/src/main.rs`

这是最核心的改动。设计要点：
- 每个WS会话创建独立的 `tokio::Runtime`
- `RealtimeSessionGuard` 持有 `runtime` + `ws_tx`（通过 `Arc<tokio::sync::Mutex>`）+ `accumulated_text`
- 音频发送线程通过 `runtime.spawn()` 异步发送，不阻塞
- WS 断连时自动 fallback 到 Error 状态

- [ ] **Step 1: 在 `PipelineCtx` 定义之前添加 guard 结构**

```rust
/// Holds the realtime WS session resources. Dropped when recording ends.
struct RealtimeSessionGuard {
    accumulated_text: Arc<Mutex<String>>,
    ws_tx: Arc<tokio::sync::Mutex<asr_realtime::WsSink>>,
    _runtime: Arc<tokio::runtime::Runtime>,
}
```

- [ ] **Step 2: 在 `PipelineCtx` struct 中 `secondary_hotkey_config` 之后添加**

```rust
    realtime_session: Arc<Mutex<Option<RealtimeSessionGuard>>>,
```

- [ ] **Step 3: 在 `main()` 中 pipeline 初始化之前添加**

```rust
let realtime_session: Arc<Mutex<Option<RealtimeSessionGuard>>> = Arc::new(Mutex::new(None));
```

在 `PipelineCtx` 初始化中添加：

```rust
    realtime_session: realtime_session.clone(),
```

- [ ] **Step 4: 在 `handle_hotkey_transition` 之前添加 `try_start_realtime` 函数**

```rust
/// Try to start a realtime transcription session.
/// Returns true if WS connection succeeded and state was set to RealtimeRecording.
/// Returns false on any error (caller should fallback to normal Recording).
fn try_start_realtime(
    cfg: &config::AppConfig,
    state_arc: &Arc<Mutex<AppState>>,
    buffer_arc: &Arc<Mutex<AudioBuffer>>,
    app_handle: &tauri::AppHandle,
    realtime_session_arc: &Arc<Mutex<Option<RealtimeSessionGuard>>>,
) -> bool {
    // Create a dedicated tokio runtime for this WS session
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => Arc::new(rt),
        Err(e) => {
            log_debug(&format!("[realtime] Failed to create runtime: {}", e));
            return false;
        }
    };

    let app_handle_clone = app_handle.clone();
    let state_arc_clone = state_arc.clone();
    let accumulated_text: Arc<Mutex<String>>;

    // Connect WebSocket (blocking the rdev callback thread briefly)
    let (ws_tx, acc_text) = match rt.block_on(async {
        asr_realtime::connect_realtime(
            &cfg.backend_url,
            &cfg.model,
            &cfg.asr_api_key,
            cfg.language.as_deref(),
        )
        .await
    }) {
        Ok(result) => result,
        Err(e) => {
            log_debug(&format!("[realtime] Connection failed: {}, falling back", e));
            return false;
        }
    };
    accumulated_text = acc_text;

    // Spawn WS reader task on the runtime
    asr_realtime::spawn_reader_task(
        /* ws_rx is consumed inside connect_realtime's split, we need to adjust */
        // Actually, connect_realtime only returns ws_tx and accumulated_text.
        // The ws_rx is consumed by spawn_reader_task... but wait,
        // connect_realtime returns (ws_tx, accumulated_text) but doesn't spawn the reader.
        // We need to also return ws_rx from connect_realtime.
        // FIX: connect_realtime needs to return ws_rx too. See Step 4b.
    );

    true
}
```

**等一下 — 发现设计问题。** `connect_realtime` 目前返回 `(WsSink, Arc<Mutex<String>>)` 但没有返回 `ws_rx`。Reader task 需要 `ws_rx`。需要修改 `connect_realtime` 的返回值。

- [ ] **Step 4b: 修改 `asr_realtime.rs` 的 `connect_realtime` 返回值**

将返回类型改为：

```rust
pub async fn connect_realtime(
    backend_url: &str,
    model: &str,
    api_key: &str,
    language: Option<&str>,
) -> Result<(WsSink, WsStreamRx, Arc<Mutex<String>>), String>
```

在函数体中，将 `let (mut ws_tx, mut ws_rx) = ws_stream.split();` 的 `mut ws_rx` 改为 `ws_rx`（wait_event 只需 `&mut`），并在 `Ok(...)` 中返回：

```rust
    Ok((ws_tx, ws_rx, accumulated_text))
```

同步修改 `spawn_reader_task` 签名，移除 `ws_rx` 参数（它在 connect_realtime 里已不再被消费）：

`spawn_reader_task` 的第一个参数改为 `ws_rx: WsStreamRx`。

- [ ] **Step 4c: 修正 `try_start_realtime` 完整实现**

```rust
fn try_start_realtime(
    cfg: &config::AppConfig,
    state_arc: &Arc<Mutex<AppState>>,
    buffer_arc: &Arc<Mutex<AudioBuffer>>,
    app_handle: &tauri::AppHandle,
    realtime_session_arc: &Arc<Mutex<Option<RealtimeSessionGuard>>>,
) -> bool {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => Arc::new(rt),
        Err(e) => {
            log_debug(&format!("[realtime] Runtime creation failed: {}", e));
            return false;
        }
    };

    // Connect (briefly blocks the rdev callback thread)
    let connect_result = rt.block_on(async {
        asr_realtime::connect_realtime(
            &cfg.backend_url,
            &cfg.model,
            &cfg.asr_api_key,
            cfg.language.as_deref(),
        )
        .await
    });

    let (ws_tx, ws_rx, accumulated_text) = match connect_result {
        Ok(result) => result,
        Err(e) => {
            log_debug(&format!("[realtime] Connection failed: {}", e));
            return false;
        }
    };

    // Spawn WS reader on the runtime
    asr_realtime::spawn_reader_task(
        ws_rx,
        accumulated_text.clone(),
        app_handle.clone(),
        state_arc.clone(),
        &rt,
    );

    // Wrap ws_tx in Arc<tokio::sync::Mutex> for shared access
    let ws_tx = Arc::new(tokio::sync::Mutex::new(ws_tx));

    // Transition state to RealtimeRecording
    {
        let mut s = state_arc.lock().unwrap();
        let _ = transition_to(&mut s.recording, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            accumulated_text: String::new(),
        });
    }

    // Store session guard
    {
        let mut rs = realtime_session_arc.lock().unwrap();
        *rs = Some(RealtimeSessionGuard {
            accumulated_text: accumulated_text.clone(),
            ws_tx: ws_tx.clone(),
            _runtime: rt.clone(),
        });
    }

    // Spawn audio sender thread
    let state_for_sender = state_arc.clone();
    let buffer_for_sender = buffer_arc.clone();
    let ws_tx_for_sender = ws_tx.clone();
    let rt_for_sender = rt.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // Check if still recording
            {
                let s = state_for_sender.lock().unwrap();
                if !matches!(s.recording, RecordingState::RealtimeRecording { .. }) {
                    break;
                }
            }
            let pcm = {
                let mut buf = buffer_for_sender.lock().unwrap();
                buf.take_recent_pcm_bytes()
            };
            if pcm.is_empty() {
                continue;
            }
            let b64 = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                &pcm,
            );
            let msg = serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": b64,
            });
            let msg_text = msg.to_string();
            let ws_tx_clone = ws_tx_for_sender.clone();
            let _ = rt_for_sender.spawn(async move {
                let mut tx = ws_tx_clone.lock().await;
                let _ = tx.send(Message::Text(msg_text)).await;
            });
        }
        log_debug("[realtime] Audio sender thread ended");
    });

    true
}
```

需要在 `main.rs` 顶部添加 import：

```rust
use base64::Engine;
use tokio_tungstenite::tungstenite::Message;
```

- [ ] **Step 5: 修改 `handle_hotkey_transition` 的 `Confirmed` 分支**

替换 Confirmed 分支中的状态转换和胶囊显示逻辑。在 `if !matches!(s.recording, RecordingState::Idle)` 检查之后，`drop(s)` 之后：

```rust
    drop(s);

    let cfg = config::AppConfig::load(&config::config_path());
    let realtime_session_arc;
    {
        let p = ctx.lock().unwrap();
        realtime_session_arc = p.realtime_session.clone();
    };

    // Try realtime if enabled
    let realtime_ok = if cfg.realtime_asr {
        try_start_realtime(&cfg, &state_arc, &buffer_arc, &app_handle, &realtime_session_arc)
    } else {
        false
    };

    if !realtime_ok {
        let mut s = state_arc.lock().unwrap();
        let _ = transition_to(&mut s.recording, RecordingState::Recording {
            started_at: std::time::Instant::now(),
        });
        drop(s);
    }
```

音频流创建代码保持不变（两种模式都需要采集音频）。在 `open_capsule_window` 和 emit 时：

```rust
    let phase = if realtime_ok { "realtime_recording" } else { "recording" };
    if let Some(win) = app_handle.get_webview_window("capsule") {
        let _ = win.emit("capsule-state", serde_json::json!({
            "phase": phase, "rms": 0.0, "elapsed_ms": 0, "error": null
        }));
    }
```

- [ ] **Step 6: 修改 `handle_hotkey_transition` 的 `Released` / `Timeout` 分支**

在变量提取块中添加 `realtime_session_arc`：

```rust
    let realtime_session_arc;
    {
        let p = ctx.lock().unwrap();
        // ... existing extractions ...
        realtime_session_arc = p.realtime_session.clone();
    };
```

在现有 `if !matches!(s.recording, RecordingState::Recording { .. })` 检查之前，添加 RealtimeRecording 处理：

```rust
    // Handle RealtimeRecording release
    if matches!(s.recording, RecordingState::RealtimeRecording { .. }) {
        // 1. Get accumulated text
        let final_text = {
            let mut rs = realtime_session_arc.lock().unwrap();
            rs.as_ref()
                .map(|g| g.accumulated_text.lock().unwrap().clone())
                .unwrap_or_default()
        };
        log_debug(&format!("[realtime] Final text: '{}'", final_text));

        // 2. Transition state (signals sender thread to stop)
        let _ = transition_to(&mut s.recording, RecordingState::Done);
        drop(s);

        // 3. Stop audio capture and release mic
        let mut buf = buffer_arc.lock().unwrap();
        buf.stop_capture();
        buf.clear();
        drop(buf);
        {
            let mut stream_guard = stream_arc.0.lock().unwrap();
            *stream_guard = None;
        }

        // 4. Drop session guard (closes WS, stops runtime)
        {
            let mut rs = realtime_session_arc.lock().unwrap();
            *rs = None;
        }

        // 5. Inject text
        if final_text.is_empty() {
            let mut s = state_arc.lock().unwrap();
            let _ = transition_to(&mut s.recording, RecordingState::Error("实时转录返回空文本".into()));
        } else {
            match source {
                HotkeySource::Primary => {
                    let cfg = config::AppConfig::load(&config::config_path());
                    handle_default_mode(final_text, &cfg, &state_arc, &app_handle);
                }
                HotkeySource::Secondary => {
                    let cfg = config::AppConfig::load(&config::config_path());
                    let mut s = state_arc.lock().unwrap();
                    let _ = transition_to(&mut s.recording, RecordingState::ModeSelection {
                        text: final_text.clone(),
                    });
                    drop(s);
                    if let Some(win) = app_handle.get_webview_window("capsule") {
                        let _ = win.emit("mode-selection", serde_json::json!({
                            "text": final_text,
                            "modes": cfg.processing_modes
                        }));
                    }
                }
            }
        }
        return;
    }
```

- [ ] **Step 7: 修改 `handle_esc_cancel` 添加 RealtimeRecording 处理**

在 `RecordingState::Recording` 分支之后添加：

```rust
        RecordingState::RealtimeRecording { .. } => {
            log_debug("[cancel] ESC during RealtimeRecording → Idle");
            let _ = transition_to(&mut s.recording, RecordingState::Idle);
            drop(s);
            // Drop session guard
            {
                let p = ctx.lock().unwrap();
                let mut rs = p.realtime_session.lock().unwrap();
                *rs = None;
            }
            let mut buf = {
                let p = ctx.lock().unwrap();
                p.buffer.clone()
            };
            {
                let mut b = buf.lock().unwrap();
                b.stop_capture();
                b.clear();
            }
            {
                let p = ctx.lock().unwrap();
                let mut stream_guard = p.stream.0.lock().unwrap();
                *stream_guard = None;
            }
            close_capsule_window(&app_handle);
            true
        }
```

> 注意：`handle_esc_cancel` 中需要从 `ctx` 获取 `realtime_session`。由于 `ctx` 是 `Arc<Mutex<PipelineCtx>>`，可以直接通过 `ctx.lock().unwrap().realtime_session` 访问。

- [ ] **Step 8: 验证编译**

Run: `cd D:/workspace/airType/src-tauri && cargo check 2>&1 | tail -15`
Expected: 编译成功。可能需要微调类型/导入。

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/main.rs src-tauri/src/asr_realtime.rs
git commit -m "feat(pipeline): integrate realtime WS transcription with audio sender thread"
```

---

### Task 8: 前端 — 胶囊 UI 实时文字显示

**Files:**
- Modify: `ui/capsule.html`

- [ ] **Step 1: 在 `</style>` 之前添加 CSS**

```css
.realtime-text-area {
  flex: 1;
  overflow: hidden;
  position: relative;
  height: 100%;
  display: flex;
  align-items: center;
  white-space: nowrap;
  min-width: 0;
}
.realtime-text-area.fading::before {
  content: '';
  position: absolute;
  left: 0; top: 0; bottom: 0;
  width: 50px;
  background: linear-gradient(to right, #181818 0%, #181818 40%, transparent 100%);
  z-index: 1;
  pointer-events: none;
}
.realtime-text {
  color: #fff;
  font-size: 13px;
  font-weight: 400;
  letter-spacing: 0.2px;
}
.realtime-cursor {
  display: inline-block;
  width: 2px;
  height: 14px;
  background: #1ed760;
  margin-left: 1px;
  vertical-align: middle;
  animation: cursorBlink 0.8s ease-in-out infinite;
}
@keyframes cursorBlink {
  0%,100% { opacity: 1; }
  50% { opacity: 0; }
}
```

- [ ] **Step 2: 在 capsule div 中 `</div><!-- timer -->` 之后添加 HTML**

```html
    <div class="realtime-text-area" id="realtimeTextArea" style="display:none;">
      <span class="realtime-text" id="realtimeText"></span><span class="realtime-cursor"></span>
    </div>
```

- [ ] **Step 3: 在 `runPhase` 的 switch 中 `case 'recording':` 之后添加**

```javascript
    case 'realtime_recording':
      showRealtimeRecording();
      break;
```

- [ ] **Step 4: 在 `showRecording` 函数之后添加**

```javascript
async function showRealtimeRecording() {
  fullReset();
  const c = $('capsule');
  c.className = 'capsule visible';
  void c.offsetWidth;
  c.animate([
    { opacity: 0, transform: 'translateY(16px) scale(0.92)' },
    { opacity: 1, transform: 'translateY(0) scale(1)' }
  ], { duration: 400, easing: 'cubic-bezier(0.34, 1.56, 0.64, 1)', fill: 'none' });

  $('waveform').style.display = 'flex';
  $('divider').style.display = '';
  $('timer').style.display = 'none';
  $('realtimeTextArea').style.display = 'flex';

  startBars();
  startClock();
}

function updateRealtimeText(text) {
  const textEl = $('realtimeText');
  const areaEl = $('realtimeTextArea');
  textEl.textContent = text;

  const textWidth = textEl.offsetWidth;
  const MAX_CONTENT = 440;
  const MIN_W = 150;
  const MAX_W = 520;

  if (textWidth <= MAX_CONTENT) {
    const target = Math.max(MIN_W, textWidth + 80);
    areaEl.classList.remove('fading');
    textEl.style.transform = '';
    invoke('resize_capsule_window', {
      width: Math.min(target, MAX_W), height: 40.0
    }).catch(() => {});
  } else {
    invoke('resize_capsule_window', {
      width: MAX_W, height: 40.0
    }).catch(() => {});
    areaEl.classList.add('fading');
    textEl.style.transform = `translateX(-${textWidth - MAX_CONTENT}px)`;
  }
}
```

- [ ] **Step 5: 在 `listenCapsuleEvents` 函数中添加**

```javascript
  window.__TAURI__.event.listen('realtime-delta', (event) => {
    if (event.payload && event.payload.text !== undefined) {
      updateRealtimeText(event.payload.text);
    }
  });
```

- [ ] **Step 6: 在 `fullReset` 函数中添加清理**

```javascript
  $('realtimeTextArea').style.display = 'none';
  $('realtimeTextArea').classList.remove('fading');
  $('realtimeText').textContent = '';
  $('realtimeText').style.transform = '';
```

- [ ] **Step 7: Commit**

```bash
git add ui/capsule.html
git commit -m "feat(capsule): add realtime transcription text display with scrolling"
```

---

### Task 9: 前端 — 设置页面 toggle 开关

**Files:**
- Modify: `ui/settings.html`

- [ ] **Step 1: 在高级标签页 ASR 模型行之后、后端状态之前添加**

```html
    <div class="setting-row">
      <div>
        <div class="setting-label">实时转录</div>
        <div class="setting-hint">需要 ASR 后端支持 WebSocket Realtime 端点</div>
      </div>
      <div class="toggle" id="toggleRealtimeAsr" onclick="toggleRealtimeAsr()"></div>
    </div>
```

- [ ] **Step 2: 在 `applyConfig` 函数中 `if (cfg.model)` 行之后添加**

```javascript
  const rtToggle = document.getElementById('toggleRealtimeAsr');
  if (cfg.realtime_asr === true) rtToggle.classList.add('on');
  else rtToggle.classList.remove('on');
```

- [ ] **Step 3: 在 `saveConfig` 函数的 cfg 对象中添加**

```javascript
    realtimeAsr: document.getElementById('toggleRealtimeAsr').classList.contains('on'),
```

- [ ] **Step 4: 在 `toggleEnabled` 函数之后添加**

```javascript
async function toggleRealtimeAsr() {
  const toggle = document.getElementById('toggleRealtimeAsr');
  toggle.classList.toggle('on');
  await saveConfig();
}
```

- [ ] **Step 5: Commit**

```bash
git add ui/settings.html
git commit -m "feat(settings): add realtime transcription toggle in advanced tab"
```

---

### Task 10: 端到端集成测试

- [ ] **Step 1: 编译**

Run: `cd D:/workspace/airType/src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 2: 测试 realtime_asr=false** — 走现有 HTTP 批量转录，无变化

- [ ] **Step 3: 测试 realtime_asr=true + 后端支持 WS** — 胶囊显示实时文字，松开注入

- [ ] **Step 4: 测试 realtime_asr=true + 后端不支持 WS** — 自动 fallback 到 HTTP 批量

- [ ] **Step 5: 测试 ESC 取消** — 胶囊隐藏，无文字注入

- [ ] **Step 6: 测试长时间录音** — 文字超宽后左滚 + 渐变消失

---

## Self-Review Checklist

- [x] **P0: WS 认证头** — Task 5 `connect_realtime` 添加了 `Authorization: Bearer` header
- [x] **P0: futures-util 依赖** — Task 1 添加
- [x] **P0: ws_tx 所有权** — Task 7 使用 `Arc<tokio::sync::Mutex<ws_tx>>`，发送线程和 guard 共享
- [x] **P0: tokio Runtime** — Task 7 每个会话创建独立 `Runtime::new()`，`block_on` 从 rdev 线程调用（安全）
- [x] **P1: 音频采样率** — `session.update` 设 `rate: 16000`，匹配 cpal 采集。如果用 OpenAI API 需改为 24000 + 添加 resample
- [x] **P1: WS 中途断连** — Task 5 `spawn_reader_task` 中检测 recv error → transition to Error
- [x] **P2: 无 dropguard typo** — 使用 `rs.as_ref().map()` 而非解构
- [x] **P2: 无死代码** — 移除了未使用的 `RealtimeEvent` enum
- [x] **P2: 命名清晰** — `build_ws_url(backend_url, model)` 参数名准确
- [x] **Event names 对齐 API** — `input_audio_buffer.append`, `conversation.item.input_audio_transcription.delta/completed`, `session.update/created/updated`
