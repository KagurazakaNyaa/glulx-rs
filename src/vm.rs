mod acceleration;
mod datetime;
mod events;
mod grid;
mod presentation;
mod save;
mod session;
mod sound;
mod streams;
mod strings;
mod unicode;
mod windows;
use events::Request;
pub use grid::{GRID_CELL_HEIGHT, GRID_CELL_WIDTH, GRID_FONT_SIZE, GridCell};
pub(crate) use presentation::WindowDescriptor;
pub use presentation::{BufferImage, ResolvedStyle, TextAppearance, TextRun, WindowView};
#[cfg(test)]
mod conformance;

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use thiserror::Error;

use crate::{Story, memory::Memory};

/// A graphical host reports the characters represented by its active fonts.
/// Available fonts belong to the host and are not part of a saved VM state.
pub type GlyphSupport = std::sync::Arc<dyn Fn(char) -> bool + Send + Sync>;
pub type TextMetrics = std::sync::Arc<dyn Fn(ResolvedStyle) -> [u32; 2] + Send + Sync>;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    WaitingForLine,
    WaitingForChar,
    WaitingForFile,
    WaitingForEvent,
    Halted,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRequest {
    Line { maximum_length: u32 },
    Character,
    File { writing: bool },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ImageRequest {
    /// A host may provide a predecoded image; desktop snapshots retain `data`
    /// instead, so this cache is never serialized.
    #[serde(skip)]
    pub decoded: Option<std::sync::Arc<image::RgbaImage>>,
    pub window: u32,
    pub resource: u32,
    pub data: Vec<u8>,
    pub position: [i32; 2],
    pub requested_size: Option<[u32; 2]>,
    #[serde(default)]
    pub hyperlink: u32,
    pub canvas_size: [u32; 2],
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum GraphicsRequest {
    Draw(ImageRequest),
    Fill {
        window: u32,
        color: u32,
        // Origins are signed; width and height retain their unsigned u32 bit
        // patterns. Keeping this shape also reads existing desktop sessions.
        rect: [i32; 4],
        canvas_size: [u32; 2],
    },
    Resize {
        window: u32,
        background: u32,
        canvas_size: [u32; 2],
    },
    Clear {
        window: u32,
        color: u32,
        canvas_size: [u32; 2],
    },
    Close {
        window: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextBufferEvent {
    Text { window: u32, text: String },
    Clear { window: u32 },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
enum Width {
    Byte,
    Short,
    Word,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
enum Operand {
    Zero,
    Constant(u32),
    Memory(u32),
    Stack,
    Local(u32),
}

const DECODE_CACHE_SIZE: usize = 2048;

#[derive(Clone, Copy)]
struct DecodeEntry {
    address: u32,
    next_pc: u32,
    opcode: u32,
    operands: [Operand; 8],
    valid: bool,
}

impl Default for DecodeEntry {
    fn default() -> Self {
        Self {
            address: 0,
            next_pc: 0,
            opcode: 0,
            operands: [const { Operand::Zero }; 8],
            valid: false,
        }
    }
}

struct DecodeCache {
    entries: Box<[DecodeEntry]>,
}

impl Default for DecodeCache {
    fn default() -> Self {
        Self {
            entries: vec![DecodeEntry::default(); DECODE_CACHE_SIZE].into_boxed_slice(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
enum Destination {
    Discard,
    Memory(u32),
    Stack,
    Local(u32),
}

const INLINE_ARGUMENTS: usize = 64;

struct ArgumentBuffer {
    inline: [u32; INLINE_ARGUMENTS],
    heap: Option<Vec<u32>>,
    length: usize,
}

impl ArgumentBuffer {
    fn as_slice(&self) -> &[u32] {
        match &self.heap {
            Some(values) => values,
            None => &self.inline[..self.length],
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct LineRequest {
    buffer: u32,
    max_len: u32,
    unicode: bool,
    initial: String,
    echo: bool,
    #[serde(default)]
    terminators: Vec<u32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct PendingSelect {
    event_address: u32,
    destination: Destination,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct UndoState {
    // Legacy desktop sessions serialized a full Memory here. New snapshots
    // retain only changed 256-byte pages; validation migrates the old form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    memory: Option<Memory>,
    #[serde(default)]
    memory_len: u32,
    #[serde(default)]
    memory_pages: BTreeMap<u32, crate::memory::MemoryPage>,
    stack: Stack,
    pc: u32,
    destination: Destination,
    heap_next: u32,
    heap_blocks: BTreeMap<u32, u32>,
}

impl UndoState {
    fn byte_len(&self) -> usize {
        self.memory.as_ref().map_or_else(
            || {
                self.memory_pages
                    .len()
                    .saturating_mul(256)
                    .saturating_add(self.memory_pages.len().saturating_mul(8))
            },
            Memory::snapshot_byte_len,
        ) + self.stack.bytes.len()
            + self.heap_blocks.len() * 8
    }
}

const WINTYPE_TEXT_BUFFER: u32 = 3;
const WINTYPE_TEXT_GRID: u32 = 4;
const WINTYPE_GRAPHICS: u32 = 5;
const MAX_TEXT_BUFFER_CHARS: usize = 131_072;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct GlkWindow {
    rock: u32,
    kind: u32,
    stream: u32,
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    grid: Vec<char>,
    #[serde(default)]
    grid_styles: Vec<u32>,
    #[serde(default)]
    grid_hyperlinks: Vec<u32>,
    background_color: u32,
    parent: u32,
    children: Option<[u32; 2]>,
    // Physical child order is fixed when a split is created. None identifies
    // older desktop sessions, whose order is inferred once from their layout.
    #[serde(default)]
    children_reversed: Option<bool>,
    method: u32,
    split_size: u32,
    key: u32,
    echo_stream: u32,
    write_count: u32,
    rect: [u32; 4],
    #[serde(default)]
    content_revision: u64,
    runs: Vec<TextRun>,
    #[serde(default)]
    text_chars: usize,
    style: u32,
    hyperlink: u32,
    hints: BTreeMap<(u32, u32), u32>,
    echo_line: bool,
    terminators: Vec<u32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct GlkStream {
    rock: u32,
    target: GlkStreamTarget,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
enum GlkStreamTarget {
    Window(u32),
    File(streams::FileStream),
    Memory {
        address: u32,
        length: u32,
        position: u32,
        #[serde(default)]
        extent: Option<u32>,
        write_count: u32,
        read_count: u32,
        mode: u32,
        unicode: bool,
    },
}

impl GlkWindow {
    fn new(rock: u32, kind: u32, stream: u32, width: u32, height: u32) -> Self {
        let grid = if kind == WINTYPE_TEXT_GRID {
            vec![' '; width.saturating_mul(height) as usize]
        } else {
            Vec::new()
        };
        Self {
            rock,
            kind,
            stream,
            width,
            height,
            cursor_x: 0,
            cursor_y: 0,
            grid_styles: vec![0; grid.len()],
            grid_hyperlinks: vec![0; grid.len()],
            grid,
            background_color: 0x00ff_ffff,
            parent: 0,
            children: None,
            children_reversed: None,
            method: 0,
            split_size: 0,
            key: 0,
            echo_stream: 0,
            write_count: 0,
            rect: [0; 4],
            content_revision: 0,
            runs: Vec::new(),
            text_chars: 0,
            style: 0,
            hyperlink: 0,
            hints: BTreeMap::new(),
            echo_line: true,
            terminators: Vec::new(),
        }
    }

    fn trim_text_history(&mut self) {
        if self.kind != WINTYPE_TEXT_BUFFER || self.text_chars <= MAX_TEXT_BUFFER_CHARS {
            return;
        }
        let mut excess = self.text_chars - MAX_TEXT_BUFFER_CHARS;
        while excess != 0 {
            let Some(first) = self.runs.first_mut() else {
                break;
            };
            let length = first.text.chars().count();
            if length == 0 {
                self.runs.remove(0);
                continue;
            }
            if excess >= length {
                self.text_chars -= length;
                excess -= length;
                self.runs.remove(0);
            } else {
                first.text = first.text.chars().skip(excess).collect();
                self.text_chars -= excess;
                excess = 0;
            }
        }
    }

    fn recount_text_chars(&mut self) {
        self.text_chars = self.runs.iter().map(|run| run.text.chars().count()).sum();
    }

    fn grid_text(&self) -> String {
        self.grid
            .chunks(self.width.max(1) as usize)
            .map(|line| line.iter().collect::<String>().trim_end().to_owned())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct Stack {
    bytes: Vec<u8>,
    frame_ptr: u32,
    maximum: u32,
}

impl Stack {
    fn new(maximum: u32) -> Self {
        Self {
            bytes: Vec::with_capacity(maximum.min(1024 * 1024) as usize),
            frame_ptr: 0,
            maximum,
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.frame_ptr = 0;
    }

    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }

    fn push_u32(&mut self, value: u32) -> Result<(), VmError> {
        if self
            .len()
            .checked_add(4)
            .is_none_or(|end| end > self.maximum)
        {
            return Err(VmError::StackOverflow);
        }
        self.bytes.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    fn pop_u32(&mut self) -> Result<u32, VmError> {
        let floor = self.frame_end()?;
        if self.len() < floor + 4 {
            return Err(VmError::StackUnderflow);
        }
        self.pop_raw_u32()
    }

    fn pop_raw_u32(&mut self) -> Result<u32, VmError> {
        if self.bytes.len() < 4 {
            return Err(VmError::StackUnderflow);
        }
        let start = self.bytes.len() - 4;
        let value = u32::from_be_bytes(self.bytes[start..].try_into().expect("length checked"));
        self.bytes.truncate(start);
        Ok(value)
    }

    fn frame_end(&self) -> Result<u32, VmError> {
        if self.bytes.is_empty() {
            return Ok(0);
        }
        Ok(self.frame_ptr + self.read_raw_u32(self.frame_ptr)?)
    }

    fn local_address(&self, offset: u32) -> Result<u32, VmError> {
        let locals_pos = self.read_raw_u32(self.frame_ptr + 4)?;
        let address = self
            .frame_ptr
            .checked_add(locals_pos)
            .and_then(|base| base.checked_add(offset))
            .ok_or(VmError::InvalidLocal(offset))?;
        if address >= self.frame_end()? {
            return Err(VmError::InvalidLocal(offset));
        }
        Ok(address)
    }

    fn checked_local_address(&self, offset: u32, width: Width) -> Result<u32, VmError> {
        let address = self.local_address(offset)?;
        let size = match width {
            Width::Byte => 1,
            Width::Short => 2,
            Width::Word => 4,
        };
        if address
            .checked_add(size)
            .is_none_or(|end| end > self.frame_end().unwrap_or(0))
        {
            return Err(VmError::InvalidLocal(offset));
        }
        Ok(address)
    }

    fn read_local(&self, offset: u32, width: Width) -> Result<u32, VmError> {
        let address = self.checked_local_address(offset, width)?;
        match width {
            Width::Byte => Ok(self.read_raw(address, 1)?[0] as u32),
            Width::Short => Ok(u16::from_be_bytes(
                self.read_raw(address, 2)?
                    .try_into()
                    .expect("length checked"),
            ) as u32),
            Width::Word => self.read_raw_u32(address),
        }
    }

    fn write_local(&mut self, offset: u32, value: u32, width: Width) -> Result<(), VmError> {
        let address = self.checked_local_address(offset, width)? as usize;
        match width {
            Width::Byte => self.bytes[address] = value as u8,
            Width::Short => {
                self.bytes[address..address + 2].copy_from_slice(&(value as u16).to_be_bytes())
            }
            Width::Word => self.bytes[address..address + 4].copy_from_slice(&value.to_be_bytes()),
        }
        Ok(())
    }

    fn read_raw_u32(&self, address: u32) -> Result<u32, VmError> {
        Ok(u32::from_be_bytes(
            self.read_raw(address, 4)?
                .try_into()
                .expect("length checked"),
        ))
    }

    fn read_raw(&self, address: u32, length: u32) -> Result<&[u8], VmError> {
        let end = address
            .checked_add(length)
            .filter(|end| *end <= self.len())
            .ok_or(VmError::StackUnderflow)?;
        Ok(&self.bytes[address as usize..end as usize])
    }

    fn truncate(&mut self, length: u32) -> Result<(), VmError> {
        if length > self.len() {
            return Err(VmError::StackUnderflow);
        }
        self.bytes.truncate(length as usize);
        Ok(())
    }

    fn peek(&self, depth: u32) -> Result<u32, VmError> {
        let address = self
            .len()
            .checked_sub(depth.saturating_add(1).saturating_mul(4))
            .filter(|address| *address >= self.frame_end().unwrap_or(u32::MAX))
            .ok_or(VmError::StackUnderflow)?;
        self.read_raw_u32(address)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Vm {
    #[serde(skip)]
    resource_limits: crate::memory::ResourceLimits,
    #[serde(skip)]
    glyph_support: Option<GlyphSupport>,
    #[serde(skip)]
    text_metrics: Option<TextMetrics>,
    #[serde(default)]
    acceleration: acceleration::Acceleration,
    #[serde(default)]
    accelerated_return: Option<u32>,
    #[serde(default)]
    text_appearance: TextAppearance,
    #[serde(skip)]
    image_info: BTreeMap<u32, Option<[u32; 2]>>,
    #[serde(skip)]
    decoded_cache: DecodeCache,
    #[serde(skip)]
    decode_cache_hits: u64,
    #[serde(skip)]
    decode_cache_misses: u64,
    #[serde(skip)]
    presentation_revision: u64,
    #[serde(skip)]
    presentation_pending: bool,
    #[serde(skip)]
    instructions_executed: u64,
    #[serde(skip)]
    poll_calls: u64,
    #[serde(skip)]
    poll_yields: u64,
    graphical_host: bool,
    #[serde(skip)]
    terminal_host: bool,
    #[serde(skip)]
    audio: sound::AudioDevice,
    channels: BTreeMap<u32, sound::Channel>,
    next_channel: u32,
    story: Story,
    memory: Memory,
    stack: Stack,
    pc: u32,
    state: RunState,
    io_system: u32,
    io_rock: u32,
    output: String,
    #[serde(skip)]
    text_buffer_events: Option<Vec<TextBufferEvent>>,
    requests: BTreeMap<u32, Request>,
    events: std::collections::VecDeque<[u32; 4]>,
    #[serde(skip)]
    timer: Option<(std::time::Duration, std::time::Instant)>,
    pending_select: Option<PendingSelect>,
    random_state: u32,
    unsupported_glk: HashSet<u32>,
    protection: Option<(u32, u32)>,
    undo: std::collections::VecDeque<UndoState>,
    glk_windows: BTreeMap<u32, GlkWindow>,
    glk_streams: BTreeMap<u32, GlkStream>,
    filerefs: BTreeMap<u32, streams::FileRef>,
    next_fileref: u32,
    file_request: Option<streams::FileRequest>,
    style_hints: BTreeMap<(u32, u32, u32), u32>,
    mouse_requests: HashSet<u32>,
    hyperlink_requests: HashSet<u32>,
    viewport_size: [u32; 2],
    glk_root: u32,
    glk_current_stream: u32,
    glk_next_window: u32,
    glk_next_stream: u32,
    input_window: u32,
    graphics: Vec<GraphicsRequest>,
    heap_next: u32,
    heap_blocks: BTreeMap<u32, u32>,
}

impl Vm {
    pub fn resource_limits(&self) -> crate::memory::ResourceLimits {
        self.resource_limits
    }

    pub fn set_resource_limits(&mut self, limits: crate::memory::ResourceLimits) {
        self.resource_limits = limits.normalized();
        self.image_info.clear();
        self.trim_undo(0);
    }

    fn undo_cost(&self) -> usize {
        self.undo.iter().map(UndoState::byte_len).sum()
    }

    fn trim_undo(&mut self, incoming: usize) {
        let budget = crate::memory::ResourceLimits::bytes(self.resource_limits.undo_mib);
        while !self.undo.is_empty()
            && ((self.undo.len() >= 16 && incoming != 0)
                || self.undo_cost().saturating_add(incoming) > budget)
        {
            self.undo.pop_front();
        }
    }

    /// Apply a host policy to the live memory and every retained undo snapshot.
    pub fn set_memory_limit(&mut self, maximum: u32) -> Result<(), VmError> {
        let required = self
            .undo
            .iter()
            .map(|undo| undo.memory.as_ref().map_or(undo.memory_len, Memory::len))
            .chain(std::iter::once(self.memory.len()))
            .max()
            .unwrap();
        if required > maximum {
            return Err(VmError::MemoryLimit { required, maximum });
        }
        self.memory.set_maximum(maximum)?;
        for undo in &mut self.undo {
            if let Some(memory) = &mut undo.memory {
                memory.set_maximum(maximum)?;
            } else if undo.memory_len > maximum {
                return Err(VmError::MemoryLimit {
                    required: undo.memory_len,
                    maximum,
                });
            }
        }
        Ok(())
    }

    pub fn new(story: Story) -> Result<Self, VmError> {
        Self::new_with_memory_limit(story, crate::memory::MAX_MEMORY_SIZE)
    }

    /// Maximum game address-space size in bytes; excludes stack, undo and host resources.
    pub fn new_with_memory_limit(story: Story, maximum: u32) -> Result<Self, VmError> {
        let memory = Memory::new_with_limit(&story, maximum)?;
        let heap_next = memory.len();
        let stack_size = story.header.stack_size;
        let start_func = story.header.start_func;
        let mut vm = Self {
            resource_limits: crate::memory::ResourceLimits::default(),
            glyph_support: None,
            text_metrics: None,
            acceleration: acceleration::Acceleration::default(),
            accelerated_return: None,
            text_appearance: TextAppearance::default(),
            image_info: BTreeMap::new(),
            decoded_cache: DecodeCache::default(),
            decode_cache_hits: 0,
            decode_cache_misses: 0,
            presentation_revision: 0,
            presentation_pending: false,
            instructions_executed: 0,
            poll_calls: 0,
            poll_yields: 0,
            graphical_host: true,
            terminal_host: false,
            audio: sound::AudioDevice::default(),
            channels: BTreeMap::new(),
            next_channel: 1,
            story,
            memory,
            stack: Stack::new(stack_size),
            pc: 0,
            state: RunState::Running,
            io_system: 0,
            io_rock: 0,
            output: String::new(),
            text_buffer_events: None,
            requests: BTreeMap::new(),
            events: std::collections::VecDeque::new(),
            timer: None,
            pending_select: None,
            random_state: unpredictable_seed(),
            unsupported_glk: HashSet::new(),
            protection: None,
            undo: std::collections::VecDeque::new(),
            glk_windows: BTreeMap::new(),
            glk_streams: BTreeMap::new(),
            filerefs: BTreeMap::new(),
            next_fileref: 1,
            file_request: None,
            style_hints: BTreeMap::new(),
            mouse_requests: HashSet::new(),
            hyperlink_requests: HashSet::new(),
            viewport_size: [640, 480],
            glk_root: 0,
            glk_current_stream: 0,
            glk_next_window: 1,
            glk_next_stream: 1,
            input_window: 0,
            graphics: Vec::new(),
            heap_next,
            heap_blocks: BTreeMap::new(),
        };
        vm.enter_function(start_func, &[])?;
        Ok(vm)
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn pc(&self) -> u32 {
        self.pc
    }

    pub fn input_request(&self) -> Option<InputRequest> {
        match self.state {
            RunState::WaitingForLine => match self.requests.get(&self.input_window) {
                Some(Request::Line(_)) => Some(InputRequest::Line {
                    maximum_length: self.line_input_max_len(),
                }),
                _ => None,
            },
            RunState::WaitingForChar => Some(InputRequest::Character),
            RunState::WaitingForFile => self.file_request.as_ref().map(|r| InputRequest::File {
                writing: r.mode != 2,
            }),
            _ => None,
        }
    }

    /// The editor remains present while a timer/Arrange handler runs with an
    /// outstanding request. `input_request` still controls event submission.
    pub fn pending_input_request(&self) -> Option<InputRequest> {
        if matches!(self.state, RunState::Halted | RunState::WaitingForFile) {
            return self.input_request();
        }
        match self.requests.get(&self.input_window) {
            Some(Request::Line(_)) => Some(InputRequest::Line {
                maximum_length: self.line_input_max_len(),
            }),
            Some(Request::Character { .. }) => Some(InputRequest::Character),
            None => None,
        }
    }

    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.output)
    }

    pub(crate) fn enable_text_buffer_events(&mut self) {
        self.text_buffer_events.get_or_insert_with(Vec::new);
    }

    pub(crate) fn disable_text_buffer_events(&mut self) {
        self.text_buffer_events = None;
    }

    pub(crate) fn take_text_buffer_events(&mut self) -> Vec<TextBufferEvent> {
        self.text_buffer_events
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    pub fn status_text(&self) -> String {
        self.glk_windows
            .values()
            .filter(|window| window.kind == WINTYPE_TEXT_GRID)
            .map(GlkWindow::grid_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    pub fn take_graphics(&mut self) -> Vec<GraphicsRequest> {
        std::mem::take(&mut self.graphics)
    }

    pub fn run_steps(&mut self, budget: usize) -> Result<RunState, VmError> {
        self.run_steps_until(budget, false)
    }

    pub(crate) fn presentation_revision(&self) -> u64 {
        self.presentation_revision
    }

    pub(crate) fn run_presentation_steps(&mut self, budget: usize) -> Result<RunState, VmError> {
        self.run_steps_until(budget, true)
    }

    fn run_steps_until(
        &mut self,
        budget: usize,
        yield_at_presentation: bool,
    ) -> Result<RunState, VmError> {
        let revision = self.presentation_revision;
        self.poll_events()?;
        for _ in 0..budget {
            if self.state != RunState::Running
                || (yield_at_presentation && self.presentation_revision != revision)
            {
                break;
            }
            self.instructions_executed = self.instructions_executed.wrapping_add(1);
            self.step()?;
        }
        Ok(self.state)
    }

    pub(crate) fn diagnostic_summary(&self) -> String {
        format!(
            "vm={:?} pc={:#x} viewport={:?} windows={} requests={} events={:?} select={} graphics={} instructions={} polls={} poll_yields={} decode_hits={} decode_misses={}",
            self.state,
            self.pc,
            self.viewport_size,
            self.glk_windows.len(),
            self.requests.len(),
            self.events.iter().map(|event| event[0]).collect::<Vec<_>>(),
            self.pending_select.is_some(),
            self.graphics.len(),
            self.instructions_executed,
            self.poll_calls,
            self.poll_yields,
            self.decode_cache_hits,
            self.decode_cache_misses
        )
    }

    pub fn provide_input(&mut self, text: &str) -> Result<(), VmError> {
        if self.state == RunState::WaitingForFile {
            return self.provide_file(text);
        }
        self.provide_window_input(self.input_window, text)
    }

    pub fn restart(&mut self) -> Result<(), VmError> {
        self.presentation_revision = 0;
        self.presentation_pending = true;
        if let Some(events) = &mut self.text_buffer_events {
            events.clear();
        }
        self.accelerated_return = None;
        self.memory.restart(self.protection);
        self.stack.clear();
        self.pc = 0;
        self.state = RunState::Running;
        self.pending_select = None;
        self.input_window = 0;
        self.heap_next = self.memory.len();
        self.heap_blocks.clear();
        self.enter_function(self.story.header.start_func, &[])
    }

    pub fn stop(&mut self) {
        let _ = self.flush_streams();
        self.state = RunState::Halted;
    }

    fn step(&mut self) -> Result<(), VmError> {
        if let Some(value) = self.accelerated_return.take() {
            return self.return_from_function(value);
        }
        let instruction_address = self.pc;
        let (opcode, operands) = self.fetch_decoded(instruction_address)?;
        macro_rules! load {
            ($index:expr) => {
                self.load_operand(&operands[$index], Width::Word)?
            };
            ($index:expr, $width:expr) => {
                self.load_operand(&operands[$index], $width)?
            };
        }
        macro_rules! store {
            ($index:expr, $value:expr) => {
                self.store_operand(&operands[$index], $value, Width::Word)?
            };
            ($index:expr, $value:expr, $width:expr) => {
                self.store_operand(&operands[$index], $value, $width)?
            };
        }

        match opcode {
            0x00 => {}
            0x10 => {
                let value = load!(0).wrapping_add(load!(1));
                store!(2, value);
            }
            0x11 => {
                let value = load!(0).wrapping_sub(load!(1));
                store!(2, value);
            }
            0x12 => {
                let value = load!(0).wrapping_mul(load!(1));
                store!(2, value);
            }
            0x13 => {
                let (a, b) = (load!(0) as i32, load!(1) as i32);
                if b == 0 {
                    return Err(VmError::DivisionByZero);
                }
                store!(2, a.wrapping_div(b) as u32);
            }
            0x14 => {
                let (a, b) = (load!(0) as i32, load!(1) as i32);
                if b == 0 {
                    return Err(VmError::DivisionByZero);
                }
                store!(2, a.wrapping_rem(b) as u32);
            }
            0x15 => {
                let value = (load!(0) as i32).wrapping_neg() as u32;
                store!(1, value);
            }
            0x18 => {
                let value = load!(0) & load!(1);
                store!(2, value);
            }
            0x19 => {
                let value = load!(0) | load!(1);
                store!(2, value);
            }
            0x1a => {
                let value = load!(0) ^ load!(1);
                store!(2, value);
            }
            0x1b => {
                let value = !load!(0);
                store!(1, value);
            }
            0x1c => {
                let (a, b) = (load!(0), load!(1));
                store!(2, if b >= 32 { 0 } else { a << b });
            }
            0x1d => {
                let (a, b) = (load!(0) as i32, load!(1));
                store!(
                    2,
                    if b >= 32 {
                        if a < 0 { u32::MAX } else { 0 }
                    } else {
                        (a >> b) as u32
                    }
                );
            }
            0x1e => {
                let (a, b) = (load!(0), load!(1));
                store!(2, if b >= 32 { 0 } else { a >> b });
            }
            0x20 => {
                let offset = load!(0);
                self.branch(offset)?;
            }
            0x22..=0x2d => {
                let a = load!(0);
                let (condition, offset) = if opcode <= 0x23 {
                    (if opcode == 0x22 { a == 0 } else { a != 0 }, load!(1))
                } else {
                    let b = load!(1);
                    let condition = match opcode {
                        0x24 => a == b,
                        0x25 => a != b,
                        0x26 => (a as i32) < (b as i32),
                        0x27 => (a as i32) >= (b as i32),
                        0x28 => (a as i32) > (b as i32),
                        0x29 => (a as i32) <= (b as i32),
                        0x2a => a < b,
                        0x2b => a >= b,
                        0x2c => a > b,
                        _ => a <= b,
                    };
                    (condition, load!(2))
                };
                if condition {
                    self.branch(offset)?;
                }
            }
            0x30 => {
                let address = load!(0);
                let count = load!(1);
                let destination = self.destination(&operands[2])?;
                let arguments = self.pop_arguments(count)?;
                self.call(address, arguments.as_slice(), destination)?;
            }
            0x31 => {
                let value = load!(0);
                self.return_from_function(value)?;
            }
            0x32 => {
                let offset = load!(1);
                let destination = self.destination(&operands[0])?;
                let (destination_type, destination_address) = destination_parts(&destination);
                self.push_call_stub(destination_type, destination_address, self.pc)?;
                let token = self.stack.len();
                self.store_destination(&destination, token, Width::Word)?;
                self.branch(offset)?;
            }
            0x33 => {
                let value = load!(0);
                let token = load!(1);
                self.throw_to(token, value)?;
            }
            0x34 => {
                let address = load!(0);
                let count = load!(1);
                let arguments = self.pop_arguments(count)?;
                let frame = self.stack.frame_ptr;
                self.stack.truncate(frame)?;
                self.enter_function(address, arguments.as_slice())?;
            }
            0x40 => {
                let value = load!(0);
                store!(1, value);
            }
            0x41 => {
                let value = load!(0, Width::Short) & 0xffff;
                store!(1, value, Width::Short);
            }
            0x42 => {
                let value = load!(0, Width::Byte) & 0xff;
                store!(1, value, Width::Byte);
            }
            0x44 => {
                let value = load!(0) as i16 as i32 as u32;
                store!(1, value);
            }
            0x45 => {
                let value = load!(0) as i8 as i32 as u32;
                store!(1, value);
            }
            0x48..=0x4b => {
                let base = load!(0);
                let index = load!(1) as i32;
                let value = match opcode {
                    0x48 => self
                        .memory
                        .read32(base.wrapping_add((index as u32).wrapping_mul(4)))?,
                    0x49 => self
                        .memory
                        .read16(base.wrapping_add((index as u32).wrapping_mul(2)))?
                        as u32,
                    0x4a => self.memory.read8(base.wrapping_add(index as u32))? as u32,
                    _ => {
                        let byte = self.memory.read8(base.wrapping_add((index >> 3) as u32))?;
                        ((byte >> (index & 7)) & 1) as u32
                    }
                };
                store!(2, value);
            }
            0x4c..=0x4f => {
                let base = load!(0);
                let index = load!(1) as i32;
                let value = load!(2);
                match opcode {
                    0x4c => self
                        .memory
                        .write32(base.wrapping_add((index as u32).wrapping_mul(4)), value)?,
                    0x4d => self.memory.write16(
                        base.wrapping_add((index as u32).wrapping_mul(2)),
                        value as u16,
                    )?,
                    0x4e => self
                        .memory
                        .write8(base.wrapping_add(index as u32), value as u8)?,
                    _ => {
                        let address = base.wrapping_add((index >> 3) as u32);
                        let mask = 1u8 << (index & 7);
                        let old = self.memory.read8(address)?;
                        self.memory
                            .write8(address, if value == 0 { old & !mask } else { old | mask })?;
                    }
                }
            }
            0x50 => {
                let count = (self.stack.len() - self.stack.frame_end()?) / 4;
                store!(0, count);
            }
            0x51 => {
                let depth = load!(0);
                let value = self.stack.peek(depth)?;
                store!(1, value);
            }
            0x52 => {
                let a = self.stack.pop_u32()?;
                let b = self.stack.pop_u32()?;
                self.stack.push_u32(a)?;
                self.stack.push_u32(b)?;
            }
            0x53 => {
                let count = load!(0);
                let places = load!(1) as i32;
                self.roll_stack(count, places)?;
            }
            0x54 => {
                let count = load!(0);
                let available = (self.stack.len() - self.stack.frame_end()?) / 4;
                if count > available {
                    return Err(VmError::StackUnderflow);
                }
                let count_bytes = count.checked_mul(4).ok_or(VmError::StackOverflow)?;
                if self
                    .stack
                    .len()
                    .checked_add(count_bytes)
                    .is_none_or(|end| end > self.stack.maximum)
                {
                    return Err(VmError::StackOverflow);
                }
                let start = self.stack.len() - count_bytes;
                self.stack
                    .bytes
                    .extend_from_within(start as usize..self.stack.len() as usize);
            }
            0x70 => {
                let value = load!(0);
                self.stream_character(u32::from(value as u8))?;
            }
            0x71 => {
                let value = load!(0);
                self.push_call_stub(11, 0, self.pc)?;
                self.resume_number(value, 0)?;
            }
            0x72 => {
                let address = load!(0);
                self.stream_string(address)?;
            }
            0x73 => {
                let value = load!(0);
                self.stream_character(value)?;
            }
            0x100 => {
                let selector = load!(0);
                let argument = load!(1);
                let value = self.gestalt(selector, argument);
                store!(2, value);
            }
            0x101 => return Err(VmError::DebugTrap(load!(0))),
            0x102 => {
                let value = self.memory.len();
                store!(0, value);
            }
            0x103 => {
                let size = load!(0);
                let result = if self.heap_blocks.is_empty() && self.memory.resize(size)? {
                    0
                } else {
                    1
                };
                store!(1, result);
            }
            0x104 => self.pc = load!(0),
            0x110 => {
                let range = load!(0) as i32;
                let raw = self.next_random();
                let value = if range > 0 {
                    raw % range as u32
                } else if range < 0 {
                    (raw % range.unsigned_abs()).wrapping_neg()
                } else {
                    raw
                };
                store!(1, value);
            }
            0x111 => {
                let seed = load!(0);
                self.random_state = if seed == 0 {
                    unpredictable_seed()
                } else {
                    seed
                };
            }
            0x120 => self.stop(),
            0x121 => {
                let sum = self
                    .story
                    .image
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != 8)
                    .fold(0u32, |sum, (_, bytes)| {
                        sum.wrapping_add(u32::from_be_bytes(*bytes))
                    });
                store!(0, u32::from(sum != self.story.header.checksum));
            }
            0x122 => self.restart()?,
            0x123 | 0x124 => {
                let stream = load!(0);
                let destination = self.destination(&operands[1])?;
                if self.io_system != 2 {
                    return Err(VmError::InvalidSaveIo);
                }
                let success = if opcode == 0x123 {
                    self.encode_save(&destination)
                        .and_then(|data| self.write_save_stream(stream, &data))
                        .is_ok()
                } else {
                    self.read_save_stream(stream)
                        .and_then(|data| self.decode_save(&data))
                        .is_ok()
                };
                if opcode == 0x123 || !success {
                    self.store_destination(&destination, u32::from(!success), Width::Word)?;
                }
            }
            0x125 => {
                let _stage = crate::diagnostics::stage("undo-save");
                let destination = self.destination(&operands[0])?;
                let budget = crate::memory::ResourceLimits::bytes(self.resource_limits.undo_mib);
                let fixed_cost = self
                    .stack
                    .bytes
                    .len()
                    .saturating_add(self.heap_blocks.len().saturating_mul(8));
                if budget == 0 || fixed_cost > budget {
                    self.store_destination(&destination, 1, Width::Word)?;
                    return Ok(());
                }
                let maximum_pages = (budget - fixed_cost) / (256 + 8);
                let previous_pages = self.undo.back().map(|undo| &undo.memory_pages);
                let Some(memory_pages) = self
                    .memory
                    .snapshot_pages_with_limit(previous_pages, maximum_pages)
                else {
                    self.store_destination(&destination, 1, Width::Word)?;
                    return Ok(());
                };
                let snapshot = UndoState {
                    memory: None,
                    memory_len: self.memory.len(),
                    memory_pages,
                    stack: self.stack.clone(),
                    pc: self.pc,
                    destination: destination.clone(),
                    heap_next: self.heap_next,
                    heap_blocks: self.heap_blocks.clone(),
                };
                let cost = snapshot.byte_len();
                if cost > budget {
                    self.store_destination(&destination, 1, Width::Word)?;
                    return Ok(());
                }
                self.trim_undo(cost);
                self.undo.push_back(snapshot);
                self.memory.clear_dirty_pages();
                self.store_destination(&destination, 0, Width::Word)?;
            }
            0x126 => {
                let failure_destination = self.destination(&operands[0])?;
                if let Some(undo) = self.undo.pop_back() {
                    if let Some(memory) = undo.memory {
                        self.memory.restore(&memory, self.protection)?;
                    } else {
                        self.memory.restore_pages(
                            undo.memory_len,
                            &undo.memory_pages,
                            self.protection,
                        )?;
                    }
                    self.stack = undo.stack;
                    self.pc = undo.pc;
                    self.pending_select = None;
                    self.state = RunState::Running;
                    self.heap_next = undo.heap_next;
                    self.heap_blocks = undo.heap_blocks;
                    self.store_destination(&undo.destination, u32::MAX, Width::Word)?;
                } else {
                    self.store_destination(&failure_destination, 1, Width::Word)?;
                }
            }
            0x127 => {
                let start = load!(0);
                let length = load!(1);
                self.protection = (length != 0).then_some((start, length));
            }
            0x128 => store!(0, u32::from(self.undo.is_empty())),
            0x129 => {
                self.undo.pop_back();
            }
            0x130 => {
                let selector = load!(0);
                let count = load!(1);
                let destination = self.destination(&operands[2])?;
                self.glk(selector, count, destination)?;
            }
            0x140 => {
                let value = self.story.header.decoding_table;
                store!(0, value);
            }
            0x141 => {
                self.story.header.decoding_table = load!(0);
            }
            0x148 => {
                store!(0, self.io_system);
                store!(1, self.io_rock);
            }
            0x149 => {
                self.io_system = match load!(0) {
                    supported @ (0..=2) => supported,
                    _ => 0,
                };
                self.io_rock = load!(1);
            }
            0x150 | 0x151 => {
                let key = load!(0);
                let key_size = load!(1);
                let start = load!(2);
                let structure_size = load!(3);
                let structure_count = load!(4);
                let key_offset = load!(5);
                let options = load!(6);
                let result = if opcode == 0x150 {
                    self.linear_search(
                        key,
                        key_size,
                        start,
                        structure_size,
                        structure_count,
                        key_offset,
                        options,
                    )?
                } else {
                    self.binary_search(
                        key,
                        key_size,
                        start,
                        structure_size,
                        structure_count,
                        key_offset,
                        options,
                    )?
                };
                store!(7, result);
            }
            0x152 => {
                let key = load!(0);
                let key_size = load!(1);
                let start = load!(2);
                let key_offset = load!(3);
                let next_offset = load!(4);
                let options = load!(5);
                let result =
                    self.linked_search(key, key_size, start, key_offset, next_offset, options)?;
                store!(6, result);
            }
            0x160..=0x163 => {
                let address = load!(0);
                let argument_count = (opcode - 0x160) as usize;
                let mut arguments = [0; 3];
                for index in 0..argument_count {
                    arguments[index] = load!(index + 1);
                }
                let destination = self.destination(&operands[argument_count + 1])?;
                self.call(address, &arguments[..argument_count], destination)?;
            }
            0x170 => {
                let length = load!(0);
                let address = load!(1);
                self.memory.zero(address, length)?;
            }
            0x171 => {
                let length = load!(0);
                let source = load!(1);
                let destination = load!(2);
                self.memory.copy(source, destination, length)?;
            }
            0x178 => {
                let size = load!(0);
                let address = self.malloc(size)?;
                store!(1, address);
            }
            0x179 => {
                let address = load!(0);
                self.mfree(address)?;
            }
            0x180 => {
                let index = load!(0);
                let address = load!(1);
                self.set_acceleration(index, address)?;
            }
            0x181 => {
                let index = load!(0);
                let value = load!(1);
                self.acceleration.set_parameter(index, value);
            }
            0x190 => {
                let value = (load!(0) as i32 as f32).to_bits();
                store!(1, value);
            }
            0x191 => {
                let value = float_to_int(f32::from_bits(load!(0)) as f64, false);
                store!(1, value);
            }
            0x192 => {
                let value = float_to_int(f32::from_bits(load!(0)) as f64, true);
                store!(1, value);
            }
            0x198 => {
                let value = f32::from_bits(load!(0)).ceil().to_bits();
                store!(1, value);
            }
            0x199 => {
                let value = f32::from_bits(load!(0)).floor().to_bits();
                store!(1, value);
            }
            0x1a0..=0x1a3 => {
                let (a, b) = (f32::from_bits(load!(0)), f32::from_bits(load!(1)));
                let value = match opcode {
                    0x1a0 => a + b,
                    0x1a1 => a - b,
                    0x1a2 => a * b,
                    _ => a / b,
                };
                store!(2, value.to_bits());
            }
            0x1a4 => {
                let a = f32::from_bits(load!(0));
                let b = f32::from_bits(load!(1));
                let quotient = modulo_quotient(a as f64, b as f64) as f32;
                let remainder = a % b;
                store!(2, remainder.to_bits());
                store!(3, quotient.to_bits());
            }
            0x1a8..=0x1ab => {
                let a = f32::from_bits(load!(0));
                let value = match opcode {
                    0x1a8 => a.sqrt(),
                    0x1a9 => a.exp(),
                    0x1aa => a.ln(),
                    _ => {
                        let b = f32::from_bits(load!(1));
                        if a == 1.0 || b == 0.0 { 1.0 } else { a.powf(b) }
                    }
                };
                let destination = if opcode == 0x1ab { 2 } else { 1 };
                store!(destination, value.to_bits());
            }
            0x1b0..=0x1b6 => {
                let a = f32::from_bits(load!(0));
                let value = match opcode {
                    0x1b0 => a.sin(),
                    0x1b1 => a.cos(),
                    0x1b2 => a.tan(),
                    0x1b3 => a.asin(),
                    0x1b4 => a.acos(),
                    0x1b5 => a.atan(),
                    _ => a.atan2(f32::from_bits(load!(1))),
                };
                let destination = if opcode == 0x1b6 { 2 } else { 1 };
                store!(destination, value.to_bits());
            }
            0x1c0..=0x1c1 => {
                let a = f32::from_bits(load!(0));
                let b = f32::from_bits(load!(1));
                let tolerance = f32::from_bits(load!(2));
                let equal = floats_equal(a, b, tolerance);
                let offset = load!(3);
                if (opcode == 0x1c0 && equal) || (opcode == 0x1c1 && !equal) {
                    self.branch(offset)?;
                }
            }
            0x1c2..=0x1c5 => {
                let a = f32::from_bits(load!(0));
                let b = f32::from_bits(load!(1));
                let condition = match opcode {
                    0x1c2 => a < b,
                    0x1c3 => a <= b,
                    0x1c4 => a > b,
                    _ => a >= b,
                };
                let offset = load!(2);
                if condition {
                    self.branch(offset)?;
                }
            }
            0x1c8..=0x1c9 => {
                let value = f32::from_bits(load!(0));
                let condition = if opcode == 0x1c8 {
                    value.is_nan()
                } else {
                    value.is_infinite()
                };
                let offset = load!(1);
                if condition {
                    self.branch(offset)?;
                }
            }
            0x200..=0x204
            | 0x208..=0x209
            | 0x210..=0x215
            | 0x218..=0x21b
            | 0x220..=0x226
            | 0x230..=0x235
            | 0x238..=0x239 => {
                let loads = match opcode {
                    0x200 | 0x203 => 1,
                    0x201 | 0x202 | 0x204 | 0x208 | 0x209 | 0x218..=0x21a | 0x220..=0x225 => 2,
                    0x238 | 0x239 => 3,
                    0x232..=0x235 => 5,
                    0x230 | 0x231 => 7,
                    _ => 4,
                };
                let mut args = [0; 7];
                for (index, _) in operands.iter().take(loads).enumerate() {
                    args[index] = load!(index);
                }
                let double = |index: usize| {
                    f64::from_bits(((args[index] as u64) << 32) | args[index + 1] as u64)
                };
                if matches!(opcode, 0x201 | 0x202 | 0x204) {
                    let value = if opcode == 0x204 {
                        (double(0) as f32).to_bits()
                    } else {
                        float_to_int(double(0), opcode == 0x202)
                    };
                    store!(2, value);
                } else if opcode >= 0x230 {
                    let a = double(0);
                    let condition = match opcode {
                        0x230 => doubles_equal(a, double(2), double(4)),
                        0x231 => !doubles_equal(a, double(2), double(4)),
                        0x232 => a < double(2),
                        0x233 => a <= double(2),
                        0x234 => a > double(2),
                        0x235 => a >= double(2),
                        0x238 => a.is_nan(),
                        _ => a.is_infinite(),
                    };
                    if condition {
                        self.branch(args[loads - 1])?;
                    }
                } else {
                    let value = match opcode {
                        0x200 => args[0] as i32 as f64,
                        0x203 => f32::from_bits(args[0]) as f64,
                        0x208 => double(0).ceil(),
                        0x209 => double(0).floor(),
                        0x210 => double(0) + double(2),
                        0x211 => double(0) - double(2),
                        0x212 => double(0) * double(2),
                        0x213 => double(0) / double(2),
                        0x214 => double(0) % double(2),
                        0x215 => modulo_quotient(double(0), double(2)),
                        0x218 => double(0).sqrt(),
                        0x219 => double(0).exp(),
                        0x21a => double(0).ln(),
                        0x21b => {
                            let (a, b) = (double(0), double(2));
                            if a == 1.0 || b == 0.0 { 1.0 } else { a.powf(b) }
                        }
                        0x220 => double(0).sin(),
                        0x221 => double(0).cos(),
                        0x222 => double(0).tan(),
                        0x223 => double(0).asin(),
                        0x224 => double(0).acos(),
                        0x225 => double(0).atan(),
                        _ => double(0).atan2(double(2)),
                    }
                    .to_bits();
                    store!(loads, value as u32);
                    store!(loads + 1, (value >> 32) as u32);
                }
            }
            _ => {
                return Err(VmError::UnsupportedOpcode {
                    opcode,
                    address: instruction_address,
                });
            }
        }
        Ok(())
    }

    fn fetch_decoded(&mut self, address: u32) -> Result<(u32, [Operand; 8]), VmError> {
        let cacheable = address < self.memory.ram_start();
        let index = (address as usize >> 2) & (DECODE_CACHE_SIZE - 1);
        if cacheable {
            let entry = self.decoded_cache.entries[index];
            if entry.valid && entry.address == address {
                self.decode_cache_hits = self.decode_cache_hits.wrapping_add(1);
                self.pc = entry.next_pc;
                return Ok((entry.opcode, entry.operands));
            }
            self.decode_cache_misses = self.decode_cache_misses.wrapping_add(1);
        }
        let opcode = self.fetch_opcode()?;
        let count = operand_count(opcode).ok_or(VmError::UnsupportedOpcode { opcode, address })?;
        let operands = self.fetch_operands(count)?;
        if cacheable && self.pc <= self.memory.ram_start() {
            self.decoded_cache.entries[index] = DecodeEntry {
                address,
                next_pc: self.pc,
                opcode,
                operands,
                valid: true,
            };
        }
        Ok((opcode, operands))
    }

    fn fetch_opcode(&mut self) -> Result<u32, VmError> {
        let first = self.fetch8()?;
        if first < 0x80 {
            Ok(first as u32)
        } else if first < 0xc0 {
            Ok(((first as u32 & 0x7f) << 8) | self.fetch8()? as u32)
        } else {
            Ok(((first as u32 & 0x3f) << 24)
                | (self.fetch8()? as u32) << 16
                | (self.fetch8()? as u32) << 8
                | self.fetch8()? as u32)
        }
    }

    fn fetch_operands(&mut self, count: usize) -> Result<[Operand; 8], VmError> {
        // Glulx instructions have at most eight operands. Read every mode byte
        // before reading operand data, including the unused high nibble.
        let mut modes = [0u8; 4];
        for mode in modes.iter_mut().take(count.div_ceil(2)) {
            *mode = self.fetch8()?;
        }
        let mut operands = [const { Operand::Zero }; 8];
        for (index, operand) in operands.iter_mut().enumerate().take(count) {
            let mode = (modes[index / 2] >> ((index % 2) * 4)) & 0xf;
            *operand = self.fetch_operand(mode)?;
        }
        Ok(operands)
    }

    fn fetch_operand(&mut self, mode: u8) -> Result<Operand, VmError> {
        Ok(match mode {
            0 => Operand::Zero,
            1 => Operand::Constant(self.fetch8()? as i8 as i32 as u32),
            2 => Operand::Constant(self.fetch16()? as i16 as i32 as u32),
            3 => Operand::Constant(self.fetch32()?),
            5 => Operand::Memory(self.fetch8()? as u32),
            6 => Operand::Memory(self.fetch16()? as u32),
            7 => Operand::Memory(self.fetch32()?),
            8 => Operand::Stack,
            9 => Operand::Local(self.fetch8()? as u32),
            10 => Operand::Local(self.fetch16()? as u32),
            11 => Operand::Local(self.fetch32()?),
            13 => Operand::Memory(self.memory.ram_start().wrapping_add(self.fetch8()? as u32)),
            14 => Operand::Memory(self.memory.ram_start().wrapping_add(self.fetch16()? as u32)),
            15 => Operand::Memory(self.memory.ram_start().wrapping_add(self.fetch32()?)),
            _ => return Err(VmError::InvalidAddressMode(mode)),
        })
    }

    fn fetch8(&mut self) -> Result<u8, VmError> {
        let value = self.memory.read8(self.pc)?;
        self.pc = self.pc.wrapping_add(1);
        Ok(value)
    }

    fn fetch16(&mut self) -> Result<u16, VmError> {
        let value = self.memory.read16(self.pc)?;
        self.pc = self.pc.wrapping_add(2);
        Ok(value)
    }

    fn fetch32(&mut self) -> Result<u32, VmError> {
        let value = self.memory.read32(self.pc)?;
        self.pc = self.pc.wrapping_add(4);
        Ok(value)
    }

    fn load_operand(&mut self, operand: &Operand, width: Width) -> Result<u32, VmError> {
        match operand {
            Operand::Zero => Ok(0),
            Operand::Constant(value) => Ok(*value),
            Operand::Memory(address) => match width {
                Width::Byte => Ok(self.memory.read8(*address)? as u32),
                Width::Short => Ok(self.memory.read16(*address)? as u32),
                Width::Word => self.memory.read32(*address),
            },
            Operand::Stack => self.stack.pop_u32(),
            Operand::Local(offset) => self.stack.read_local(*offset, width),
        }
    }

    fn pop_arguments(&mut self, count: u32) -> Result<ArgumentBuffer, VmError> {
        let count = count as usize;
        let available = (self.stack.len() - self.stack.frame_end()?) / 4;
        if count > available as usize {
            return Err(VmError::StackUnderflow);
        }
        if count <= INLINE_ARGUMENTS {
            let mut values = [0; INLINE_ARGUMENTS];
            for value in &mut values[..count] {
                *value = self.stack.pop_u32()?;
            }
            Ok(ArgumentBuffer {
                inline: values,
                heap: None,
                length: count,
            })
        } else {
            let mut values = Vec::new();
            values
                .try_reserve_exact(count)
                .map_err(|_| VmError::MemoryAllocation(count as u32))?;
            for _ in 0..count {
                values.push(self.stack.pop_u32()?);
            }
            Ok(ArgumentBuffer {
                inline: [0; INLINE_ARGUMENTS],
                length: count,
                heap: Some(values),
            })
        }
    }

    fn store_operand(
        &mut self,
        operand: &Operand,
        value: u32,
        width: Width,
    ) -> Result<(), VmError> {
        let destination = self.destination(operand)?;
        self.store_destination(&destination, value, width)
    }

    fn destination(&self, operand: &Operand) -> Result<Destination, VmError> {
        match operand {
            Operand::Zero => Ok(Destination::Discard),
            Operand::Memory(address) => Ok(Destination::Memory(*address)),
            Operand::Stack => Ok(Destination::Stack),
            Operand::Local(offset) => Ok(Destination::Local(*offset)),
            Operand::Constant(_) => Err(VmError::InvalidStoreMode),
        }
    }

    fn store_destination(
        &mut self,
        destination: &Destination,
        value: u32,
        width: Width,
    ) -> Result<(), VmError> {
        match destination {
            Destination::Discard => Ok(()),
            Destination::Memory(address) => match width {
                Width::Byte => self.memory.write8(*address, value as u8),
                Width::Short => self.memory.write16(*address, value as u16),
                Width::Word => self.memory.write32(*address, value),
            },
            Destination::Stack => self.stack.push_u32(value),
            Destination::Local(offset) => self.stack.write_local(*offset, value, width),
        }
    }

    fn enter_function(&mut self, address: u32, arguments: &[u32]) -> Result<(), VmError> {
        if let Some(index) = self.acceleration.functions.get(&address).copied() {
            let value = self.accelerate(index, arguments)?;
            // A minimal frame and deferred return keep string/filter continuations
            // iterative, including strings with thousands of accelerated callbacks.
            self.stack.frame_ptr = self.stack.len();
            self.stack.push_u32(12)?;
            self.stack.push_u32(12)?;
            self.stack.push_u32(0)?;
            self.accelerated_return = Some(value);
            return Ok(());
        }
        let function_type = self.memory.read8(address)?;
        if !matches!(function_type, 0xc0 | 0xc1) {
            return Err(VmError::InvalidFunction(address));
        }
        let mut cursor = address + 1;
        let mut format = Vec::new();
        let mut groups = Vec::new();
        loop {
            let local_type = self.memory.read8(cursor)?;
            let count = self.memory.read8(cursor + 1)?;
            cursor += 2;
            format.extend_from_slice(&[local_type, count]);
            if local_type == 0 && count == 0 {
                break;
            }
            if !matches!(local_type, 1 | 2 | 4) || count == 0 {
                return Err(VmError::InvalidLocalsFormat(address));
            }
            groups.push((local_type as u32, count as u32));
        }

        let frame_ptr = self.stack.len();
        while format.len() % 4 != 0 {
            format.push(0);
        }
        let locals_pos = 8 + format.len() as u32;
        let mut local_cursor = locals_pos;
        let mut positions = Vec::new();
        for (size, count) in groups {
            local_cursor = align(local_cursor, size);
            for _ in 0..count {
                positions.push((local_cursor, size));
                local_cursor += size;
            }
        }
        let frame_len = align(local_cursor, 4);
        let frame_end = frame_ptr
            .checked_add(frame_len)
            .filter(|end| *end <= self.stack.maximum)
            .ok_or(VmError::StackOverflow)?;
        self.stack.frame_ptr = frame_ptr;
        self.stack.push_u32(frame_len)?;
        self.stack.push_u32(locals_pos)?;
        self.stack.bytes.extend_from_slice(&format);
        self.stack.bytes.resize(frame_end as usize, 0);

        if function_type == 0xc1 {
            for ((position, size), value) in positions.iter().zip(arguments.iter()) {
                self.stack.write_local(
                    position - locals_pos,
                    *value,
                    match size {
                        1 => Width::Byte,
                        2 => Width::Short,
                        _ => Width::Word,
                    },
                )?;
            }
        } else {
            for value in arguments.iter().rev() {
                self.stack.push_u32(*value)?;
            }
            self.stack.push_u32(arguments.len() as u32)?;
        }
        self.pc = cursor;
        Ok(())
    }

    fn call(
        &mut self,
        address: u32,
        arguments: &[u32],
        destination: Destination,
    ) -> Result<(), VmError> {
        if address == 0 {
            return self.store_destination(&destination, 0, Width::Word);
        }
        let (destination_type, destination_address) = destination_parts(&destination);
        self.stack.push_u32(destination_type)?;
        self.stack.push_u32(destination_address)?;
        self.stack.push_u32(self.pc)?;
        self.stack.push_u32(self.stack.frame_ptr)?;
        self.enter_function(address, arguments)
    }

    fn throw_to(&mut self, token: u32, value: u32) -> Result<(), VmError> {
        if token < 16 || token > self.stack.len() || !token.is_multiple_of(4) {
            return Err(VmError::InvalidCatchToken(token));
        }
        self.stack.truncate(token)?;
        let frame_ptr = self.stack.pop_raw_u32()?;
        let pc = self.stack.pop_raw_u32()?;
        let destination_address = self.stack.pop_raw_u32()?;
        let destination_type = self.stack.pop_raw_u32()?;
        let destination = match destination_type {
            0 => Destination::Discard,
            1 => Destination::Memory(destination_address),
            2 => Destination::Local(destination_address),
            3 => Destination::Stack,
            _ => return Err(VmError::InvalidCatchToken(token)),
        };
        if frame_ptr > self.stack.len() {
            return Err(VmError::InvalidCatchToken(token));
        }
        self.stack.frame_ptr = frame_ptr;
        self.pc = pc;
        self.store_destination(&destination, value, Width::Word)
    }

    fn return_from_function(&mut self, value: u32) -> Result<(), VmError> {
        let frame_ptr = self.stack.frame_ptr;
        self.stack.truncate(frame_ptr)?;
        if self.stack.len() == 0 {
            self.stop();
            return Ok(());
        }
        let old_frame_ptr = self.stack.pop_raw_u32()?;
        let pc = self.stack.pop_raw_u32()?;
        let address = self.stack.pop_raw_u32()?;
        let destination_type = self.stack.pop_raw_u32()?;
        self.stack.frame_ptr = old_frame_ptr;
        self.pc = pc;
        let destination = match destination_type {
            0 => Destination::Discard,
            1 => Destination::Memory(address),
            2 => Destination::Local(address),
            3 => Destination::Stack,
            10 => return self.resume_compressed(pc, address as u8),
            12 => return self.resume_number(pc, address),
            13 => return self.resume_c_string(pc),
            14 => return self.resume_unicode_string(pc),
            _ => return Err(VmError::InvalidCallStub),
        };
        self.store_destination(&destination, value, Width::Word)
    }

    fn branch(&mut self, offset: u32) -> Result<(), VmError> {
        match offset {
            0 => self.return_from_function(0),
            1 => self.return_from_function(1),
            _ => {
                self.pc = self.pc.wrapping_add(offset).wrapping_sub(2);
                Ok(())
            }
        }
    }

    fn roll_stack(&mut self, count: u32, places: i32) -> Result<(), VmError> {
        if count == 0 || places == 0 {
            return Ok(());
        }
        let available = (self.stack.len() - self.stack.frame_end()?) / 4;
        if count > available {
            return Err(VmError::StackUnderflow);
        }
        let rotation = places.rem_euclid(count as i32) as usize;
        if rotation == 0 {
            return Ok(());
        }
        let count = count as usize;
        let count_bytes = count.checked_mul(4).ok_or(VmError::StackUnderflow)?;
        let split = count - rotation;
        let start = self.stack.len() as usize - count_bytes;
        let end = self.stack.len() as usize;
        reverse_stack_words(&mut self.stack.bytes[start..start + split * 4]);
        reverse_stack_words(&mut self.stack.bytes[start + split * 4..end]);
        reverse_stack_words(&mut self.stack.bytes[start..end]);
        Ok(())
    }

    fn malloc(&mut self, size: u32) -> Result<u32, VmError> {
        if size == 0 {
            return Ok(0);
        }
        let Some(size) = size.checked_add(3).map(|value| value & !3) else {
            return Ok(0);
        };
        if self.heap_blocks.is_empty() {
            self.heap_next = self.memory.len();
        }
        // First-fit over live allocations implicitly coalesces all free gaps.
        let mut address = self.heap_next;
        for (&start, &length) in &self.heap_blocks {
            if start - address >= size {
                break;
            }
            address = start + length;
        }
        let Some(end) = address.checked_add(size) else {
            return Ok(0);
        };
        if end > self.memory.len() {
            let Some(memory_size) = end.checked_add(0xff).map(|value| value & !0xff) else {
                return Ok(0);
            };
            if !self.memory.resize(memory_size)? {
                return Ok(0);
            }
        }
        self.heap_blocks.insert(address, size);
        Ok(address)
    }

    fn mfree(&mut self, address: u32) -> Result<(), VmError> {
        if self.heap_blocks.remove(&address).is_none() {
            return Err(VmError::InvalidHeapAddress(address));
        }
        if self.heap_blocks.is_empty() {
            self.memory.resize(self.heap_next)?;
        }
        Ok(())
    }

    fn glk_write_char(&mut self, stream: u32, character: char) {
        self.presentation_pending = true;
        let stream = if stream == 0 {
            self.glk_current_stream
        } else {
            stream
        };
        let Some(target) = self.glk_streams.get(&stream) else {
            self.output.push(character);
            return;
        };
        let target = match target.target {
            GlkStreamTarget::Window(id) => GlkStreamTarget::Window(id),
            _ => {
                let _ = self.write_stream_value(stream, character as u32);
                return;
            }
        };
        match target {
            GlkStreamTarget::Window(window_id) => {
                if self
                    .requests
                    .get(&window_id)
                    .is_some_and(|request| matches!(request, Request::Line(_)))
                {
                    return;
                }
                if let Some(window) = self.glk_windows.get_mut(&window_id) {
                    window.write_count = window.write_count.wrapping_add(1);
                }
                let kind = self
                    .glk_windows
                    .get(&window_id)
                    .map(|window| window.kind)
                    .unwrap_or(WINTYPE_TEXT_BUFFER);
                if kind == WINTYPE_TEXT_GRID {
                    if let Some(window) = self.glk_windows.get_mut(&window_id) {
                        window.put_grid_char(character);
                        window.content_revision = window.content_revision.wrapping_add(1);
                    }
                } else if kind == WINTYPE_TEXT_BUFFER {
                    let window = self.glk_windows.get_mut(&window_id).unwrap();
                    if let Some(run) = window.runs.last_mut().filter(|r| {
                        r.image.is_none()
                            && !r.flow_break
                            && r.style == window.style
                            && r.hyperlink == window.hyperlink
                    }) {
                        run.text.push(character);
                    } else {
                        window.runs.push(TextRun {
                            text: character.to_string(),
                            image: None,
                            flow_break: false,
                            style: window.style,
                            hyperlink: window.hyperlink,
                        });
                    }
                    window.text_chars += 1;
                    window.trim_text_history();
                    window.content_revision = window.content_revision.wrapping_add(1);
                    self.output.push(character);
                    if window.style != 8
                        && let Some(events) = &mut self.text_buffer_events
                    {
                        if let Some(TextBufferEvent::Text { window, text }) = events.last_mut()
                            && *window == window_id
                        {
                            text.push(character);
                        } else {
                            events.push(TextBufferEvent::Text {
                                window: window_id,
                                text: character.to_string(),
                            });
                        }
                    }
                }
                let echo = self
                    .glk_windows
                    .get(&window_id)
                    .map_or(0, |w| w.echo_stream);
                if echo != 0 {
                    self.glk_windows.get_mut(&window_id).unwrap().echo_stream = 0;
                    self.glk_write_char(echo, character);
                    self.glk_windows.get_mut(&window_id).unwrap().echo_stream = echo;
                }
            }
            GlkStreamTarget::Memory { .. } | GlkStreamTarget::File(_) => {
                let _ = self.write_stream_value(stream, character as u32);
            }
        }
    }

    fn glk_write_text(&mut self, stream: u32, text: &str) {
        for character in text.chars() {
            self.glk_write_char(stream, character);
        }
    }

    fn gestalt(&self, selector: u32, argument: u32) -> u32 {
        match selector {
            0 => 0x0003_0103,
            1 => 0x0000_0100,
            2..=3 => 1,
            4 => u32::from(matches!(argument, 0..=2)),
            5 => 1,
            6 => 1,
            7 => 1,
            8 => {
                if self.heap_blocks.is_empty() {
                    0
                } else {
                    self.heap_next
                }
            }
            9 => 1,
            10 => u32::from(acceleration::supported(argument)),
            11..=13 => 1,
            _ => 0,
        }
    }

    fn glk(
        &mut self,
        selector: u32,
        argument_count: u32,
        destination: Destination,
    ) -> Result<(), VmError> {
        // Conservatively retain presentation boundaries after other host calls
        // (including input/window/media changes). A poll by itself is not a
        // change and must not throttle computation to one poll per UI frame.
        if selector != 0xc1 {
            self.presentation_pending = true;
        }
        let arguments = self.pop_arguments(argument_count)?;
        let arguments = arguments.as_slice();
        let result = match selector {
            0x0001 => {
                self.stop();
                0
            }
            0x0002 | 0x0003 => 0,
            0x0004 => self.glk_gestalt(
                arguments.first().copied().unwrap_or(0),
                arguments.get(1).copied().unwrap_or(0),
            ),
            0x0005 => {
                let selector = arguments.first().copied().unwrap_or(0);
                let argument = arguments.get(1).copied().unwrap_or(0);
                if selector == 3 && arguments.get(3).copied().unwrap_or(0) > 0 {
                    self.write_glk_reference(arguments.get(2).copied().unwrap_or(0), 1)?;
                }
                self.glk_gestalt(selector, argument)
            }
            0x0020 => {
                let previous = arguments.first().copied().unwrap_or(0);
                let next = self
                    .glk_windows
                    .range(previous.saturating_add(1)..)
                    .next()
                    .map(|(&id, w)| (id, w.rock))
                    .unwrap_or((0, 0));
                self.write_glk_reference(arguments.get(1).copied().unwrap_or(0), next.1)?;
                next.0
            }
            0x0021 => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|window| window.rock)
                .unwrap_or(0),
            0x0022 => self.glk_root,
            0x0023 => self.open_window(arguments),
            0x0024 => {
                let (read, write) = self.close_window(arguments.first().copied().unwrap_or(0));
                let address = arguments.get(1).copied().unwrap_or(0);
                self.write_glk_reference(address, read)?;
                self.write_glk_reference(
                    if matches!(address, 0 | u32::MAX) {
                        address
                    } else {
                        address.wrapping_add(4)
                    },
                    write,
                )?;
                0
            }
            0x0025 => {
                let (width, height) = self
                    .glk_windows
                    .get(&arguments.first().copied().unwrap_or(0))
                    .map_or((0, 0), |w| (w.width, w.height));
                self.write_glk_reference(arguments.get(1).copied().unwrap_or(0), width)?;
                self.write_glk_reference(arguments.get(2).copied().unwrap_or(0), height)?;
                0
            }
            0x0026 => {
                self.set_arrangement(arguments);
                0
            }
            0x0027 => {
                let (method, size, key) = self
                    .glk_windows
                    .get(&arguments.first().copied().unwrap_or(0))
                    .map_or((0, 0, 0), |w| (w.method, w.split_size, w.key));
                for (index, value) in [method, size, key].into_iter().enumerate() {
                    self.write_glk_reference(
                        arguments.get(index + 1).copied().unwrap_or(0),
                        value,
                    )?;
                }
                0
            }
            0x0028 => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|window| window.kind)
                .unwrap_or(0),
            0x0029 => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map_or(0, |w| w.parent),
            0x0030 => {
                let id = arguments.first().copied().unwrap_or(0);
                self.glk_windows
                    .get(&id)
                    .and_then(|w| self.glk_windows.get(&w.parent))
                    .and_then(|w| w.children)
                    .and_then(|children| children.into_iter().find(|child| *child != id))
                    .unwrap_or(0)
            }
            0x002a => {
                let window_id = arguments.first().copied().unwrap_or(0);
                if self
                    .requests
                    .get(&window_id)
                    .is_some_and(|request| matches!(request, Request::Line(_)))
                {
                    return self.store_destination(&destination, 0, Width::Word);
                }
                if let Some(window) = self.glk_windows.get_mut(&window_id) {
                    window.content_revision = window.content_revision.wrapping_add(1);
                    window.runs.clear();
                    window.text_chars = 0;
                    window.grid.fill(' ');
                    window.grid_styles.fill(0);
                    window.grid_hyperlinks.fill(0);
                    window.cursor_x = 0;
                    window.cursor_y = 0;
                    if window.kind == WINTYPE_GRAPHICS {
                        self.graphics.push(GraphicsRequest::Clear {
                            window: window_id,
                            color: window.background_color,
                            canvas_size: [window.width, window.height],
                        });
                    } else if window.kind == WINTYPE_TEXT_BUFFER
                        && let Some(events) = &mut self.text_buffer_events
                    {
                        events.push(TextBufferEvent::Clear { window: window_id });
                    }
                }
                0
            }
            0x002b => {
                if let Some(window) = self
                    .glk_windows
                    .get_mut(&arguments.first().copied().unwrap_or(0))
                {
                    window.cursor_x = arguments.get(1).copied().unwrap_or(0);
                    window.cursor_y = arguments.get(2).copied().unwrap_or(0);
                }
                0
            }
            0x002c => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|window| window.stream)
                .unwrap_or(0),
            0x002d => {
                let id = arguments.first().copied().unwrap_or(0);
                let echo = arguments.get(1).copied().unwrap_or(0);
                if self.valid_echo(id, echo)
                    && let Some(window) = self.glk_windows.get_mut(&id)
                {
                    window.echo_stream = echo;
                }
                0
            }
            0x002e => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map_or(0, |w| w.echo_stream),
            0x002f => {
                self.glk_current_stream = self
                    .glk_windows
                    .get(&arguments.first().copied().unwrap_or(0))
                    .map(|window| window.stream)
                    .unwrap_or(0);
                0
            }
            0x0040 => {
                let previous = arguments.first().copied().unwrap_or(0);
                let next = self
                    .glk_streams
                    .range(previous.saturating_add(1)..)
                    .next()
                    .map(|(&id, w)| (id, w.rock))
                    .unwrap_or((0, 0));
                self.write_glk_reference(arguments.get(1).copied().unwrap_or(0), next.1)?;
                next.0
            }
            0x0041 => self
                .glk_streams
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|stream| stream.rock)
                .unwrap_or(0),
            0x0042 | 0x0138 => self.open_file_stream(arguments, selector == 0x0138),
            0x0049 | 0x013a => self.open_resource_stream(arguments, selector == 0x013a),
            0x0060..=0x0068 => {
                if selector == 0x0062 {
                    self.file_request = Some(streams::FileRequest {
                        selected_path: None,
                        notice: None,
                        usage: arguments.first().copied().unwrap_or(0),
                        mode: arguments.get(1).copied().unwrap_or(2),
                        rock: arguments.get(2).copied().unwrap_or(0),
                        destination,
                    });
                    self.state = RunState::WaitingForFile;
                    return Ok(());
                }
                self.fileref_call(selector, arguments)?
            }
            0x0043 | 0x0139 => {
                let address = arguments.first().copied().unwrap_or(0);
                let length = if address == 0 {
                    0
                } else {
                    arguments.get(1).copied().unwrap_or(0)
                };
                let rock = arguments.get(3).copied().unwrap_or(0);
                let mode = arguments.get(2).copied().unwrap_or(3);
                let width = if selector == 0x0139 { 4 } else { 1 };
                if !matches!(mode, 1..=3)
                    || (address != 0
                        && length
                            .checked_mul(width)
                            .and_then(|n| address.checked_add(n))
                            .is_none_or(|end| {
                                end > self.memory.len()
                                    || (mode != 2 && address < self.memory.ram_start())
                            }))
                {
                    return self.store_destination(&destination, 0, Width::Word);
                }
                let stream_id = self.glk_next_stream;
                self.glk_next_stream = self.glk_next_stream.wrapping_add(1).max(1);
                self.glk_streams.insert(
                    stream_id,
                    GlkStream {
                        rock,
                        target: GlkStreamTarget::Memory {
                            address,
                            length,
                            position: 0,
                            extent: Some(if mode == 1 { 0 } else { length }),
                            write_count: 0,
                            read_count: 0,
                            mode: arguments.get(2).copied().unwrap_or(3),
                            unicode: selector == 0x0139,
                        },
                    },
                );
                stream_id
            }
            0x0044 => {
                let stream_id = arguments.first().copied().unwrap_or(0);
                let (read_count, write_count) = self.close_stream(stream_id);
                if self.glk_current_stream == stream_id {
                    self.glk_current_stream = 0;
                }
                if arguments.get(1) == Some(&u32::MAX) {
                    // Glulx's -1 reference writes each struct field to the stack.
                    self.stack.push_u32(read_count)?;
                    self.stack.push_u32(write_count)?;
                }
                if let Some(result_address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    self.memory.write32(result_address, read_count)?;
                    self.memory.write32(result_address + 4, write_count)?;
                }
                0
            }
            0x0045 => {
                self.seek_stream(arguments);
                0
            }
            0x0046 => self.stream_position(arguments.first().copied().unwrap_or(0)),
            0x0047 => {
                self.glk_current_stream = arguments.first().copied().unwrap_or(0);
                0
            }
            0x0048 => self.glk_current_stream,
            0x0080 | 0x0081 => {
                let stream = if selector == 0x0081 {
                    arguments.first().copied().unwrap_or(0)
                } else {
                    0
                };
                self.glk_write_char(
                    stream,
                    char::from(arguments.last().copied().unwrap_or(0) as u8),
                );
                0
            }
            0x0082 => {
                let text = self.glk_byte_string(arguments.first().copied().unwrap_or(0))?;
                self.glk_write_text(0, &text);
                0
            }
            0x0083 => {
                let text = self.glk_byte_string(arguments.get(1).copied().unwrap_or(0))?;
                self.glk_write_text(arguments.first().copied().unwrap_or(0), &text);
                0
            }
            0x0084 => {
                self.glk_put_buffer(
                    0,
                    arguments.first().copied().unwrap_or(0),
                    arguments.get(1).copied().unwrap_or(0),
                )?;
                0
            }
            0x0085 => {
                self.glk_put_buffer(
                    arguments.first().copied().unwrap_or(0),
                    arguments.get(1).copied().unwrap_or(0),
                    arguments.get(2).copied().unwrap_or(0),
                )?;
                0
            }
            0x0090..=0x0092 | 0x0130..=0x0132 => self.read_stream_call(selector, arguments)?,
            0x00a0 | 0x00a1 => {
                let value = arguments.first().copied().unwrap_or(0) & 255;
                if selector == 0x00a0 && matches!(value,0x41..=0x5a|0xc0..=0xd6|0xd8..=0xde) {
                    value + 32
                } else if selector == 0x00a1 && matches!(value,0x61..=0x7a|0xe0..=0xf6|0xf8..=0xfe)
                {
                    value - 32
                } else {
                    value
                }
            }
            0x0086 | 0x0087 | 0x00b0..=0x00b3 | 0x0100..=0x0101 => {
                self.style_call(selector, arguments)?
            }
            0x00d4 | 0x0102 => {
                let id = arguments.first().copied().unwrap_or(0);
                if let Some(window) = self.glk_windows.get(&id) {
                    if selector == 0xd4 && matches!(window.kind, 4 | 5) {
                        self.mouse_requests.insert(id);
                    }
                    if selector == 0x102 && matches!(window.kind, 3..=5) {
                        self.hyperlink_requests.insert(id);
                    }
                }
                0
            }
            0x00d5 => {
                self.mouse_requests
                    .remove(&arguments.first().copied().unwrap_or(0));
                0
            }
            0x0103 => {
                self.hyperlink_requests
                    .remove(&arguments.first().copied().unwrap_or(0));
                0
            }
            0x00c0 => {
                self.select_event(arguments.first().copied().unwrap_or(0), destination)?;
                return Ok(());
            }
            0x00c1 => {
                self.select_poll(arguments.first().copied().unwrap_or(0))?;
                0
            }
            0x00d0 | 0x0141 => {
                self.request_line(arguments, selector == 0x0141)?;
                0
            }
            0x00d1 => {
                self.cancel_line(arguments)?;
                0
            }
            0x00d2 | 0x0140 => {
                let window = arguments.first().copied().unwrap_or(0);
                if self
                    .glk_windows
                    .get(&window)
                    .is_some_and(|w| matches!(w.kind, WINTYPE_TEXT_BUFFER | WINTYPE_TEXT_GRID))
                {
                    self.requests.entry(window).or_insert(Request::Character {
                        unicode: selector == 0x0140,
                    });
                }
                0
            }
            0x00d3 => {
                let window = arguments.first().copied().unwrap_or(0);
                if matches!(self.requests.get(&window), Some(Request::Character { .. })) {
                    self.requests.remove(&window);
                }
                0
            }
            0x0150 => {
                if let Some(w) = self
                    .glk_windows
                    .get_mut(&arguments.first().copied().unwrap_or(0))
                    .filter(|w| w.kind == WINTYPE_TEXT_BUFFER)
                {
                    w.echo_line = arguments.get(1).copied().unwrap_or(0) != 0;
                }
                0
            }
            0x0151 => {
                let address = arguments.get(1).copied().unwrap_or(0);
                let count = arguments.get(2).copied().unwrap_or(0);
                let mut keys = Vec::new();
                for i in 0..count {
                    keys.push(if address == u32::MAX {
                        self.stack.pop_u32()?
                    } else {
                        self.memory.read32(address.wrapping_add(i * 4))?
                    });
                }
                keys.retain(|key| matches!(*key, 0xffff_fff8 | 0xffff_ffe4..=0xffff_ffef));
                if let Some(w) = self
                    .glk_windows
                    .get_mut(&arguments.first().copied().unwrap_or(0))
                {
                    w.terminators = keys;
                }
                0
            }
            0x00d6 => {
                self.request_timer(arguments.first().copied().unwrap_or(0));
                0
            }
            0x00e0 => {
                let dimensions = self.picture_dimensions(arguments.first().copied().unwrap_or(0));
                if let Some([width, height]) = dimensions {
                    self.write_glk_reference(arguments.get(1).copied().unwrap_or(0), width)?;
                    self.write_glk_reference(arguments.get(2).copied().unwrap_or(0), height)?;
                    1
                } else {
                    0
                }
            }
            0x00e1 | 0x00e2 | 0x00ec => self.draw_image(selector, arguments),
            0x00e8 => {
                self.flow_break(arguments.first().copied().unwrap_or(0));
                0
            }
            0x00e9 | 0x00ea => {
                let window_id = arguments.first().copied().unwrap_or(0);
                if let Some(window) = self
                    .glk_windows
                    .get(&window_id)
                    .filter(|window| window.kind == WINTYPE_GRAPHICS)
                {
                    let offset = usize::from(selector == 0x00ea);
                    let color = if selector == 0x00ea {
                        arguments.get(1).copied().unwrap_or(0)
                    } else {
                        window.background_color
                    };
                    self.graphics.push(GraphicsRequest::Fill {
                        window: window_id,
                        color,
                        rect: [
                            arguments.get(offset + 1).copied().unwrap_or(0) as i32,
                            arguments.get(offset + 2).copied().unwrap_or(0) as i32,
                            arguments.get(offset + 3).copied().unwrap_or(0) as i32,
                            arguments.get(offset + 4).copied().unwrap_or(0) as i32,
                        ],
                        canvas_size: [window.width, window.height],
                    });
                }
                0
            }
            0x00eb => {
                let window_id = arguments.first().copied().unwrap_or(0);
                if let Some(window) = self.glk_windows.get_mut(&window_id) {
                    window.background_color = arguments.get(1).copied().unwrap_or(0) & 0x00ff_ffff;
                }
                0
            }
            0x00f0..=0x00f4 | 0x00f7..=0x00ff => self.sound_call(selector, arguments)?,
            0x0160..=0x0161 | 0x0168..=0x016f => self.datetime_call(selector, arguments)?,
            0x0120..=0x0124 => self.unicode_transform(selector, arguments)?,
            0x0128 | 0x012b => {
                let value = arguments.last().copied().unwrap_or(0);
                let stream = if selector == 0x012b {
                    arguments.first().copied().unwrap_or(0)
                } else {
                    0
                };
                self.glk_write_char(stream, char::from_u32(value).unwrap_or('\u{fffd}'));
                0
            }
            0x0129 | 0x012c => {
                let offset = usize::from(selector == 0x012c);
                let stream = if offset == 1 {
                    arguments.first().copied().unwrap_or(0)
                } else {
                    0
                };
                self.glk_put_unicode_string(stream, arguments.get(offset).copied().unwrap_or(0))?;
                0
            }
            0x012a | 0x012d => {
                let offset = usize::from(selector == 0x012d);
                self.glk_put_unicode_buffer(
                    if offset == 1 {
                        arguments.first().copied().unwrap_or(0)
                    } else {
                        0
                    },
                    arguments.get(offset).copied().unwrap_or(0),
                    arguments.get(offset + 1).copied().unwrap_or(0),
                )?;
                0
            }
            _ => {
                self.unsupported_glk.insert(selector);
                0
            }
        };
        self.store_destination(&destination, result, Width::Word)
    }

    pub fn set_glyph_support(&mut self, support: GlyphSupport) {
        self.glyph_support = Some(support);
    }

    pub fn set_text_metrics(&mut self, metrics: TextMetrics) {
        if self
            .text_metrics
            .as_ref()
            .is_some_and(|current| std::sync::Arc::ptr_eq(current, &metrics))
        {
            return;
        }
        self.text_metrics = Some(metrics);
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
        self.layout_windows();
        if self.glk_root != 0 && !self.events.iter().any(|event| event[0] == 5) {
            self.events.push_back([5, 0, 0, 0]);
        }
    }

    fn window_text_metrics(&self, window: &GlkWindow) -> [u32; 2] {
        let style = ResolvedStyle::resolve(window.kind, 0, &window.hints, self.text_appearance);
        self.text_metrics
            .as_ref()
            .map_or([8, 16], |metrics| metrics(style).map(|value| value.max(1)))
    }

    fn glk_gestalt(&self, selector: u32, argument: u32) -> u32 {
        let printable = char::from_u32(argument).is_some_and(|c| !c.is_control());
        match selector {
            0 => 0x0000_0706,
            1 => u32::from(
                printable
                    || argument == 0xffff_fffa
                    || ((self.graphical_host || self.terminal_host)
                        && events::is_special_key(argument)
                        && argument != u32::MAX),
            ),
            2 => u32::from(printable),
            3 => {
                let visible = printable
                    && char::from_u32(argument).is_some_and(|c| {
                        self.glyph_support
                            .as_ref()
                            .map_or(!self.graphical_host || c.is_ascii(), |support| support(c))
                    });
                if visible || argument == 10 { 2 } else { 0 }
            }
            4 => u32::from(self.graphical_host && matches!(argument, 4 | 5)), // MouseInput
            5 => 1,                                                           // Timer
            6 => u32::from(self.graphical_host),                              // Graphics
            7 => u32::from(
                self.graphical_host && matches!(argument, WINTYPE_TEXT_BUFFER | WINTYPE_GRAPHICS),
            ),
            8..=10 | 21 => u32::from(self.sound_available()), // Sound capabilities
            11 => 1,                                          // Hyperlinks
            13 => u32::from(self.sound_available()),          // MOD tracker music
            12 => u32::from(self.graphical_host && matches!(argument, 3..=5)), // HyperlinkInput
            17..=18 => u32::from(self.graphical_host || self.terminal_host), // Line input echo / terminators
            19 => u32::from(
                (self.graphical_host || self.terminal_host)
                    && matches!(argument, 0xffff_fff8 | 0xffff_ffe4..=0xffff_ffef),
            ),
            14 => u32::from(self.graphical_host), // GraphicsTransparency
            15..=16 => 1,                         // Unicode / UnicodeNorm
            20 => 1,                              // DateTime
            22 => 1,                              // ResourceStream
            23 => 0, // GraphicsCharInput is not provided by the graphical host.
            24 => u32::from(
                self.graphical_host && matches!(argument, WINTYPE_TEXT_BUFFER | WINTYPE_GRAPHICS),
            ),
            _ => 0,
        }
    }

    fn glk_put_buffer(&mut self, stream: u32, address: u32, length: u32) -> Result<(), VmError> {
        for index in 0..length {
            self.glk_write_char(stream, char::from(self.memory.read8(address + index)?));
        }
        Ok(())
    }

    fn glk_put_unicode_buffer(
        &mut self,
        stream: u32,
        address: u32,
        length: u32,
    ) -> Result<(), VmError> {
        for index in 0..length {
            let value = self.memory.read32(address + index * 4)?;
            self.glk_write_char(stream, char::from_u32(value).unwrap_or('\u{fffd}'));
        }
        Ok(())
    }

    fn glk_byte_string(&self, address: u32) -> Result<String, VmError> {
        let kind = self.memory.read8(address)?;
        if kind != 0xe0 {
            return Err(VmError::InvalidString { address, kind });
        }
        self.memory.c_string(address + 1)
    }
    fn glk_put_unicode_string(&mut self, stream: u32, address: u32) -> Result<(), VmError> {
        let kind = self.memory.read8(address)?;
        if kind != 0xe2 {
            return Err(VmError::InvalidString { address, kind });
        }
        let mut cursor = address + 4;
        loop {
            let value = self.memory.read32(cursor)?;
            if value == 0 {
                return Ok(());
            }
            self.glk_write_char(stream, char::from_u32(value).unwrap_or('\u{fffd}'));
            cursor += 4;
        }
    }

    fn next_random(&mut self) -> u32 {
        // A full-period Weyl sequence followed by a bijective integer mixer;
        // unlike nonzero-state xorshift, this can return every 32-bit value.
        self.random_state = self.random_state.wrapping_add(0x9e37_79b9);
        let mut value = self.random_state;
        value = (value ^ (value >> 16)).wrapping_mul(0x21f0_aaad);
        value = (value ^ (value >> 15)).wrapping_mul(0x735a_2d97);
        value ^ (value >> 15)
    }

    #[allow(clippy::too_many_arguments)]
    fn linear_search(
        &self,
        key: u32,
        key_size: u32,
        start: u32,
        structure_size: u32,
        structure_count: u32,
        key_offset: u32,
        options: u32,
    ) -> Result<u32, VmError> {
        let key_bytes = key.to_be_bytes();
        let key = self.search_key(&key_bytes, key_size, options)?;
        let mut index = 0u32;
        while structure_count == u32::MAX || index < structure_count {
            let structure = start.wrapping_add(index.wrapping_mul(structure_size));
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            if candidate[..key_size as usize] == key[..key_size as usize] {
                return Ok(search_result(structure, index, options));
            }
            if options & 0x02 != 0 && candidate[..key_size as usize].iter().all(|byte| *byte == 0) {
                break;
            }
            index = index.wrapping_add(1);
        }
        Ok(search_failure(options))
    }

    #[allow(clippy::too_many_arguments)]
    fn binary_search(
        &self,
        key: u32,
        key_size: u32,
        start: u32,
        structure_size: u32,
        structure_count: u32,
        key_offset: u32,
        options: u32,
    ) -> Result<u32, VmError> {
        let key_bytes = key.to_be_bytes();
        let key = self.search_key(&key_bytes, key_size, options)?;
        let mut low = 0u32;
        let mut high = structure_count;
        while low < high {
            let index = low + (high - low) / 2;
            let structure = start.wrapping_add(index.wrapping_mul(structure_size));
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            match candidate[..key_size as usize].cmp(&key[..key_size as usize]) {
                Ordering::Less => low = index + 1,
                Ordering::Greater => high = index,
                Ordering::Equal => return Ok(search_result(structure, index, options)),
            }
        }
        Ok(search_failure(options))
    }

    fn linked_search(
        &self,
        key: u32,
        key_size: u32,
        mut structure: u32,
        key_offset: u32,
        next_offset: u32,
        options: u32,
    ) -> Result<u32, VmError> {
        let key_bytes = key.to_be_bytes();
        let key = self.search_key(&key_bytes, key_size, options)?;
        let mut remaining = self.memory.len() / 4 + 1;
        while structure != 0 && remaining > 0 {
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            if candidate[..key_size as usize] == key[..key_size as usize] {
                return Ok(structure);
            }
            if options & 0x02 != 0 && candidate[..key_size as usize].iter().all(|byte| *byte == 0) {
                break;
            }
            structure = self.memory.read32(structure.wrapping_add(next_offset))?;
            remaining -= 1;
        }
        if structure != 0 && remaining == 0 {
            return Err(VmError::InvalidLinkedList);
        }
        Ok(0)
    }

    fn search_key(&self, key: &[u8; 4], key_size: u32, options: u32) -> Result<[u8; 4], VmError> {
        if options & 0x01 != 0 {
            return self.memory_key(u32::from_be_bytes(*key), key_size);
        }
        if !matches!(key_size, 1 | 2 | 4) {
            return Err(VmError::InvalidSearchKeySize(key_size));
        }
        let mut result = [0; 4];
        result[..key_size as usize].copy_from_slice(&key[4 - key_size as usize..]);
        Ok(result)
    }

    fn memory_key(&self, address: u32, key_size: u32) -> Result<[u8; 4], VmError> {
        if key_size == 0 {
            return Ok([0; 4]);
        }
        let mut result = [0; 4];
        for (offset, byte) in result.iter_mut().take(key_size as usize).enumerate() {
            *byte = self.memory.read8(
                address
                    .checked_add(offset as u32)
                    .ok_or(VmError::MemoryRead(address))?,
            )?;
        }
        Ok(result)
    }
}

fn unpredictable_seed() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the platform's entropy source by the standard library.
    let seed = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (seed as u32 ^ (seed >> 32) as u32).max(1)
}

fn float_to_int(value: f64, nearest: bool) -> u32 {
    if value.is_nan() {
        return if value.is_sign_negative() {
            0x8000_0000
        } else {
            0x7fff_ffff
        };
    }
    (if nearest {
        value.round()
    } else {
        value.trunc()
    } as i32) as u32
}

fn align(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
}

fn reverse_stack_words(bytes: &mut [u8]) {
    let length = bytes.len();
    for index in 0..length / 4 / 2 {
        let left = index * 4;
        let right = length - (index + 1) * 4;
        for offset in 0..4 {
            bytes.swap(left + offset, right + offset);
        }
    }
}

fn destination_parts(destination: &Destination) -> (u32, u32) {
    match destination {
        Destination::Discard => (0, 0),
        Destination::Memory(address) => (1, *address),
        Destination::Local(offset) => (2, *offset),
        Destination::Stack => (3, 0),
    }
}

fn search_result(address: u32, index: u32, options: u32) -> u32 {
    if options & 0x04 != 0 { index } else { address }
}

fn search_failure(options: u32) -> u32 {
    if options & 0x04 != 0 { u32::MAX } else { 0 }
}

fn modulo_quotient(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() || a.is_infinite() || b == 0.0 {
        f64::NAN
    } else if b.is_infinite() {
        0.0f64.copysign(a / b)
    } else {
        ((a - a % b) / b).copysign(a / b)
    }
}

fn doubles_equal(a: f64, b: f64, tolerance: f64) -> bool {
    if a.is_nan() || b.is_nan() || tolerance.is_nan() {
        return false;
    }
    if a.is_infinite() && b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= tolerance.abs()
}

fn floats_equal(a: f32, b: f32, tolerance: f32) -> bool {
    if a.is_nan() || b.is_nan() || tolerance.is_nan() {
        return false;
    }
    if a.is_infinite() && b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= tolerance.abs()
}

fn operand_count(opcode: u32) -> Option<usize> {
    Some(match opcode {
        0x200..=0x204 | 0x238..=0x239 => 3,
        0x208..=0x209 | 0x218..=0x21a | 0x220..=0x225 => 4,
        0x210..=0x215 | 0x21b | 0x226 => 6,
        0x230..=0x231 => 7,
        0x232..=0x235 => 5,
        0x00 | 0x52 | 0x120 | 0x122 | 0x129 => 0,
        0x20
        | 0x31
        | 0x50
        | 0x54
        | 0x70..=0x73
        | 0x101..=0x102
        | 0x104
        | 0x111
        | 0x121
        | 0x125..=0x126
        | 0x128
        | 0x179
        | 0x140..=0x141 => 1,
        0x15
        | 0x1b
        | 0x22..=0x23
        | 0x32..=0x34
        | 0x40..=0x42
        | 0x44..=0x45
        | 0x51
        | 0x53
        | 0x103
        | 0x110
        | 0x123..=0x124
        | 0x127
        | 0x148..=0x149
        | 0x160
        | 0x170
        | 0x178
        | 0x180..=0x181
        | 0x190..=0x192
        | 0x198..=0x199
        | 0x1a8..=0x1aa
        | 0x1b0..=0x1b5
        | 0x1c8..=0x1c9 => 2,
        0x10..=0x14
        | 0x18..=0x1a
        | 0x1c..=0x1e
        | 0x24..=0x2d
        | 0x30
        | 0x48..=0x4f
        | 0x100
        | 0x130
        | 0x161
        | 0x171
        | 0x1a0..=0x1a3
        | 0x1ab
        | 0x1b6
        | 0x1c2..=0x1c5 => 3,
        0x162 | 0x1a4 | 0x1c0..=0x1c1 => 4,
        0x163 => 5,
        0x152 => 7,
        0x150..=0x151 => 8,
        _ => return None,
    })
}

#[derive(Debug, Error)]
pub enum VmError {
    #[error("memory limit {0} must be nonzero and a multiple of 256 bytes")]
    InvalidMemoryLimit(u32),
    #[error(
        "game needs {required} bytes, exceeding the configured VM memory limit of {maximum} bytes"
    )]
    MemoryLimit { required: u32, maximum: u32 },
    #[error("could not allocate {0} bytes of game memory")]
    MemoryAllocation(u32),
    #[error("invalid or incompatible save file")]
    InvalidSave,
    #[error("invalid or unrepresentable date/time")]
    InvalidTime,
    #[error("save/restore requires Glk I/O")]
    InvalidSaveIo,
    #[error("stream I/O failed")]
    StreamIo,
    #[error("invalid heap allocation address {0:#010x}")]
    InvalidHeapAddress(u32),
    #[error("memory read outside the VM address space at {0:#010x}")]
    MemoryRead(u32),
    #[error("memory write outside the VM address space at {0:#010x}")]
    MemoryWrite(u32),
    #[error("attempted to write read-only VM memory at {0:#010x}")]
    RomWrite(u32),
    #[error("VM stack overflow")]
    StackOverflow,
    #[error("VM stack underflow")]
    StackUnderflow,
    #[error("invalid local variable offset {0:#x}")]
    InvalidLocal(u32),
    #[error("invalid address mode {0:#x}")]
    InvalidAddressMode(u8),
    #[error("constant operand cannot be used as a store destination")]
    InvalidStoreMode,
    #[error("unsupported opcode {opcode:#x} at {address:#010x}")]
    UnsupportedOpcode { opcode: u32, address: u32 },
    #[error("division by zero")]
    DivisionByZero,
    #[error("invalid function at {0:#010x}")]
    InvalidFunction(u32),
    #[error("invalid locals format in function at {0:#010x}")]
    InvalidLocalsFormat(u32),
    #[error("invalid call stub")]
    InvalidCallStub,
    #[error("invalid catch token {0:#x}")]
    InvalidCatchToken(u32),
    #[error("invalid string decoding table: {0}")]
    InvalidDecodingTable(&'static str),
    #[error("unsupported string decoding node type {0:#x}")]
    UnsupportedStringNode(u8),
    #[error("search key size {0} is invalid for a direct key")]
    InvalidSearchKeySize(u32),
    #[error("linked search encountered a cycle")]
    InvalidLinkedList,
    #[error("invalid string type {kind:#x} at {address:#010x}")]
    InvalidString { address: u32, kind: u8 },
    #[error("debug trap {0:#x}")]
    DebugTrap(u32),
    #[error("input was supplied while the VM was not waiting for it")]
    UnexpectedInput,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(super) fn push_glk_arguments(vm: &mut Vm, arguments: &[u32]) {
        for argument in arguments.iter().rev() {
            vm.stack.push_u32(*argument).unwrap();
        }
    }

    pub(crate) fn image_with_program(program: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 0x200];
        bytes[0..4].copy_from_slice(b"Glul");
        for (offset, value) in [
            (4, 0x0003_0103u32),
            (8, 0x100),
            (12, 0x200),
            (16, 0x300),
            (20, 0x100),
            (24, 0x40),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        bytes[0x40] = 0xc1;
        bytes[0x41] = 0;
        bytes[0x42] = 0;
        bytes[0x43..0x43 + program.len()].copy_from_slice(program);
        let checksum = (0..bytes.len())
            .step_by(4)
            .map(|o| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        bytes[32..36].copy_from_slice(&checksum.to_be_bytes());
        bytes
    }

    fn update_checksum(image: &mut [u8]) {
        image[32..36].fill(0);
        let checksum = image
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_be_bytes(*word))
            .fold(0u32, u32::wrapping_add);
        image[32..36].copy_from_slice(&checksum.to_be_bytes());
    }

    #[test]
    fn heap_reuses_coalesced_gaps_and_releases_memory() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        let original = vm.memory.len();
        assert!(vm.memory.resize(original + 256).unwrap());
        let base = vm.memory.len();
        let a = vm.malloc(5).unwrap();
        let b = vm.malloc(8).unwrap();
        let c = vm.malloc(8).unwrap();
        assert_eq!((a, b, c), (base, base + 8, base + 16));
        vm.memory.write32(c, 77).unwrap();
        vm.mfree(a).unwrap();
        vm.mfree(b).unwrap();
        assert_eq!(vm.malloc(16).unwrap(), a);
        assert_eq!(vm.memory.read32(c).unwrap(), 77);
        assert_eq!(vm.malloc(u32::MAX).unwrap(), 0);
        assert_eq!(vm.malloc(0).unwrap(), 0);
        vm.mfree(a).unwrap();
        assert!(matches!(vm.mfree(a), Err(VmError::InvalidHeapAddress(_))));
        vm.mfree(c).unwrap();
        assert_eq!(vm.memory.len(), base);
        assert_eq!(vm.gestalt(8, 0), 0);
    }

    #[test]
    fn heap_opcodes_block_resize_until_last_free() {
        let program = [
            0x81, 0x78, 0xd1, 8, 0x20, // malloc 8 -> RAM+32
            0x81, 0x03, 0xd2, 0x04, 0x00, 0x24, // setmemsize 1024 -> RAM+36
            0x81, 0x79, 0x0d, 0x20, // mfree RAM+32
            0x81, 0x03, 0xd2, 0x04, 0x00, 0x28, 0x81, 0x20,
        ];
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&program), None).unwrap()).unwrap();
        assert_eq!(vm.run_steps(10).unwrap(), RunState::Halted);
        assert_eq!(vm.memory.read32(0x124).unwrap(), 1);
        assert_eq!(vm.memory.read32(0x128).unwrap(), 0);
        assert_eq!(vm.memory.len(), 1024);
    }

    #[test]
    fn graphical_gestalt_does_not_claim_graphics_character_input() {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        vm.set_graphical_host(true);
        assert_eq!(vm.glk_gestalt(22, 0), 1);
        assert_eq!(vm.glk_gestalt(23, 0), 0);
    }

    #[test]
    fn text_buffer_history_is_bounded_without_truncating_glk_output() {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        let window = vm.open_window(&[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
        let stream = vm.glk_windows[&window].stream;
        let text: String = std::iter::repeat_n('x', MAX_TEXT_BUFFER_CHARS + 1).collect();
        vm.glk_write_text(stream, &text);
        assert_eq!(vm.output.chars().count(), MAX_TEXT_BUFFER_CHARS + 1);
        assert_eq!(
            vm.glk_windows[&window]
                .runs
                .iter()
                .map(|run| run.text.chars().count())
                .sum::<usize>(),
            MAX_TEXT_BUFFER_CHARS
        );
        assert_eq!(vm.glk_windows[&window].text_chars, MAX_TEXT_BUFFER_CHARS);
    }

    #[test]
    fn undo_and_restart_restore_heap_ownership() {
        let program = [0x81, 0x25, 0x00, 0x81, 0x26, 0x00];
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&program), None).unwrap()).unwrap();
        let base = vm.memory.len();
        let a = vm.malloc(8).unwrap();
        vm.memory.write32(a, 123).unwrap();
        vm.run_steps(1).unwrap();
        vm.mfree(a).unwrap();
        vm.malloc(512).unwrap();
        vm.run_steps(1).unwrap();
        assert_eq!(vm.heap_blocks.get(&a), Some(&8));
        assert_eq!(vm.memory.read32(a).unwrap(), 123);
        vm.mfree(a).unwrap();
        assert_eq!(vm.memory.len(), base);
        vm.malloc(8).unwrap();
        vm.restart().unwrap();
        assert!(vm.heap_blocks.is_empty());
        assert_eq!(vm.memory.len(), base);
    }

    #[test]
    fn stream_close_stack_reference_returns_fields_and_consumes_arguments() {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        let depth = vm.stack.len();
        push_glk_arguments(&mut vm, &[0x140, 8, 1, 0]);
        vm.glk(0x43, 4, Destination::Stack).unwrap();
        let stream = vm.stack.pop_u32().unwrap();
        assert_eq!(vm.stack.len(), depth);
        vm.glk_write_text(stream, "hello");
        push_glk_arguments(&mut vm, &[stream, u32::MAX]);
        vm.glk(0x44, 2, Destination::Discard).unwrap();
        assert_eq!(vm.stack.pop_u32().unwrap(), 5);
        assert_eq!(vm.stack.pop_u32().unwrap(), 0);
        assert_eq!(vm.stack.len(), depth);
        assert!(vm.take_output().is_empty());
    }

    #[test]
    fn stack_roll_reorders_words_without_allocating_a_copy() {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        for value in 1..=4 {
            vm.stack.push_u32(value).unwrap();
        }
        vm.roll_stack(4, 1).unwrap();
        let mut values = Vec::new();
        for _ in 0..4 {
            values.push(vm.stack.pop_u32().unwrap());
        }
        assert_eq!(values, [3, 2, 1, 4]);
    }

    #[test]
    fn runs_arithmetic_and_returns_from_top_level() {
        // add 7 9, push; return pop
        let program = [0x10, 0x11, 0x08, 7, 9, 0x31, 0x08];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
    }

    #[test]
    fn streams_text_through_glk_io_system() {
        // setiosys 2 0; streamchar 'A'; quit
        let program = [0x81, 0x49, 0x11, 2, 0, 0x70, 0x01, b'A', 0x81, 0x20];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "A");
    }

    #[test]
    fn filter_io_resumes_numbers_c_strings_and_unicode_strings() {
        let program = [
            0x81, 0x49, 0x21, 0x01, 0x00, 0xc0, // setiosys filter, function 0xc0
            0x71, 0x01, 0xf4, // streamnum -12
            0x72, 0x02, 0x00, 0xa0, // streamstr C string
            0x72, 0x02, 0x00, 0xb0, // streamstr Unicode string
            0x81, 0x20, // quit
        ];
        let mut image = image_with_program(&program);
        image[0xa0..0xa4].copy_from_slice(&[0xe0, b'H', b'i', 0]);
        image[0xb0..0xbc].copy_from_slice(&[0xe2, 0, 0, 0, 0, 0, 0x03, 0xbb, 0, 0, 0, 0]);
        image[0xc0..0xd5].copy_from_slice(&[
            0xc1, 0x04, 0x01, 0x00, 0x00, // function with one 32-bit local
            0x81, 0x49, 0x11, 0x02, 0x00, // setiosys Glk
            0x73, 0x09, 0x00, // streamunichar local 0
            0x81, 0x49, 0x21, 0x01, 0x00, 0xc0, // restore filter I/O
            0x31, 0x00, // return 0
        ]);
        update_checksum(&mut image);

        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(64).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "-12Hi\u{03bb}");
    }

    #[test]
    fn filter_io_resumes_compressed_strings_after_each_character() {
        let program = [
            0x81, 0x49, 0x21, 0x01, 0x00, 0xc0, // setiosys filter, function 0xc0
            0x72, 0x02, 0x00, 0xa0, // streamstr compressed string
            0x81, 0x20, // quit
        ];
        let mut image = image_with_program(&program);
        image[28..32].copy_from_slice(&0x80u32.to_be_bytes());
        image[0x80..0x84].copy_from_slice(&28u32.to_be_bytes());
        image[0x84..0x88].copy_from_slice(&3u32.to_be_bytes());
        image[0x88..0x8c].copy_from_slice(&0x90u32.to_be_bytes());
        image[0x90] = 0;
        image[0x91..0x95].copy_from_slice(&0x99u32.to_be_bytes());
        image[0x95..0x99].copy_from_slice(&0x9bu32.to_be_bytes());
        image[0x99..0x9b].copy_from_slice(&[2, b'B']);
        image[0x9b] = 1;
        image[0xa0..0xa2].copy_from_slice(&[0xe1, 0b0000_0010]);
        image[0xc0..0xd5].copy_from_slice(&[
            0xc1, 0x04, 0x01, 0x00, 0x00, // function with one 32-bit local
            0x81, 0x49, 0x11, 0x02, 0x00, // setiosys Glk
            0x73, 0x09, 0x00, // streamunichar local 0
            0x81, 0x49, 0x21, 0x01, 0x00, 0xc0, // restore filter I/O
            0x31, 0x00, // return 0
        ]);
        update_checksum(&mut image);

        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(32).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "B");
    }

    #[test]
    fn waits_for_glk_line_input_and_resumes() {
        let program = [
            0x40, 0x80, // push initial length 0
            0x40, 0x81, 0x10, // push maximum length 16
            0x40, 0x82, 0x01, 0x40, // push input buffer address
            0x40, 0x81, 0x01, // push window handle
            0x81, 0x30, 0x12, 0x00, 0x00, 0xd0, 0x04, // glk request_line_event
            0x40, 0x82, 0x01, 0x10, // push event structure address
            0x81, 0x30, 0x12, 0x00, 0x00, 0xc0, 0x01, // glk select
            0x81, 0x20, // quit
        ];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.open_window(&[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]), 1);

        assert_eq!(vm.run_steps(32).unwrap(), RunState::WaitingForLine);
        assert_eq!(
            vm.input_request(),
            Some(InputRequest::Line { maximum_length: 16 })
        );
        vm.provide_input("look").unwrap();
        assert_eq!(vm.run_steps(4).unwrap(), RunState::Halted);
    }

    #[test]
    fn compressed_string_can_call_an_embedded_function() {
        // setiosys 2 0; streamstr 0xa0; quit
        let program = [
            0x81, 0x49, 0x11, 0x02, 0x00, 0x72, 0x02, 0x00, 0xa0, 0x81, 0x20,
        ];
        let mut image = image_with_program(&program);
        image[28..32].copy_from_slice(&0x80u32.to_be_bytes());
        image[0x80..0x84].copy_from_slice(&31u32.to_be_bytes());
        image[0x84..0x88].copy_from_slice(&3u32.to_be_bytes());
        image[0x88..0x8c].copy_from_slice(&0x90u32.to_be_bytes());
        image[0x90] = 0;
        image[0x91..0x95].copy_from_slice(&0x99u32.to_be_bytes());
        image[0x95..0x99].copy_from_slice(&0x9eu32.to_be_bytes());
        image[0x99] = 8;
        image[0x9a..0x9e].copy_from_slice(&0xb0u32.to_be_bytes());
        image[0x9e] = 1;
        image[0xa0] = 0xe1;
        image[0xa1] = 0b0000_0010;
        image[0xb0..0xb8].copy_from_slice(&[0xc1, 0x00, 0x00, 0x70, 0x01, b'B', 0x31, 0x00]);
        image[32..36].fill(0);
        let checksum = (0..image.len())
            .step_by(4)
            .map(|o| u32::from_be_bytes(image[o..o + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        image[32..36].copy_from_slice(&checksum.to_be_bytes());

        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(16).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "B");
    }

    #[test]
    fn decoded_cache_reuses_rom_instruction_metadata() {
        let story = Story::from_bytes(&image_with_program(&[0x20, 0x01, 0xff]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(128).unwrap(), RunState::Running);
        assert!(vm.decode_cache_hits > 0);
        assert!(vm.decode_cache_misses > 0);
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_instruction_dispatch() {
        let story = Story::from_bytes(&image_with_program(&[0x20, 0x01, 0xff]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        let iterations = std::hint::black_box(5_000_000usize);
        let started = std::time::Instant::now();
        assert_eq!(vm.run_steps(iterations).unwrap(), RunState::Running);
        assert_eq!(vm.pc, 0x43);
        let elapsed = started.elapsed();
        eprintln!(
            "BENCHMARK name=instruction_dispatch iterations={iterations} elapsed_ns={} ns_per_iteration={:.3}",
            elapsed.as_nanos(),
            elapsed.as_secs_f64() * 1_000_000_000.0 / iterations as f64
        );
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_text_output() {
        for chars in [4_096usize, 8_192, 16_384] {
            let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
            let mut vm = Vm::new(story).unwrap();
            let window = text_buffer_glk(&mut vm, 0x23, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
            let stream = text_buffer_glk(&mut vm, 0x2c, &[window]);
            let text = "x".repeat(chars);
            let started = std::time::Instant::now();
            vm.glk_write_text(stream, &text);
            let elapsed = started.elapsed();
            assert_eq!(vm.take_output().chars().count(), chars);
            eprintln!(
                "BENCHMARK name=text_output chars={chars} elapsed_ns={} ns_per_char={:.3}",
                elapsed.as_nanos(),
                elapsed.as_secs_f64() * 1_000_000_000.0 / chars as f64
            );
        }
    }

    // Run optimized, single-threaded; optionally enforce a local timing budget.
    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_search_records() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        let records = 16_384;
        vm.memory.resize(0x100 + records * 8).unwrap();
        for index in 0..records {
            vm.memory.write32(0x100 + index * 8, index + 1).unwrap();
        }
        let iterations = 1000;
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            assert_eq!(
                std::hint::black_box(&vm)
                    .linear_search(std::hint::black_box(records), 4, 0x100, 8, records, 0, 0,)
                    .unwrap(),
                0x100 + (records - 1) * 8
            );
        }
        let elapsed = started.elapsed();
        eprintln!(
            "BENCHMARK name=linear_search records={records} iterations={iterations} elapsed_ns={} ns_per_iteration={:.3}",
            elapsed.as_nanos(),
            elapsed.as_secs_f64() * 1_000_000_000.0 / iterations as f64
        );
        if let Ok(budget) = std::env::var("GLULX_SEARCH_BUDGET_MS") {
            assert!(elapsed.as_secs_f64() * 1000.0 <= budget.parse::<f64>().unwrap());
        }
    }

    #[test]
    fn searches_handle_indirect_keys_zero_termination_and_invalid_memory() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.memory.write32(0x100, 0x11223344).unwrap();
        vm.memory.write32(0x108, 0x55667788).unwrap();
        vm.memory.write32(0x104, 0x108).unwrap();
        vm.memory.write32(0x10c, 0).unwrap();
        for size in [1, 2, 4] {
            assert_eq!(
                vm.linear_search(0x108, size, 0x100, 8, 2, 0, 1).unwrap(),
                0x108
            );
            assert_eq!(vm.binary_search(0x108, size, 0x100, 8, 2, 0, 5).unwrap(), 1);
            assert_eq!(
                vm.linked_search(0x108, size, 0x100, 0, 4, 1).unwrap(),
                0x108
            );
        }
        assert_eq!(
            vm.linear_search(99, 4, 0x110, 8, u32::MAX, 0, 2).unwrap(),
            0
        );
        assert!(vm.binary_search(0, 3, 0x100, 8, 2, 0, 0).is_err());
        assert!(
            vm.linear_search(0, 4, vm.memory.len() - 2, 8, 1, 0, 0)
                .is_err()
        );
        assert!(vm.linear_search(u32::MAX, 4, 0x100, 8, 1, 0, 1).is_err());
    }

    #[test]
    fn large_story_requirement_is_checked_by_vm_policy_not_parser() {
        let mut image = image_with_program(&[0x81, 0x20]);
        image[16..20].copy_from_slice(&(2048u32 * 1024 * 1024).to_be_bytes());
        update_checksum(&mut image);
        let story = Story::from_bytes(&image, None).unwrap();
        assert!(matches!(Vm::new(story), Err(VmError::MemoryLimit { .. })));
    }

    #[test]
    fn undo_budget_can_disable_snapshots_and_evict_oldest() {
        let story = Story::from_bytes(
            &image_with_program(&[0x81, 0x25, 0x0d, 0x20, 0x81, 0x20]),
            None,
        )
        .unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.set_resource_limits(crate::memory::ResourceLimits {
            undo_mib: 0,
            ..Default::default()
        });
        vm.run_steps(1).unwrap();
        assert!(vm.undo.is_empty());
        assert_eq!(vm.memory.read32(0x120).unwrap(), 1);
        vm.pc = 0x43;
        vm.set_resource_limits(crate::memory::ResourceLimits {
            undo_mib: 1,
            ..Default::default()
        });
        vm.run_steps(1).unwrap();
        assert_eq!(vm.undo.len(), 1);
        assert_eq!(vm.memory.read32(0x120).unwrap(), 0);
        vm.set_resource_limits(crate::memory::ResourceLimits {
            undo_mib: 0,
            ..Default::default()
        });
        assert!(vm.undo.is_empty());
    }

    #[test]
    fn undo_budget_counts_snapshot_pages_not_the_story_image() {
        let mut image = image_with_program(&[0x81, 0x25, 0x0d, 0x20, 0x81, 0x20]);
        let size = 2 * 1024 * 1024;
        image[12..16].copy_from_slice(&(size as u32).to_be_bytes());
        image[16..20].copy_from_slice(&(size as u32).to_be_bytes());
        image.resize(size, 0);
        update_checksum(&mut image);
        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.set_resource_limits(crate::memory::ResourceLimits {
            undo_mib: 1,
            ..Default::default()
        });

        vm.run_steps(1).unwrap();

        assert_eq!(vm.undo.len(), 1);
        assert_eq!(vm.memory.read32(0x120).unwrap(), 0);
    }

    #[test]
    fn saves_and_sessions_obey_the_host_memory_limit() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new_with_memory_limit(story.clone(), 0x400).unwrap();
        assert!(vm.memory.resize(0x400).unwrap());
        let save = vm.encode_save(&Destination::Discard).unwrap();
        let mut small = Vm::new_with_memory_limit(story, 0x300).unwrap();
        assert!(small.decode_save(&save).is_err());
        assert_eq!(small.memory.len(), 0x300);
        small.set_memory_limit(0x400).unwrap();
        small.decode_save(&save).unwrap();
        assert_eq!(small.memory.maximum(), 0x400);
        let serialized = serde_json::to_vec(&vm).unwrap();
        let mut restored: Vm = serde_json::from_slice(&serialized).unwrap();
        assert!(restored.set_memory_limit(0x200).is_err());
        restored.set_memory_limit(0x400).unwrap();
        assert_eq!(restored.validate_session().unwrap().memory.maximum(), 0x400);
    }

    #[test]
    fn select_poll_still_publishes_output_before_following_instructions() {
        let poll = [0x40, 0x82, 1, 0, 0x81, 0x30, 0x12, 0, 0, 0xc1, 1];
        let mut code = vec![0x81, 0x49, 0x11, 2, 0, 0x70, 1, b'A'];
        code.extend(poll);
        code.extend([0x70, 1, b'B']);
        code.extend(poll);
        code.extend([0x81, 0x20]);
        let mut vm = Vm::new(Story::from_bytes(&image_with_program(&code), None).unwrap()).unwrap();
        assert_eq!(vm.run_presentation_steps(1024).unwrap(), RunState::Running);
        assert_eq!(vm.take_output(), "A");
        assert_eq!(vm.run_presentation_steps(1024).unwrap(), RunState::Running);
        assert_eq!(vm.take_output(), "B");
        assert_eq!(vm.run_presentation_steps(1024).unwrap(), RunState::Halted);
    }

    #[test]
    fn empty_select_poll_does_not_force_one_render_frame_per_poll() {
        let mut code = Vec::new();
        for _ in 0..16 {
            code.extend([0x40, 0x82, 0x01, 0x00]); // event address
            code.extend([0x81, 0x30, 0x12, 0, 0, 0xc1, 1]); // select_poll
        }
        code.extend([0x81, 0x20]);
        let mut vm = Vm::new(Story::from_bytes(&image_with_program(&code), None).unwrap()).unwrap();
        let mut slices = 0;
        while vm.state() == RunState::Running {
            vm.run_presentation_steps(1024).unwrap();
            slices += 1;
        }
        eprintln!("16 empty polls required {slices} presentation slices");
        assert_eq!(
            slices, 1,
            "nonblocking event polls must not wait for a new render frame"
        );
    }

    #[test]
    fn search_opcodes_find_array_and_linked_records() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        for (index, key) in [1u32, 2, 3].into_iter().enumerate() {
            let address = 0x100 + index as u32 * 8;
            vm.memory.write32(address, key).unwrap();
            vm.memory
                .write32(address + 4, if key == 3 { 0 } else { address + 8 })
                .unwrap();
        }

        assert_eq!(vm.linear_search(2, 4, 0x100, 8, 3, 0, 0).unwrap(), 0x108);
        assert_eq!(vm.binary_search(3, 4, 0x100, 8, 3, 0, 0x04).unwrap(), 2);
        assert_eq!(vm.linked_search(3, 4, 0x100, 0, 4, 0).unwrap(), 0x110);
    }

    #[test]
    fn dispatches_official_glk_put_string_selector() {
        let program = [
            0x40, 0x82, 0x00, 0xbf, // push C string address
            0x81, 0x30, 0x12, 0x00, 0x00, 0x82, 0x01, // glk put_string
            0x81, 0x20, // quit
        ];
        let mut image = image_with_program(&program);
        image[0xbf] = 0xe0;
        image[0xc0..0xc3].copy_from_slice(b"Hi\0");
        image[32..36].fill(0);
        let checksum = (0..image.len())
            .step_by(4)
            .map(|o| u32::from_be_bytes(image[o..o + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        image[32..36].copy_from_slice(&checksum.to_be_bytes());

        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "Hi");
    }

    #[test]
    fn glk_memory_stream_captures_output_without_leaking_to_transcript() {
        let mut image = image_with_program(&[0x81, 0x20]);
        image[0xbf] = 0xe0;
        image[0xc0..0xc4].copy_from_slice(b"Hex\0");
        update_checksum(&mut image);
        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        push_glk_arguments(&mut vm, &[0x140, 8, 1, 99]);
        vm.glk(0x0043, 4, Destination::Memory(0x120)).unwrap();
        let stream = vm.memory.read32(0x120).unwrap();
        push_glk_arguments(&mut vm, &[stream]);
        vm.glk(0x0047, 1, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[0xbf]);
        vm.glk(0x0082, 1, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[stream]);
        vm.glk(0x0041, 1, Destination::Memory(0x124)).unwrap();
        push_glk_arguments(&mut vm, &[stream, 0x128]);
        vm.glk(0x0044, 2, Destination::Discard).unwrap();

        assert_eq!(vm.memory.read8(0x140).unwrap(), b'H');
        assert_eq!(vm.memory.read8(0x141).unwrap(), b'e');
        assert_eq!(vm.memory.read8(0x142).unwrap(), b'x');
        assert_eq!(vm.memory.read32(0x124).unwrap(), 99);
        assert_eq!(vm.memory.read32(0x128).unwrap(), 0);
        assert_eq!(vm.memory.read32(0x12c).unwrap(), 3);
        assert_eq!(vm.take_output(), "");
    }

    #[test]
    fn text_grid_output_is_separate_from_the_story_transcript() {
        let mut image = image_with_program(&[0x81, 0x20]);
        image[0xbf] = 0xe0;
        image[0xc0..0xc9].copy_from_slice(b"Score: 7\0");
        image[32..36].fill(0);
        let checksum = (0..image.len())
            .step_by(4)
            .map(|offset| u32::from_be_bytes(image[offset..offset + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        image[32..36].copy_from_slice(&checksum.to_be_bytes());
        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        push_glk_arguments(&mut vm, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 11]);
        vm.glk(0x0023, 5, Destination::Memory(0x120)).unwrap();
        let main = vm.memory.read32(0x120).unwrap();
        push_glk_arguments(&mut vm, &[main, 0x12, 1, WINTYPE_TEXT_GRID, 22]);
        vm.glk(0x0023, 5, Destination::Memory(0x124)).unwrap();
        let grid = vm.memory.read32(0x124).unwrap();
        push_glk_arguments(&mut vm, &[grid]);
        vm.glk(0x002f, 1, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[0xbf]);
        vm.glk(0x0082, 1, Destination::Discard).unwrap();

        assert_eq!(vm.status_text(), "Score: 7");
        assert_eq!(vm.take_output(), "");
    }

    fn text_buffer_glk(vm: &mut Vm, selector: u32, arguments: &[u32]) -> u32 {
        push_glk_arguments(vm, arguments);
        vm.glk(selector, arguments.len() as u32, Destination::Stack)
            .unwrap();
        vm.stack.pop_u32().unwrap()
    }

    #[test]
    fn text_buffer_events_capture_narrative_and_window_clear_boundaries() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.enable_text_buffer_events();
        let main = text_buffer_glk(&mut vm, 0x23, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
        let main_stream = text_buffer_glk(&mut vm, 0x2c, &[main]);
        let grid = text_buffer_glk(&mut vm, 0x23, &[main, 0x12, 1, WINTYPE_TEXT_GRID, 0]);
        let grid_stream = text_buffer_glk(&mut vm, 0x2c, &[grid]);
        text_buffer_glk(&mut vm, 0x81, &[main_stream, 'A' as u32]);
        text_buffer_glk(&mut vm, 0x81, &[main_stream, 'B' as u32]);
        text_buffer_glk(&mut vm, 0x81, &[grid_stream, 'G' as u32]);
        text_buffer_glk(&mut vm, 0x2a, &[grid]);
        text_buffer_glk(&mut vm, 0x24, &[grid, 0]);
        text_buffer_glk(&mut vm, 0x87, &[main_stream, 8]);
        text_buffer_glk(&mut vm, 0x81, &[main_stream, 'I' as u32]);
        text_buffer_glk(&mut vm, 0x87, &[main_stream, 0]);
        text_buffer_glk(&mut vm, 0x81, &[main_stream, 'C' as u32]);
        text_buffer_glk(&mut vm, 0x2a, &[main]);
        text_buffer_glk(&mut vm, 0x81, &[main_stream, 'D' as u32]);
        text_buffer_glk(&mut vm, 0x24, &[main, 0]);
        assert_eq!(
            vm.take_text_buffer_events(),
            vec![
                TextBufferEvent::Text {
                    window: main,
                    text: "ABC".to_owned(),
                },
                TextBufferEvent::Clear { window: main },
                TextBufferEvent::Text {
                    window: main,
                    text: "D".to_owned(),
                },
                TextBufferEvent::Clear { window: main },
            ]
        );
        assert_eq!(vm.take_output(), "ABICD");
        assert!(vm.take_text_buffer_events().is_empty());
    }

    #[test]
    fn text_buffer_events_are_opt_in_and_reset_without_losing_capture() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        let window = text_buffer_glk(&mut vm, 0x23, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
        let stream = text_buffer_glk(&mut vm, 0x2c, &[window]);
        text_buffer_glk(&mut vm, 0x81, &[stream, 'A' as u32]);
        assert!(vm.take_text_buffer_events().is_empty());
        assert!(vm.text_buffer_events.is_none());
        vm.enable_text_buffer_events();
        text_buffer_glk(&mut vm, 0x81, &[stream, 'B' as u32]);
        vm.enable_text_buffer_events();
        assert_eq!(
            vm.take_text_buffer_events(),
            vec![TextBufferEvent::Text {
                window,
                text: "B".to_owned(),
            }]
        );
        text_buffer_glk(&mut vm, 0x81, &[stream, 'C' as u32]);
        vm.restart().unwrap();
        assert!(vm.take_text_buffer_events().is_empty());
        text_buffer_glk(&mut vm, 0x81, &[stream, 'D' as u32]);
        assert_eq!(
            vm.take_text_buffer_events(),
            vec![TextBufferEvent::Text {
                window,
                text: "D".to_owned(),
            }]
        );
        assert_eq!(vm.take_output(), "ABCD");
    }

    #[test]
    fn disabling_text_buffer_events_drops_pending_capture_without_affecting_output() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        let window = text_buffer_glk(&mut vm, 0x23, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
        let stream = text_buffer_glk(&mut vm, 0x2c, &[window]);
        vm.enable_text_buffer_events();
        text_buffer_glk(&mut vm, 0x81, &[stream, 'A' as u32]);
        vm.disable_text_buffer_events();
        assert!(vm.take_text_buffer_events().is_empty());
        text_buffer_glk(&mut vm, 0x81, &[stream, 'B' as u32]);
        assert!(vm.take_text_buffer_events().is_empty());
        assert_eq!(vm.take_output(), "AB");
    }

    #[test]
    fn text_buffer_events_are_not_stored_in_vm_snapshots() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.enable_text_buffer_events();
        let window = text_buffer_glk(&mut vm, 0x23, &[0, 0, 0, WINTYPE_TEXT_BUFFER, 0]);
        let stream = text_buffer_glk(&mut vm, 0x2c, &[window]);
        text_buffer_glk(&mut vm, 0x81, &[stream, 'A' as u32]);
        let snapshot = serde_json::to_value(&vm).unwrap();
        assert!(snapshot.get("text_buffer_events").is_none());
        let mut restored: Vm = serde_json::from_value(snapshot).unwrap();
        assert!(restored.take_text_buffer_events().is_empty());
        assert!(restored.text_buffer_events.is_none());
    }

    #[test]
    fn graphics_windows_emit_fill_clear_and_close_commands() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        push_glk_arguments(&mut vm, &[0, 0, 0, WINTYPE_GRAPHICS, 33]);
        vm.glk(0x0023, 5, Destination::Memory(0x120)).unwrap();
        let window = vm.memory.read32(0x120).unwrap();

        push_glk_arguments(&mut vm, &[window, 0x112233]);
        vm.glk(0x00eb, 2, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[window, 1, 2, 3, 4]);
        vm.glk(0x00e9, 5, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[window, 0xaabbcc, 5, 6, 7, 8]);
        vm.glk(0x00ea, 6, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[window]);
        vm.glk(0x002a, 1, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[window, 0]);
        vm.glk(0x0024, 2, Destination::Discard).unwrap();

        let requests = vm.take_graphics();
        assert!(matches!(
            requests[0],
            GraphicsRequest::Resize {
                background: 0xffffff,
                ..
            }
        ));
        let requests = &requests[1..];
        assert!(matches!(
            requests[0],
            GraphicsRequest::Fill {
                color: 0x112233,
                rect: [1, 2, 3, 4],
                ..
            }
        ));
        assert!(matches!(
            requests[1],
            GraphicsRequest::Fill {
                color: 0xaabbcc,
                rect: [5, 6, 7, 8],
                ..
            }
        ));
        assert!(matches!(
            requests[2],
            GraphicsRequest::Clear {
                color: 0x112233,
                ..
            }
        ));
        assert!(matches!(requests[3], GraphicsRequest::Close { .. }));
    }

    #[test]
    fn graphics_resizes_capture_background_and_geometry_without_waiting_for_draws() {
        let story = Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        push_glk_arguments(&mut vm, &[0, 0, 0, WINTYPE_GRAPHICS, 0]);
        vm.glk(0x23, 5, Destination::Memory(0x120)).unwrap();
        let window = vm.memory.read32(0x120).unwrap();
        vm.take_graphics();
        push_glk_arguments(&mut vm, &[window, 0x123456]);
        vm.glk(0xeb, 2, Destination::Discard).unwrap();
        assert!(vm.take_graphics().is_empty()); // Background changes do not repaint.
        vm.resize_windows(400, 300);
        vm.resize_windows(0, 0);
        vm.resize_windows(800, 600);
        let requests = vm.take_graphics();
        assert_eq!(requests.len(), 3);
        for (request, expected) in requests.iter().zip([[400, 300], [0, 0], [800, 600]]) {
            assert!(
                matches!(request, GraphicsRequest::Resize { window: id, background: 0x123456, canvas_size }
                if *id == window && *canvas_size == expected)
            );
        }
        let restored: Vec<GraphicsRequest> =
            serde_json::from_str(&serde_json::to_string(&requests).unwrap()).unwrap();
        assert_eq!(restored.len(), 3);
        push_glk_arguments(
            &mut vm,
            &[window, 0xabcdef, i32::MIN as u32, 0, u32::MAX, u32::MAX],
        );
        vm.glk(0xea, 6, Destination::Discard).unwrap();
        assert!(matches!(
            vm.take_graphics()[0],
            GraphicsRequest::Fill {
                rect: [i32::MIN, 0, -1, -1],
                ..
            }
        ));
    }

    #[test]
    fn undo_restores_the_saved_state_and_then_reports_no_older_state() {
        // saveundo to RAM+0x20; restoreundo to RAM+0x24; quit
        let program = [0x81, 0x25, 0x0d, 0x20, 0x81, 0x26, 0x0d, 0x24, 0x81, 0x20];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.memory.read32(0x120).unwrap(), u32::MAX);
        assert_eq!(vm.memory.read32(0x124).unwrap(), 1);
    }

    #[test]
    fn undo_availability_can_be_queried_and_discarded() {
        let program = [
            0x81, 0x25, 0x0d, 0x20, // saveundo
            0x81, 0x28, 0x0d, 0x24, // hasundo
            0x81, 0x29, // discardundo
            0x81, 0x28, 0x0d, 0x28, // hasundo
            0x81, 0x20, // quit
        ];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.memory.read32(0x120).unwrap(), 0);
        assert_eq!(vm.memory.read32(0x124).unwrap(), 0);
        assert_eq!(vm.memory.read32(0x128).unwrap(), 1);
    }

    #[test]
    fn float_equality_handles_nan_infinity_and_tolerance() {
        assert!(floats_equal(1.0, 1.01, 0.02));
        assert!(floats_equal(f32::INFINITY, f32::INFINITY, 0.0));
        assert!(floats_equal(f32::INFINITY, 1.0, f32::INFINITY));
        assert!(!floats_equal(
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY
        ));
        assert!(!floats_equal(f32::NAN, f32::NAN, f32::INFINITY));
    }

    #[test]
    fn catch_and_throw_restore_the_continuation() {
        // catch RAM+0x20, branch over quit; throw 77 to saved token
        let program = [0x32, 0x1d, 0x20, 0x04, 0x81, 0x20, 0x33, 0xd1, 77, 0x20];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.memory.read32(0x120).unwrap(), 77);
    }

    #[test]
    fn floating_modulo_returns_remainder_and_integral_quotient() {
        // fmod 5.5 2.0 to RAM+0x20 and RAM+0x24; quit
        let mut program = vec![0x81, 0xa4, 0x33, 0xdd];
        program.extend_from_slice(&5.5f32.to_bits().to_be_bytes());
        program.extend_from_slice(&2.0f32.to_bits().to_be_bytes());
        program.extend_from_slice(&[0x20, 0x24, 0x81, 0x20]);
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        assert_eq!(vm.run_steps(4).unwrap(), RunState::Halted);
        assert_eq!(f32::from_bits(vm.memory.read32(0x120).unwrap()), 1.5);
        assert_eq!(f32::from_bits(vm.memory.read32(0x124).unwrap()), 2.0);
    }
}
