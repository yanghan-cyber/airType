use crate::llm::LlmConfig;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Returns the path to config.json relative to the executable's directory.
/// This ensures the config is found regardless of the current working directory.
pub fn config_path() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    exe_dir.join("config.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessingMode {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub system_prompt: String,
    pub user_template: String,
    #[serde(default)]
    pub show_in_popup: bool,
    #[serde(default)]
    pub popup_order: u32,
}

impl ProcessingMode {
    pub fn direct() -> Self {
        Self {
            id: "direct".to_string(),
            name: "跳过 AI".to_string(),
            icon: "📝".to_string(),
            system_prompt: String::new(),
            user_template: "{asr_text}".to_string(),
            show_in_popup: false,
            popup_order: 0,
        }
    }

    pub fn polish() -> Self {
        Self {
            id: "polish".to_string(),
            name: "文本润色".to_string(),
            icon: "✨".to_string(),
            system_prompt: r#"# Role
你是一个严格的 ASR 语音识别后处理助手。核心原则：最小改动。只修正确凿的识别错误和口语噪音，绝不改变用户原意、语气和表达风格。

## 必须修正的项目
1. **深度去除口吃与冗余（最高优先级）**：
   - **单字/多字结巴**：强制合并无意义的连续重复字词（如"正正正正式"→"正式"，"我我我"→"我"，"就是就是"→"就是"）。
   - **思维停顿与自我纠正**：智能识别并删除说话人在组织语言时的无效片段、重叠和无意义的语气助词（如"啊，正式"→"正式"，"这个就是我上面就是我"→"这个就是我上面"）。
2. **标点符号**：合理添加标点符号进行断句，使句子易读。
3. **数字格式**：把中文数字转为阿拉伯数字（"一百"→"100"）。成语俗语保留。
4. **同音错字**：只有在上下文明显不通顺时才修正（如"呈序"→"程序"）。
5. **口语填充词**：删除纯填充的"嗯""呃"。"那个""就是""然后"仅在完全无指代时删除。

## 谨慎处理的项目（拿不准时必须保留原文）
- 任何可能改变用户个人表达习惯的内容。

## 严格禁止
- 绝对禁止润色、美化或提升正式度。
- 绝对禁止输出任何解释、问候或附加内容。

## Examples (请严格参考以下清理逻辑)

### 场景一：严重口吃与自我重复（去除思维卡壳的废话）
**Input:** <user_input>正式啊，对，确实，我现在就是想说一下这种正式的认识啊，对，正式的认识我应该怎么去解决呢？</user_input>
**Output:** 确实，我现在就是想说一下这种正式的认识，我应该怎么去解决呢？

### 场景二：中英同音词误认（精准还原技术与专业术语）
**Input:** <user_input>但是你现在只写了，只有一种的 Free。Free Short，你应该多考虑几种 Free Short 的情况呀。</user_input>
**Output:** 但是你现在只写了一种 Few-shot，你应该多考虑几种 Few-shot 的情况呀。

### 场景三：中英夹杂与常见开发框架错音
**Input:** <user_input>我们在双 A 一百的服务器上，用那个微楼 M 部署了一个本地的那个，发丝特 API 服务。</user_input>
**Output:** 我们在双 A100 的服务器上，用 vLLM 部署了一个本地的 FastAPI 服务。

### 场景四：思维反转与中途改口（只保留最终意图）
**Input:** <user_input>这个界面的 UI 布局...哎不对，那个前端的组件库，我们直接用那种扁平化设计的就行。</user_input>
**Output:** 那个前端的组件库，我们直接用扁平化设计的就行。

### 场景五：长句拖沓与无意义口语填充
**Input:** <user_input>就是说啊，我们今天主要讨论的那个，嗯，也就是那个后端的音频服务的架构设计吧。</user_input>
**Output:** 我们今天主要讨论后端音频服务的架构设计。

# Execution
请严格遵循上述规则，直接输出清理修正后的纯文本。只处理 `<user_input>` 标签内的内容：
"#.to_string(),
            user_template: "<user_input>\n{asr_text}\n</user_input>".to_string(),
            show_in_popup: true,
            popup_order: 1,
        }
    }

    pub fn translate_en() -> Self {
        Self {
            id: "translate_en".to_string(),
            name: "翻译 中⇔英".to_string(),
            icon: "🌐".to_string(),
            system_prompt: r#"# Role
你是一位精通多国语言的专业翻译官，尤其擅长中英互译。

# Task
将用户输入的文本准确地翻译成目标语言（若输入中文则翻译为英文，若输入英文则翻译为中文）。要求译文准确、流畅、自然，符合目标语言的母语表达习惯。

# Rules
1. **信达雅原则**：准确传达原文的含义，不遗漏、不曲解；表达通顺流畅；用词优雅贴切。
2. **语境适应**：根据原文的文体（如正式邮件、技术文档、日常对话、新闻报道）自动调整译文的语气和正式程度。
3. **保留格式**：严格保留原文中的特殊排版、标点符号、换行、Markdown 标记、特殊符号以及代码块等。
4. **专业术语**：遇到通用的专有名词、缩写，需使用标准的惯用表达，不可生硬直译。
5. **静默输出**：绝不输出任何解释性文字、问候语或"翻译结果如下"等前缀。只输出最终的纯翻译文本。

# Examples
**Input:** The system architecture is designed to be highly scalable and fault-tolerant.
**Output:** 该系统架构的设计旨在实现高可扩展性与容错性。

**Input:** 麻烦确认一下明天的会议时间，另外请把最新的排期表发给我，谢谢。
**Output:** Please confirm the time for tomorrow's meeting. Additionally, please send me the latest schedule. Thank you.

# Execution
请严格遵循上述所有规则，直接翻译以下 `<text_to_translate>` 标签内的文本内容："#.to_string(),
            user_template: "<text_to_translate>\n{asr_text}\n</text_to_translate>".to_string(),
            show_in_popup: true,
            popup_order: 2,
        }
    }

    pub fn formal_polish() -> Self {
        Self {
            id: "formal_polish".to_string(),
            name: "正式润色".to_string(),
            icon: "🖋️".to_string(),
            system_prompt: r#"# Role
你是一个专业的 ASR 语音文本后处理与商务/学术润色专家。

# Task
接收原始语音识别文本，在绝对保持核心事实不变的前提下，将其转化为专业、流畅、得体的正式书面表达，适用于工作邮件、项目申报、学术报告及高管汇报。

# Rules
1. **基础去噪**：修正 ASR 同音错字，添加准确标点，将中文数字规范为阿拉伯数字格式，彻底清除口语填充词（呃、啊、那个、就是说）和无意义结巴。
2. **适度正式化（核心边界）**：
   - 提升词汇的专业度（例如："搞一下"→"推进/落实"，"弄出来"→"研发/构建"）。
   - 允许调整语序、合并短句，使逻辑连贯、主谓宾结构完整。
   - **拒绝过度包装**：语言风格必须保持平实、客观、精炼。绝不使用华丽辞藻，严禁生成带有强烈"AI味"的官腔套话。
3. **专业术语绝对保护**：原文中涉及的任何英文缩写、科研名词、技术架构、专有名词，必须原样保留或修正为行业标准术语，绝不能为了句式通顺而进行强行翻译或省略。
4. **静默输出**：只输出处理后的纯文本，没有任何前后缀、问候或解释。

# Examples
**Input:** <user_input>那个我们接下来要搞一下双显卡服务器上的大模型本地化部署，把那个推理服务给跑起来，准备下个月的评审。</user_input>
**Output:** 我们接下来将推进双显卡服务器上的大模型本地化部署工作，启动推理服务，以备战下个月的评审。

**Input:** <user_input>这个新发现的代谢物吧，它主要就是通过抑制那个通路，然后导致了免疫系统的变化，最后参与了这个狭窄的形成。</user_input>
**Output:** 该新发现的代谢物主要通过抑制相关通路，驱动免疫系统变化，进而参与狭窄的形成。

**Input:** <user_input>辛苦你把那个昨天开会的纪要整理整理发给我哈，万一老板今天下午要看的话就麻烦了。</user_input>
**Output:** 请您协助整理昨天的会议纪要并发送给我，以备领导今日下午审阅，辛苦了。

# Execution
请严格遵循上述规则，对以下 `<user_input>` 内的文本进行正式化润色：
"#.to_string(),
            user_template: "<user_input>\n{asr_text}\n</user_input>".to_string(),
            show_in_popup: true,
            popup_order: 3,
        }
    }

    pub fn apply_template(&self, asr_text: &str) -> String {
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
        self.user_template
            .replace("{asr_text}", asr_text)
            .replace("{timestamp}", &timestamp)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub hotkey: String,
    pub enabled: bool,
    pub model: String,
    pub hotwords: Vec<String>,
    pub language: Option<String>,
    pub backend_url: String,
    pub asr_api_key: String,
    pub capsule_x: Option<i32>,
    pub capsule_y: Option<i32>,
    pub capsule_default_position: String,
    pub capsule_default_offset: u32,

    pub hotkey_secondary: Option<String>,
    pub default_processing_mode: String,
    pub llm: LlmConfig,
    pub processing_modes: Vec<ProcessingMode>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Win".to_string(),
            enabled: true,
            model: "Qwen3-ASR-0.6B".to_string(),
            hotwords: vec![],
            language: None,
            backend_url: "https://api.openai.com/v1".to_string(),
            asr_api_key: String::new(),
            capsule_x: None,
            capsule_y: None,
            capsule_default_position: "bottom".to_string(),
            capsule_default_offset: 70,

            hotkey_secondary: None,
            default_processing_mode: "direct".to_string(),
            llm: LlmConfig::default(),
            processing_modes: vec![
                ProcessingMode::direct(),
                ProcessingMode::polish(),
                ProcessingMode::translate_en(),
                ProcessingMode::formal_polish(),
            ],
        }
    }
}

fn validate_modifier(name: &str) -> Result<(), String> {
    match name {
        "Ctrl" | "Alt" | "Shift" | "Win" | "Meta"
        | "LCtrl" | "LAlt" | "LShift" | "LWin" | "LMeta"
        | "RCtrl" | "RAlt" | "RShift" | "RWin" | "RMeta" => Ok(()),
        _ => Err(format!("Unknown modifier: {}", name)),
    }
}

fn validate_key_name(name: &str) -> Result<(), String> {
    let valid = name.len() == 1 && name.chars().next().unwrap().is_ascii_alphanumeric()
        || matches!(name,
            "Ctrl" | "Alt" | "Shift" | "Win" | "Meta"
            | "LCtrl" | "LAlt" | "LShift" | "LWin" | "LMeta"
            | "RCtrl" | "RAlt" | "RShift" | "RWin" | "RMeta"
            | "Space" | "Return" | "Enter" | "Escape" | "Esc" | "Tab"
            | "Backspace" | "Delete"
            | "Left" | "Right" | "Up" | "Down"
            | "F1"|"F2"|"F3"|"F4"|"F5"|"F6"|"F7"|"F8"|"F9"|"F10"|"F11"|"F12"
        );
    if valid { Ok(()) } else { Err(format!("Unknown key: {}", name)) }
}

impl AppConfig {
    pub fn load(path: &PathBuf) -> Self {
        if path.exists() {
            let content = fs::read_to_string(path).unwrap_or_default();
            let mut cfg: AppConfig = serde_json::from_str(&content).unwrap_or_default();
            // Migrate old `port` field to `backend_url`
            if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&content) {
                if raw.get("backend_url").is_none() {
                    if let Some(port) = raw.get("port").and_then(|v| v.as_u64()) {
                        cfg.backend_url = format!("http://localhost:{}", port);
                    }
                }
            }
            cfg
        } else {
            let cfg = Self::default();
            cfg.save(path);
            cfg
        }
    }

    pub fn save(&self, path: &PathBuf) {
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, content);
        }
    }

    pub fn parse_hotkey(&self) -> Result<(Vec<&str>, &str), String> {
        let parts: Vec<&str> = self.hotkey.split('+').collect();
        if parts.is_empty() || parts.iter().all(|p| p.trim().is_empty()) {
            return Err("Hotkey cannot be empty".to_string());
        }
        if parts.len() == 1 {
            // Single key hotkey
            let key = parts[0];
            validate_key_name(key)?;
            return Ok((vec![], key));
        }
        let main_key = parts.last().unwrap();
        let modifiers = &parts[..parts.len() - 1];
        for m in modifiers {
            validate_modifier(m)?;
        }
        validate_key_name(main_key)?;
        Ok((modifiers.to_vec(), main_key))
    }

    pub fn context_string(&self) -> String {
        self.hotwords.join(",")
    }

    pub fn model_path(&self) -> String {
        format!("models/{}", self.model)
    }

    /// Get modes for popup, sorted by popup_order, excluding "direct".
    pub fn popup_modes(&self) -> Vec<ProcessingMode> {
        let mut modes: Vec<ProcessingMode> = self.processing_modes.iter()
            .filter(|m| m.id != "direct" && m.show_in_popup)
            .cloned()
            .collect();
        modes.sort_by_key(|m| m.popup_order);
        modes
    }

    /// Get all AI modes (excluding "direct"), sorted by popup_order.
    pub fn all_ai_modes(&self) -> Vec<ProcessingMode> {
        let mut modes: Vec<ProcessingMode> = self.processing_modes.iter()
            .filter(|m| m.id != "direct")
            .cloned()
            .collect();
        modes.sort_by_key(|m| m.popup_order);
        modes
    }

    pub fn capsule_position(&self, screen_w: f64, screen_h: f64, win_w: f64, win_h: f64, scale: f64) -> (i32, i32) {
        let (x, y) = if let (Some(sx), Some(sy)) = (self.capsule_x, self.capsule_y) {
            (sx, sy)
        } else {
            let x = ((screen_w - win_w) / 2.0) as i32;
            let offset = (self.capsule_default_offset as f64 * scale) as i32;
            let y = if self.capsule_default_position == "bottom" {
                (screen_h - win_h) as i32 - offset
            } else {
                offset
            };
            (x, y)
        };
        // Clamp to visible screen area
        let max_x = (screen_w - win_w).max(0.0) as i32;
        let max_y = (screen_h - win_h).max(0.0) as i32;
        (x.clamp(0, max_x), y.clamp(0, max_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_values() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.hotkey, "Ctrl+Win");
        assert!(cfg.enabled);
        assert_eq!(cfg.model, "Qwen3-ASR-0.6B");
        assert!(cfg.hotwords.is_empty());
        assert!(cfg.language.is_none());
        assert_eq!(cfg.backend_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_load_save_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = AppConfig::default();
        cfg.hotwords = vec!["魔搭社区".to_string(), "Claude".to_string()];
        cfg.model = "Qwen3-ASR-1.7B".to_string();
        cfg.save(&path);
        let loaded = AppConfig::load(&path);
        assert_eq!(loaded.hotwords, vec!["魔搭社区", "Claude"]);
        assert_eq!(loaded.model, "Qwen3-ASR-1.7B");
    }

    #[test]
    fn test_load_missing_file_uses_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let cfg = AppConfig::load(&path);
        assert_eq!(cfg.hotkey, "Ctrl+Win");
        assert!(cfg.hotwords.is_empty());
    }

    #[test]
    fn test_load_invalid_json_uses_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        fs::write(&path, "not json at all").unwrap();
        let cfg = AppConfig::load(&path);
        assert_eq!(cfg.hotkey, "Ctrl+Win");
    }

    #[test]
    fn test_parse_hotkey_valid() {
        let cfg = AppConfig { hotkey: "Ctrl+Win".to_string(), ..Default::default() };
        let (mods, key) = cfg.parse_hotkey().unwrap();
        assert_eq!(mods, vec!["Ctrl"]);
        assert_eq!(key, "Win");
    }

    #[test]
    fn test_parse_hotkey_multi_modifier() {
        let cfg = AppConfig { hotkey: "Ctrl+Alt+Space".to_string(), ..Default::default() };
        let (mods, key) = cfg.parse_hotkey().unwrap();
        assert_eq!(mods, vec!["Ctrl", "Alt"]);
        assert_eq!(key, "Space");
    }

    #[test]
    fn test_parse_hotkey_single_key() {
        let cfg = AppConfig { hotkey: "F6".to_string(), ..Default::default() };
        let (mods, key) = cfg.parse_hotkey().unwrap();
        assert!(mods.is_empty());
        assert_eq!(key, "F6");
    }

    #[test]
    fn test_parse_hotkey_single_key_rejected() {
        // Empty hotkey should be rejected
        let cfg = AppConfig { hotkey: "".to_string(), ..Default::default() };
        assert!(cfg.parse_hotkey().is_err());
    }

    #[test]
    fn test_parse_hotkey_empty_rejected() {
        let cfg = AppConfig { hotkey: "".to_string(), ..Default::default() };
        assert!(cfg.parse_hotkey().is_err());
    }

    #[test]
    fn test_parse_hotkey_unknown_modifier() {
        let cfg = AppConfig { hotkey: "Foo+A".to_string(), ..Default::default() };
        assert!(cfg.parse_hotkey().is_err());
    }

    #[test]
    fn test_context_string_empty() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.context_string(), "");
    }

    #[test]
    fn test_context_string_multiple() {
        let cfg = AppConfig { hotwords: vec!["a".into(), "b".into(), "c".into()], ..Default::default() };
        assert_eq!(cfg.context_string(), "a,b,c");
    }

    #[test]
    fn test_context_string_single() {
        let cfg = AppConfig { hotwords: vec!["魔搭".into()], ..Default::default() };
        assert_eq!(cfg.context_string(), "魔搭");
    }

    #[test]
    fn test_model_path() {
        let cfg = AppConfig { model: "Qwen3-ASR-1.7B".into(), ..Default::default() };
        assert_eq!(cfg.model_path(), "models/Qwen3-ASR-1.7B");
    }

    #[test]
    fn test_migrate_port_to_backend_url() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"hotkey":"Ctrl+Win","enabled":true,"model":"Qwen3-ASR-0.6B","hotwords":[],"port":9999}"#).unwrap();
        let cfg = AppConfig::load(&path);
        assert_eq!(cfg.backend_url, "http://localhost:9999");
    }

    #[test]
    fn test_no_migration_when_backend_url_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"hotkey":"Ctrl+Win","backend_url":"http://example.com:8080","port":9999}"#).unwrap();
        let cfg = AppConfig::load(&path);
        assert_eq!(cfg.backend_url, "http://example.com:8080");
    }

    #[test]
    fn test_processing_mode_direct() {
        let mode = ProcessingMode::direct();
        assert_eq!(mode.id, "direct");
        assert_eq!(mode.apply_template("hello"), "hello");
    }

    #[test]
    fn test_processing_mode_polish() {
        let mode = ProcessingMode::polish();
        assert_eq!(mode.id, "polish");
        assert!(mode.system_prompt.contains("润色"));
    }

    #[test]
    fn test_processing_mode_template() {
        let mode = ProcessingMode {
            id: "test".to_string(),
            name: "Test".to_string(),
            icon: "🧪".to_string(),
            system_prompt: String::new(),
            user_template: "请处理：{asr_text}，时间：{timestamp}".to_string(),
            show_in_popup: false,
            popup_order: 0,
        };
        let result = mode.apply_template("你好");
        assert!(result.contains("你好"));
        assert!(result.contains("请处理"));
    }

    #[test]
    fn test_default_config_has_llm() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.llm.model, "gpt-4o-mini");
        assert_eq!(cfg.default_processing_mode, "direct");
        assert!(!cfg.processing_modes.is_empty());
    }

    #[test]
    fn test_config_load_save_with_llm() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        let mut cfg = AppConfig::default();
        cfg.llm.api_key = "test-key".to_string();
        cfg.llm.model = "gpt-4".to_string();
        cfg.save(&path);
        let loaded = AppConfig::load(&path);
        assert_eq!(loaded.llm.api_key, "test-key");
        assert_eq!(loaded.llm.model, "gpt-4");
    }
}
