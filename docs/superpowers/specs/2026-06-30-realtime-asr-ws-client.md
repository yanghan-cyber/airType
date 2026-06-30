# 实时语音转录（WebSocket 客户端）设计

> Status: Approved ｜ Date: 2026-06-30
> 对接后端：`airtype-backend` 的 `/v1/realtime/transcriptions`（2-pass：流式 partial + 离线 sentence，服务端 Silero VAD）

## 目标

按下录音键即建 WebSocket，边录边推 PCM，胶囊窗实时显示文字（partial 覆盖刷新 + sentence 定稿）；松手发 stop 拿定稿后注入光标。与现有 HTTP 批量模式开关共存，WS 连不上自动 fallback。

## 协议（以后端 `entrypoints/protocol.py` 实际实现为准）

**端点：** `ws(s)://<host>:<port>/v1/realtime/transcriptions`（默认 `127.0.0.1:8178`，从 `backend_url` 派生：`http→ws` / `https→wss`）

**音频：** 16kHz / mono / PCM16 LE，**二进制裸字节**（非 base64）。每 **320ms**（5120 样本）一帧 —— 后端实测 Zipformer 解码节拍，更密不增加 partial 频率。

**客户端 → 服务端：**
- 连上发 `{"type":"start"}`
- 录音中持续发 binary PCM 帧
- 松手发 `{"type":"stop"}` 强制 finalize 当前段

**服务端 → 客户端：**
| type | 字段 | 用途 |
|---|---|---|
| `connected` | session_id | 连接成功 |
| `started` | — | start 生效 |
| `speech_started` | segment_id | VAD 检测到说话 |
| `partial` | text, segment_id | 同段**覆盖式**刷新（实时低准） |
| `sentence` | text, segment_id | 该段**定稿**（offline LLM 纠正，高准） |
| `speech_stopped` | segment_id | VAD 静音端点 |
| `stopped` | — | stop 确认（此前会先吐最后一句 sentence） |
| `error` | message | 错误 |

**文本语义：** 同一 segment_id 内多个 partial 覆盖刷新；收到该 id 的 sentence 即定稿；下一段换新 id。

## 运行时模型（性能 + 延迟）

- **全局 tokio Runtime**：app 启动建一次，进程内常驻。**不**按录音会话重建 —— 消除启停延迟。
- **WS 连接按需建立/断开**：按下键 → spawn 会话任务建 WS、收发；松手 stop 拿定稿后 → 关 WS、任务结束。连接不常驻、不空转。
- 会话任务内 sender/receiver 两个并发 task，`tokio::sync::mpsc` 串联音频块。参考后端 `scripts/realtime_client.py`。

## 状态机（state.rs）

新增变体：
```rust
RealtimeRecording {
    started_at: Instant,
    finalized: Vec<String>,   // 已收 sentence 的定稿句
    current_partial: String,  // 当前段未定稿 partial
}
```
显示文本 = `finalized.concat() + current_partial`。

## 音频路径

实时模式走与批量不同的路径：
- `audio.rs` 新增 `take_recent_samples(&mut self) -> Vec<i16>`（细粒度取增量，不清空全量）
- 一个定时器（320ms）取出增量 → i16 LE bytes → mpsc → 会话 sender → WS binary
- 批量模式的 `take_pcm_bytes()` 不变；两条路径互斥
- RMS 仍从同路音频算，波形动画不变

## 文本累积与注入

- `partial` → 按 segment_id 更新 `current_partial`（新 id 出现则旧 partial 丢弃）→ emit `realtime-text` 到前端
- `sentence` → push 进 `finalized`，清空 `current_partial`
- 松手 → 发 `stop` → 等 `stopped` → 注入 `finalized.concat()`
- ESC → 关 WS、丢弃累积、不注入

## Fallback

按下键 `realtime_asr==true` 时：建 WS（超时 ~1.5s）；失败/超时/error → 切回 `Recording` 批量模式。`realtime_asr==false` → 直接批量，零改动。

## 配置（config.rs）

```rust
#[serde(default)]
pub realtime_asr: bool,   // 默认 false
```

## 后端固有延迟（非客户端可优化）

- 首 partial 约 `realtime_first_partial_ms`（480ms）后到达
- 整句 sentence 需 VAD 静音 `silence_duration_ms`（500ms）触发

## 文件改动

| 文件 | 改动 |
|---|---|
| `asr_realtime.rs` | 新增 — WS 连接 / sender / receiver / 文本累积 / 事件 emit |
| `main.rs` | 全局 Runtime、hotkey 转换分支实时/批量、注册模块 |
| `state.rs` | `RealtimeRecording` 变体 + 转移规则 |
| `audio.rs` | `take_recent_samples()` |
| `config.rs` | `realtime_asr` 字段 |
| `Cargo.toml` | tokio（rt-multi-thread）、tokio-tungstenite |
| `capsule.html` | 实时文字区 + `realtime-text` 监听 |
| `settings.html` | 高级页"实时转录"开关 |
