use reqwest::blocking::Client;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TranscribeResult {
    pub text: String,
}

#[derive(Debug)]
pub enum AsrError {
    ConnectionFailed(String),
    ModelNotLoaded,
    InvalidAudio(String),
    Timeout,
    HttpError(u16, String),
}

impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrError::ConnectionFailed(e) => write!(f, "无法连接后端: {}", e),
            AsrError::ModelNotLoaded => write!(f, "模型未加载"),
            AsrError::InvalidAudio(e) => write!(f, "无效音频: {}", e),
            AsrError::Timeout => write!(f, "请求超时"),
            AsrError::HttpError(code, msg) => write!(f, "HTTP {}: {}", code, msg),
        }
    }
}

fn pcm_to_wav(pcm: &[u8], sample_rate: u32, channels: u16, bits_per_sample: u16) -> Vec<u8> {
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = pcm.len() as u32;
    let file_size = 36 + data_size;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());      // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes());        // PCM format
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

pub struct AsrClient {
    base_url: String,
    api_key: String,
    client: Client,
}

impl AsrClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .no_proxy()
            .build()
            .expect("Failed to create HTTP client");
        Self {
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            client,
        }
    }

    fn authed_request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        let mut req = self.client.request(method, url);
        if !self.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.api_key));
        }
        req
    }

    pub fn health(&self) -> Result<(), AsrError> {
        let resp = self.authed_request(reqwest::Method::GET, &format!("{}/models", self.base_url))
            .timeout(Duration::from_secs(5))
            .send()
            .map_err(|e| {
                if e.is_timeout() { AsrError::Timeout }
                else { AsrError::ConnectionFailed(e.to_string()) }
            })?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(AsrError::ConnectionFailed(format!("HTTP {}", resp.status())))
        }
    }

    pub fn list_models(&self) -> Result<Vec<String>, AsrError> {
        let resp = self.authed_request(reqwest::Method::GET, &format!("{}/models", self.base_url))
            .timeout(Duration::from_secs(10))
            .send()
            .map_err(|e| AsrError::ConnectionFailed(e.to_string()))?;
        let body: serde_json::Value = resp.json()
            .map_err(|e| AsrError::ConnectionFailed(e.to_string()))?;
        let models = body["data"].as_array()
            .map(|arr| {
                let mut ids: Vec<String> = arr.iter()
                    .filter_map(|m| m["id"].as_str().map(String::from))
                    .collect();
                ids.sort();
                ids
            })
            .unwrap_or_default();
        Ok(models)
    }

    pub fn transcribe(
        &self,
        pcm_bytes: &[u8],
        sample_rate: u32,
        model: &str,
        language: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<TranscribeResult, AsrError> {
        let url = format!("{}/audio/transcriptions", self.base_url);
        let wav_bytes = pcm_to_wav(pcm_bytes, sample_rate, 1, 16);

        let file_part = reqwest::blocking::multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav").unwrap();

        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", file_part)
            .text("model", model.to_string())
            .text("response_format", "json".to_string());

        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }
        if let Some(ctx) = prompt {
            if !ctx.is_empty() {
                form = form.text("prompt", ctx.to_string());
            }
        }

        let resp = self.authed_request(reqwest::Method::POST, &url)
            .multipart(form)
            .send()
            .map_err(|e| {
                if e.is_timeout() { AsrError::Timeout }
                else { AsrError::ConnectionFailed(e.to_string()) }
            })?;

        let status = resp.status();
        let body: serde_json::Value = resp.json()
            .map_err(|e| AsrError::ConnectionFailed(e.to_string()))?;

        if status == reqwest::StatusCode::OK {
            Ok(TranscribeResult {
                text: body["text"].as_str().unwrap_or("").to_string(),
            })
        } else if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            Err(AsrError::ModelNotLoaded)
        } else if status == reqwest::StatusCode::BAD_REQUEST {
            Err(AsrError::InvalidAudio(
                body["error"].as_str().unwrap_or("Unknown error").to_string()
            ))
        } else {
            Err(AsrError::HttpError(
                status.as_u16(),
                body["error"].as_str().unwrap_or("Unknown error").to_string(),
            ))
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asr_error_display() {
        assert!(AsrError::ConnectionFailed("refused".into()).to_string().contains("refused"));
        assert!(AsrError::ModelNotLoaded.to_string().contains("模型"));
        assert!(AsrError::InvalidAudio("too short".into()).to_string().contains("too short"));
        assert!(AsrError::Timeout.to_string().contains("超时"));
        assert!(AsrError::HttpError(500, "oops".into()).to_string().contains("500"));
    }

    #[test]
    fn test_client_new() {
        let client = AsrClient::new("https://api.openai.com/v1", "");
        assert_eq!(client.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn test_client_custom_url() {
        let client = AsrClient::new("http://192.168.1.100:9999", "test-key");
        assert_eq!(client.base_url, "http://192.168.1.100:9999");
        assert_eq!(client.api_key, "test-key");
    }

    #[test]
    fn test_transcribe_result_fields() {
        let result = TranscribeResult {
            text: "你好世界".to_string(),
        };
        assert_eq!(result.text, "你好世界");
    }

    #[test]
    fn test_pcm_to_wav_header() {
        let pcm = vec![0u8; 100];
        let wav = pcm_to_wav(&pcm, 16000, 1, 16);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 100);
    }

    #[test]
    fn test_pcm_to_wav_sample_rate() {
        let pcm = vec![0u8; 100];
        let wav = pcm_to_wav(&pcm, 44100, 2, 16);
        let sr = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        assert_eq!(sr, 44100);
        let ch = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        assert_eq!(ch, 2);
    }
}
