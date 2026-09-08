//! External resource selection, identity checks and loose-file import.

use std::path::{Path, PathBuf};

use super::{StoryError, blorb_chunks, parse_resource_index, read_u32};

pub(super) fn read_resource_file(path: &Path) -> Result<Vec<u8>, StoryError> {
    std::fs::read(path).map_err(|source| StoryError::ResourceIo {
        path: path.to_owned(),
        source,
    })
}

pub(super) fn discover_resources(story_path: &Path) -> Result<Option<PathBuf>, StoryError> {
    let directory = story_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let entries = std::fs::read_dir(directory).map_err(|source| StoryError::ResourceIo {
        path: directory.to_owned(),
        source,
    })?;
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| StoryError::ResourceIo {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        if path.file_stem() != story_path.file_stem()
            || path.file_name() == story_path.file_name()
            || path.is_dir()
        {
            continue;
        }
        if let Some(priority) = path
            .extension()
            .and_then(|suffix| suffix.to_str())
            .and_then(|suffix| {
                ["blorb", "blb", "gblorb", "glb"]
                    .iter()
                    .position(|extension| suffix.eq_ignore_ascii_case(extension))
            })
        {
            candidates.push((priority, path));
        }
    }
    candidates.sort();
    if candidates.len() > 1 && candidates[0].0 == candidates[1].0 {
        return Err(StoryError::AmbiguousResources {
            first: candidates[0].1.clone(),
            second: candidates[1].1.clone(),
        });
    }
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

pub(super) fn validate_blorb_identity(
    bytes: &[u8],
    image: &[u8],
    resource_only: bool,
) -> Result<(), StoryError> {
    let chunks = blorb_chunks(bytes)?;
    if resource_only {
        let resources = parse_resource_index(bytes)?;
        if resources
            .keys()
            .any(|(usage, _)| *usage == u32::from_be_bytes(*b"Exec"))
            || chunks.iter().any(|(start, _)| {
                matches!(
                    &bytes[*start..*start + 4],
                    b"GLUL"
                        | b"ZCOD"
                        | b"TAD2"
                        | b"TAD3"
                        | b"HUGO"
                        | b"ALAN"
                        | b"ADRI"
                        | b"LEVE"
                        | b"AGT "
                        | b"MAGS"
                        | b"ADVS"
                        | b"EXEC"
                )
            })
        {
            return Err(StoryError::ResourcesContainExecutable);
        }
    }
    let mut identity_seen = false;
    for (start, end) in chunks {
        if &bytes[start..start + 4] == b"IFhd" {
            if identity_seen {
                return Err(StoryError::InvalidBlorb("duplicate IFhd"));
            }
            identity_seen = true;
            if end - start - 8 != 128 {
                return Err(StoryError::InvalidBlorb(
                    "Glulx IFhd must contain exactly 128 bytes",
                ));
            }
            if image.get(..128) != Some(&bytes[start + 8..end]) {
                return Err(StoryError::ResourceIdentityMismatch);
            }
        }
    }
    Ok(())
}

pub(super) type LooseKind = ([u8; 4], Option<([u8; 4], u32)>);

fn loose_resource_kind(path: &Path, bytes: &[u8]) -> Result<Option<LooseKind>, StoryError> {
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(None);
    };
    let stem = stem.to_ascii_uppercase();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if stem == "STORY" {
        return Err(StoryError::ResourcesContainExecutable);
    }
    let metadata_tag = match stem.as_str() {
        "IDENT" => Some(*b"IFhd"),
        "FRONTIS" => Some(*b"Fspc"),
        "RESDESC" => Some(*b"RDes"),
        "METADATA" => Some(*b"IFmd"),
        "PALETTE" => Some(*b"Plte"),
        "RELEASE" => Some(*b"RelN"),
        "RESOL" => Some(*b"Reso"),
        "ADAPTPAL" => Some(*b"APal"),
        "LOOPING" => Some(*b"Loop"),
        _ => None,
    };
    if let Some(tag) = metadata_tag {
        if extension.is_empty() || extension == "bin" || (tag == *b"IFmd" && extension == "xml") {
            return Ok(Some((tag, None)));
        }
        return Err(StoryError::UnsupportedResourceFile(path.to_owned()));
    }
    let Some((prefix, digits)) = ["PIC", "SND", "DATA"]
        .iter()
        .find_map(|prefix| stem.strip_prefix(prefix).map(|digits| (*prefix, digits)))
        .filter(|(_, digits)| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    else {
        return Ok(None);
    };
    let number = digits
        .parse::<u32>()
        .map_err(|_| StoryError::UnsupportedResourceFile(path.to_owned()))?;
    let (usage, tag) = match (prefix, extension.as_str()) {
        ("PIC", "png") => (*b"Pict", *b"PNG "),
        ("PIC", "jpg" | "jpeg") => (*b"Pict", *b"JPEG"),
        ("PIC", "") if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => (*b"Pict", *b"PNG "),
        ("PIC", "") if bytes.starts_with(b"\xff\xd8") => (*b"Pict", *b"JPEG"),
        ("SND", "aif" | "aiff") => (*b"Snd ", *b"FORM"),
        ("SND", "ogg") => (*b"Snd ", *b"OGGV"),
        ("SND", "mp3") => (*b"Snd ", *b"MP3 "),
        ("SND", "mod" | "xm" | "s3m" | "it") => (*b"Snd ", *b"MOD "),
        ("SND", "song") => (*b"Snd ", *b"SONG"),
        ("SND", "") if bytes.starts_with(b"FORM") => (*b"Snd ", *b"FORM"),
        ("SND", "") if bytes.starts_with(b"OggS") => (*b"Snd ", *b"OGGV"),
        ("SND", "")
            if bytes.starts_with(b"ID3")
                || bytes
                    .get(..2)
                    .is_some_and(|b| b[0] == 0xff && b[1] & 0xe0 == 0xe0) =>
        {
            (*b"Snd ", *b"MP3 ")
        }
        ("SND", "")
            if bytes.starts_with(b"Extended Module: ")
                || bytes.starts_with(b"IMPM")
                || bytes.get(44..48) == Some(b"SCRM")
                || matches!(bytes.get(1080..1084), Some(b"M.K." | b"M!K!" | b"4CHN")) =>
        {
            (*b"Snd ", *b"MOD ")
        }
        ("DATA", "txt") => (*b"Data", *b"TEXT"),
        ("DATA", "" | "bin" | "bina") => (*b"Data", *b"BINA"),
        ("DATA", "form" | "iff") => (*b"Data", *b"FORM"),
        _ => return Err(StoryError::UnsupportedResourceFile(path.to_owned())),
    };
    if tag == *b"FORM"
        && (bytes.len() < 12
            || !bytes.starts_with(b"FORM")
            || read_u32(bytes, 4)? as usize != bytes.len() - 8
            || (usage == *b"Snd " && bytes.get(8..12) != Some(b"AIFF")))
    {
        return Err(StoryError::UnsupportedResourceFile(path.to_owned()));
    }
    Ok(Some((tag, Some((usage, number)))))
}

pub(super) fn directory_resources(directory: &Path) -> Result<Vec<u8>, StoryError> {
    let paths = std::fs::read_dir(directory)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|source| StoryError::ResourceIo {
            path: directory.to_owned(),
            source,
        })?;
    let mut paths = paths;
    paths.sort();
    let mut chunks = Vec::new();
    let mut metadata = std::collections::HashSet::new();
    for path in paths {
        if path.is_dir() {
            continue;
        }
        // Read only named resources, not every unrelated file in the directory.
        let name = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        let numbered = ["PIC", "SND", "DATA"].iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        });
        let metadata_name = matches!(
            name.as_str(),
            "STORY"
                | "IDENT"
                | "FRONTIS"
                | "RESDESC"
                | "METADATA"
                | "PALETTE"
                | "RELEASE"
                | "RESOL"
                | "ADAPTPAL"
                | "LOOPING"
        );
        if !numbered && !metadata_name {
            continue;
        }
        let bytes = read_resource_file(&path)?;
        if let Some((tag, usage)) = loose_resource_kind(&path, &bytes)? {
            if usage.is_none() && !metadata.insert(tag) {
                return Err(StoryError::InvalidBlorb("duplicate resource metadata file"));
            }
            let bytes = if tag == *b"FORM" {
                bytes[8..].to_vec()
            } else {
                bytes
            };
            chunks.push((tag, usage, bytes));
        }
    }
    let count = chunks
        .iter()
        .filter(|(_, usage, _)| usage.is_some())
        .count();
    let mut bytes = b"FORM\0\0\0\0IFRSRIdx".to_vec();
    let index_length = count
        .checked_mul(12)
        .and_then(|n| n.checked_add(4))
        .ok_or(StoryError::InvalidBlorb("resource directory is too large"))?;
    let index_length = u32::try_from(index_length)
        .map_err(|_| StoryError::InvalidBlorb("resource directory is too large"))?;
    bytes.extend_from_slice(&index_length.to_be_bytes());
    bytes.extend_from_slice(&(count as u32).to_be_bytes());
    bytes.resize(20 + index_length as usize, 0);
    let mut entry = 24;
    for (tag, usage, data) in chunks {
        let offset = u32::try_from(bytes.len())
            .map_err(|_| StoryError::InvalidBlorb("resource directory is too large"))?;
        if let Some((usage, number)) = usage {
            bytes[entry..entry + 4].copy_from_slice(&usage);
            bytes[entry + 4..entry + 8].copy_from_slice(&number.to_be_bytes());
            bytes[entry + 8..entry + 12].copy_from_slice(&offset.to_be_bytes());
            entry += 12;
        }
        bytes.extend_from_slice(&tag);
        let length = u32::try_from(data.len())
            .map_err(|_| StoryError::InvalidBlorb("resource file is too large"))?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(&data);
        if data.len() % 2 != 0 {
            bytes.push(0);
        }
    }
    let length = u32::try_from(bytes.len() - 8)
        .map_err(|_| StoryError::InvalidBlorb("resource directory is too large"))?;
    bytes[4..8].copy_from_slice(&length.to_be_bytes());
    Ok(bytes)
}
