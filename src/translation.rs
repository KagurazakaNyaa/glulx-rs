use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslationSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub target_language: String,
    pub system_prompt: String,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_owned(),
            api_key: String::new(),
            model: "tencent/Hy-MT2-1.8B".to_owned(),
            target_language: "zh-CN".to_owned(),
            system_prompt: "Translate interactive-fiction prose into {target}. Preserve paragraphs, names, punctuation, and game commands. Return only the translation.".to_owned(),
        }
    }
}

#[derive(Debug)]
struct Request {
    id: u64,
    text: String,
    settings: TranslationSettings,
}

#[derive(Debug)]
pub struct TranslationResult {
    pub id: u64,
    pub source: String,
    pub result: Result<String, String>,
}

pub enum Submission {
    Cached(String),
    Queued(u64),
}

pub struct Translator {
    requests: Sender<Request>,
    results: Receiver<TranslationResult>,
    cache: HashMap<String, String>,
    next_id: u64,
}

impl Translator {
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel::<Request>();
        let (result_tx, result_rx) = mpsc::channel();
        thread::Builder::new()
            .name("glulx-translation".to_owned())
            .spawn(move || translation_worker(request_rx, result_tx))
            .expect("translation worker thread");
        Self {
            requests: request_tx,
            results: result_rx,
            cache: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn submit(&mut self, text: String, settings: &TranslationSettings) -> Submission {
        if let Some(translation) = self.cache.get(&text) {
            return Submission::Cached(translation.clone());
        }
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            text,
            settings: settings.clone(),
        };
        let _ = self.requests.send(request);
        Submission::Queued(id)
    }

    pub fn poll(&mut self) -> Vec<TranslationResult> {
        let mut ready = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            if let Ok(translation) = &result.result {
                self.cache
                    .insert(result.source.clone(), translation.clone());
            }
            ready.push(result);
        }
        ready
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

fn translation_worker(requests: Receiver<Request>, results: Sender<TranslationResult>) {
    let client = reqwest::blocking::Client::builder().build();
    while let Ok(request) = requests.recv() {
        let result = match &client {
            Ok(client) => translate(client, &request),
            Err(error) => Err(format!("cannot create HTTP client: {error}")),
        };
        let _ = results.send(TranslationResult {
            id: request.id,
            source: request.text,
            result,
        });
    }
}

fn translate(client: &reqwest::blocking::Client, request: &Request) -> Result<String, String> {
    let body = request_body(request);
    let mut builder = client.post(&request.settings.endpoint).json(&body);
    if !request.settings.api_key.trim().is_empty() {
        builder = builder.bearer_auth(request.settings.api_key.trim());
    }
    let response = builder
        .send()
        .map_err(|error| format!("translation request failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("translation server rejected the request: {error}"))?;
    let value: serde_json::Value = response
        .json()
        .map_err(|error| format!("invalid translation response: {error}"))?;
    parse_response(&value)
}

fn request_body(request: &Request) -> serde_json::Value {
    let prompt = request
        .settings
        .system_prompt
        .replace("{target}", &request.settings.target_language);
    json!({
        "model": request.settings.model,
        "temperature": 0.2,
        "messages": [
            { "role": "system", "content": prompt },
            { "role": "user", "content": request.text }
        ]
    })
}

fn parse_response(value: &serde_json::Value) -> Result<String, String> {
    value
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "translation response has no choices[0].message.content".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prompt_has_target_placeholder() {
        assert!(
            TranslationSettings::default()
                .system_prompt
                .contains("{target}")
        );
    }

    #[test]
    fn builds_openai_compatible_turn_request_and_parses_response() {
        let settings = TranslationSettings {
            target_language: "zh-CN".to_owned(),
            system_prompt: "Translate to {target}".to_owned(),
            ..TranslationSettings::default()
        };
        let request = Request {
            id: 1,
            text: "Hello".to_owned(),
            settings,
        };

        let body = request_body(&request);
        assert_eq!(body["messages"][0]["content"], "Translate to zh-CN");
        assert_eq!(body["messages"][1]["content"], "Hello");
        let response = json!({"choices": [{"message": {"content": "\u{4f60}\u{597d}"}}]});
        assert_eq!(parse_response(&response).unwrap(), "\u{4f60}\u{597d}");
    }
}
