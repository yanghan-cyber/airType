use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

fn llm_log(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("airtype_debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let _ = writeln!(f, "[{}] [llm] {}", ts, msg);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_params: Option<String>,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "gpt-4o-mini".to_string(),
            temperature: 0.7,
            max_tokens: 2048,
            extra_params: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug)]
pub enum LlmError {
    ConnectionFailed(String),
    AuthenticationFailed,
    ModelNotFound(String),
    RequestTimeout,
    ApiError(u16, String),
    InvalidResponse(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::ConnectionFailed(e) => write!(f, "无法连接 LLM 服务: {}", e),
            LlmError::AuthenticationFailed => write!(f, "API 认证失败"),
            LlmError::ModelNotFound(m) => write!(f, "模型未找到: {}", m),
            LlmError::RequestTimeout => write!(f, "请求超时"),
            LlmError::ApiError(code, msg) => write!(f, "API 错误 {}: {}", code, msg),
            LlmError::InvalidResponse(msg) => write!(f, "无效响应: {}", msg),
        }
    }
}

pub struct LlmClient {
    pub config: LlmConfig,
    client: Client,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");
        Self { config, client }
    }

    pub fn chat_completion(
        &self,
        system_prompt: &str,
        user_content: &str,
    ) -> Result<LlmResponse, LlmError> {
        let url = format!("{}/chat/completions", self.config.base_url);

        let mut messages = Vec::new();
        if !system_prompt.is_empty() {
            messages.push(serde_json::json!({"role": "system", "content": system_prompt}));
        }
        messages.push(serde_json::json!({"role": "user", "content": user_content}));

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages,
            "temperature": self.config.temperature,
            "max_tokens": self.config.max_tokens,
        });
        if let Some(ref extra) = self.config.extra_params {
            if let Ok(extra_val) = serde_json::from_str::<serde_json::Value>(extra) {
                if let serde_json::Value::Object(map) = extra_val {
                    for (k, v) in map {
                        body.as_object_mut().unwrap().insert(k, v);
                    }
                }
            }
        }
        llm_log(&format!("Request body: {}", body.to_string().chars().take(300).collect::<String>()));

        let resp = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| {
                let detail = format!("{:?}", e);
                llm_log(&format!("Request failed: {}", detail));
                if e.is_timeout() { LlmError::RequestTimeout }
                else { LlmError::ConnectionFailed(detail) }
            })?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().map_err(|e| LlmError::InvalidResponse(e.to_string()))?;
        llm_log(&format!("Response body: {}", body.to_string().chars().take(500).collect::<String>()));

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LlmError::AuthenticationFailed);
        }

        if !status.is_success() {
            let error_msg = body["error"]["message"].as_str().unwrap_or("Unknown error");
            return Err(LlmError::ApiError(status.as_u16(), error_msg.to_string()));
        }

        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .ok_or_else(|| LlmError::InvalidResponse("Missing content".into()))?
            .to_string();

        let usage = body["usage"].as_object().map(|u| Usage {
            prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
        });

        Ok(LlmResponse { content, usage })
    }

    pub fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let url = format!("{}/models", self.config.base_url);

        let resp = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .timeout(Duration::from_secs(10))
            .send()
            .map_err(|e| LlmError::ConnectionFailed(e.to_string()))?;

        let status = resp.status();
        let body: serde_json::Value = resp.json()
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(LlmError::AuthenticationFailed);
        }

        if !status.is_success() {
            let error_msg = body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error");
            return Err(LlmError::ApiError(status.as_u16(), error_msg.to_string()));
        }

        let models = body["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    pub fn health(&self) -> Result<bool, LlmError> {
        let url = format!("{}/models", self.config.base_url);

        let resp = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .timeout(Duration::from_secs(5))
            .send()
            .map_err(|e| LlmError::ConnectionFailed(e.to_string()))?;

        Ok(resp.status().is_success())
    }
}
