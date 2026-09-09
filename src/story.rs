use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use thiserror::Error;

mod resources;
#[cfg(test)]
use resources::LooseKind;
use resources::{
    directory_resources, discover_resources, read_resource_file, validate_blorb_identity,
};

const GLUL_MAGIC: u32 = 0x476c_756c;
const FORM_MAGIC: &[u8; 4] = b"FORM";
const IFRS_MAGIC: &[u8; 4] = b"IFRS";

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoryHeader {
    pub version: u32,
    pub ram_start: u32,
    pub ext_start: u32,
    pub end_mem: u32,
    pub stack_size: u32,
    pub start_func: u32,
    pub decoding_table: u32,
    pub checksum: u32,
}

impl StoryHeader {
    fn parse(bytes: &[u8]) -> Result<Self, StoryError> {
        if bytes.len() < 36 {
            return Err(StoryError::TooSmall);
        }
        if read_u32(bytes, 0)? != GLUL_MAGIC {
            return Err(StoryError::BadMagic);
        }
        let header = Self {
            version: read_u32(bytes, 4)?,
            ram_start: read_u32(bytes, 8)?,
            ext_start: read_u32(bytes, 12)?,
            end_mem: read_u32(bytes, 16)?,
            stack_size: read_u32(bytes, 20)?,
            start_func: read_u32(bytes, 24)?,
            decoding_table: read_u32(bytes, 28)?,
            checksum: read_u32(bytes, 32)?,
        };
        header.validate(bytes)?;
        Ok(header)
    }

    fn validate(&self, bytes: &[u8]) -> Result<(), StoryError> {
        if !(0x0002_0000..=0x0003_01ff).contains(&self.version) {
            return Err(StoryError::UnsupportedVersion(self.version));
        }
        if self.end_mem > crate::memory::MAX_MEMORY_SIZE {
            return Err(StoryError::MemoryLimit(self.end_mem));
        }
        if self.ram_start < 0x100
            || !self.ram_start.is_multiple_of(0x100)
            || !self.ext_start.is_multiple_of(0x100)
            || !self.end_mem.is_multiple_of(0x100)
            || !self.stack_size.is_multiple_of(0x100)
            || self.ram_start > self.ext_start
            || self.ext_start > self.end_mem
        {
            return Err(StoryError::InvalidLayout);
        }
        if self.ext_start as usize > bytes.len() {
            return Err(StoryError::Truncated {
                expected: self.ext_start as usize,
                actual: bytes.len(),
            });
        }
        if self.ext_start as usize != bytes.len() {
            return Err(StoryError::UnexpectedLength {
                expected: self.ext_start as usize,
                actual: bytes.len(),
            });
        }
        if self.start_func >= self.ext_start {
            return Err(StoryError::InvalidStartFunction(self.start_func));
        }

        let mut sum = 0u32;
        for offset in (0..self.ext_start as usize).step_by(4) {
            let word = if offset == 32 {
                0
            } else {
                read_u32(bytes, offset)?
            };
            sum = sum.wrapping_add(word);
        }
        if sum != self.checksum {
            return Err(StoryError::Checksum {
                expected: self.checksum,
                actual: sum,
            });
        }
        Ok(())
    }

    pub fn version_string(&self) -> String {
        format!(
            "{}.{}.{}",
            self.version >> 16,
            (self.version >> 8) & 0xff,
            self.version & 0xff
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub title: String,
    pub author: String,
    pub headline: String,
    pub description: String,
    pub ifid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDescription {
    pub usage: [u8; 4],
    pub number: u32,
    pub text: String,
}

/// Select an external resource archive or directory before starting the VM.
/// `None` disables discovery; a story's own bundled resources remain available.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ResourceSelection {
    #[default]
    Auto,
    None,
    Path(PathBuf),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct ExternalResources {
    path: Option<PathBuf>,
    // Directories are stored as an equivalent resource-only Blorb. Sessions
    // retain every byte and never consult the directory or archive again.
    bytes: Vec<u8>,
    original_title: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Story {
    pub path: Option<PathBuf>,
    pub title: String,
    pub header: StoryHeader,
    pub image: Vec<u8>,
    /// The original story container, separate from any selected resources.
    pub container: Option<Vec<u8>>,
    #[serde(default)]
    external_resources: Option<ExternalResources>,
    #[serde(skip)]
    resources: HashMap<(u32, u32), (usize, usize)>,
}

impl Story {
    pub(crate) fn snapshot_byte_len(&self) -> usize {
        self.image
            .len()
            .saturating_add(self.container.as_ref().map_or(0, Vec::len))
            .saturating_add(
                self.external_resources
                    .as_ref()
                    .map_or(0, |resources| resources.bytes.len()),
            )
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoryError> {
        Self::open_with_resources(path, ResourceSelection::Auto)
    }

    /// Auto discovers resources only beside a raw executable, in this order:
    /// `.blorb`, `.blb`, `.gblorb`, `.glb`. Stems match exactly; suffixes ignore
    /// ASCII case. Multiple candidates at one priority are an error. An explicit
    /// path bypasses discovery, including errors in unused sibling archives.
    pub fn open_with_resources(
        path: impl AsRef<Path>,
        selection: ResourceSelection,
    ) -> Result<Self, StoryError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let mut story = Self::from_bytes(&bytes, path.file_stem().and_then(|s| s.to_str()))?;
        story.path = Some(path.to_path_buf());
        let resource_path = match selection {
            ResourceSelection::Path(path) => Some(path),
            ResourceSelection::Auto if story.container.is_none() => discover_resources(path)?,
            _ => None,
        };
        if let Some(path) = resource_path {
            story.attach_resource_path(path)?;
        }
        Ok(story)
    }

    pub fn from_bytes(bytes: &[u8], title: Option<&str>) -> Result<Self, StoryError> {
        let (image, container, resources) = if bytes.starts_with(FORM_MAGIC) {
            (
                extract_glul_chunk(bytes)?.to_vec(),
                Some(bytes.to_vec()),
                parse_resource_index(bytes)?,
            )
        } else {
            (bytes.to_vec(), None, HashMap::new())
        };
        let header = StoryHeader::parse(&image)?;
        let image = image[..header.ext_start as usize].to_vec();
        if let Some(bytes) = &container {
            validate_blorb_identity(bytes, &image, false)?;
        }
        let mut story = Self {
            path: None,
            title: title.unwrap_or("Untitled Glulx story").to_owned(),
            header,
            image,
            container,
            external_resources: None,
            resources,
        };
        let metadata = story.metadata();
        if !metadata.title.is_empty() {
            story.title = metadata.title;
        }
        Ok(story)
    }

    /// Atomically attach an archive without an executable. An optional Glulx
    /// IFhd must match the first 128 bytes of the original executable.
    pub fn attach_blorb(&mut self, bytes: &[u8]) -> Result<(), StoryError> {
        let resources = parse_resource_index(bytes)?;
        validate_blorb_identity(bytes, &self.image, true)?;
        let original_title = self
            .external_resources
            .as_ref()
            .map_or(&self.title, |external| &external.original_title)
            .clone();
        self.external_resources = Some(ExternalResources {
            path: None,
            bytes: bytes.to_vec(),
            original_title: original_title.clone(),
        });
        self.resources = resources;
        let metadata = self.metadata();
        self.title = if metadata.title.is_empty() {
            original_title
        } else {
            metadata.title
        };
        Ok(())
    }

    /// Attach a resource-only Blorb or an explicitly selected loose-resource
    /// directory. Files are read now; subsequent resource calls and session
    /// restoration do not depend on their continued existence.
    pub fn attach_resource_path(&mut self, path: impl AsRef<Path>) -> Result<(), StoryError> {
        let path = path.as_ref();
        let bytes = if path.is_dir() {
            directory_resources(path)?
        } else {
            read_resource_file(path)?
        };
        self.attach_blorb(&bytes)?;
        self.external_resources.as_mut().unwrap().path = Some(path.to_path_buf());
        Ok(())
    }

    pub fn resource_path(&self) -> Option<&Path> {
        match &self.external_resources {
            Some(external) => external.path.as_deref(),
            None if self.container.is_some() => self.path.as_deref(),
            None => None,
        }
    }

    pub(crate) fn validated_session_story(&self) -> Result<Self, StoryError> {
        let title = self
            .external_resources
            .as_ref()
            .map_or(&self.title, |external| &external.original_title);
        let mut story = Self::from_bytes(
            self.container.as_deref().unwrap_or(&self.image),
            Some(title),
        )?;
        if let Some(external) = &self.external_resources {
            story.attach_blorb(&external.bytes)?;
            story.external_resources.as_mut().unwrap().path = external.path.clone();
        }
        story.path = self.path.clone();
        Ok(story)
    }

    fn active_container(&self) -> Option<&[u8]> {
        self.external_resources
            .as_ref()
            .map(|external| external.bytes.as_slice())
            .or(self.container.as_deref())
    }

    pub fn metadata(&self) -> Metadata {
        let Some(data) = self.container_chunk(*b"IFmd") else {
            return Metadata::default();
        };
        let Ok(xml) = std::str::from_utf8(data) else {
            return Metadata::default();
        };
        let Ok(document) = roxmltree::Document::parse(xml) else {
            return Metadata::default();
        };
        let text = |name| {
            document
                .descendants()
                .find(|node| node.has_tag_name(name))
                .map(|node| {
                    node.descendants()
                        .filter(|n| n.is_text())
                        .filter_map(|n| n.text())
                        .collect::<String>()
                })
                .unwrap_or_default()
        };
        Metadata {
            title: text("title"),
            author: text("author"),
            headline: text("headline"),
            description: text("description"),
            ifid: text("ifid"),
        }
    }
    pub fn cover(&self) -> Option<&[u8]> {
        let number = read_u32(self.container_chunk(*b"Fspc")?, 0).ok()?;
        self.resource(*b"Pict", number)
    }
    pub fn resource_descriptions(&self) -> Vec<ResourceDescription> {
        let parse = || -> Option<Vec<ResourceDescription>> {
            let data = self.container_chunk(*b"RDes")?;
            let count = read_u32(data, 0).ok()?;
            let mut cursor = 4usize;
            let mut entries: Vec<ResourceDescription> = Vec::new();
            for _ in 0..count {
                let usage = read_u32(data, cursor).ok()?.to_be_bytes();
                let number = read_u32(data, cursor + 4).ok()?;
                let length = read_u32(data, cursor + 8).ok()? as usize;
                cursor = cursor.checked_add(12)?;
                let end = cursor.checked_add(length)?;
                let text = std::str::from_utf8(data.get(cursor..end)?).ok()?;
                cursor = end;
                if matches!(&usage, b"Pict" | b"Snd ")
                    && !entries
                        .iter()
                        .any(|entry| entry.usage == usage && entry.number == number)
                {
                    entries.push(ResourceDescription {
                        usage,
                        number,
                        text: text.to_owned(),
                    });
                }
            }
            (cursor == data.len()).then_some(entries)
        };
        parse().unwrap_or_default()
    }
    pub fn container_chunk(&self, tag: [u8; 4]) -> Option<&[u8]> {
        let bytes = self.active_container()?;
        blorb_chunks(bytes)
            .ok()?
            .into_iter()
            .find(|(start, _)| bytes[*start..*start + 4] == tag)
            .map(|(start, end)| &bytes[start + 8..end])
    }
    pub fn sound_resource(&self, number: u32) -> Option<&[u8]> {
        self.resource_file(*b"Snd ", number)
    }
    pub fn resource_file(&self, usage: [u8; 4], number: u32) -> Option<&[u8]> {
        let &(start, end) = self.resources.get(&(u32::from_be_bytes(usage), number))?;
        let bytes = self.active_container()?;
        if &bytes[start - 8..start - 4] == b"FORM" {
            bytes.get(start - 8..end)
        } else {
            bytes.get(start..end)
        }
    }
    pub fn resource_type(&self, usage: [u8; 4], number: u32) -> Option<[u8; 4]> {
        let &(start, _) = self.resources.get(&(u32::from_be_bytes(usage), number))?;
        self.active_container()?
            .get(start - 8..start - 4)?
            .try_into()
            .ok()
    }

    pub fn resource(&self, usage: [u8; 4], number: u32) -> Option<&[u8]> {
        let (start, end) = self.resources.get(&(u32::from_be_bytes(usage), number))?;
        self.active_container()?.get(*start..*end)
    }
}

type ResourceIndex = HashMap<(u32, u32), (usize, usize)>;

fn blorb_chunks(bytes: &[u8]) -> Result<Vec<(usize, usize)>, StoryError> {
    if bytes.len() < 12 || &bytes[..4] != b"FORM" || &bytes[8..12] != IFRS_MAGIC {
        return Err(StoryError::InvalidBlorb("missing IFRS form type"));
    }
    let end = (read_u32(bytes, 4)? as usize)
        .checked_add(8)
        .filter(|end| *end <= bytes.len() && *end >= 12)
        .ok_or(StoryError::InvalidBlorb("FORM length exceeds file size"))?;
    let mut chunks = Vec::new();
    let mut cursor = 12usize;
    while cursor < end {
        if cursor + 8 > end {
            return Err(StoryError::InvalidBlorb("truncated chunk header"));
        }
        let length = read_u32(bytes, cursor + 4)? as usize;
        let next = cursor
            .checked_add(8)
            .and_then(|v| v.checked_add(length))
            .filter(|v| *v <= end)
            .ok_or(StoryError::InvalidBlorb("chunk length exceeds FORM"))?;
        chunks.push((cursor, next));
        cursor = next + length % 2;
    }
    if cursor != end {
        return Err(StoryError::InvalidBlorb("missing chunk padding"));
    }
    Ok(chunks)
}

fn parse_resource_index(bytes: &[u8]) -> Result<ResourceIndex, StoryError> {
    let chunks = blorb_chunks(bytes)?;
    if chunks
        .first()
        .is_none_or(|(start, _)| &bytes[*start..*start + 4] != b"RIdx")
    {
        return Err(StoryError::InvalidBlorb("first chunk must be RIdx"));
    }
    let mut resources = HashMap::new();
    let mut index_seen = false;
    for &(start, end) in &chunks {
        if &bytes[start..start + 4] != b"RIdx" {
            continue;
        }
        if index_seen {
            return Err(StoryError::InvalidBlorb("duplicate RIdx"));
        }
        index_seen = true;
        let count = read_u32(bytes, start + 8)? as usize;
        if count
            .checked_mul(12)
            .and_then(|v| v.checked_add(start + 12))
            != Some(end)
        {
            return Err(StoryError::InvalidBlorb("invalid resource count"));
        }
        for i in 0..count {
            let entry = start + 12 + i * 12;
            let usage = read_u32(bytes, entry)?;
            let number = read_u32(bytes, entry + 4)?;
            let offset = read_u32(bytes, entry + 8)? as usize;
            let &(chunk, end) = chunks.iter().find(|(chunk, _)| *chunk == offset).ok_or(
                StoryError::InvalidBlorb("resource offset is not a chunk boundary"),
            )?;
            if resources
                .insert((usage, number), (chunk + 8, end))
                .is_some()
            {
                return Err(StoryError::InvalidBlorb("duplicate resource number"));
            }
        }
    }
    Ok(resources)
}

fn extract_glul_chunk(bytes: &[u8]) -> Result<&[u8], StoryError> {
    let chunks = blorb_chunks(bytes)?;
    let resources = parse_resource_index(bytes)?;
    if let Some(&(start, end)) = resources.get(&(u32::from_be_bytes(*b"Exec"), 0)) {
        if &bytes[start - 8..start - 4] != b"GLUL" {
            return Err(StoryError::MissingExecutable);
        }
        return Ok(&bytes[start..end]);
    }
    chunks
        .into_iter()
        .find(|(start, _)| &bytes[*start..*start + 4] == b"GLUL")
        .map(|(start, end)| &bytes[start + 8..end])
        .ok_or(StoryError::MissingExecutable)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, StoryError> {
    let raw: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(StoryError::TooSmall)?
        .try_into()
        .expect("slice length checked");
    Ok(u32::from_be_bytes(raw))
}

#[derive(Debug, Error)]
pub enum StoryError {
    #[error("cannot read resource path {path}: {source}")]
    ResourceIo {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("ambiguous resource archives: {first} and {second}; select one explicitly")]
    AmbiguousResources { first: PathBuf, second: PathBuf },
    #[error("separate resources contain an executable; open that story by itself instead")]
    ResourcesContainExecutable,
    #[error("resource IFhd does not match the first 128 bytes of this Glulx story")]
    ResourceIdentityMismatch,
    #[error("unsupported loose resource filename or format: {0}")]
    UnsupportedResourceFile(PathBuf),
    #[error("story requests {0} bytes, exceeding the 256 MiB VM memory limit")]
    MemoryLimit(u32),
    #[error("cannot read story file: {0}")]
    Io(#[from] std::io::Error),
    #[error("file is too small to contain a Glulx header")]
    TooSmall,
    #[error("file is neither a Glulx executable nor a supported Blorb")]
    BadMagic,
    #[error("Glulx version {0:#010x} is not supported")]
    UnsupportedVersion(u32),
    #[error("Glulx memory or stack layout is invalid")]
    InvalidLayout,
    #[error("story is truncated: expected at least {expected} bytes, got {actual}")]
    Truncated { expected: usize, actual: usize },
    #[error("story length mismatch: expected {expected} bytes from EXTSTART, got {actual}")]
    UnexpectedLength { expected: usize, actual: usize },
    #[error("invalid start function address {0:#010x}")]
    InvalidStartFunction(u32),
    #[error("story checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    Checksum { expected: u32, actual: u32 },
    #[error("invalid Blorb: {0}")]
    InvalidBlorb(&'static str),
    #[error("Blorb does not contain a GLUL executable chunk")]
    MissingExecutable,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct ResourceDirectory(pub PathBuf);

    impl ResourceDirectory {
        pub(crate) fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "glulx-resource-test-{}-{nanos}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for ResourceDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn resource_blorb(chunks: &[(LooseKind, &[u8])]) -> Vec<u8> {
        let count = chunks
            .iter()
            .filter(|((_, usage), _)| usage.is_some())
            .count();
        let mut bytes = b"FORM\0\0\0\0IFRSRIdx".to_vec();
        bytes.extend_from_slice(&(4 + count as u32 * 12).to_be_bytes());
        bytes.extend_from_slice(&(count as u32).to_be_bytes());
        bytes.resize(24 + count * 12, 0);
        let mut index = 24;
        for ((tag, usage), data) in chunks {
            if let Some((usage, number)) = usage {
                let start = bytes.len() as u32;
                bytes[index..index + 4].copy_from_slice(usage);
                bytes[index + 4..index + 8].copy_from_slice(&number.to_be_bytes());
                bytes[index + 8..index + 12].copy_from_slice(&start.to_be_bytes());
                index += 12;
            }
            bytes.extend_from_slice(tag);
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(data);
            if data.len() % 2 != 0 {
                bytes.push(0);
            }
        }
        let length = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&length.to_be_bytes());
        bytes
    }

    fn minimal_image() -> Vec<u8> {
        let mut bytes = vec![0; 0x100];
        bytes[0..4].copy_from_slice(b"Glul");
        for (offset, value) in [
            (4, 0x0003_0103u32),
            (8, 0x100),
            (12, 0x100),
            (16, 0x200),
            (20, 0x100),
            (24, 0x24),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        let checksum = (0..bytes.len())
            .step_by(4)
            .map(|offset| u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        bytes[32..36].copy_from_slice(&checksum.to_be_bytes());
        bytes
    }

    #[test]
    fn parses_valid_story_header() {
        let story = Story::from_bytes(&minimal_image(), Some("Test")).unwrap();
        assert_eq!(story.title, "Test");
        assert_eq!(story.header.version_string(), "3.1.3");
        assert_eq!(story.header.end_mem, 0x200);
    }

    #[test]
    fn extracts_glul_from_blorb() {
        let image = minimal_image();
        let form_len = 4 + 12 + 8 + image.len();
        let mut blorb = Vec::new();
        blorb.extend_from_slice(b"FORM");
        blorb.extend_from_slice(&(form_len as u32).to_be_bytes());
        blorb.extend_from_slice(b"IFRSRIdx\0\0\0\x04\0\0\0\0");
        blorb.extend_from_slice(b"GLUL");
        blorb.extend_from_slice(&(image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(&image);
        assert_eq!(Story::from_bytes(&blorb, None).unwrap().image, image);
    }

    #[test]
    fn verifies_executable_length_in_raw_and_blorb_stories() {
        let mut image = minimal_image();
        image.extend_from_slice(&[0; 256]);
        assert!(matches!(
            Story::from_bytes(&image, None),
            Err(StoryError::UnexpectedLength {
                expected: 256,
                actual: 512
            })
        ));
        let mut blorb = b"FORM".to_vec();
        blorb.extend_from_slice(&(4 + 12 + 8 + image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(b"IFRSRIdx\0\0\0\x04\0\0\0\0GLUL");
        blorb.extend_from_slice(&(image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(&image);
        assert!(matches!(
            Story::from_bytes(&blorb, None),
            Err(StoryError::UnexpectedLength {
                expected: 256,
                actual: 512
            })
        ));
    }

    #[test]
    fn indexes_picture_resources_from_blorb() {
        let image = minimal_image();
        let picture = b"test-png";
        let picture_chunk = 12 + 8 + 16 + 8 + image.len();
        let mut blorb = Vec::new();
        blorb.extend_from_slice(b"FORM");
        blorb.extend_from_slice(&0u32.to_be_bytes());
        blorb.extend_from_slice(b"IFRS");
        blorb.extend_from_slice(b"RIdx");
        blorb.extend_from_slice(&16u32.to_be_bytes());
        blorb.extend_from_slice(&1u32.to_be_bytes());
        blorb.extend_from_slice(b"Pict");
        blorb.extend_from_slice(&7u32.to_be_bytes());
        blorb.extend_from_slice(&(picture_chunk as u32).to_be_bytes());
        blorb.extend_from_slice(b"GLUL");
        blorb.extend_from_slice(&(image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(&image);
        blorb.extend_from_slice(b"PNG ");
        blorb.extend_from_slice(&(picture.len() as u32).to_be_bytes());
        blorb.extend_from_slice(picture);
        if !picture.len().is_multiple_of(2) {
            blorb.push(0);
        }
        let form_length = blorb.len() as u32 - 8;
        blorb[4..8].copy_from_slice(&form_length.to_be_bytes());

        let story = Story::from_bytes(&blorb, None).unwrap();
        assert_eq!(story.resource(*b"Pict", 7), Some(picture.as_slice()));
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut image = minimal_image();
        image[40] = 1;
        assert!(matches!(
            Story::from_bytes(&image, None),
            Err(StoryError::Checksum { .. })
        ));
    }
    #[test]
    fn rejects_invalid_index_and_truncated_trailing_chunks() {
        let image = minimal_image();
        let mut blorb = b"FORM\0\0\0\0IFRS".to_vec();
        blorb.extend_from_slice(b"GLUL");
        blorb.extend_from_slice(&(image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(&image);
        blorb.extend_from_slice(b"RIdx");
        blorb.extend_from_slice(&16u32.to_be_bytes());
        blorb.extend_from_slice(&1u32.to_be_bytes());
        blorb.extend_from_slice(b"Data");
        blorb.extend_from_slice(&1u32.to_be_bytes());
        blorb.extend_from_slice(&13u32.to_be_bytes());
        let length = (blorb.len() - 8) as u32;
        blorb[4..8].copy_from_slice(&length.to_be_bytes());
        assert!(matches!(
            Story::from_bytes(&blorb, None),
            Err(StoryError::InvalidBlorb(_))
        ));
        blorb.truncate(12 + 8 + image.len());
        blorb.extend_from_slice(b"BAD");
        let length = (blorb.len() - 8) as u32;
        blorb[4..8].copy_from_slice(&length.to_be_bytes());
        assert!(matches!(
            Story::from_bytes(&blorb, None),
            Err(StoryError::InvalidBlorb(_))
        ));
    }
    #[test]
    fn reads_ifiction_bibliography() {
        let image = minimal_image();
        let xml=b"<ifindex><story><bibliographic><title>A &amp; B</title><author>Writer</author></bibliographic><identification><ifid>TEST</ifid></identification></story></ifindex>";
        let mut blorb = b"FORM\0\0\0\0IFRS".to_vec();
        for (tag, data) in [
            (b"RIdx", &[0u8; 4][..]),
            (b"GLUL", image.as_slice()),
            (b"IFmd", xml.as_slice()),
        ] {
            blorb.extend_from_slice(tag);
            blorb.extend_from_slice(&(data.len() as u32).to_be_bytes());
            blorb.extend_from_slice(data);
            if !data.len().is_multiple_of(2) {
                blorb.push(0);
            }
        }
        let length = (blorb.len() - 8) as u32;
        blorb[4..8].copy_from_slice(&length.to_be_bytes());
        let story = Story::from_bytes(&blorb, None).unwrap();
        assert_eq!(story.title, "A & B");
        assert_eq!(story.metadata().ifid, "TEST");
        assert_eq!(story.metadata().author, "Writer");
    }

    #[test]
    fn index_must_be_first_and_resource_descriptions_keep_utf8_and_padding() {
        let image = minimal_image();
        let mut descriptions = 3u32.to_be_bytes().to_vec();
        for (usage, number, text) in [
            (b"Pict", 7u32, "A fox"),
            (b"Snd ", 2, "钟声"),
            (b"Pict", 7, "Duplicate"),
        ] {
            descriptions.extend_from_slice(usage);
            descriptions.extend_from_slice(&number.to_be_bytes());
            descriptions.extend_from_slice(&(text.len() as u32).to_be_bytes());
            descriptions.extend_from_slice(text.as_bytes());
        }
        let build = |tags: &[([u8; 4], &[u8])]| {
            let mut bytes = b"FORM\0\0\0\0IFRS".to_vec();
            for (tag, data) in tags {
                bytes.extend_from_slice(tag);
                bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
                bytes.extend_from_slice(data);
                if data.len() % 2 != 0 {
                    bytes.push(0);
                }
            }
            let len = bytes.len() as u32 - 8;
            bytes[4..8].copy_from_slice(&len.to_be_bytes());
            bytes
        };
        let tags = [
            (*b"RIdx", &[0u8; 4][..]),
            (*b"GLUL", image.as_slice()),
            (*b"RDes", descriptions.as_slice()),
        ];
        let story = Story::from_bytes(&build(&tags), None).unwrap();
        let entries = story.resource_descriptions();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "A fox");
        assert_eq!(entries[1].text, "钟声");
        assert!(Story::from_bytes(&build(&tags[1..]), None).is_err());
        assert!(Story::from_bytes(&build(&[tags[1], tags[0]]), None).is_err());
        assert!(Story::from_bytes(&build(&[tags[0], tags[0], tags[1]]), None).is_err());
        let broken = &descriptions[..descriptions.len() - 1];
        let story =
            Story::from_bytes(&build(&[tags[0], tags[1], (*b"RDes", broken)]), None).unwrap();
        assert!(story.resource_descriptions().is_empty());
    }

    #[test]
    fn resource_only_blorb_serves_all_resources_without_changing_executable() {
        let image = minimal_image();
        let xml = b"<ifindex><story><bibliographic><title>Resources</title><author>Artist</author></bibliographic></story></ifindex>";
        let description = b"\0\0\0\x01Pict\0\0\0\x01\0\0\0\x03Fox";
        let chunks = [
            ((*b"PNG ", Some((*b"Pict", 1))), &b"picture"[..]),
            ((*b"FORM", Some((*b"Snd ", 2))), &b"AIFF"[..]),
            ((*b"TEXT", Some((*b"Data", 3))), &b"text\n"[..]),
            ((*b"BINA", Some((*b"Data", 4))), &b"\0\0\0A"[..]),
            ((*b"FORM", Some((*b"Data", 5))), &b"TEST"[..]),
            ((*b"Fspc", None), &b"\0\0\0\x01"[..]),
            ((*b"RDes", None), &description[..]),
            ((*b"IFmd", None), &xml[..]),
            ((*b"IFhd", None), &image[..128]),
        ];
        let resources = resource_blorb(&chunks);
        assert!(matches!(
            Story::from_bytes(&resources, None),
            Err(StoryError::MissingExecutable)
        ));
        let mut story = Story::from_bytes(&image, Some("Executable")).unwrap();
        let header = story.header;
        story.attach_blorb(&resources).unwrap();
        assert_eq!(story.image, image);
        assert_eq!(story.header, header);
        assert!(story.container.is_none());
        assert_eq!(story.title, "Resources");
        assert_eq!(story.metadata().author, "Artist");
        assert_eq!(story.cover(), Some(&b"picture"[..]));
        assert_eq!(story.resource_descriptions()[0].text, "Fox");
        assert_eq!(story.sound_resource(2), Some(&b"FORM\0\0\0\x04AIFF"[..]));
        assert_eq!(
            story.resource_file(*b"Data", 5),
            Some(&b"FORM\0\0\0\x04TEST"[..])
        );
        let mut bundled_chunks = chunks.to_vec();
        bundled_chunks.push(((*b"GLUL", Some((*b"Exec", 0))), &image));
        let bundled = Story::from_bytes(&resource_blorb(&bundled_chunks), None).unwrap();
        for (usage, number) in [
            (*b"Pict", 1),
            (*b"Snd ", 2),
            (*b"Data", 3),
            (*b"Data", 4),
            (*b"Data", 5),
        ] {
            assert_eq!(
                story.resource_type(usage, number),
                bundled.resource_type(usage, number)
            );
            assert_eq!(
                story.resource_file(usage, number),
                bundled.resource_file(usage, number)
            );
        }
        story.attach_blorb(&resource_blorb(&[])).unwrap();
        assert_eq!(story.title, "Executable");
        assert!(story.cover().is_none());
    }

    #[test]
    fn identity_and_executable_conflicts_fail_atomically() {
        let image = minimal_image();
        let valid = resource_blorb(&[((*b"TEXT", Some((*b"Data", 1))), b"retained")]);
        let mut story = Story::from_bytes(&image, Some("Original")).unwrap();
        story.attach_blorb(&valid).unwrap();
        let original = serde_json::to_string(&story).unwrap();
        let mut wrong_identity = image[..128].to_vec();
        wrong_identity[127] ^= 1;
        for invalid in [
            resource_blorb(&[((*b"IFhd", None), &wrong_identity)]),
            resource_blorb(&[((*b"IFhd", None), &image[..13])]),
            resource_blorb(&[
                ((*b"IFhd", None), &image[..128]),
                ((*b"IFhd", None), &image[..128]),
            ]),
            resource_blorb(&[((*b"GLUL", Some((*b"Exec", 0))), &image)]),
            resource_blorb(&[((*b"GLUL", None), &image)]),
            resource_blorb(&[((*b"TEXT", Some((*b"Exec", 1))), b"unknown")]),
            resource_blorb(&[
                ((*b"TEXT", Some((*b"Data", 1))), b"first"),
                ((*b"TEXT", Some((*b"Data", 1))), b"duplicate"),
            ]),
            b"FORM\0\0\0\x04IFRS".to_vec(),
        ] {
            assert!(story.attach_blorb(&invalid).is_err());
            assert_eq!(serde_json::to_string(&story).unwrap(), original);
            assert_eq!(story.resource(*b"Data", 1), Some(&b"retained"[..]));
        }
        let bundled = resource_blorb(&[
            ((*b"GLUL", Some((*b"Exec", 0))), &image),
            ((*b"IFhd", None), &wrong_identity),
        ]);
        assert!(matches!(
            Story::from_bytes(&bundled, None),
            Err(StoryError::ResourceIdentityMismatch)
        ));
        assert!(matches!(
            story.attach_blorb(&resource_blorb(&[((*b"IFhd", None), &wrong_identity)])),
            Err(StoryError::ResourceIdentityMismatch)
        ));
    }

    #[test]
    fn discovery_has_deterministic_priority_explicit_override_and_opt_out() {
        let directory = ResourceDirectory::new();
        let path = directory.0.join("story.ulx");
        std::fs::write(&path, minimal_image()).unwrap();
        let first = directory.0.join("story.BLORB");
        let second = directory.0.join("story.blb");
        std::fs::write(
            &first,
            resource_blorb(&[((*b"TEXT", Some((*b"Data", 1))), b"first")]),
        )
        .unwrap();
        std::fs::write(
            &second,
            resource_blorb(&[((*b"TEXT", Some((*b"Data", 1))), b"second")]),
        )
        .unwrap();
        let auto = Story::open(&path).unwrap();
        assert_eq!(auto.resource_path(), Some(first.as_path()));
        assert_eq!(auto.resource(*b"Data", 1), Some(&b"first"[..]));
        let explicit =
            Story::open_with_resources(&path, ResourceSelection::Path(second.clone())).unwrap();
        assert_eq!(explicit.resource(*b"Data", 1), Some(&b"second"[..]));
        assert!(
            Story::open_with_resources(&path, ResourceSelection::None)
                .unwrap()
                .resource(*b"Data", 1)
                .is_none()
        );
        std::fs::write(&first, b"broken").unwrap();
        assert!(Story::open(&path).is_err());
        assert!(Story::open_with_resources(&path, ResourceSelection::Path(second)).is_ok());
        assert!(Story::open_with_resources(&path, ResourceSelection::None).is_ok());
        let bundled = directory.0.join("story.gblorb");
        std::fs::write(
            &bundled,
            resource_blorb(&[((*b"GLUL", Some((*b"Exec", 0))), &minimal_image())]),
        )
        .unwrap();
        assert!(Story::open(&bundled).is_ok());
        let unusual_suffix = directory.0.join("lone.blorb");
        std::fs::write(&unusual_suffix, minimal_image()).unwrap();
        assert!(
            Story::open(&unusual_suffix)
                .unwrap()
                .resource_path()
                .is_none()
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn discovery_rejects_same_priority_case_aliases() {
        let directory = ResourceDirectory::new();
        let path = directory.0.join("story.ulx");
        std::fs::write(&path, minimal_image()).unwrap();
        for name in ["story.blorb", "story.BLORB"] {
            std::fs::write(directory.0.join(name), resource_blorb(&[])).unwrap();
        }
        assert!(matches!(
            Story::open(&path),
            Err(StoryError::AmbiguousResources { .. })
        ));
        assert!(
            Story::open_with_resources(
                &path,
                ResourceSelection::Path(directory.0.join("story.blorb"))
            )
            .is_ok()
        );
    }

    #[test]
    fn explicit_loose_directory_preserves_types_form_headers_and_metadata() {
        let directory = ResourceDirectory::new();
        let image = minimal_image();
        for (name, bytes) in [
            ("PIC1.png", &b"picture"[..]),
            ("pic2.JPEG", &b"jpeg"[..]),
            ("SND3.aiff", &b"FORM\0\0\0\x04AIFF"[..]),
            ("SND4.it", &b"IMPM"[..]),
            ("DATA5.txt", "é\n".as_bytes()),
            ("DATA6.bin", &b"\0\0\0A"[..]),
            ("DATA7.form", &b"FORM\0\0\0\x04TEST"[..]),
            ("DATA8", &b"raw"[..]),
            ("FRONTIS", &b"\0\0\0\x01"[..]),
            ("IDENT", &image[..128]),
            (
                "METADATA.xml",
                &b"<ifindex><title>Loose story</title></ifindex>"[..],
            ),
            ("README.md", &b"ignored"[..]),
        ] {
            std::fs::write(directory.0.join(name), bytes).unwrap();
        }
        let mut story = Story::from_bytes(&image, None).unwrap();
        story.attach_resource_path(&directory.0).unwrap();
        assert_eq!(story.resource_path(), Some(directory.0.as_path()));
        assert_eq!(story.title, "Loose story");
        assert_eq!(story.cover(), Some(&b"picture"[..]));
        for (usage, number, tag) in [
            (*b"Pict", 2, *b"JPEG"),
            (*b"Snd ", 3, *b"FORM"),
            (*b"Snd ", 4, *b"MOD "),
            (*b"Data", 5, *b"TEXT"),
            (*b"Data", 6, *b"BINA"),
            (*b"Data", 7, *b"FORM"),
            (*b"Data", 8, *b"BINA"),
        ] {
            assert_eq!(story.resource_type(usage, number), Some(tag));
        }
        assert_eq!(story.sound_resource(3), Some(&b"FORM\0\0\0\x04AIFF"[..]));
        assert_eq!(
            story.resource_file(*b"Data", 7),
            Some(&b"FORM\0\0\0\x04TEST"[..])
        );
        let snapshot = serde_json::to_string(&story).unwrap();
        for (name, bytes) in [
            ("SND9.aiff", &b"FORM\0\0\0\x04WAVE"[..]),
            ("DATA6.txt", &b"duplicate"[..]),
            ("PIC4294967296.png", &b"overflow"[..]),
            ("STORY.ulx", &image[..]),
            ("SND9.wav", &b"unsupported"[..]),
        ] {
            let path = directory.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(story.attach_resource_path(&directory.0).is_err(), "{name}");
            assert_eq!(serde_json::to_string(&story).unwrap(), snapshot);
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(&directory.0).unwrap();
        assert_eq!(story.resource(*b"Data", 5), Some("é\n".as_bytes()));
    }
}
