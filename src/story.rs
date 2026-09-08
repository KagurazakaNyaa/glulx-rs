use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use thiserror::Error;

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

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Story {
    pub path: Option<PathBuf>,
    pub title: String,
    pub header: StoryHeader,
    pub image: Vec<u8>,
    pub container: Option<Vec<u8>>,
    #[serde(skip)]
    resources: HashMap<(u32, u32), (usize, usize)>,
}

impl Story {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoryError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let mut story = Self::from_bytes(&bytes, path.file_stem().and_then(|s| s.to_str()))?;
        story.path = Some(path.to_path_buf());
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
        let mut story = Self {
            path: None,
            title: title.unwrap_or("Untitled Glulx story").to_owned(),
            header,
            image,
            container,
            resources,
        };
        let metadata = story.metadata();
        if !metadata.title.is_empty() {
            story.title = metadata.title;
        }
        Ok(story)
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
    pub fn container_chunk(&self, tag: [u8; 4]) -> Option<&[u8]> {
        let bytes = self.container.as_ref()?;
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
        let bytes = self.container.as_ref()?;
        if &bytes[start - 8..start - 4] == b"FORM" {
            bytes.get(start - 8..end)
        } else {
            bytes.get(start..end)
        }
    }
    pub fn resource_type(&self, usage: [u8; 4], number: u32) -> Option<[u8; 4]> {
        let &(start, _) = self.resources.get(&(u32::from_be_bytes(usage), number))?;
        self.container
            .as_ref()?
            .get(start - 8..start - 4)?
            .try_into()
            .ok()
    }

    pub fn resource(&self, usage: [u8; 4], number: u32) -> Option<&[u8]> {
        let (start, end) = self.resources.get(&(u32::from_be_bytes(usage), number))?;
        self.container.as_ref()?.get(*start..*end)
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
mod tests {
    use super::*;

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
        let form_len = 4 + 8 + image.len();
        let mut blorb = Vec::new();
        blorb.extend_from_slice(b"FORM");
        blorb.extend_from_slice(&(form_len as u32).to_be_bytes());
        blorb.extend_from_slice(b"IFRS");
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
        blorb.extend_from_slice(&(4 + 8 + image.len() as u32).to_be_bytes());
        blorb.extend_from_slice(b"IFRSGLUL");
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
        for (tag, data) in [(b"GLUL", image.as_slice()), (b"IFmd", xml.as_slice())] {
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
}
