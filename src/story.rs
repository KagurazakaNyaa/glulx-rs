use std::path::{Path, PathBuf};

use thiserror::Error;

const GLUL_MAGIC: u32 = 0x476c_756c;
const FORM_MAGIC: &[u8; 4] = b"FORM";
const IFRS_MAGIC: &[u8; 4] = b"IFRS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct Story {
    pub path: Option<PathBuf>,
    pub title: String,
    pub header: StoryHeader,
    pub image: Vec<u8>,
    pub container: Option<Vec<u8>>,
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
        let (image, container) = if bytes.starts_with(FORM_MAGIC) {
            (extract_glul_chunk(bytes)?.to_vec(), Some(bytes.to_vec()))
        } else {
            (bytes.to_vec(), None)
        };
        let header = StoryHeader::parse(&image)?;
        let image = image[..header.ext_start as usize].to_vec();
        Ok(Self {
            path: None,
            title: title.unwrap_or("Untitled Glulx story").to_owned(),
            header,
            image,
            container,
        })
    }
}

fn extract_glul_chunk(bytes: &[u8]) -> Result<&[u8], StoryError> {
    if bytes.len() < 12 || &bytes[8..12] != IFRS_MAGIC {
        return Err(StoryError::InvalidBlorb("missing IFRS form type"));
    }
    let declared = read_u32(bytes, 4)? as usize + 8;
    if declared > bytes.len() {
        return Err(StoryError::InvalidBlorb("FORM length exceeds file size"));
    }
    let mut offset = 12usize;
    while offset.checked_add(8).is_some_and(|end| end <= declared) {
        let chunk_type = &bytes[offset..offset + 4];
        let length = read_u32(bytes, offset + 4)? as usize;
        let start = offset + 8;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= declared)
            .ok_or(StoryError::InvalidBlorb("chunk length exceeds FORM"))?;
        if chunk_type == b"GLUL" {
            return Ok(&bytes[start..end]);
        }
        offset = end + (length & 1);
    }
    Err(StoryError::MissingExecutable)
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
    fn rejects_checksum_mismatch() {
        let mut image = minimal_image();
        image[40] = 1;
        assert!(matches!(
            Story::from_bytes(&image, None),
            Err(StoryError::Checksum { .. })
        ));
    }
}
