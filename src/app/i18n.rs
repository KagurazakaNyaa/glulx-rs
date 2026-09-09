//! UI language is independent of story text and machine translation settings.
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanguagePreference {
    #[default]
    #[serde(rename = "auto")]
    System,
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh")]
    Chinese,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

impl LanguagePreference {
    pub fn resolve(self) -> Language {
        static SYSTEM: OnceLock<Language> = OnceLock::new();
        let system = if self == Self::System {
            *SYSTEM.get_or_init(|| Language::from_locale(sys_locale::get_locale().as_deref()))
        } else {
            Language::English
        };
        self.resolve_with(system)
    }

    fn resolve_with(self, system: Language) -> Language {
        match self {
            Self::System => system,
            Self::English => Language::English,
            Self::Chinese => Language::Chinese,
        }
    }
}

impl Language {
    fn from_locale(locale: Option<&str>) -> Self {
        match locale.and_then(|s| s.split(['-', '_', '.', '@']).next()) {
            Some(code) if code.eq_ignore_ascii_case("zh") => Self::Chinese,
            _ => Self::English,
        }
    }

    pub fn text(self, key: &str) -> &str {
        lookup(self.catalog(), english(), key).unwrap_or(key)
    }

    fn catalog(self) -> &'static Catalog {
        static CHINESE: OnceLock<Catalog> = OnceLock::new();
        match self {
            Self::English => english(),
            Self::Chinese => {
                CHINESE.get_or_init(|| parse_catalog(include_str!("../../assets/locales/zh.json")))
            }
        }
    }

    /// Numbered placeholders allow translators to reorder arguments. Inserted
    /// user data is never interpreted as part of the template.
    pub fn format(self, key: &str, arguments: &[&dyn std::fmt::Display]) -> String {
        interpolate(self.text(key), arguments)
    }

    /// VM/service diagnostics still use English strings. Their compatibility
    /// patterns live alongside the catalogs, without changing CLI output.
    pub fn message(self, message: &str) -> String {
        if english().contains_key(message) {
            return self.text(message).to_owned();
        }
        if let Some((key, _)) = english()
            .iter()
            .find(|(_, value)| value.as_str() == message)
        {
            return self.text(key).to_owned();
        }
        static RULES: OnceLock<Vec<DiagnosticRule>> = OnceLock::new();
        let rules = RULES.get_or_init(|| {
            serde_json::from_str(include_str!("../../assets/locales/diagnostics.json"))
                .expect("valid bundled diagnostic patterns")
        });
        for rule in rules {
            if let Some(arguments) = match_diagnostic(&rule.source, message) {
                let arguments: Vec<&dyn std::fmt::Display> = arguments
                    .iter()
                    .map(|s| s as &dyn std::fmt::Display)
                    .collect();
                return self.format(&rule.key, &arguments);
            }
        }
        message.to_owned()
    }
}

type Catalog = std::collections::BTreeMap<String, String>;

fn parse_catalog(json: &str) -> Catalog {
    serde_json::from_str(json).expect("valid bundled language catalog")
}

fn english() -> &'static Catalog {
    static ENGLISH: OnceLock<Catalog> = OnceLock::new();
    ENGLISH.get_or_init(|| parse_catalog(include_str!("../../assets/locales/en.json")))
}

fn lookup<'a>(selected: &'a Catalog, fallback: &'a Catalog, key: &str) -> Option<&'a str> {
    selected
        .get(key)
        .or_else(|| fallback.get(key))
        .map(String::as_str)
}

fn interpolate(template: &str, arguments: &[&dyn std::fmt::Display]) -> String {
    use std::fmt::Write;
    let mut result = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        if let Some(end) = rest.find('}')
            && let Ok(index) = rest[1..end].parse::<usize>()
            && let Some(argument) = arguments.get(index)
        {
            let _ = write!(result, "{argument}");
            rest = &rest[end + 1..];
        } else {
            result.push('{');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

#[derive(Deserialize)]
struct DiagnosticRule {
    source: String,
    key: String,
}

fn match_diagnostic<'a>(pattern: &str, message: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = pattern.split("{}");
    let mut rest = message.strip_prefix(parts.next()?)?;
    let mut arguments = Vec::new();
    let parts: Vec<_> = parts.collect();
    for (index, delimiter) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        let end = if last {
            rest.strip_suffix(delimiter)?.len()
        } else {
            rest.rfind(delimiter)?
        };
        arguments.push(&rest[..end]);
        rest = &rest[end + delimiter.len()..];
    }
    rest.is_empty().then_some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_have_matching_keys_and_placeholders() {
        let en = english();
        let zh = Language::Chinese.catalog();
        assert_eq!(en.keys().collect::<Vec<_>>(), zh.keys().collect::<Vec<_>>());
        fn placeholders(text: &str) -> std::collections::BTreeSet<usize> {
            text.split('{')
                .skip(1)
                .map(|part| {
                    part.split_once('}')
                        .expect("closed placeholder")
                        .0
                        .parse()
                        .expect("numbered placeholder")
                })
                .collect()
        }
        for (key, value) in en {
            assert!(!value.is_empty(), "empty English text: {key}");
            assert!(!zh[key].is_empty(), "empty Chinese text: {key}");
            assert_eq!(placeholders(value), placeholders(&zh[key]), "{key}");
        }
        let rules: Vec<DiagnosticRule> =
            serde_json::from_str(include_str!("../../assets/locales/diagnostics.json")).unwrap();
        for rule in rules {
            let count = rule.source.matches("{}").count();
            assert!(count > 0);
            assert_eq!(
                placeholders(&en[&rule.key]),
                (0..count).collect(),
                "{}",
                rule.key
            );
        }
    }

    #[test]
    fn missing_translations_fall_back_and_arguments_can_be_reordered() {
        let fallback = Catalog::from([("greeting".into(), "Hello {0}".into())]);
        assert_eq!(
            lookup(&Catalog::new(), &fallback, "greeting"),
            Some("Hello {0}")
        );
        assert_eq!(
            interpolate("{1}: {0}; {1}", &[&"file {1}", &"result"]),
            "result: file {1}; result"
        );
        assert_eq!(
            Language::Chinese
                .message("Could not read /tmp/{}.json: invalid. Correct the file and restart."),
            "无法读取 /tmp/{}.json: invalid。请修正文件并重新启动。"
        );
        let diagnostic = "some raw error\nProgram counter: 0x00001234";
        assert_eq!(Language::English.message(diagnostic), diagnostic);
        assert_eq!(
            Language::Chinese.message(diagnostic),
            "some raw error\n程序计数器：0x00001234"
        );
    }

    #[test]
    fn ui_references_only_existing_catalog_keys() {
        for source in [
            include_str!("../app.rs"),
            include_str!("windows.rs"),
            include_str!("font_dialog/linux.rs"),
            include_str!("font_dialog/macos.rs"),
            include_str!("font_dialog/windows.rs"),
        ] {
            for call in ["language.text(", "language.format("] {
                for rest in source.split(call).skip(1) {
                    if let Some(literal) = rest.trim_start().strip_prefix('"') {
                        let key = literal.split('"').next().unwrap();
                        assert!(english().contains_key(key), "unextracted UI text: {key}");
                    }
                }
            }
            for literal in source.split('"').skip(1).step_by(2) {
                if literal.starts_with("ui.") || literal.starts_with("language.") {
                    assert!(english().contains_key(literal), "unknown UI key: {literal}");
                }
            }
        }
    }

    #[test]
    fn menu_and_status_follow_language_changes() {
        use super::super::PlayerApp;
        use eframe::egui;
        let context = egui::Context::default();
        let mut app = PlayerApp::new(
            &eframe::CreationContext::_new_kittest(context.clone()),
            None,
        );
        app.status = "Waiting for a command".into();
        for (preference, menu, status) in [
            (LanguagePreference::Chinese, "文件", "等待输入命令"),
            (LanguagePreference::English, "File", "Waiting for a command"),
        ] {
            app.settings.language = preference;
            let mut output = context.run_ui(egui::RawInput::default(), |root| {
                app.menu_bar(root);
                app.status_bar(root);
            });
            fn collect(shape: &egui::epaint::Shape, labels: &mut Vec<String>) {
                match shape {
                    egui::epaint::Shape::Vec(shapes) => {
                        shapes.iter().for_each(|shape| collect(shape, labels))
                    }
                    egui::epaint::Shape::Text(text) => labels.push(text.galley.job.text.clone()),
                    _ => {}
                }
            }
            let mut labels = Vec::new();
            for shape in &output.shapes {
                collect(&shape.shape, &mut labels);
            }
            output.textures_delta.clear();
            assert!(labels.iter().any(|label| label == menu), "{labels:?}");
            assert!(labels.iter().any(|label| label == status), "{labels:?}");
        }
    }

    #[test]
    fn detects_chinese_variants_and_falls_back_to_english() {
        for locale in ["zh", "zh-CN", "zh_TW.UTF-8", "ZH-Hant-HK", "zh_SG@variant"] {
            assert_eq!(Language::from_locale(Some(locale)), Language::Chinese);
        }
        for locale in [
            None,
            Some(""),
            Some("C"),
            Some("POSIX"),
            Some("fr-FR"),
            Some("en_US.UTF-8"),
            Some("zhfoo"),
        ] {
            assert_eq!(Language::from_locale(locale), Language::English);
        }
    }

    #[test]
    fn explicit_preference_overrides_system_and_survives_settings_roundtrip() {
        use super::super::PlayerSettings;
        for (preference, expected) in [
            (LanguagePreference::English, Language::English),
            (LanguagePreference::Chinese, Language::Chinese),
        ] {
            for system in [Language::Chinese, Language::English] {
                assert_eq!(preference.resolve_with(system), expected);
            }
            let settings = PlayerSettings {
                language: preference,
                ..Default::default()
            };
            let restored: PlayerSettings =
                serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
            assert_eq!(restored.language, preference);
        }
        let legacy: PlayerSettings = serde_json::from_str(r#"{"font_size":24}"#).unwrap();
        assert_eq!(legacy.language, LanguagePreference::System);
        assert_eq!(
            legacy.language.resolve_with(Language::Chinese),
            Language::Chinese
        );
    }

    #[test]
    fn translates_at_display_time_and_preserves_user_data() {
        let status = "Running /games/English {} 中文.ulx";
        assert_eq!(Language::English.message(status), status);
        assert_eq!(
            Language::Chinese.message(status),
            "正在运行 /games/English {} 中文.ulx"
        );
        assert_eq!(
            Language::Chinese.format("ui.resources", &[&"{} Open /目录"]),
            "资源：{} Open /目录"
        );
        assert_eq!(
            Language::Chinese.text("Unknown future label"),
            "Unknown future label"
        );
        assert_eq!(
            Language::Chinese.message("Replace existing file /save.ulx? Enter yes or no."),
            "是否覆盖现有文件 /save.ulx？请输入 yes 或 no。"
        );
    }
}
