# 流式实时转录功能设计

## 概述

为 AirType 添加实时流式语音转录功能。用户录音时可实时看到转录文字在胶囊中显示，松开按键后直接注入最终文本到光标位置。

## 配置变更

### AppConfig 新增字段（`config.rs`）

```rust
pub struct AppConfig {
    // ... 现有字段 ...

    /// 是否启用实时转录（WebSocket Realtime）
    pub realtime_asr: bool,
}
```

- 默认值：`false`
- 序列化名：`realtime_asr`
- 向后兼容：`#[serde(default)]` 使旧配置文件加载时自动为 false

### 设置页面 UI 变更（`settings.html`）

在"高级"标签页的 ASR 模型选择下方，新增一行：

```
[实时转录]  [toggle 开关]
            需要 ASR 后端支持 WebSocket Realtime 端点
```

使用现有的 `.toggle` 组件样式，与其他开关一致。

## 三级 Fallback 策略

当用户按下录音键时：

```
1. realtime_asr == true？
   ├─ 是 → 尝试 WebSocket Realtime 连接
   │        ├─ 成功 → 进入实时转录模式
   │        └─ 失败 → fallback 到步骤 2
   └─ 否 → 进入步骤 2

2. 当前的 HTTP 批量转录模式（现有逻辑）
   ├─ 成功 → 正常流程
   └─ 失败 → 显示错误（现有逻辑）
```

## 后端实现

### 新增模块：`asr_realtime.rs`

WebSocket Realtime Transcription 客户端，遵循 OpenAI Realtime Transcription API 协议格式。

**连接流程：**
1. 将 `backend_url` 的 `http(s)://` 替换为 `ws(s)://`，拼接 `/realtime?model={model}`
2. 建立 WebSocket 连接
3. 收到 `session.created` → 确认连接成功
4. 发送 `session.update` 配置转录会话：
   ```json
   {
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
             "model": "{model}",
             "language": "{language}"
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
   }
   ```
5. 收到 `session.updated` → 配置已生效，可开始发送音频
6. 录音期间持续发送 `input_audio_buffer.append` 事件

**音频发送：**
- 音频格式：PCM16, 16kHz, 单声道（当前录音参数，通过 session.update 的 `audio.input.format.rate` 告知服务端）
- 发送间隔：每 200ms 发送一个 `input_audio_buffer.append` 事件
- 音频数据：Base64 编码后放入 JSON 事件的 `audio` 字段
- 事件格式：
  ```json
  {
    "type": "input_audio_buffer.append",
    "audio": "<base64_pcm16>"
  }
  ```

**事件处理：**
- `session.created` → 连接成功
- `session.updated` → 会话配置更新确认，可开始发送音频
- `conversation.item.input_audio_transcription.delta` → 增量转录片段（append-only），通过 Tauri event 推送到前端拼接显示
  ```json
  {
    "type": "conversation.item.input_audio_transcription.delta",
    "item_id": "item_003",
    "content_index": 0,
    "delta": "你好，"
  }
  ```
- `conversation.item.input_audio_transcription.completed` → 单个语音轮次完成，包含该轮次的完整转录
  ```json
  {
    "type": "conversation.item.input_audio_transcription.completed",
    "item_id": "item_003",
    "content_index": 0,
    "transcript": "你好，今天天气怎么样？"
  }
  ```
- `error` → 触发 fallback 或显示错误

**结束录音：**
- 直接关闭 WebSocket 连接（服务端会处理连接关闭）
- 使用已累积的完整转录文本作为最终结果注入光标

**关键结构：**

```rust
pub struct RealtimeAsrClient {
    ws_url: String,
    model: String,
    api_key: String,
}

pub enum RealtimeEvent {
    SessionCreated,                                    // 连接就绪
    SessionUpdated,                                    // 配置确认
    TranscriptDelta { item_id: String, delta: String }, // 增量文字片段（append-only）
    TranscriptCompleted { item_id: String, transcript: String }, // 单轮次完成
    Error { message: String },                         // 错误
}
```

**delta 拼接逻辑：**
- delta 是 append-only 增量片段（如 " hear"、"  skies"），不是累积文本
- 后端维护一个 `accumulated_text: String`，每收到 delta 就 `push_str`
- 每个 completed 事件也追加到 accumulated_text（轮次间用空格分隔）
- 每次拼接后通过 Tauri event 推送完整累积文本到前端
- 前端无需自己拼接，直接使用后端推送的完整文本

### 修改 `asr.rs`

现有 `transcribe` 方法不变，保持作为 fallback。

### 修改 `state.rs`

`RecordingState` 新增变体：

```rust
enum RecordingState {
    Idle,
    Recording { started_at: Instant },
    RealtimeRecording { started_at: Instant, accumulated_text: String },  // 新增
    Processing,
    // ... 其余不变
}
```

`RealtimeRecording` 状态下：
- 后端维护 `accumulated_text: String`，每收到 delta 就拼接
- 前端持续显示音波 + 迷你计时器 + 实时文字（后端推送完整累积文本）
- 松开按键后直接用 `accumulated_text` 注入光标位置

### 修改 `commands.rs` / `main.rs`

录音启动时判断 `realtime_asr` 配置：
- `true` → 尝试建立 WebSocket 连接，成功则进入 `RealtimeRecording` 状态
- `false` 或 WebSocket 失败 → 进入现有 `Recording` 状态

### 修改录音循环（`audio.rs`）

现有录音循环中已有音频 buffer 回调。在 `RealtimeRecording` 模式下：
- 每 200ms 累积 PCM 数据
- 将累积的 PCM 数据 Base64 编码
- 通过 WebSocket 发送 `input_audio_buffer.append` 事件
- 同时继续向前端发送 RMS 数据（驱动音波动画）

## 前端实现

### 胶囊 UI 变更（`capsule.html`）

**新增 CSS：**

```css
/* 实时转录文字容器 */
.realtime-text-area {
  flex: 1;
  overflow: hidden;
  position: relative;
  height: 100%;
  display: flex;
  align-items: center;
  white-space: nowrap;
}

/* 左侧渐变消失效果（文字滚动时） */
.realtime-text-area.fading::before {
  content: '';
  position: absolute;
  left: 0; top: 0; bottom: 0;
  width: 50px;
  background: linear-gradient(to right, #181818 0%, #181818 40%, transparent 100%);
  z-index: 1;
  pointer-events: none;
}
```

**录音状态下胶囊布局：**
```
[音波+迷你计时器] [分隔线] [实时文字区域（可滚动）]
```

- 音波区域下方显示迷你计时器（小字体 `font-size: 9px`）
- 文字短时胶囊宽度自适应
- 超过最大宽度（520px）后锁定宽度，文字向左滚动，左侧渐变消失
- 最新文字始终可见在右侧，有闪烁光标效果

**前端事件监听：**

新增 Tauri 事件 `realtime-delta`：
```javascript
window.__TAURI__.event.listen('realtime-delta', (event) => {
  // event.payload.text 是完整的累积转录文本（后端已拼接好）
  updateRealtimeText(event.payload.text);
});
```

`updateRealtimeText` 函数：
1. 更新文字内容（完整累积文本，无需前端拼接）
2. 测量文字宽度
3. 未超限 → 调整胶囊宽度（动态 resize 窗口）
4. 超限 → 锁定宽度，设置 `transform: translateX(-offset)`，添加 fading class

**录音结束：**
- 不再播放 loading/done 动画
- 直接隐藏胶囊
- 文本已通过后端注入光标位置

### 胶囊窗口大小调整

实时模式下，窗口宽度需要动态变化：
- 短文字：调用 `resize_capsule_window` 设置精确宽度
- 长文字：锁定为最大宽度 520px

## 数据流总结

### 实时转录模式
```
用户按下录音键
  → 后端连接 WebSocket (ws(s)://{backend_url}/realtime?model={model})
  → 收到 session.created → 连接成功
  → 发送 session.update 配置转录会话（type: transcription, audio format, model, VAD）
  → 收到 session.updated → 配置确认
  → 进入 RealtimeRecording 状态
  → 前端显示音波 + 迷你计时器 + 空文字区

录音中（每 200ms）
  → audio.rs 采集并累积 PCM 数据
  → Base64 编码，发送 input_audio_buffer.append
  → 收到 conversation.item.input_audio_transcription.delta（增量片段）
  → 后端拼接 delta 到 accumulated_text
  → Tauri event 推送完整累积文本到前端
  → 前端更新文字显示 + 调整胶囊宽度

用户松开按键
  → 关闭 WebSocket 连接
  → 使用 accumulated_text 作为最终文本
  → 注入光标位置
  → 胶囊隐藏
```

### Fallback 模式（与现有行为一致）
```
WebSocket 连接失败
  → 进入普通 Recording 状态
  → 录音完成后 HTTP 批量转录
  → 走现有的 loading/done 动画流程
```

## 文件变更清单

| 文件 | 变更类型 | 说明 |
|------|---------|------|
| `src-tauri/src/config.rs` | 修改 | 新增 `realtime_asr` 字段 |
| `src-tauri/src/asr_realtime.rs` | 新增 | WebSocket Realtime Transcription 客户端 |
| `src-tauri/src/asr.rs` | 不变 | 保持现有批量转录逻辑 |
| `src-tauri/src/state.rs` | 修改 | 新增 `RealtimeRecording` 状态变体 |
| `src-tauri/src/audio.rs` | 修改 | 录音循环中增加 WS 音频发送逻辑 |
| `src-tauri/src/commands.rs` | 修改 | 录音启动时判断 realtime 配置 |
| `src-tauri/src/main.rs` | 修改 | 注册新模块 |
| `src-tauri/Cargo.toml` | 修改 | 添加 tokio-tungstenite、base64 依赖 |
| `ui/capsule.html` | 修改 | 实时文字显示区域 + 事件监听 |
| `ui/settings.html` | 修改 | 高级标签页新增实时转录开关 |

## 依赖

- `tokio-tungstenite`：WebSocket 客户端（需要 async runtime）
- `base64`：音频数据编码
- `serde_json`：JSON 事件解析（已有）

## 测试要点

1. WebSocket 连接成功时：实时转录正常工作，文字实时更新
2. WebSocket 连接失败时：自动 fallback 到 HTTP 批量转录
3. 配置开关关闭时：完全走现有逻辑，无影响
4. 长时间录音：文字滚动、渐变消失效果正常
5. 短时间录音（<3s）：胶囊自适应宽度，不出现滚动
6. 窗口 resize：胶囊宽度动态调整，不影响其他 UI
