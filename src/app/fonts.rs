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
    pub light_fonts: [bool; 2],
    pub error: Option<String>,
    pub fallback_count: usize,
}

impl Fonts {
    pub fn new(context: &egui::Context, extra_path: &str, system_font: &str) -> Self {
        let mut definitions = egui::FontDefinitions::default();
        let mut errors = None;
        let mut fallback_count = 0;
        let mut total_bytes = 0;
        let extra = Path::new(extra_path.trim());
        let candidates = system_fallbacks();
        if !extra_path.trim().is_empty() {
            match add_font(&mut definitions, extra, "player-extra-font") {
                Ok(length) => {
                    prefer_font(&mut definitions, "player-extra-font");
                    fallback_count += 1;
                    total_bytes += length;
                }
                Err(error) => errors = Some(format!("Could not load {}: {error}", extra.display())),
            }
        }
        if extra_path.trim().is_empty() && !system_font.is_empty() {
            match super::font_dialog::font_bytes(system_font).and_then(|bytes| {
                let index = (0..ttf_parser::fonts_in_collection(&bytes).unwrap_or(1))
                    .find(|&index| {
                        ttf_parser::Face::parse(&bytes, index).is_ok_and(|face| {
                            face.names().into_iter().any(|name| {
                                matches!(name.name_id, 1 | 16)
                                    && name.to_string().is_some_and(|name| {
                                        name.to_lowercase() == system_font.to_lowercase()
                                    })
                            })
                        })
                    })
                    .unwrap_or(0);
                install_font(&mut definitions, bytes, index, "player-system-font")
            }) {
                Ok(length) => {
                    prefer_font(&mut definitions, "player-system-font");
                    fallback_count += 1;
                    total_bytes += length;
                }
                Err(error) => errors = Some(format!("Could not load {system_font}: {error}")),
            }
        }
        for (index, path) in candidates.iter().enumerate() {
            if path == extra
                || is_light_name(&path.file_name().unwrap_or_default().to_string_lossy())
            {
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
        let mut light_fonts = [false; 2];
        let mut candidates = candidates;
        if !extra_path.trim().is_empty() {
            candidates.insert(0, extra.to_owned());
        }
        // Load a general proportional and mono face before script fallbacks.
        // Otherwise many CJK/script weight variants can exhaust the shared
        // budget before an installed mono light face is reached.
        candidates.sort_by_key(|path| {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_ascii_lowercase();
            let priority = if path == extra {
                0
            } else {
                match name.as_str() {
                    "notosansmono-light.ttf" => 1,
                    "notosans-light.ttf" | "segoeuil.ttf" => 2,
                    "notosanscjk-light.ttc" => 3,
                    _ => 4,
                }
            };
            (priority, !name.contains("cjk"), name)
        });
        for (index, path) in candidates.iter().enumerate() {
            if path != extra
                && !is_light_name(&path.file_name().unwrap_or_default().to_string_lossy())
            {
                continue;
            }
            let (count, length) = add_light_faces(
                &mut definitions,
                path,
                index,
                MAX_FALLBACK_BYTES.saturating_sub(total_bytes),
            );
            total_bytes += length;
            fallback_count += count;
        }
        for (index, proportional) in [true, false].into_iter().enumerate() {
            let family = light_family(proportional);
            light_fonts[index] = definitions
                .families
                .get(&family)
                .is_some_and(|faces| !faces.is_empty());
            let regular = definitions.families[&regular_family(proportional)].clone();
            definitions
                .families
                .entry(family)
                .or_default()
                .extend(regular);
        }
        let support = glyph_support(&definitions);
        context.set_fonts(definitions);
        let context = context.clone();
        let metrics: TextMetrics = Arc::new(move |style| {
            let font = egui::FontId::new(style.font_size, family(style));
            context.fonts_mut(|fonts| {
                [
                    fonts.glyph_width(&font, '0').ceil() as u32,
                    fonts.row_height(&font).ceil() as u32,
                ]
            })
        });
        Self {
            support,
            metrics,
            light_fonts,
            error: errors,
            fallback_count,
        }
    }
}

fn regular_family(proportional: bool) -> egui::FontFamily {
    if proportional {
        egui::FontFamily::Proportional
    } else {
        egui::FontFamily::Monospace
    }
}
fn light_family(proportional: bool) -> egui::FontFamily {
    egui::FontFamily::Name(
        if proportional {
            "Glk Light Proportional"
        } else {
            "Glk Light Monospace"
        }
        .into(),
    )
}
pub(super) fn family(style: crate::vm::ResolvedStyle) -> egui::FontFamily {
    if style.weight < 0 {
        light_family(style.proportional)
    } else {
        regular_family(style.proportional)
    }
}

fn is_light_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    (name.contains("-light.")
        || name.contains("-demilight.")
        || name.contains("-extralight.")
        || name.contains("-thin.")
        || matches!(name.as_str(), "segoeuil.ttf" | "segoeuisl.ttf"))
        && !name.contains("italic")
        && [".ttf", ".otf", ".ttc"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

fn is_monospace(face: &ttf_parser::Face<'_>) -> bool {
    if face.is_monospaced() {
        return true;
    }
    // Several released Noto Sans Mono/CJK Mono files leave post.isFixedPitch
    // unset. Their printable ASCII advances still prove a fixed-width face.
    let advance = |c| {
        face.glyph_index(c)
            .and_then(|glyph| face.glyph_hor_advance(glyph))
    };
    let Some(width) = advance('M').filter(|width| *width != 0) else {
        return false;
    };
    (' '..='~').all(|c| advance(c) == Some(width))
}

fn add_light_faces(
    definitions: &mut egui::FontDefinitions,
    path: &Path,
    number: usize,
    budget: u64,
) -> (usize, u64) {
    let length = path.metadata().map_or(0, |m| m.len());
    if length == 0 || length > MAX_FONT_BYTES || length > budget {
        return (0, 0);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (0, 0);
    };
    let mut kinds = [false; 2];
    let mut count = 0;
    let mut used = 0;
    for index in 0..ttf_parser::fonts_in_collection(&bytes).unwrap_or(1) {
        let Ok(face) = ttf_parser::Face::parse(&bytes, index) else {
            continue;
        };
        let mono = is_monospace(&face);
        if !(100..=350).contains(&face.weight().to_number())
            || face.is_italic()
            || kinds[usize::from(mono)]
            || used + length > budget
        {
            continue;
        }
        // Independently validate outlines at the same collection index;
        // weight metadata alone is not evidence of a usable font face.
        let Ok(font) = FontVec::try_from_vec_and_index(bytes.clone(), index) else {
            continue;
        };
        let name = format!("light-font-{number}-{index}");
        let mut data = egui::FontData::from_owned(font.into_vec());
        data.index = index;
        definitions.font_data.insert(name.clone(), Arc::new(data));
        definitions
            .families
            .entry(light_family(!mono))
            .or_default()
            .push(name);
        kinds[usize::from(mono)] = true;
        count += 1;
        used += length;
    }
    (count, used)
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
    install_font(definitions, bytes, 0, name)
}

fn install_font(
    definitions: &mut egui::FontDefinitions,
    bytes: Vec<u8>,
    index: u32,
    name: &str,
) -> Result<u64, String> {
    let length = bytes.len() as u64;
    // Validate outline data at the collection index supplied to egui. Keep
    // malformed or unsupported fonts out of the host's fallback definitions.
    ttf_parser::Face::parse(&bytes, index)
        .map_err(|error| format!("Invalid font data: {error} (face {index})"))?;
    let font = FontVec::try_from_vec_and_index(bytes, index).map_err(|error| error.to_string())?;
    let mut data = egui::FontData::from_owned(font.into_vec());
    data.index = index;
    definitions
        .font_data
        .insert(name.to_owned(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        definitions
            .families
            .entry(family)
            .or_default()
            .push(name.to_owned());
    }
    Ok(length)
}

fn prefer_font(definitions: &mut egui::FontDefinitions, name: &str) {
    let data = &definitions.font_data[name];
    let mono =
        ttf_parser::Face::parse(&data.font, data.index).is_ok_and(|face| is_monospace(&face));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        // Honor the game's proportional/monospace distinction. A selected
        // fixed-pitch (including HW) face remains a proportional fallback for
        // missing glyphs, but must not replace its proportional Latin face.
        if (family == egui::FontFamily::Monospace) != mono {
            continue;
        }
        let names = definitions.families.entry(family).or_default();
        names.retain(|candidate| candidate != name);
        names.insert(0, name.to_owned());
    }
}

fn glyph_support(definitions: &egui::FontDefinitions) -> GlyphSupport {
    let needed: std::collections::HashSet<_> =
        [egui::FontFamily::Proportional, egui::FontFamily::Monospace]
            .iter()
            .filter_map(|family| definitions.families.get(family))
            .flatten()
            .collect();
    let parsed: HashMap<_, _> = definitions
        .font_data
        .iter()
        .filter(|(name, _)| needed.contains(name))
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
            } else if kind.is_file()
                && (is_fallback_name(&entry.file_name().to_string_lossy())
                    || is_light_name(&entry.file_name().to_string_lossy()))
            {
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
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                    let font = egui::FontId::new(18.0, family);
                    assert!(fonts.has_glyph(&font, '中'));
                    let text = fonts.layout_no_wrap("中文".to_owned(), font, egui::Color32::BLACK);
                    assert!(!text.rows[0].visuals.mesh.vertices.is_empty());
                }
            });
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn light_faces_use_real_weight_metadata_and_render_in_named_families() {
        let candidates = system_fallbacks();
        let Some(path) = candidates
            .iter()
            .find(|path| is_light_name(&path.file_name().unwrap_or_default().to_string_lossy()))
        else {
            // System font installation varies by platform; the metadata/fallback
            // contracts below have a separate test using bundled font bytes.
            return;
        };
        let mut definitions = egui::FontDefinitions::default();
        let (count, _) = add_light_faces(&mut definitions, path, 0, MAX_FALLBACK_BYTES);
        assert!(count > 0, "{}", path.display());
        let mut families = Vec::new();
        for proportional in [true, false] {
            let family = light_family(proportional);
            if let Some(names) = definitions.families.get(&family) {
                for name in names {
                    let data = &definitions.font_data[name];
                    let face = ttf_parser::Face::parse(data.font.as_ref(), data.index).unwrap();
                    assert!((100..=350).contains(&face.weight().to_number()));
                    assert_eq!(is_monospace(&face), !proportional);
                }
                families.push(family);
            }
        }
        let context = egui::Context::default();
        context.set_fonts(definitions);
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            ui.fonts_mut(|fonts| {
                for family in &families {
                    let font = egui::FontId::new(18.0, family.clone());
                    // Egui 0.36's has_glyph compares fallback face identities,
                    // so a single-face family falsely rejects all its glyphs.
                    // Compare rendered A with the actual replacement instead.
                    let text =
                        fonts.layout_no_wrap("A".to_owned(), font.clone(), egui::Color32::BLACK);
                    let missing =
                        fonts.layout_no_wrap("\u{10ffff}".to_owned(), font, egui::Color32::BLACK);
                    assert!(!text.rows[0].visuals.mesh.vertices.is_empty());
                    assert_ne!(
                        text.rows[0].visuals.mesh.vertices,
                        missing.rows[0].visuals.mesh.vertices
                    );
                }
            });
        });
        output.drop_without_applying_deltas();
    }

    #[test]
    fn regular_or_invalid_fonts_cannot_be_mislabeled_as_light() {
        let definitions = egui::FontDefinitions::default();
        let regular = &definitions.font_data["Hack"];
        let path =
            std::env::temp_dir().join(format!("glulx-false-light-{}.ttf", std::process::id()));
        std::fs::write(&path, regular.font.as_ref()).unwrap();
        let mut target = egui::FontDefinitions::default();
        assert_eq!(
            add_light_faces(&mut target, &path, 0, MAX_FALLBACK_BYTES),
            (0, 0)
        );
        std::fs::write(&path, b"invalid font").unwrap();
        assert_eq!(
            add_light_faces(&mut target, &path, 1, MAX_FALLBACK_BYTES),
            (0, 0)
        );
        assert!(!target.families.contains_key(&light_family(true)));
        assert!(!target.families.contains_key(&light_family(false)));
        std::fs::remove_file(path).unwrap();
    }
}
#[test]
fn selected_font_has_priority_and_preserves_a_real_monospace_fallback() {
    let mut definitions = egui::FontDefinitions::default();
    let original_mono = definitions.families[&egui::FontFamily::Monospace][0].clone();
    let source = definitions.families[&egui::FontFamily::Proportional][0].clone();
    let data = definitions.font_data[&source].clone();
    install_font(&mut definitions, data.font.to_vec(), data.index, "selected").unwrap();
    prefer_font(&mut definitions, "selected");
    assert_eq!(
        definitions.families[&egui::FontFamily::Proportional][0],
        "selected"
    );
    assert_eq!(
        definitions.families[&egui::FontFamily::Monospace][0],
        original_mono
    );
}

#[test]
fn selected_fixed_pitch_font_keeps_proportional_body_text_proportional() {
    let mut definitions = egui::FontDefinitions::default();
    let original = definitions.families[&egui::FontFamily::Proportional][0].clone();
    let data = definitions.font_data["Hack"].clone();
    install_font(
        &mut definitions,
        data.font.to_vec(),
        data.index,
        "selected-fixed",
    )
    .unwrap();
    prefer_font(&mut definitions, "selected-fixed");
    assert_eq!(
        definitions.families[&egui::FontFamily::Proportional][0],
        original
    );
    assert_eq!(
        definitions.families[&egui::FontFamily::Monospace][0],
        "selected-fixed"
    );
    assert!(
        definitions.families[&egui::FontFamily::Proportional]
            .iter()
            .any(|name| name == "selected-fixed")
    );
}
