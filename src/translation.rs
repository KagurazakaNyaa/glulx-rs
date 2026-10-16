use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslationSettings {
    pub enabled: bool,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub target_language: String,
    pub system_prompt: String,
    pub use_system_prompt: bool,
    pub user_prompt: String,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub repetition_penalty: Option<f64>,
    pub max_tokens: Option<u32>,
}

impl Default for TranslationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_owned(),
            api_key: String::new(),
            model: "tencent/Hy-MT2-1.8B".to_owned(),
            target_language: "zh-CN".to_owned(),
            use_system_prompt: true,
            user_prompt: "{text}".to_owned(),
            temperature: Some(0.2),
            top_p: None,
            top_k: None,
            repetition_penalty: None,
            max_tokens: None,
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
    Rejected(String),
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TranslationKey {
    text: String,
    endpoint: String,
    api_key: String,
    body: String,
}

impl TranslationKey {
    fn new(text: String, settings: &TranslationSettings) -> Self {
        Self {
            body: build_request_body(settings, &text).to_string(),
            text,
            endpoint: settings.endpoint.clone(),
            api_key: settings.api_key.trim().to_owned(),
        }
    }
}

const REQUEST_QUEUE_CAPACITY: usize = 32;
const MAX_PENDING_TURNS: usize = 128;
const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 256;

impl TranslationKey {
    fn byte_len(&self) -> usize {
        self.text.len() + self.endpoint.len() + self.api_key.len() + self.body.len()
    }

    fn pending_bytes(&self) -> usize {
        // Two retained keys plus the worker's source/configuration/request body.
        self.byte_len().saturating_mul(3)
    }
}

#[derive(Default)]
struct TranslationCache {
    entries: HashMap<TranslationKey, (String, u64)>,
    bytes: usize,
    clock: u64,
}

impl TranslationCache {
    fn get(&mut self, key: &TranslationKey) -> Option<String> {
        let (value, used) = self.entries.get_mut(key)?;
        self.clock = self.clock.wrapping_add(1);
        *used = self.clock;
        Some(value.clone())
    }

    fn insert(&mut self, key: TranslationKey, value: String) {
        let bytes = key.byte_len().saturating_add(value.len());
        if bytes > MAX_CACHE_BYTES {
            return;
        }
        if let Some((old, _)) = self.entries.remove(&key) {
            self.bytes -= key.byte_len() + old.len();
        }
        while self.entries.len() >= MAX_CACHE_ENTRIES || self.bytes + bytes > MAX_CACHE_BYTES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            if let Some((old, _)) = self.entries.remove(&oldest) {
                self.bytes -= oldest.byte_len() + old.len();
            }
        }
        self.clock = self.clock.wrapping_add(1);
        self.entries.insert(key, (value, self.clock));
        self.bytes += bytes;
    }
}

struct QueuedRequest {
    request: Request,
    generation: u64,
}

#[derive(Default)]
struct QueueState {
    requests: VecDeque<QueuedRequest>,
    closed: bool,
}

#[derive(Default)]
struct RequestQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    generation: AtomicU64,
}

impl RequestQueue {
    fn push(&self, request: Request) -> Result<(), String> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.closed {
            return Err("translation worker is unavailable".into());
        }
        if state.requests.len() >= REQUEST_QUEUE_CAPACITY {
            return Err("translation queue is full; wait for pending translations".into());
        }
        state.requests.push_back(QueuedRequest {
            request,
            generation: self.generation.load(Ordering::Acquire),
        });
        self.changed.notify_one();
        Ok(())
    }

    fn receive(&self) -> Option<QueuedRequest> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        loop {
            if state.closed {
                return None;
            }
            if let Some(request) = state.requests.pop_front() {
                return Some(request);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    fn reset(&self, close: bool) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        self.generation.fetch_add(1, Ordering::AcqRel);
        state.requests.clear();
        state.closed |= close;
        self.changed.notify_all();
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::Acquire) == generation
    }
}

pub struct Translator {
    requests: Arc<RequestQueue>,
    results: Receiver<TranslationResult>,
    cache: TranslationCache,
    in_flight: HashMap<TranslationKey, Vec<u64>>,
    pending: HashMap<u64, TranslationKey>,
    pending_turns: usize,
    pending_bytes: usize,
    next_id: u64,
}

impl Translator {
    pub fn new() -> Self {
        let requests = Arc::new(RequestQueue::default());
        let (result_tx, result_rx) = mpsc::sync_channel(4);
        let queue = Arc::clone(&requests);
        if thread::Builder::new()
            .name("glulx-translation".to_owned())
            .spawn(move || translation_worker(queue, result_tx))
            .is_err()
        {
            requests.reset(true);
        }
        Self::with_queue(requests, result_rx)
    }

    fn with_queue(requests: Arc<RequestQueue>, results: Receiver<TranslationResult>) -> Self {
        Self {
            requests,
            results,
            cache: TranslationCache::default(),
            in_flight: HashMap::new(),
            pending: HashMap::new(),
            pending_turns: 0,
            pending_bytes: 0,
            next_id: 1,
        }
    }

    /// Discard a previous story's queued work and cache. An HTTP call already
    /// sent may finish within its timeout; its result is ignored.
    pub fn reset(&mut self) {
        self.requests.reset(false);
        self.cache = TranslationCache::default();
        self.in_flight.clear();
        self.pending.clear();
        self.pending_turns = 0;
        self.pending_bytes = 0;
        while self.results.try_recv().is_ok() {}
    }

    pub fn submit(&mut self, text: String, settings: &TranslationSettings) -> Submission {
        if text.len() > MAX_REQUEST_BYTES {
            return Submission::Rejected("translation source exceeds the 1 MiB limit".into());
        }
        let key = TranslationKey::new(text, settings);
        if let Some(translation) = self.cache.get(&key) {
            return Submission::Cached(translation);
        }
        if self.pending_turns >= MAX_PENDING_TURNS {
            return Submission::Rejected("too many pending translation turns".into());
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        // Each turn keeps its own ID even when it shares another turn's request.
        if let Some(subscribers) = self.in_flight.get_mut(&key) {
            subscribers.push(id);
            self.pending_turns += 1;
            return Submission::Queued(id);
        }
        let bytes = key.pending_bytes();
        if self.pending_bytes.saturating_add(bytes) > MAX_PENDING_BYTES {
            return Submission::Rejected("pending translations exceed the 8 MiB budget".into());
        }
        let request = Request {
            id,
            text: key.text.clone(),
            settings: settings.clone(),
        };
        if let Err(error) = self.requests.push(request) {
            return Submission::Rejected(error);
        }
        self.pending_turns += 1;
        self.pending_bytes += bytes;
        self.in_flight.insert(key.clone(), vec![id]);
        self.pending.insert(id, key);
        Submission::Queued(id)
    }

    pub fn poll(&mut self) -> Vec<TranslationResult> {
        let mut ready = Vec::new();
        while let Ok(result) = self.results.try_recv() {
            let Some(key) = self.pending.remove(&result.id) else {
                continue;
            };
            let subscribers = self.in_flight.remove(&key).unwrap_or_default();
            self.pending_turns -= subscribers.len();
            self.pending_bytes -= key.pending_bytes();
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

impl Drop for Translator {
    fn drop(&mut self) {
        self.requests.reset(true);
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

fn translation_worker(requests: Arc<RequestQueue>, results: SyncSender<TranslationResult>) {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build();
    while let Some(queued) = requests.receive() {
        if !requests.is_current(queued.generation) {
            continue;
        }
        let request = queued.request;
        let result = match &client {
            Ok(client) => translate(client, &request),
            Err(error) => Err(format!("cannot create HTTP client: {error}")),
        };
        if requests.is_current(queued.generation)
            && results
                .send(TranslationResult {
                    id: request.id,
                    source: request.text,
                    result,
                })
                .is_err()
        {
            break;
        }
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
    parse_http_response(response)
}

fn parse_http_response(response: impl Read) -> Result<String, String> {
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("invalid translation response: {error}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("translation response exceeds the 4 MiB limit".into());
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid translation response: {error}"))?;
    parse_response(&value)
}

fn request_body(request: &Request) -> serde_json::Value {
    build_request_body(&request.settings, &request.text)
}

fn build_request_body(settings: &TranslationSettings, text: &str) -> serde_json::Value {
    let mut messages = Vec::new();
    if settings.use_system_prompt && !settings.system_prompt.trim().is_empty() {
        messages.push(json!({ "role": "system", "content": settings.system_prompt.replace("{target}", &settings.target_language) }));
    }
    // Expand template fragments before inserting source text, so placeholders in
    // the story itself remain literal. A template without {text} is a prefix.
    let fragments: Vec<_> = settings
        .user_prompt
        .split("{text}")
        .map(|part| part.replace("{target}", &settings.target_language))
        .collect();
    let user = if fragments.len() > 1 {
        fragments.join(text)
    } else if fragments[0].trim().is_empty() {
        text.to_owned()
    } else {
        format!("{}\n\n{text}", fragments[0])
    };
    messages.push(json!({ "role": "user", "content": user }));
    let mut body = json!({ "model": settings.model, "messages": messages });
    for (key, value) in [
        ("temperature", settings.temperature),
        ("top_p", settings.top_p),
        ("repetition_penalty", settings.repetition_penalty),
    ] {
        if let Some(value) = value {
            body[key] = json!(value);
        }
    }
    for (key, value) in [
        ("top_k", settings.top_k),
        ("max_tokens", settings.max_tokens),
    ] {
        if let Some(value) = value {
            body[key] = json!(value);
        }
    }
    body
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

    struct TestRequests(Arc<RequestQueue>);

    impl TestRequests {
        fn try_recv(&self) -> Result<Request, mpsc::TryRecvError> {
            self.0
                .state
                .lock()
                .unwrap()
                .requests
                .pop_front()
                .map(|queued| queued.request)
                .ok_or(mpsc::TryRecvError::Empty)
        }
    }

    fn translator_channels() -> (Translator, TestRequests, mpsc::Sender<TranslationResult>) {
        let requests = Arc::new(RequestQueue::default());
        let (result_tx, result_rx) = mpsc::channel();
        let translator = Translator::with_queue(Arc::clone(&requests), result_rx);
        (translator, TestRequests(requests), result_tx)
    }

    fn queued_id(submission: Submission) -> u64 {
        match submission {
            Submission::Queued(id) => id,
            Submission::Cached(_) | Submission::Rejected(_) => {
                panic!("expected a queued translation")
            }
        }
    }

    #[test]
    fn translation_queue_rejects_excess_work() {
        let (mut translator, _requests, _) = translator_channels();
        let settings = TranslationSettings::default();
        for index in 0..32 {
            queued_id(translator.submit(format!("Turn {index}"), &settings));
        }
        assert!(!matches!(
            translator.submit("overflow".into(), &settings),
            Submission::Queued(_)
        ));
        assert_eq!(translator.pending.len(), REQUEST_QUEUE_CAPACITY);
        assert_eq!(translator.pending_turns, REQUEST_QUEUE_CAPACITY);
    }

    #[test]
    fn reset_removes_old_queued_work_and_ignores_in_flight_results() {
        let (mut translator, requests, results) = translator_channels();
        let settings = TranslationSettings::default();
        let old_id = queued_id(translator.submit("old in flight".into(), &settings));
        let old = requests.0.receive().unwrap();
        queued_id(translator.submit("old queued".into(), &settings));
        translator.reset();
        assert!(!requests.0.is_current(old.generation));
        assert!(requests.try_recv().is_err());
        assert_eq!((translator.pending_turns, translator.pending_bytes), (0, 0));
        let new_id = queued_id(translator.submit("new story".into(), &settings));
        assert_ne!(old_id, new_id);
        let new = requests.0.receive().unwrap();
        assert!(requests.0.is_current(new.generation));
        assert_eq!(new.request.text, "new story");
        for (id, source) in [(old_id, "old in flight"), (new_id, "new story")] {
            results
                .send(TranslationResult {
                    id,
                    source: source.into(),
                    result: Ok(source.into()),
                })
                .unwrap();
        }
        let ready = translator.poll();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, new_id);
        assert_eq!(translator.cache.entries.len(), 1);
        translator.reset();
        assert!(translator.cache.entries.is_empty());
    }

    #[test]
    fn translation_budgets_cover_payloads_and_duplicate_subscribers() {
        let (mut translator, requests, _) = translator_channels();
        let settings = TranslationSettings::default();
        assert!(matches!(
            translator.submit("x".repeat(MAX_REQUEST_BYTES + 1), &settings),
            Submission::Rejected(_)
        ));
        assert!(requests.try_recv().is_err());
        for index in 0..REQUEST_QUEUE_CAPACITY {
            let text = format!("{index}{}", "x".repeat(128 * 1024));
            if matches!(translator.submit(text, &settings), Submission::Rejected(_)) {
                break;
            }
        }
        assert!(translator.pending.len() < REQUEST_QUEUE_CAPACITY);
        assert!(translator.pending_bytes <= MAX_PENDING_BYTES);
        translator.reset();
        for _ in 0..MAX_PENDING_TURNS {
            queued_id(translator.submit("same".into(), &settings));
        }
        assert!(matches!(
            translator.submit("same".into(), &settings),
            Submission::Rejected(_)
        ));
        assert_eq!(requests.0.state.lock().unwrap().requests.len(), 1);
        requests.0.reset(true);
        translator.reset();
        assert!(matches!(
            translator.submit("worker unavailable".into(), &settings),
            Submission::Rejected(_)
        ));
        assert!(translator.pending.is_empty());
    }

    #[test]
    fn translation_cache_evicts_by_usage_and_payload_bytes() {
        let settings = TranslationSettings::default();
        let key = |index| TranslationKey::new(format!("Turn {index}"), &settings);
        let mut cache = TranslationCache::default();
        for index in 0..MAX_CACHE_ENTRIES {
            cache.insert(key(index), "translated".into());
        }
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(MAX_CACHE_ENTRIES), "next".into());
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(1)).is_none());
        for index in 0..3 {
            cache.insert(key(index), "x".repeat(MAX_CACHE_BYTES / 2));
            assert!(cache.bytes <= MAX_CACHE_BYTES);
        }
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.get(&key(2)).is_some());
    }

    #[test]
    fn oversized_http_responses_fail_before_json_parsing() {
        let response = std::io::repeat(b' ').take(MAX_RESPONSE_BYTES as u64 + 1);
        assert_eq!(
            parse_http_response(response).unwrap_err(),
            "translation response exceeds the 4 MiB limit"
        );
        let response =
            std::io::Cursor::new(br#"{"choices":[{"message":{"content":"translated"}}]}"#);
        assert_eq!(parse_http_response(response).unwrap(), "translated");
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
                use_system_prompt: false,
                ..settings.clone()
            },
            TranslationSettings {
                user_prompt: "Translate: {text}".into(),
                ..settings.clone()
            },
            TranslationSettings {
                temperature: None,
                ..settings.clone()
            },
            TranslationSettings {
                temperature: Some(0.7),
                ..settings.clone()
            },
            TranslationSettings {
                top_p: Some(0.6),
                ..settings.clone()
            },
            TranslationSettings {
                top_k: Some(20),
                ..settings.clone()
            },
            TranslationSettings {
                repetition_penalty: Some(1.05),
                ..settings.clone()
            },
            TranslationSettings {
                max_tokens: Some(4096),
                ..settings.clone()
            },
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
    fn user_only_translation_and_sampling_reach_the_request() {
        let settings: TranslationSettings = serde_json::from_value(json!({
            "use_system_prompt": false,
            "user_prompt": "Translate to {target}: {text}",
            "target_language": "中文",
            "temperature": 0.7, "top_p": 0.6, "top_k": 20,
            "repetition_penalty": 1.05, "max_tokens": 4096
        }))
        .unwrap();
        let body = request_body(&Request {
            id: 1,
            text: "Hello".into(),
            settings,
        });
        assert_eq!(
            body["messages"],
            json!([
                {"role": "user", "content": "Translate to 中文: Hello"}
            ])
        );
        for (key, expected) in [
            ("temperature", json!(0.7)),
            ("top_p", json!(0.6)),
            ("top_k", json!(20)),
            ("repetition_penalty", json!(1.05)),
            ("max_tokens", json!(4096)),
        ] {
            assert_eq!(body[key], expected, "{key}");
        }
    }

    #[test]
    fn omitted_parameters_and_empty_system_are_not_sent() {
        let settings: TranslationSettings = serde_json::from_value(json!({
            "system_prompt": "  ", "temperature": null
        }))
        .unwrap();
        let body = build_request_body(&settings, "Hello");
        assert_eq!(
            body,
            json!({"model": settings.model, "messages": [
                {"role": "user", "content": "Hello"}
            ]})
        );
    }

    #[test]
    fn user_prefix_and_literal_source_placeholders_are_preserved() {
        let mut settings = TranslationSettings {
            use_system_prompt: false,
            user_prompt: "Into {target}:".into(),
            ..TranslationSettings::default()
        };
        assert_eq!(
            build_request_body(&settings, "{target} {text}")["messages"][0]["content"],
            "Into zh-CN:\n\n{target} {text}"
        );
        settings.user_prompt = "Into {target}: {text}".into();
        assert_eq!(
            build_request_body(&settings, "{target} {text}")["messages"][0]["content"],
            "Into zh-CN: {target} {text}"
        );
        settings.user_prompt.clear();
        assert_eq!(
            build_request_body(&settings, "Hello")["messages"][0]["content"],
            "Hello"
        );
    }

    #[test]
    fn legacy_settings_round_trip() {
        let settings: TranslationSettings = serde_json::from_value(json!({
            "model": "local-alias", "system_prompt": "Custom {target}",
            "endpoint": "http://localhost:8000/v1/chat/completions", "api_key": "test-key"
        }))
        .unwrap();
        assert!(settings.use_system_prompt);
        assert_eq!(settings.user_prompt, "{text}");
        assert_eq!(settings.temperature, Some(0.2));
        let restored: TranslationSettings =
            serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(settings, restored);
        let body = build_request_body(&restored, "Hello");
        assert_eq!(
            body["messages"],
            json!([
                {"role": "system", "content": "Custom zh-CN"},
                {"role": "user", "content": "Hello"}
            ])
        );
        assert_eq!(body["temperature"], 0.2);
    }

    #[test]
    fn disabled_system_prompt_edits_do_not_split_pending_requests() {
        let (mut translator, requests, _) = translator_channels();
        let mut settings = TranslationSettings {
            use_system_prompt: false,
            ..TranslationSettings::default()
        };
        queued_id(translator.submit("Hello".into(), &settings));
        settings.system_prompt = "Ignored".into();
        queued_id(translator.submit("Hello".into(), &settings));
        assert!(requests.try_recv().is_ok());
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
