use super::*;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FileIdentity {
    Native(Arc<same_file::Handle>),
    Path(PathBuf),
}
impl FileIdentity {
    fn for_path(path: &Path, handle: Option<&File>) -> Self {
        let identity = if let Some(handle) = handle {
            handle.try_clone().and_then(same_file::Handle::from_file)
        } else {
            same_file::Handle::from_path(path)
        };
        if let Ok(identity) = identity {
            return Self::Native(Arc::new(identity));
        }
        Self::Path(std::fs::canonicalize(path).unwrap_or_else(|_| {
            if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir().unwrap_or_default().join(path)
            }
        }))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct FileContents {
    bytes: Vec<u8>,
    dirty: bool,
    // Retaining the open descriptor also preserves writes through renamed or
    // unlinked files. Each Glk stream keeps its own logical mark separately.
    #[serde(skip)]
    writable_file: Option<File>,
}

#[derive(Debug, Clone)]
struct SharedFile(Arc<Mutex<FileContents>>);
impl SharedFile {
    fn new(bytes: Vec<u8>, writable_file: Option<File>) -> Self {
        Self(Arc::new(Mutex::new(FileContents {
            bytes,
            dirty: false,
            writable_file,
        })))
    }
    fn lock(&self) -> MutexGuard<'_, FileContents> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
}
impl serde::Serialize for SharedFile {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.lock().serialize(serializer)
    }
}
impl<'de> serde::Deserialize<'de> for SharedFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(Arc::new(Mutex::new(serde::Deserialize::deserialize(
            deserializer,
        )?))))
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct FileRef {
    path: PathBuf,
    usage: u32,
    rock: u32,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct FileStream {
    path: Option<PathBuf>,
    // `data` only reads desktop snapshots from the decoded-character format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    data: Vec<u32>,
    #[serde(default)]
    bytes: Option<Vec<u8>>,
    #[serde(default)]
    shared: Option<SharedFile>,
    #[serde(skip)]
    identity: Option<FileIdentity>,
    position: u32,
    read_count: u32,
    write_count: u32,
    mode: u32,
    unicode: bool,
    text: bool,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct FileRequest {
    pub usage: u32,
    pub mode: u32,
    pub rock: u32,
    pub destination: Destination,
    #[serde(default)]
    pub selected_path: Option<PathBuf>,
    #[serde(default)]
    pub notice: Option<String>,
}

impl FileStream {
    fn from_bytes(bytes: &[u8], unicode: bool, text: bool) -> Self {
        Self {
            path: None,
            data: Vec::new(),
            bytes: Some(bytes.to_vec()),
            shared: None,
            identity: None,
            position: 0,
            read_count: 0,
            write_count: 0,
            mode: 2,
            unicode,
            text,
        }
    }
    fn encode_values(&self, values: &[u32]) -> Vec<u8> {
        if self.unicode && self.text {
            values
                .iter()
                .map(|&c| char::from_u32(c).unwrap_or('\u{fffd}'))
                .collect::<String>()
                .into_bytes()
        } else if self.unicode {
            values.iter().flat_map(|v| v.to_be_bytes()).collect()
        } else {
            values
                .iter()
                .map(|&v| if v > 255 { b'?' } else { v as u8 })
                .collect()
        }
    }
    fn byte_position(&self) -> u32 {
        if self.bytes.is_some() || self.shared.is_some() {
            return self.position;
        }
        // Older sessions measured a decoded character index. Convert only
        // when loading them; new snapshots always retain raw encoded bytes.
        self.encode_values(&self.data[..(self.position as usize).min(self.data.len())])
            .len() as u32
    }
    fn materialize_bytes(&mut self) {
        if self.bytes.is_none() && self.shared.is_none() {
            self.position = self.byte_position();
            self.bytes = Some(self.encode_values(&self.data));
            self.data.clear();
        }
    }
    fn position_scale(&self) -> u32 {
        // Binary Unicode files expose word offsets. UTF-8 text files and
        // resources expose native byte offsets so get/set marks round-trip.
        if self.unicode && !self.text && self.path.is_some() {
            4
        } else {
            1
        }
    }
    fn with_bytes<T>(&self, read: impl FnOnce(&[u8]) -> T) -> T {
        if let Some(shared) = &self.shared {
            read(&shared.lock().bytes)
        } else {
            read(self.bytes.as_deref().unwrap_or(&[]))
        }
    }
    fn read_value(&mut self) -> Option<u32> {
        self.materialize_bytes();
        let offset = self.position as usize;
        let decoded = self.with_bytes(|bytes| {
            let first = *bytes.get(offset)?;
            if !self.unicode {
                return Some((first as u32, 1));
            }
            if !self.text {
                return bytes
                    .get(offset..offset + 4)
                    .map(|raw| (u32::from_be_bytes(raw.try_into().unwrap()), 4));
            }
            let length = match first {
                0..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => 1,
            };
            Some(
                match bytes
                    .get(offset..offset + length)
                    .and_then(|raw| std::str::from_utf8(raw).ok())
                {
                    Some(text) => (text.chars().next().unwrap() as u32, length as u32),
                    None => (0xfffd, 1),
                },
            )
        });
        let Some((value, length)) = decoded else {
            // Consume an incomplete binary Unicode word without inventing a
            // character or counting a successful read.
            if self.unicode && !self.text {
                let length = self.with_bytes(|bytes| bytes.len() as u32);
                if self.position < length {
                    self.position = length;
                }
            }
            return None;
        };
        self.position += length;
        self.read_count = self.read_count.wrapping_add(1);
        Some(value)
    }
    fn write_value(&mut self, value: u32) -> Result<(), VmError> {
        self.materialize_bytes();
        let encoded = self.encode_values(&[value]);
        let start = self.position as usize;
        let end = start
            .checked_add(encoded.len())
            .filter(|end| *end <= u32::MAX as usize)
            .ok_or(VmError::StreamIo)?;
        let replace = |bytes: &mut Vec<u8>| -> Result<(), VmError> {
            if end > bytes.len() {
                bytes
                    .try_reserve(end - bytes.len())
                    .map_err(|_| VmError::StreamIo)?;
                bytes.resize(end, 0);
            }
            bytes[start..end].copy_from_slice(&encoded);
            Ok(())
        };
        if let Some(shared) = &self.shared {
            let mut contents = shared.lock();
            replace(&mut contents.bytes)?;
            contents.dirty = true;
        } else {
            replace(self.bytes.as_mut().unwrap())?;
        }
        self.position = end as u32;
        self.write_count = self.write_count.wrapping_add(1);
        Ok(())
    }
    fn flush(&self) -> Result<(), VmError> {
        // Resource streams and legacy snapshots have no shared writable file.
        // validate_session migrates legacy files before they resume running.
        let (Some(shared), Some(path)) = (&self.shared, &self.path) else {
            return Ok(());
        };
        let mut contents = shared.lock();
        if !contents.dirty {
            return Ok(());
        }
        if contents.writable_file.is_none() {
            contents.writable_file = Some(
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)
                    .map_err(|_| VmError::StreamIo)?,
            );
        }
        let FileContents {
            bytes,
            writable_file,
            ..
        } = &mut *contents;
        let file = writable_file.as_mut().unwrap();
        file.seek(SeekFrom::Start(0))
            .map_err(|_| VmError::StreamIo)?;
        file.write_all(bytes).map_err(|_| VmError::StreamIo)?;
        file.set_len(bytes.len() as u64)
            .map_err(|_| VmError::StreamIo)?;
        file.flush().map_err(|_| VmError::StreamIo)?;
        contents.dirty = false;
        Ok(())
    }
}

impl Vm {
    pub fn flush_streams(&self) -> Result<(), VmError> {
        for stream in self.glk_streams.values() {
            if let GlkStreamTarget::File(file) = &stream.target {
                file.flush()?;
            }
        }
        Ok(())
    }

    fn add_fileref(&mut self, path: PathBuf, usage: u32, rock: u32) -> u32 {
        let id = self.next_fileref;
        self.next_fileref += 1;
        self.filerefs.insert(id, FileRef { path, usage, rock });
        id
    }
    pub fn file_prompt_message(&self) -> String {
        let Some(request) = &self.file_request else {
            return String::new();
        };
        if let Some(path) = &request.selected_path {
            let action = if request.mode == 1 {
                "Replace"
            } else {
                "Modify"
            };
            return format!(
                "{action} existing file {}? Enter yes or no.",
                path.display()
            );
        }
        if let Some(notice) = &request.notice {
            return format!("{notice} Enter another file path (empty cancels).");
        }
        if request.mode == 2 {
            "Existing file path (empty cancels):".to_owned()
        } else {
            "File path (empty cancels):".to_owned()
        }
    }
    pub(super) fn provide_file(&mut self, name: &str) -> Result<(), VmError> {
        let mut request = self.file_request.take().ok_or(VmError::UnexpectedInput)?;
        let result = if let Some(path) = request.selected_path.take() {
            match name.trim().to_ascii_lowercase().as_str() {
                "yes" | "y" => self.add_fileref(path, request.usage, request.rock),
                "no" | "n" | "" => 0,
                _ => {
                    request.selected_path = Some(path);
                    self.file_request = Some(request);
                    return Ok(());
                }
            }
        } else if name.is_empty() {
            0
        } else {
            let path = PathBuf::from(name);
            if path.is_dir() || (request.mode == 2 && !path.is_file()) {
                request.notice = Some(format!("Not an existing file: {}.", path.display()));
                self.file_request = Some(request);
                return Ok(());
            }
            if request.mode != 2 && path.exists() {
                request.selected_path = Some(path);
                self.file_request = Some(request);
                return Ok(());
            }
            self.add_fileref(path, request.usage, request.rock)
        };
        self.store_destination(&request.destination, result, Width::Word)?;
        self.state = RunState::Running;
        Ok(())
    }
    pub(super) fn fileref_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        Ok(match selector {
            0x0060 => {
                let path = std::env::temp_dir().join(format!(
                    "glulx-{}-{:08x}.tmp",
                    std::process::id(),
                    unpredictable_seed()
                ));
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(_) => self.add_fileref(path, arg(0), arg(1)),
                    Err(_) => 0,
                }
            }
            0x0061 => {
                let name = self.glk_byte_string(arg(1))?;
                let mut name: String = name
                    .chars()
                    .filter(|c| {
                        !matches!(c, '/' | '\\' | ':' | '<' | '>' | '|' | '?' | '*' | '"')
                            && !c.is_control()
                    })
                    .collect();
                if let Some(dot) = name.find('.') {
                    name.truncate(dot);
                }
                if name.is_empty() {
                    name.push_str("null");
                }
                name.push_str(match arg(0) & 15 {
                    1 => ".glksave",
                    2 => ".txt",
                    3 => ".txt",
                    _ => ".glkdata",
                });
                let directory = self
                    .story
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .unwrap_or(std::path::Path::new("."));
                self.add_fileref(directory.join(name), arg(0), arg(2))
            }
            0x0063 => {
                self.filerefs.remove(&arg(0));
                0
            }
            0x0064 => {
                let next = self
                    .filerefs
                    .range(arg(0).saturating_add(1)..)
                    .next()
                    .map(|(&id, f)| (id, f.rock))
                    .unwrap_or((0, 0));
                self.write_glk_reference(arg(1), next.1)?;
                next.0
            }
            0x0065 => self.filerefs.get(&arg(0)).map_or(0, |f| f.rock),
            0x0066 => {
                if let Some(f) = self.filerefs.get(&arg(0)) {
                    let _ = std::fs::remove_file(&f.path);
                }
                0
            }
            0x0067 => self
                .filerefs
                .get(&arg(0))
                .map_or(0, |f| u32::from(f.path.is_file())),
            0x0068 => self
                .filerefs
                .get(&arg(1))
                .cloned()
                .map_or(0, |f| self.add_fileref(f.path, arg(0), arg(2))),
            _ => 0,
        })
    }
    pub(super) fn write_glk_reference(&mut self, address: u32, value: u32) -> Result<(), VmError> {
        match address {
            0 => Ok(()),
            u32::MAX => self.stack.push_u32(value),
            _ => self.memory.write32(address, value),
        }
    }
    fn add_file_stream(&mut self, file: FileStream, rock: u32) -> u32 {
        let id = self.glk_next_stream;
        self.glk_next_stream += 1;
        self.glk_streams.insert(
            id,
            GlkStream {
                rock,
                target: GlkStreamTarget::File(file),
            },
        );
        id
    }
    pub(super) fn open_file_stream(&mut self, args: &[u32], unicode: bool) -> u32 {
        let Some(reference) = args.first().and_then(|id| self.filerefs.get(id)) else {
            return 0;
        };
        let mode = args.get(1).copied().unwrap_or(0);
        if !matches!(mode, 1 | 2 | 3 | 5) {
            return 0;
        }
        // WriteAppend is opened without the OS append flag: Glk allows the
        // caller to seek elsewhere and overwrite after the initial end mark.
        let Ok(mut handle) = OpenOptions::new()
            .read(mode != 1)
            .write(mode != 2)
            .create(mode != 2)
            .truncate(mode == 1)
            .open(&reference.path)
        else {
            return 0;
        };
        let identity = FileIdentity::for_path(&reference.path, Some(&handle));
        let existing = self
            .glk_streams
            .values()
            .find_map(|stream| match &stream.target {
                GlkStreamTarget::File(file) if file.identity.as_ref() == Some(&identity) => {
                    file.shared.clone()
                }
                _ => None,
            });
        let shared = if let Some(shared) = existing {
            {
                let mut contents = shared.lock();
                if mode == 1 {
                    contents.bytes.clear();
                    contents.dirty = false;
                }
                if mode != 2 {
                    contents.writable_file = Some(handle);
                }
            }
            shared
        } else {
            let mut bytes = Vec::new();
            if mode != 1 && handle.read_to_end(&mut bytes).is_err() {
                return 0;
            }
            SharedFile::new(bytes, if mode == 2 { None } else { Some(handle) })
        };
        let mut file = FileStream::from_bytes(&[], unicode, reference.usage & 0x100 != 0);
        file.path = Some(reference.path.clone());
        file.mode = mode;
        file.identity = Some(identity);
        if mode == 5 {
            file.position = shared.lock().bytes.len() as u32;
        }
        file.bytes = None;
        file.shared = Some(shared);
        self.add_file_stream(file, args.get(2).copied().unwrap_or(0))
    }

    pub(super) fn restore_file_streams(&mut self) -> Result<(), VmError> {
        let mut files: std::collections::HashMap<FileIdentity, SharedFile> =
            std::collections::HashMap::new();
        for stream in self.glk_streams.values_mut() {
            let GlkStreamTarget::File(file) = &mut stream.target else {
                continue;
            };
            let Some(path) = file.path.clone() else {
                continue;
            };
            let identity = FileIdentity::for_path(&path, None);
            file.materialize_bytes();
            let shared = file.shared.take().unwrap_or_else(|| {
                // Earlier desktop saves flushed files before serializing, but
                // kept separate, potentially stale copies in each stream.
                // Rejoin them from the actual file without marking it dirty.
                let bytes =
                    std::fs::read(&path).unwrap_or_else(|_| file.bytes.take().unwrap_or_default());
                SharedFile::new(bytes, None)
            });
            if let Some(existing) = files.get(&identity) {
                if !Arc::ptr_eq(&existing.0, &shared.0) {
                    let expected = existing.lock();
                    let incoming = shared.lock();
                    if expected.bytes != incoming.bytes || expected.dirty != incoming.dirty {
                        return Err(VmError::InvalidSave);
                    }
                }
                file.shared = Some(existing.clone());
            } else {
                files.insert(identity.clone(), shared.clone());
                file.shared = Some(shared);
            }
            file.bytes = None;
            file.data.clear();
            file.identity = Some(identity);
        }
        Ok(())
    }
    pub(super) fn open_resource_stream(&mut self, args: &[u32], unicode: bool) -> u32 {
        let Some(data) = args
            .first()
            .and_then(|n| self.story.resource_file(*b"Data", *n))
        else {
            return 0;
        };
        let kind = self.story.resource_type(*b"Data", args[0]);
        if !matches!(kind,Some(tag) if matches!(&tag,b"TEXT"|b"BINA"|b"FORM")) {
            return 0;
        }
        let stream = FileStream::from_bytes(data, unicode, kind == Some(*b"TEXT"));
        self.add_file_stream(stream, args.get(1).copied().unwrap_or(0))
    }
    pub(super) fn close_stream(&mut self, id: u32) -> (u32, u32) {
        if self
            .glk_streams
            .get(&id)
            .is_some_and(|s| matches!(s.target, GlkStreamTarget::Window(_)))
        {
            return (0, 0);
        }
        if self.glk_current_stream == id {
            self.glk_current_stream = 0;
        }
        for window in self.glk_windows.values_mut() {
            if window.echo_stream == id {
                window.echo_stream = 0;
            }
        }
        match self.glk_streams.remove(&id).map(|s| s.target) {
            Some(GlkStreamTarget::Memory {
                read_count,
                write_count,
                ..
            }) => (read_count, write_count),
            Some(GlkStreamTarget::File(f)) => {
                let _ = f.flush();
                (f.read_count, f.write_count)
            }
            _ => (0, 0),
        }
    }
    pub(super) fn write_stream_value(&mut self, id: u32, value: u32) -> Result<(), VmError> {
        match &mut self
            .glk_streams
            .get_mut(&id)
            .ok_or(VmError::StreamIo)?
            .target
        {
            GlkStreamTarget::Memory {
                address,
                length,
                position,
                extent,
                write_count,
                mode,
                unicode,
                ..
            } => {
                if *mode == 2 {
                    return Err(VmError::StreamIo);
                }
                *write_count = write_count.wrapping_add(1);
                if *position < *length {
                    if *address != 0 {
                        if *unicode {
                            self.memory.write32(*address + *position * 4, value)?;
                        } else {
                            self.memory.write8(
                                *address + *position,
                                if value > 255 { b'?' } else { value as u8 },
                            )?;
                        }
                    }
                    *position += 1;
                    *extent = Some(extent.unwrap_or(*length).max(*position));
                }
                Ok(())
            }
            GlkStreamTarget::File(file) => {
                if file.mode == 2 {
                    return Err(VmError::StreamIo);
                }
                file.write_value(value)
            }
            _ => Err(VmError::StreamIo),
        }
    }
    pub(super) fn read_stream_value(&mut self, id: u32) -> Result<Option<u32>, VmError> {
        match &mut self
            .glk_streams
            .get_mut(&id)
            .ok_or(VmError::StreamIo)?
            .target
        {
            GlkStreamTarget::Memory {
                address,
                length,
                position,
                read_count,
                mode,
                unicode,
                ..
            } => {
                if *mode == 1 || *mode == 5 {
                    return Err(VmError::StreamIo);
                }
                if *position >= *length || *address == 0 {
                    return Ok(None);
                }
                let value = if *unicode {
                    self.memory.read32(*address + *position * 4)?
                } else {
                    self.memory.read8(*address + *position)? as u32
                };
                *position += 1;
                *read_count = read_count.wrapping_add(1);
                Ok(Some(value))
            }
            GlkStreamTarget::File(file) => {
                if file.mode == 1 || file.mode == 5 {
                    return Err(VmError::StreamIo);
                }
                Ok(file.read_value())
            }
            _ => Err(VmError::StreamIo),
        }
    }
    pub(super) fn seek_stream(&mut self, args: &[u32]) {
        let Some(target) = args.first().and_then(|id| self.glk_streams.get_mut(id)) else {
            return;
        };
        let (position, length, scale) = match &mut target.target {
            GlkStreamTarget::Memory {
                position,
                length,
                extent,
                ..
            } => (position, extent.unwrap_or(*length), 1),
            GlkStreamTarget::File(f) => {
                f.materialize_bytes();
                let scale = f.position_scale();
                let length = f.with_bytes(|bytes| bytes.len() as u32);
                (&mut f.position, length, scale)
            }
            _ => return,
        };
        let base = match args.get(2) {
            Some(0) => 0,
            Some(1) => *position,
            Some(2) => length,
            _ => return,
        };
        *position = (base as i64 + args.get(1).copied().unwrap_or(0) as i32 as i64 * scale as i64)
            .clamp(0, length as i64) as u32;
    }
    pub(super) fn stream_position(&self, id: u32) -> u32 {
        match self.glk_streams.get(&id).map(|s| &s.target) {
            Some(GlkStreamTarget::Memory { position, .. }) => *position,
            Some(GlkStreamTarget::File(f)) => f.byte_position() / f.position_scale(),
            _ => 0,
        }
    }
    pub(super) fn read_stream_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let stream = args.first().copied().unwrap_or(0);
        let unicode = selector >= 0x0130;
        let kind = if unicode {
            match selector & 15 {
                1 => 2,
                2 => 1,
                n => n,
            }
        } else {
            selector & 15
        };
        if kind == 0 {
            return Ok(self
                .read_stream_value(stream)?
                .map(|c| if !unicode && c > 255 { b'?' as u32 } else { c })
                .unwrap_or(u32::MAX));
        }
        let address = args.get(1).copied().unwrap_or(0);
        let length = args.get(2).copied().unwrap_or(0);
        let maximum = if kind == 1 {
            length.saturating_sub(1)
        } else {
            length
        };
        let mut count = 0;
        while count < maximum {
            let Some(value) = self.read_stream_value(stream)? else {
                break;
            };
            if unicode {
                self.memory.write32(address + count * 4, value)?;
            } else {
                self.memory.write8(
                    address + count,
                    if value > 255 { b'?' } else { value as u8 },
                )?;
            }
            count += 1;
            if kind == 1 && value == 10 {
                break;
            }
        }
        if kind == 1 && length != 0 {
            if unicode {
                self.memory.write32(address + count * 4, 0)?;
            } else {
                self.memory.write8(address + count, 0)?;
            }
        }
        Ok(count)
    }
    pub(super) fn write_save_stream(&mut self, id: u32, data: &[u8]) -> Result<(), VmError> {
        for &byte in data {
            let before = self.stream_position(id);
            self.write_stream_value(id, byte as u32)?;
            if self.stream_position(id) == before {
                return Err(VmError::StreamIo);
            }
        }
        if let Some(GlkStream {
            target: GlkStreamTarget::File(f),
            ..
        }) = self.glk_streams.get(&id)
        {
            f.flush()?;
        }
        Ok(())
    }
    pub(super) fn read_save_stream(&mut self, id: u32) -> Result<Vec<u8>, VmError> {
        let mut data = Vec::new();
        for _ in 0..8 {
            data.push(
                self.read_stream_value(id)?
                    .filter(|v| *v <= 255)
                    .ok_or(VmError::InvalidSave)? as u8,
            );
        }
        if &data[..4] != b"FORM" {
            return Err(VmError::InvalidSave);
        }
        let size = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
        for _ in 0..size {
            data.push(
                self.read_stream_value(id)?
                    .filter(|v| *v <= 255)
                    .ok_or(VmError::InvalidSave)? as u8,
            );
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::{image_with_program, push_glk_arguments};

    fn vm() -> Vm {
        Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap()
    }
    fn glk(vm: &mut Vm, selector: u32, args: &[u32]) -> u32 {
        push_glk_arguments(vm, args);
        vm.glk(selector, args.len() as u32, Destination::Stack)
            .unwrap();
        vm.stack.pop_u32().unwrap()
    }

    #[test]
    fn closing_an_echo_stream_detaches_all_windows_and_current_stream() {
        let mut vm = vm();
        let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
        let other = glk(&mut vm, 0x23, &[window, 0x21, 50, 3, 0]);
        let stream = glk(&mut vm, 0x43, &[0x100, 32, 1, 0]);
        for id in [window, other] {
            glk(&mut vm, 0x2d, &[id, stream]);
        }
        glk(&mut vm, 0x47, &[stream]);
        vm.close_stream(stream);
        assert_eq!(glk(&mut vm, 0x48, &[]), 0);
        for id in [window, other] {
            assert_eq!(glk(&mut vm, 0x2e, &[id]), 0);
        }
        vm.glk_write_char(vm.glk_windows[&window].stream, 'x');
        assert_eq!(vm.take_output(), "x");
    }

    #[test]
    fn output_memory_seek_end_tracks_written_extent_and_null_stream_counts() {
        let mut vm = vm();
        let stream = glk(&mut vm, 0x43, &[0x100, 8, 1, 0]);
        vm.seek_stream(&[stream, 0, 2]);
        assert_eq!(vm.stream_position(stream), 0);
        vm.write_stream_value(stream, 65).unwrap();
        vm.write_stream_value(stream, 66).unwrap();
        vm.seek_stream(&[stream, -1i32 as u32, 2]);
        vm.write_stream_value(stream, 67).unwrap();
        assert_eq!(
            (0..2)
                .map(|i| vm.memory.read8(0x100 + i).unwrap())
                .collect::<Vec<_>>(),
            b"AC"
        );
        assert_eq!(vm.close_stream(stream), (0, 3));
        let stream = glk(&mut vm, 0x43, &[0, u32::MAX, 3, 0]);
        for _ in 0..4 {
            vm.write_stream_value(stream, 65).unwrap();
        }
        assert_eq!(vm.stream_position(stream), 0);
        assert_eq!(vm.read_stream_value(stream).unwrap(), None);
        assert_eq!(vm.close_stream(stream), (0, 4));
    }

    #[test]
    fn readonly_memory_stream_can_read_rom() {
        let mut vm = vm();
        let stream = glk(&mut vm, 0x43, &[1, 3, 2, 0]);
        assert_ne!(stream, 0);
        assert_eq!(glk(&mut vm, 0x92, &[stream, 0x100, 3]), 3);
        assert_eq!(
            (0..3)
                .map(|i| vm.memory.read8(0x100 + i).unwrap())
                .collect::<Vec<_>>(),
            b"lul"
        );
    }

    #[test]
    fn file_modes_preserve_append_seek_and_unicode_encodings() {
        let mut vm = vm();
        let path = std::env::temp_dir().join(format!(
            "glulx-stream-contract-{:08x}",
            unpredictable_seed()
        ));
        let reference = vm.add_fileref(path.clone(), 0x100, 0);
        let stream = vm.open_file_stream(&[reference, 1, 0], true);
        for value in ['é', '中', '\n'] {
            vm.write_stream_value(stream, value as u32).unwrap();
        }
        assert_eq!(vm.close_stream(stream), (0, 3));
        assert_eq!(std::fs::read(&path).unwrap(), "é中\n".as_bytes());
        let stream = vm.open_file_stream(&[reference, 5, 0], true);
        vm.write_stream_value(stream, 'z' as u32).unwrap();
        vm.seek_stream(&[stream, 0, 0]);
        vm.write_stream_value(stream, 'a' as u32).unwrap();
        vm.close_stream(stream);
        assert_eq!(std::fs::read(&path).unwrap(), b"a\xa9\xe4\xb8\xad\nz");
        // ReadWrite keeps the rest of an existing file and starts at zero.
        let binary = vm.add_fileref(path.clone(), 0, 0);
        let stream = vm.open_file_stream(&[binary, 3, 0], false);
        assert_eq!(vm.read_stream_value(stream).unwrap(), Some(b'a' as u32));
        vm.seek_stream(&[stream, 0, 0]);
        vm.write_stream_value(stream, b'A' as u32).unwrap();
        vm.close_stream(stream);
        assert_eq!(std::fs::read(&path).unwrap(), b"A\xa9\xe4\xb8\xad\nz");
        let stream = vm.open_file_stream(&[binary, 1, 0], true);
        vm.write_stream_value(stream, 0x4e2d).unwrap();
        vm.close_stream(stream);
        assert_eq!(std::fs::read(&path).unwrap(), [0, 0, 0x4e, 0x2d]);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn unicode_resource_marks_and_utf8_file_marks_round_trip_encoded_offsets() {
        let mut vm = vm();
        let resource = vm.add_file_stream(FileStream::from_bytes("é中z".as_bytes(), true, true), 0);
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some('é' as u32));
        assert_eq!(vm.stream_position(resource), 2);
        let mark = vm.stream_position(resource);
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some('中' as u32));
        vm.seek_stream(&[resource, mark, 0]);
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some('中' as u32));
        vm.seek_stream(&[resource, -1i32 as u32, 2]);
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some('z' as u32));

        let resource = vm.add_file_stream(
            FileStream::from_bytes(&[0, 0, 0, 65, 0, 0, 0, 66], true, false),
            0,
        );
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some(65));
        assert_eq!(vm.stream_position(resource), 4);
        vm.seek_stream(&[resource, 4, 0]);
        assert_eq!(vm.read_stream_value(resource).unwrap(), Some(66));

        let mut file = FileStream::from_bytes("é中z".as_bytes(), true, true);
        file.path = Some(PathBuf::from("unused-readonly"));
        let file = vm.add_file_stream(file, 0);
        assert_eq!(vm.read_stream_value(file).unwrap(), Some('é' as u32));
        let mark = vm.stream_position(file);
        vm.read_stream_value(file).unwrap();
        vm.seek_stream(&[file, mark, 0]);
        assert_eq!(vm.read_stream_value(file).unwrap(), Some('中' as u32));
    }

    #[test]
    fn legacy_decoded_session_streams_migrate_without_losing_position() {
        let mut file = FileStream::from_bytes(&[], true, true);
        file.bytes = None;
        file.data = vec!['é' as u32, '中' as u32, 'z' as u32];
        file.position = 1;
        assert_eq!(file.byte_position(), 2);
        assert_eq!(file.read_value(), Some('中' as u32));
        assert_eq!(file.position, 5);
        assert_eq!(file.bytes.unwrap(), "é中z".as_bytes());
        assert!(file.data.is_empty());
    }

    #[test]
    fn prompted_files_require_existing_reads_and_confirm_modification() {
        let mut vm = vm();
        let path = std::env::temp_dir().join(format!(
            "glulx-prompt-contract-{:08x}",
            unpredictable_seed()
        ));
        let name = path.to_str().unwrap();
        push_glk_arguments(&mut vm, &[1, 2, 0]);
        vm.glk(0x62, 3, Destination::Memory(0x100)).unwrap();
        vm.provide_file(name).unwrap();
        assert_eq!(vm.state, RunState::WaitingForFile);
        assert!(vm.file_prompt_message().contains("Not an existing file"));
        vm.provide_file("").unwrap();
        assert_eq!(vm.memory.read32(0x100).unwrap(), 0);
        std::fs::write(&path, b"original").unwrap();
        for mode in [1, 3, 5] {
            push_glk_arguments(&mut vm, &[1, mode, 0]);
            vm.glk(0x62, 3, Destination::Memory(0x100)).unwrap();
            vm.provide_file(name).unwrap();
            assert_eq!(vm.state, RunState::WaitingForFile);
            assert!(vm.file_prompt_message().contains(if mode == 1 {
                "Replace"
            } else {
                "Modify"
            }));
            vm.provide_file("no").unwrap();
            assert_eq!(vm.memory.read32(0x100).unwrap(), 0);
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
        }
        push_glk_arguments(&mut vm, &[1, 1, 0]);
        vm.glk(0x62, 3, Destination::Memory(0x100)).unwrap();
        vm.provide_file(name).unwrap();
        vm.provide_file("yes").unwrap();
        assert_eq!(vm.state, RunState::Running);
        let reference = vm.memory.read32(0x100).unwrap();
        assert_ne!(reference, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let stream = vm.open_file_stream(&[reference, 1, 0], false);
        assert_ne!(stream, 0);
        assert!(std::fs::read(&path).unwrap().is_empty());
        vm.close_stream(stream);
        std::fs::remove_file(path).unwrap();
    }
    fn file_vm() -> (Vm, PathBuf, u32) {
        let mut vm = vm();
        let path =
            std::env::temp_dir().join(format!("glulx-shared-stream-{:08x}", unpredictable_seed()));
        std::fs::write(&path, b"ABC").unwrap();
        let reference = vm.add_fileref(path.clone(), 0, 0);
        (vm, path, reference)
    }

    #[test]
    fn simultaneous_file_streams_read_current_data_and_keep_independent_marks() {
        let (mut vm, path, reference) = file_vm();
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        let second = vm.open_file_stream(&[reference, 3, 0], false);
        vm.write_stream_value(first, b'X' as u32).unwrap();
        assert_eq!(vm.stream_position(first), 1);
        assert_eq!(vm.stream_position(second), 0);
        vm.seek_stream(&[second, 0, 0]);
        assert_eq!(vm.read_stream_value(second).unwrap(), Some(b'X' as u32));
        vm.write_stream_value(second, b'Y' as u32).unwrap();
        assert_eq!(vm.stream_position(first), 1);
        assert_eq!(vm.stream_position(second), 2);
        vm.write_stream_value(first, b'Z' as u32).unwrap();
        let reader = vm.open_file_stream(&[reference, 2, 0], false);
        assert_eq!(vm.read_stream_value(reader).unwrap(), Some(b'X' as u32));
        assert_eq!(vm.read_stream_value(reader).unwrap(), Some(b'Z' as u32));
        assert_eq!(vm.close_stream(first), (0, 2));
        assert_eq!(std::fs::read(&path).unwrap(), b"XZC");
        assert_eq!(vm.close_stream(second), (1, 1));
        assert_eq!(std::fs::read(&path).unwrap(), b"XZC");
        vm.close_stream(reader);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn clean_stream_close_and_flush_do_not_overwrite_a_newer_disk_file() {
        let (mut vm, path, reference) = file_vm();
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        let second = vm.open_file_stream(&[reference, 3, 0], false);
        vm.write_stream_value(first, b'X' as u32).unwrap();
        vm.close_stream(first);
        assert_eq!(std::fs::read(&path).unwrap(), b"XBC");
        std::fs::write(&path, b"newer external contents").unwrap();
        vm.flush_streams().unwrap();
        vm.close_stream(second);
        assert_eq!(std::fs::read(&path).unwrap(), b"newer external contents");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn append_and_truncate_share_contents_without_moving_other_stream_marks() {
        let (mut vm, path, reference) = file_vm();
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        vm.seek_stream(&[first, 2, 0]);
        let append = vm.open_file_stream(&[reference, 5, 0], false);
        assert_eq!(vm.stream_position(append), 3);
        vm.write_stream_value(append, b'D' as u32).unwrap();
        assert_eq!(vm.stream_position(first), 2);
        assert_eq!(vm.read_stream_value(first).unwrap(), Some(b'C' as u32));
        assert_eq!(vm.read_stream_value(first).unwrap(), Some(b'D' as u32));
        let truncate = vm.open_file_stream(&[reference, 1, 0], false);
        assert!(std::fs::read(&path).unwrap().is_empty());
        assert_eq!(vm.stream_position(first), 4);
        assert_eq!(vm.read_stream_value(first).unwrap(), None);
        vm.write_stream_value(truncate, b'Q' as u32).unwrap();
        vm.seek_stream(&[first, 0, 0]);
        assert_eq!(vm.read_stream_value(first).unwrap(), Some(b'Q' as u32));
        for stream in [append, truncate, first] {
            vm.close_stream(stream);
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"Q");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn byte_and_unicode_handles_share_bytes_and_use_their_own_mark_units() {
        let (mut vm, path, reference) = file_vm();
        let unicode = vm.open_file_stream(&[reference, 1, 0], true);
        vm.write_stream_value(unicode, '中' as u32).unwrap();
        let byte = vm.open_file_stream(&[reference, 3, 0], false);
        assert_eq!(vm.read_stream_value(byte).unwrap(), Some(0));
        assert_eq!(vm.read_stream_value(byte).unwrap(), Some(0));
        assert_eq!(vm.read_stream_value(byte).unwrap(), Some(0x4e));
        assert_eq!(
            (vm.stream_position(unicode), vm.stream_position(byte)),
            (1, 3)
        );
        let truncate = vm.open_file_stream(&[reference, 1, 0], false);
        assert_eq!(vm.stream_position(unicode), 1);
        vm.close_stream(truncate);
        // The Unicode output handle retains word offset 1 after truncation.
        vm.write_stream_value(unicode, 65).unwrap();
        vm.close_stream(byte);
        vm.close_stream(unicode);
        assert_eq!(std::fs::read(&path).unwrap(), [0, 0, 0, 0, 0, 0, 0, 65]);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn aliases_share_an_inode_but_replaced_paths_do_not_retarget_open_streams() {
        let (mut vm, path, reference) = file_vm();
        let alias = path.with_extension("alias");
        std::fs::hard_link(&path, &alias).unwrap();
        let other_reference = vm.add_fileref(alias.clone(), 0, 0);
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        let other = vm.open_file_stream(&[other_reference, 3, 0], false);
        vm.write_stream_value(first, b'X' as u32).unwrap();
        assert_eq!(vm.read_stream_value(other).unwrap(), Some(b'X' as u32));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"NEW").unwrap();
        let replacement = vm.open_file_stream(&[reference, 3, 0], false);
        assert_eq!(
            vm.read_stream_value(replacement).unwrap(),
            Some(b'N' as u32)
        );
        vm.close_stream(first);
        vm.close_stream(other);
        vm.close_stream(replacement);
        assert_eq!(std::fs::read(&path).unwrap(), b"NEW");
        assert_eq!(std::fs::read(&alias).unwrap(), b"XBC");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(alias).unwrap();
    }

    #[test]
    fn session_restoration_rejoins_shared_contents_and_preserves_unflushed_writes() {
        let (mut vm, path, reference) = file_vm();
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        let second = vm.open_file_stream(&[reference, 3, 0], false);
        vm.write_stream_value(first, b'X' as u32).unwrap();
        vm.seek_stream(&[second, 2, 0]);
        let serialized = serde_json::to_string(&vm).unwrap();
        drop(vm);
        assert_eq!(std::fs::read(&path).unwrap(), b"ABC");
        let mut restored = serde_json::from_str::<Vm>(&serialized)
            .unwrap()
            .validate_session()
            .unwrap();
        assert_eq!(
            (
                restored.stream_position(first),
                restored.stream_position(second)
            ),
            (1, 2)
        );
        restored.write_stream_value(second, b'Z' as u32).unwrap();
        assert_eq!(
            restored.read_stream_value(first).unwrap(),
            Some(b'B' as u32)
        );
        assert_eq!(
            restored.read_stream_value(first).unwrap(),
            Some(b'Z' as u32)
        );
        restored.close_stream(first);
        restored.close_stream(second);
        assert_eq!(std::fs::read(&path).unwrap(), b"XBZ");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_independent_session_caches_rejoin_disk_without_overwriting_it() {
        let (mut vm, path, reference) = file_vm();
        let first = vm.open_file_stream(&[reference, 3, 0], false);
        let second = vm.open_file_stream(&[reference, 3, 0], false);
        let mut serialized = serde_json::to_value(&vm).unwrap();
        for stream in serialized["glk_streams"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            if let Some(file) = stream["target"].get_mut("File") {
                file.as_object_mut().unwrap().remove("shared");
                file["bytes"] = serde_json::json!([65, 66, 67]);
            }
        }
        drop(vm);
        std::fs::write(&path, b"XYZ").unwrap();
        let mut restored = serde_json::from_value::<Vm>(serialized)
            .unwrap()
            .validate_session()
            .unwrap();
        assert_eq!(
            restored.read_stream_value(first).unwrap(),
            Some(b'X' as u32)
        );
        assert_eq!(
            restored.read_stream_value(second).unwrap(),
            Some(b'X' as u32)
        );
        restored.close_stream(first);
        restored.close_stream(second);
        assert_eq!(std::fs::read(&path).unwrap(), b"XYZ");
        std::fs::remove_file(path).unwrap();
    }
}
