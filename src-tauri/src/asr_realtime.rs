//! Realtime ASR over WebSocket (client side).
//!
//! Talks the airtype-backend `/v1/realtime/transcriptions` protocol:
//!   client → server : {"type":"start"} / binary PCM16 / {"type":"stop"}
//!   server → client : connected / started / partial / sentence / stopped / error
//!
//! One session == one recording. Launched on key-press, finalized on key-release.

use crate::audio::{self, AudioBuffer};
use crate::log::log_debug;
use crate::state::{AppState, RecordingState};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const CONNECT_TIMEOUT_MS: u64 = 1500;
const HANDSHAKE_TIMEOUT_MS: u64 = 1500;
const CHUNK_INTERVAL_MS: u64 = 320;
const FINALIZE_TIMEOUT_MS: u64 = 3000;
const STOP_ACK_TIMEOUT_MS: u64 = 2500;
const TARGET_SR: u32 = 16000;
/// Samples per WS frame. The streaming engine expects ~320ms chunks; sending
/// one giant frame (e.g. the whole 2s backlog) makes it drop early content.
/// Backlog from the handshake gap is split into chunks of this size.
const SAMPLES_PER_CHUNK: usize = (TARGET_SR as usize) * (CHUNK_INTERVAL_MS as usize) / 1000;

#[derive(Debug)]
enum SessionCmd {
    Stop,
    Cancel,
}

/// Handle to a running realtime session. Owned by the pipeline while recording.
pub struct RealtimeSession {
    cmd_tx: mpsc::Sender<SessionCmd>,
    connected: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    final_rx: Option<oneshot::Receiver<String>>,
    join: Option<JoinHandle<()>>,
}

impl RealtimeSession {
    /// Spawn the session on the given runtime. Returns immediately; the WS
    /// handshake happens in the background (see `is_connected` / `is_failed`).
    #[allow(clippy::too_many_arguments)]
    pub fn launch(
        runtime: &tokio::runtime::Runtime,
        backend_url: &str,
        api_key: &str,
        buffer: Arc<Mutex<AudioBuffer>>,
        native_sr: u32,
        state: Arc<Mutex<AppState>>,
        app_handle: AppHandle,
    ) -> RealtimeSession {
        let connected = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (final_tx, final_rx) = oneshot::channel::<String>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<SessionCmd>(8);

        let ws_url = build_ws_url(backend_url);
        log_debug(&format!("[realtime] launching session → {}", ws_url));

        let join = runtime.spawn(run(
            ws_url,
            api_key.to_string(),
            buffer,
            native_sr,
            state,
            app_handle,
            cmd_rx,
            Some(final_tx),
            connected.clone(),
            failed.clone(),
        ));

        RealtimeSession {
            cmd_tx,
            connected,
            failed,
            final_rx: Some(final_rx),
            join: Some(join),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// Send stop, wait for the trailing `sentence`/`stopped`, return final text.
    pub async fn finalize(mut self) -> Result<String, String> {
        let _ = self.cmd_tx.send(SessionCmd::Stop).await;
        let rx = self.final_rx.take().ok_or("finalize already consumed")?;
        match tokio::time::timeout(Duration::from_millis(FINALIZE_TIMEOUT_MS), rx).await {
            Ok(Ok(text)) => Ok(text),
            _ => {
                if let Some(j) = self.join.take() {
                    j.abort();
                }
                Err("realtime finalize timeout".into())
            }
        }
    }

    /// Abort the session without injecting text.
    pub async fn cancel(mut self) {
        let _ = self.cmd_tx.send(SessionCmd::Cancel).await;
        if let Some(j) = self.join.take() {
            j.abort();
        }
    }
}

/// Derive the WS endpoint from backend_url.
/// Assumes backend_url ends with `/v1` (e.g. http://127.0.0.1:8178/v1).
///
/// NOTE: `localhost` is rewritten to `127.0.0.1`. On Windows, `localhost`
/// resolves to IPv6 `::1` first; local backends typically listen on IPv4
/// `0.0.0.0` only, so an IPv6 WS connect hangs until timeout (reqwest's
/// HTTP path falls back to IPv4 on its own, which is why batch worked while
/// realtime timed out). Pinning IPv4 makes the WS connect instant.
fn build_ws_url(backend_url: &str) -> String {
    let ws = backend_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let ws = ws.replace("://localhost", "://127.0.0.1");
    let base = ws.trim_end_matches('/');
    format!("{}/realtime/transcriptions", base)
}

async fn run(
    ws_url: String,
    api_key: String,
    buffer: Arc<Mutex<AudioBuffer>>,
    native_sr: u32,
    state: Arc<Mutex<AppState>>,
    app_handle: AppHandle,
    mut cmd_rx: mpsc::Receiver<SessionCmd>,
    mut final_tx: Option<oneshot::Sender<String>>,
    connected: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) {
    // ── Connect ──
    let mut req = match ws_url.as_str().into_client_request() {
        Ok(r) => r,
        Err(e) => {
            log_debug(&format!("[realtime] invalid url: {}", e));
            failed.store(true, Ordering::SeqCst);
            return;
        }
    };
    if !api_key.is_empty() {
        if let Ok(hv) = format!("Bearer {}", api_key).parse() {
            req.headers_mut().insert("Authorization", hv);
        }
    }
    // NOTE: connect without a timeout wrapper so the real handshake error
    // surfaces (a timeout masks the underlying cause). The handshake itself
    // is fast when the server is reachable.
    //
    // For ws:// (local backends), bypass connect_async's DNS resolution, which
    // adds a fixed ~2.2s stall on Windows (tokio's getaddrinfo). Connect the
    // TCP stream directly to the parsed host:port, then run the WS handshake
    // over it via client_async. wss:// still uses connect_async (rare, needs TLS).
    let is_tls = ws_url.starts_with("wss://");
    if !is_tls {
        let (host, port) = match extract_host_port(&ws_url) {
            Some(hp) => hp,
            None => {
                log_debug("[realtime] failed to parse host:port from url");
                failed.store(true, Ordering::SeqCst);
                return;
            }
        };
        let tcp = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(s) => s,
            Err(e) => {
                log_debug(&format!("[realtime] TCP connect failed: {}", e));
                failed.store(true, Ordering::SeqCst);
                return;
            }
        };
        match client_async(req, tcp).await {
            Ok((s, _resp)) => {
                session_loop(s, buffer, native_sr, state, app_handle, cmd_rx, final_tx, connected, failed).await;
            }
            Err(e) => {
                log_debug(&format!("[realtime] handshake error: {}", e));
                failed.store(true, Ordering::SeqCst);
            }
        }
    } else {
        match connect_async(req).await {
            Ok((s, _resp)) => {
                session_loop(s, buffer, native_sr, state, app_handle, cmd_rx, final_tx, connected, failed).await;
            }
            Err(e) => {
                log_debug(&format!("[realtime] tls connect error: {}", e));
                failed.store(true, Ordering::SeqCst);
            }
        }
    }
}

/// Streaming session loop over an established WS stream. Generic over the
/// underlying transport so both `client_async` (TcpStream) and `connect_async`
/// (MaybeTlsStream) flows share one implementation.
async fn session_loop<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    buffer: Arc<Mutex<AudioBuffer>>,
    native_sr: u32,
    state: Arc<Mutex<AppState>>,
    app_handle: AppHandle,
    mut cmd_rx: mpsc::Receiver<SessionCmd>,
    mut final_tx: Option<oneshot::Sender<String>>,
    connected: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // ── Handshake: expect `connected`, then send `start` ──
    let handshake = async {
        if let Some(Ok(Message::Text(t))) = ws_rx.next().await {
            let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
            if v.get("type").and_then(|x| x.as_str()) == Some("connected") {
                return Ok(());
            }
        }
        Err(())
    };
    if tokio::time::timeout(Duration::from_millis(HANDSHAKE_TIMEOUT_MS), handshake)
        .await
        .is_err()
    {
        log_debug("[realtime] handshake timeout (no connected event)");
        failed.store(true, Ordering::SeqCst);
        return;
    }
    if ws_tx
        .send(Message::Text(r#"{"type":"start"}"#.to_string().into()))
        .await
        .is_err()
    {
        failed.store(true, Ordering::SeqCst);
        return;
    }
    connected.store(true, Ordering::SeqCst);
    log_debug("[realtime] connected + start sent");

    // ── Streaming loop ──
    // Local authoritative text (decoupled from AppState so finalize is robust
    // even after the pipeline transitions away from RealtimeRecording).
    let mut finalized: Vec<String> = Vec::new();
    let mut current_partial = String::new();

    let mut tick = tokio::time::interval(Duration::from_millis(CHUNK_INTERVAL_MS));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stop_sent = false;
    let mut stop_deadline: Option<Instant> = None;

    loop {
        let mut do_break = false;
        tokio::select! {
            _ = tick.tick(), if !stop_sent => {
                let chunk = buffer.lock().unwrap().take_recent_samples();
                if !chunk.is_empty() {
                    let pcm16 = to_pcm16_16k(&chunk, native_sr);
                    // Split into SAMPLES_PER_CHUNK frames so a 2s handshake
                    // backlog isn't sent as one giant truncated frame.
                    for sub in pcm16.chunks(SAMPLES_PER_CHUNK.max(1)) {
                        let bytes: Vec<u8> = sub.iter().flat_map(|&s| s.to_le_bytes()).collect();
                        let _ = ws_tx.send(Message::Binary(bytes.into())).await;
                    }
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCmd::Stop) => {
                        // flush any unsent audio before finalizing
                        let chunk = buffer.lock().unwrap().take_recent_samples();
                        if !chunk.is_empty() {
                            let pcm16 = to_pcm16_16k(&chunk, native_sr);
                            for sub in pcm16.chunks(SAMPLES_PER_CHUNK.max(1)) {
                                let bytes: Vec<u8> = sub.iter().flat_map(|&s| s.to_le_bytes()).collect();
                                let _ = ws_tx.send(Message::Binary(bytes.into())).await;
                            }
                        }
                        log_debug("[realtime] stop cmd → sending stop");
                        let _ = ws_tx.send(Message::Text(r#"{"type":"stop"}"#.to_string().into())).await;
                        stop_sent = true;
                        stop_deadline = Some(Instant::now() + Duration::from_millis(STOP_ACK_TIMEOUT_MS));
                    }
                    Some(SessionCmd::Cancel) | None => {
                        log_debug("[realtime] cancel → ending session");
                        do_break = true;
                    }
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                        let etype = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
                        match etype {
                            "partial" => {
                                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                current_partial = text.clone();
                                update_state_partial(&state, text);
                                emit_text(&finalized, &current_partial, &app_handle);
                            }
                            "sentence" => {
                                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                finalized.push(text.clone());
                                current_partial.clear();
                                update_state_sentence(&state, text);
                                emit_text(&finalized, &current_partial, &app_handle);
                            }
                            "stopped" => {
                                log_debug("[realtime] stopped → finalize");
                                if let Some(tx) = final_tx.take() {
                                    let _ = tx.send(gather(&finalized, &current_partial));
                                }
                                do_break = true;
                            }
                            "error" => {
                                let m = v.get("message").and_then(|x| x.as_str()).unwrap_or("realtime error");
                                log_debug(&format!("[realtime] server error: {}", m));
                                failed.store(true, Ordering::SeqCst);
                                do_break = true;
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        log_debug(&format!("[realtime] ws error: {}", e));
                        failed.store(true, Ordering::SeqCst);
                        do_break = true;
                    }
                    None => {
                        log_debug("[realtime] ws closed by server");
                        do_break = true;
                    }
                }
            }
        }
        if do_break {
            break;
        }
        if let Some(dl) = stop_deadline {
            if Instant::now() >= dl {
                log_debug("[realtime] stop-ack timeout → finalize with current text");
                if let Some(tx) = final_tx.take() {
                    let _ = tx.send(gather(&finalized, &current_partial));
                }
                break;
            }
        }
    }
    let _ = ws_tx.close().await;
    log_debug("[realtime] session ended");
}

fn gather(finalized: &[String], current_partial: &str) -> String {
    let mut t = finalized.concat();
    t.push_str(current_partial);
    t
}

/// Extract (host, port) from a ws:// URL for direct TCP connection (bypass DNS).
/// Returns None on malformed URLs.
fn extract_host_port(ws_url: &str) -> Option<(String, u16)> {
    let after_scheme = ws_url.strip_prefix("ws://")
        .or_else(|| ws_url.strip_prefix("wss://"))?;
    let authority = after_scheme.split('/').next()?;
    let authority = authority.split('?').next()?;
    // strip userinfo if present
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    // strip IPv6 brackets
    if let Some(rest) = authority.strip_prefix('[') {
        let host = rest.split(']').next()?;
        let port_part = rest.split(']').nth(1).unwrap_or("");
        let port = port_part.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(443);
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) => Some((h.to_string(), p.parse().ok()?)),
        None => Some((authority.to_string(), if ws_url.starts_with("wss://") { 443 } else { 80 })),
    }
}

fn update_state_partial(state: &Arc<Mutex<AppState>>, text: String) {
    if let Ok(mut s) = state.lock() {
        if let RecordingState::RealtimeRecording { current_partial, .. } = &mut s.recording {
            *current_partial = text;
        }
    }
}

fn update_state_sentence(state: &Arc<Mutex<AppState>>, text: String) {
    if let Ok(mut s) = state.lock() {
        if let RecordingState::RealtimeRecording {
            finalized,
            current_partial,
            ..
        } = &mut s.recording
        {
            // Mirror the local accumulation so the capsule stays in sync.
            // (Local copy is authoritative; this is display-only.)
            finalized.push(text);
            current_partial.clear();
        }
    }
}

fn emit_text(finalized: &[String], current_partial: &str, app: &AppHandle) {
    let text = gather(finalized, current_partial);
    let _ = app.emit("realtime-text", serde_json::json!({ "text": text }));
}

/// Resample native-rate i16 samples to 16kHz i16 samples.
fn to_pcm16_16k(samples: &[i16], native_sr: u32) -> Vec<i16> {
    if samples.is_empty() {
        return Vec::new();
    }
    let f: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
    let r = audio::resample(&f, native_sr, TARGET_SR);
    r.iter()
        .map(|&x| (x.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ws_url_with_v1() {
        let url = build_ws_url("http://127.0.0.1:8178/v1");
        assert_eq!(url, "ws://127.0.0.1:8178/v1/realtime/transcriptions");
    }

    #[test]
    fn test_build_ws_url_https() {
        let url = build_ws_url("https://api.example.com/v1");
        assert_eq!(url, "wss://api.example.com/v1/realtime/transcriptions");
    }

    #[test]
    fn test_build_ws_url_trailing_slash() {
        let url = build_ws_url("http://127.0.0.1:8178/v1/");
        assert_eq!(url, "ws://127.0.0.1:8178/v1/realtime/transcriptions");
    }

    #[test]
    fn test_build_ws_url_localhost_forced_ipv4() {
        // localhost must be pinned to 127.0.0.1 (Windows IPv6 ::1 hangs)
        let url = build_ws_url("http://localhost:8178/v1");
        assert_eq!(url, "ws://127.0.0.1:8178/v1/realtime/transcriptions");
    }

    #[test]
    fn test_build_ws_url_https_localhost() {
        let url = build_ws_url("https://localhost:8178/v1");
        assert_eq!(url, "wss://127.0.0.1:8178/v1/realtime/transcriptions");
    }

    #[test]
    fn test_to_pcm16_16k_empty() {
        assert!(to_pcm16_16k(&[], 48000).is_empty());
    }

    #[test]
    fn test_to_pcm16_16k_same_rate() {
        // 16k → 16k, no resampling needed; 2 samples → 2 samples
        let s = to_pcm16_16k(&[0, 16384], 16000);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], 0);
    }

    #[test]
    fn test_to_pcm16_16k_downsample() {
        // 48000 → 16000 is 1/3, so 48000 samples → 16000 samples
        let samples = vec![0i16; 48000];
        let s = to_pcm16_16k(&samples, 48000);
        assert_eq!(s.len(), 16000);
    }

    #[test]
    fn test_gather_concat() {
        let finalized = vec!["你好".to_string(), "世界".to_string()];
        let text = gather(&finalized, "啊");
        assert_eq!(text, "你好世界啊");
    }

    #[test]
    fn test_gather_empty() {
        assert_eq!(gather(&[], ""), "");
    }

    #[test]
    fn test_extract_host_port_basic() {
        assert_eq!(extract_host_port("ws://127.0.0.1:8178/v1/realtime"), Some(("127.0.0.1".into(), 8178)));
    }

    #[test]
    fn test_extract_host_port_localhost() {
        assert_eq!(extract_host_port("ws://localhost:8080/path"), Some(("localhost".into(), 8080)));
    }

    #[test]
    fn test_extract_host_port_no_port() {
        assert_eq!(extract_host_port("ws://example.com/path"), Some(("example.com".into(), 80)));
    }

    #[test]
    fn test_extract_host_port_wss_default() {
        assert_eq!(extract_host_port("wss://example.com/path"), Some(("example.com".into(), 443)));
    }
}
