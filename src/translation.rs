use std::{
    collections::HashMap,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, PartialEq, Eq, Hash)]
struct TranslationKey {
    text: String,
    endpoint: String,
    api_key: String,
    model: String,
    target_language: String,
    system_prompt: String,
}

impl TranslationKey {
    fn new(text: String, settings: &TranslationSettings) -> Self {
        Self {
            text,
            endpoint: settings.endpoint.clone(),
            api_key: settings.api_key.trim().to_owned(),
            model: settings.model.clone(),
            target_language: settings.target_language.clone(),
            system_prompt: settings
                .system_prompt
                .replace("{target}", &settings.target_language),
        }
    }
}

pub struct Translator {
    requests: Sender<Request>,
    results: Receiver<TranslationResult>,
    cache: HashMap<TranslationKey, String>,
    in_flight: HashMap<TranslationKey, Vec<u64>>,
    pending: HashMap<u64, TranslationKey>,
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
            in_flight: HashMap::new(),
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn submit(&mut self, text: String, settings: &TranslationSettings) -> Submission {
        let key = TranslationKey::new(text, settings);
        if let Some(translation) = self.cache.get(&key) {
            return Submission::Cached(translation.clone());
        }
        let id = self.next_id;
        self.next_id += 1;
        // Each turn keeps its own ID even when it shares another turn's request.
        if let Some(subscribers) = self.in_flight.get_mut(&key) {
            subscribers.push(id);
            return Submission::Queued(id);
        }
        let request = Request {
            id,
            text: key.text.clone(),
            settings: settings.clone(),
        };
        self.in_flight.insert(key.clone(), vec![id]);
        self.pending.insert(id, key);
        let _ = self.requests.send(request);
        Submission::Queued(id)
    }

    pub fn poll(&mut self) -> Vec<TranslationResult> {
        let mut ready = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            let Some(key) = self.pending.remove(&result.id) else {
                continue;
            };
            let subscribers = self.in_flight.remove(&key).unwrap_or_default();
            if let Ok(translation) = &result.result {
                self.cache.insert(key, translation.clone());
            }
            for id in subscribers {
                ready.push(TranslationResult {
                    id,
                    source: result.source.clone(),
                    result: result.result.clone(),
                });
            }
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

    fn translator_channels() -> (Translator, Receiver<Request>, Sender<TranslationResult>) {
        let (request_tx, request_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let translator = Translator {
            requests: request_tx,
            results: result_rx,
            cache: HashMap::new(),
            in_flight: HashMap::new(),
            pending: HashMap::new(),
            next_id: 1,
        };
        (translator, request_rx, result_tx)
    }

    fn queued_id(submission: Submission) -> u64 {
        match submission {
            Submission::Queued(id) => id,
            Submission::Cached(_) => panic!("expected a queued translation"),
        }
    }

    #[test]
    fn duplicate_pending_turns_share_a_request_and_each_receive_the_translation() {
        let (mut translator, requests, results) = translator_channels();
        let settings = TranslationSettings::default();
        let first_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        let second_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        assert_ne!(first_id, second_id);
        let request = requests.try_recv().unwrap();
        assert!(
            requests.try_recv().is_err(),
            "one HTTP request for both turns"
        );
        results
            .send(TranslationResult {
                id: request.id,
                source: request.text,
                result: Ok("你好".to_owned()),
            })
            .unwrap();

        let ready = translator.poll();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].id, first_id);
        assert_eq!(ready[1].id, second_id);
        for result in ready {
            assert_eq!(result.source, "Hello");
            assert_eq!(result.result.as_deref(), Ok("你好"));
        }
        assert!(translator.poll().is_empty());
        assert!(matches!(
            translator.submit("Hello".to_owned(), &settings),
            Submission::Cached(text) if text == "你好"
        ));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn equivalent_request_configuration_shares_pending_work() {
        let (mut translator, requests, results) = translator_channels();
        let settings = TranslationSettings {
            enabled: true,
            api_key: " account-key ".to_owned(),
            system_prompt: "Translate to {target}".to_owned(),
            ..TranslationSettings::default()
        };
        let first_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        let equivalent = TranslationSettings {
            enabled: false,
            api_key: "account-key".to_owned(),
            system_prompt: "Translate to zh-CN".to_owned(),
            ..settings
        };
        let second_id = queued_id(translator.submit("Hello".to_owned(), &equivalent));
        assert_ne!(first_id, second_id);
        let request = requests.try_recv().unwrap();
        assert!(requests.try_recv().is_err());
        results
            .send(TranslationResult {
                id: request.id,
                source: request.text,
                result: Ok("你好".to_owned()),
            })
            .unwrap();
        let ready = translator.poll();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].id, first_id);
        assert_eq!(ready[1].id, second_id);
        assert!(matches!(
            translator.submit("Hello".to_owned(), &equivalent),
            Submission::Cached(text) if text == "你好"
        ));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn translation_cache_separates_request_settings_but_ignores_enable_switch() {
        let (mut translator, requests, results) = translator_channels();
        let settings = TranslationSettings::default();
        let base_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        let request = requests.try_recv().unwrap();
        results
            .send(TranslationResult {
                id: request.id,
                source: request.text,
                result: Ok("你好".to_owned()),
            })
            .unwrap();
        assert_eq!(translator.poll()[0].id, base_id);

        let toggled = TranslationSettings {
            enabled: !settings.enabled,
            ..settings.clone()
        };
        assert!(matches!(
            translator.submit("Hello".to_owned(), &toggled),
            Submission::Cached(text) if text == "你好"
        ));
        assert!(requests.try_recv().is_err());

        let variants = [
            TranslationSettings {
                endpoint: "http://localhost:9090/v1/chat/completions".to_owned(),
                ..settings.clone()
            },
            TranslationSettings {
                api_key: "other-account".to_owned(),
                ..settings.clone()
            },
            TranslationSettings {
                model: "another-model".to_owned(),
                ..settings.clone()
            },
            TranslationSettings {
                target_language: "fr".to_owned(),
                ..settings.clone()
            },
            TranslationSettings {
                system_prompt: "Another prompt for {target}".to_owned(),
                ..settings.clone()
            },
        ];
        let mut submitted = Vec::new();
        for (index, variant) in variants.iter().enumerate() {
            let id = queued_id(translator.submit("Hello".to_owned(), variant));
            let request = requests.try_recv().unwrap();
            assert_eq!(request.id, id);
            submitted.push(id);
            results
                .send(TranslationResult {
                    id: request.id,
                    source: request.text,
                    result: Ok(format!("translation {index}")),
                })
                .unwrap();
        }
        let ready = translator.poll();
        assert_eq!(
            ready.iter().map(|result| result.id).collect::<Vec<_>>(),
            submitted
        );
        for (index, variant) in variants.iter().enumerate() {
            assert!(matches!(
                translator.submit("Hello".to_owned(), variant),
                Submission::Cached(text) if text == format!("translation {index}")
            ));
        }
        assert!(matches!(
            translator.submit("Hello".to_owned(), &settings),
            Submission::Cached(text) if text == "你好"
        ));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn duplicate_failure_reaches_all_turns_and_can_be_retried() {
        let (mut translator, requests, results) = translator_channels();
        let settings = TranslationSettings::default();
        let first_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        let second_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        let request = requests.try_recv().unwrap();
        assert!(requests.try_recv().is_err());
        results
            .send(TranslationResult {
                id: request.id,
                source: request.text,
                result: Err("server unavailable".to_owned()),
            })
            .unwrap();
        let ready = translator.poll();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].id, first_id);
        assert_eq!(ready[1].id, second_id);
        assert!(
            ready
                .iter()
                .all(|result| result.result == Err("server unavailable".to_owned()))
        );

        let retry_id = queued_id(translator.submit("Hello".to_owned(), &settings));
        assert_ne!(retry_id, first_id);
        assert_ne!(retry_id, second_id);
        let retry = requests.try_recv().unwrap();
        assert_eq!(retry.id, retry_id);
        results
            .send(TranslationResult {
                id: retry.id,
                source: retry.text,
                result: Ok("你好".to_owned()),
            })
            .unwrap();
        let ready = translator.poll();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, retry_id);
        assert_eq!(ready[0].result.as_deref(), Ok("你好"));
        assert!(matches!(
            translator.submit("Hello".to_owned(), &settings),
            Submission::Cached(text) if text == "你好"
        ));
        assert!(requests.try_recv().is_err());
    }

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
