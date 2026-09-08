use super::*;
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct FileRef {
    path: PathBuf,
    usage: u32,
    rock: u32,
}
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub(super) struct FileStream {
    path: Option<PathBuf>,
    data: Vec<u32>,
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
}

impl FileStream {
    fn from_bytes(bytes: &[u8], unicode: bool, text: bool) -> Self {
        let data = if unicode && text {
            String::from_utf8_lossy(bytes)
                .chars()
                .map(|c| c as u32)
                .collect()
        } else if unicode {
            bytes
                .chunks_exact(4)
                .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                .collect()
        } else {
            bytes.iter().map(|b| *b as u32).collect()
        };
        Self {
            path: None,
            data,
            position: 0,
            read_count: 0,
            write_count: 0,
            mode: 2,
            unicode,
            text,
        }
    }
    fn flush(&self) -> Result<(), VmError> {
        if self.mode == 2 {
            return Ok(());
        }
        let Some(path) = &self.path else {
            return Ok(());
        };
        let data: Vec<u8> = if self.unicode && self.text {
            self.data
                .iter()
                .map(|&c| char::from_u32(c).unwrap_or('\u{fffd}'))
                .collect::<String>()
                .into_bytes()
        } else if self.unicode {
            self.data.iter().flat_map(|v| v.to_be_bytes()).collect()
        } else {
            self.data.iter().map(|&v| v as u8).collect()
        };
        std::fs::write(path, data).map_err(|_| VmError::StreamIo)
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
    pub(super) fn provide_file(&mut self, name: &str) -> Result<(), VmError> {
        let request = self.file_request.take().ok_or(VmError::UnexpectedInput)?;
        let result = if name.is_empty() {
            0
        } else {
            self.add_fileref(PathBuf::from(name), request.usage, request.rock)
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
        let bytes = if mode == 1 {
            Vec::new()
        } else {
            match std::fs::read(&reference.path) {
                Ok(bytes) => bytes,
                Err(e) if mode != 2 && e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(_) => return 0,
            }
        };
        let mut file = FileStream::from_bytes(&bytes, unicode, reference.usage & 0x100 != 0);
        file.path = Some(reference.path.clone());
        file.mode = mode;
        if mode == 5 {
            file.position = file.data.len() as u32;
        }
        if file.flush().is_err() {
            return 0;
        }
        self.add_file_stream(file, args.get(2).copied().unwrap_or(0))
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
                }
                Ok(())
            }
            GlkStreamTarget::File(file) => {
                if file.mode == 2 {
                    return Err(VmError::StreamIo);
                }
                let position = file.position as usize;
                if position >= file.data.len() {
                    file.data
                        .try_reserve(position + 1 - file.data.len())
                        .map_err(|_| VmError::StreamIo)?;
                    file.data.resize(position + 1, 0);
                }
                file.data[position] = if !file.unicode && value > 255 {
                    b'?' as u32
                } else {
                    value
                };
                file.position += 1;
                file.write_count = file.write_count.wrapping_add(1);
                Ok(())
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
                let value = file.data.get(file.position as usize).copied();
                if value.is_some() {
                    file.position += 1;
                    file.read_count = file.read_count.wrapping_add(1);
                }
                Ok(value)
            }
            _ => Err(VmError::StreamIo),
        }
    }
    pub(super) fn seek_stream(&mut self, args: &[u32]) {
        let Some(target) = args.first().and_then(|id| self.glk_streams.get_mut(id)) else {
            return;
        };
        let (position, length, clamp) = match &mut target.target {
            GlkStreamTarget::Memory {
                position, length, ..
            } => (position, *length, true),
            GlkStreamTarget::File(f) => (&mut f.position, f.data.len() as u32, false),
            _ => return,
        };
        let base = match args.get(2) {
            Some(0) => 0,
            Some(1) => *position,
            Some(2) => length,
            _ => return,
        };
        let result = (base as i64 + args.get(1).copied().unwrap_or(0) as i32 as i64)
            .clamp(0, u32::MAX as i64) as u32;
        *position = if clamp { result.min(length) } else { result };
    }
    pub(super) fn stream_position(&self, id: u32) -> u32 {
        match self.glk_streams.get(&id).map(|s| &s.target) {
            Some(GlkStreamTarget::Memory { position, .. }) => *position,
            Some(GlkStreamTarget::File(f)) => f.position,
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
