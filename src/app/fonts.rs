//! Discover usable outline fonts and report the same fallback coverage to Glk.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ab_glyph::{Font, FontArc, FontVec};
use eframe::egui;

use crate::vm::{GlyphSupport, TextMetrics};

const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FALLBACK_BYTES: u64 = 128 * 1024 * 1024;

pub(super) struct Fonts {
    pub support: GlyphSupport,
    pub metrics: TextMetrics,
    pub error: Option<String>,
    pub fallback_count: usize,
}

impl Fonts {
    pub fn new(context: &egui::Context, extra_path: &str) -> Self {
        let mut definitions = egui::FontDefinitions::default();
        let mut errors = None;
        let mut fallback_count = 0;
        let mut total_bytes = 0;
        let extra = Path::new(extra_path.trim());
        if !extra_path.trim().is_empty() {
            match add_font(&mut definitions, extra, "player-extra-font") {
                Ok(length) => {
                    fallback_count += 1;
                    total_bytes += length;
                }
                Err(error) => errors = Some(format!("Could not load {}: {error}", extra.display())),
            }
        }
        for (index, path) in system_fallbacks().iter().enumerate() {
            if path == extra {
                continue;
            }
            let length = path.metadata().map_or(0, |m| m.len());
            if length == 0 || length > MAX_FONT_BYTES || total_bytes + length > MAX_FALLBACK_BYTES {
                continue;
            }
            if let Ok(length) =
                add_font(&mut definitions, path, &format!("system-fallback-{index}"))
            {
                fallback_count += 1;
                total_bytes += length;
            }
        }
        let support = glyph_support(&definitions);
        context.set_fonts(definitions);
        let context = context.clone();
        let metrics: TextMetrics = Arc::new(move |style| {
            let font = egui::FontId::new(
                style.font_size,
                if style.proportional {
                    egui::FontFamily::Proportional
                } else {
                    egui::FontFamily::Monospace
                },
            );
            context.fonts(|fonts| {
                [
                    fonts.glyph_width(&font, '0').ceil() as u32,
                    fonts.row_height(&font).ceil() as u32,
                ]
            })
        });
        Self {
            support,
            metrics,
            error: errors,
            fallback_count,
        }
    }
}

fn add_font(
    definitions: &mut egui::FontDefinitions,
    path: &Path,
    name: &str,
) -> Result<u64, String> {
    let length = path.metadata().map_err(|error| error.to_string())?.len();
    if length > MAX_FONT_BYTES {
        return Err("Font file exceeds 64 MiB".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    // Use exactly the parser/index which egui will use. Reject unsupported or
    // malformed font data here instead of allowing egui to panic on a repaint.
    let font = FontVec::try_from_vec_and_index(bytes, 0).map_err(|error| error.to_string())?;
    definitions.font_data.insert(
        name.to_owned(),
        Arc::new(egui::FontData::from_owned(font.into_vec())),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .push(name.to_owned());
    }
    Ok(length)
}

fn glyph_support(definitions: &egui::FontDefinitions) -> GlyphSupport {
    let parsed: HashMap<_, _> = definitions
        .font_data
        .iter()
        .filter_map(|(name, data)| {
            FontVec::try_from_vec_and_index(data.font.to_vec(), data.index)
                .ok()
                .map(|font| (name.clone(), FontArc::from(font)))
        })
        .collect();
    let families: Vec<Vec<FontArc>> = [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
        .iter()
        .map(|family| {
            definitions
                .families
                .get(family)
                .into_iter()
                .flatten()
                .filter_map(|name| parsed.get(name).cloned())
                .collect()
        })
        .collect();
    let cache = Mutex::new(HashMap::new());
    Arc::new(move |character| {
        let mut cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache.entry(character).or_insert_with(|| {
            families.iter().all(|fonts| {
                fonts
                    .iter()
                    .find_map(|font| {
                        let id = font.glyph_id(character);
                        (id.0 != 0).then(|| character.is_whitespace() || font.outline(id).is_some())
                    })
                    .unwrap_or(false)
            })
        })
    })
}

fn system_fallbacks() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ];
    if let Some(user_home) = std::env::var_os("HOME") {
        let root = PathBuf::from(user_home);
        roots.extend([
            root.join(".fonts"),
            root.join(".local/share/fonts"),
            root.join("Library/Fonts"),
        ]);
    }
    if let Some(windows) = std::env::var_os("WINDIR") {
        roots.push(PathBuf::from(windows).join("Fonts"));
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local_app_data).join("Microsoft/Windows/Fonts"));
    }
    let mut pending: Vec<_> = roots.into_iter().map(|root| (root, 0)).collect();
    let mut files = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 4 {
                pending.push((entry.path(), depth + 1));
            } else if kind.is_file() && is_fallback_name(&entry.file_name().to_string_lossy()) {
                files.push(entry.path());
            }
        }
    }
    // Give CJK coverage priority before smaller script-specific fonts when the
    // combined memory limit is reached. Duplicated roots must not load twice.
    files.sort_by_key(|path| {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        (!name.contains("cjk"), name, path.clone())
    });
    files.dedup();
    files
}

fn is_fallback_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.starts_with("notosans") && name.contains("-regular."))
        || matches!(
            name.as_str(),
            "dejavusans.ttf"
                | "symbola.ttf"
                | "unifont.ttf"
                | "arialuni.ttf"
                | "seguisym.ttf"
                | "msgothic.ttc"
                | "simsun.ttc"
                | "malgun.ttf"
                | "meiryo.ttc"
                | "mingliu.ttc"
                | "arial unicode.ttf"
                | "pingfang.ttc"
                | "hiragino sans gb.ttc"
                | "apple symbols.ttf"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_font_coverage_rejects_missing_glyphs_and_preserves_ascii() {
        let support = glyph_support(&egui::FontDefinitions::default());
        for character in ' '..='~' {
            assert!(support(character), "{character:?}");
        }
        assert!(!support('中'));
        assert!(!support('\u{10ffff}'));
        assert!(support('é'));
    }

    #[test]
    fn user_fallback_enables_glyphs_and_invalid_fonts_leave_definitions_intact() {
        let defaults = egui::FontDefinitions::default();
        let font = defaults.font_data["Hack"].clone();
        let path = std::env::temp_dir().join(format!("glulx-font-{}.ttf", std::process::id()));
        std::fs::write(&path, font.font.as_ref()).unwrap();
        let mut definitions = egui::FontDefinitions::empty();
        assert!(!glyph_support(&definitions)('A'));
        add_font(&mut definitions, &path, "test-user-font").unwrap();
        let support = glyph_support(&definitions);
        assert!(support('A'));
        assert!(support('é'));
        let before = definitions.font_data.len();
        std::fs::write(&path, b"not an OpenType font").unwrap();
        assert!(add_font(&mut definitions, &path, "invalid").is_err());
        assert_eq!(definitions.font_data.len(), before);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn system_font_selection_covers_portable_cjk_families_without_weight_duplicates() {
        for name in [
            "NotoSansCJK-Regular.ttc",
            "NotoSansSymbols2-Regular.ttf",
            "PingFang.ttc",
            "simsun.ttc",
        ] {
            assert!(is_fallback_name(name), "{name}");
        }
        for name in [
            "NotoSansCJK-Bold.ttc",
            "NotoSansThai-Light.ttf",
            "LastResort.otf",
            "fonts.conf",
        ] {
            assert!(!is_fallback_name(name), "{name}");
        }
    }

    #[test]
    fn discovered_cjk_font_coverage_matches_real_egui_glyphs() {
        let Some(path) = system_fallbacks().into_iter().find(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .contains("cjk")
        }) else {
            return;
        };
        let mut definitions = egui::FontDefinitions::default();
        add_font(&mut definitions, &path, "test-cjk").unwrap();
        let support = glyph_support(&definitions);
        assert!(support('中'));
        assert!(support('文'));
        let context = egui::Context::default();
        context.set_fonts(definitions);
        let _ = context.run(egui::RawInput::default(), |context| {
            context.fonts(|fonts| {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    let font = egui::FontId::new(18.0, family);
                    assert!(fonts.has_glyph(&font, '中'));
                    let text = fonts.layout_no_wrap("中文".to_owned(), font, egui::Color32::BLACK);
                    assert!(!text.rows[0].visuals.mesh.vertices.is_empty());
                }
            });
        });
    }
}
