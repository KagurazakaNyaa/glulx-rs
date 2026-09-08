use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

use thiserror::Error;

use crate::{Story, memory::Memory};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Running,
    WaitingForLine,
    WaitingForChar,
    Halted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRequest {
    Line { maximum_length: u32 },
    Character,
}

#[derive(Debug, Clone)]
pub struct ImageRequest {
    pub window: u32,
    pub resource: u32,
    pub data: Vec<u8>,
    pub position: [i32; 2],
    pub requested_size: Option<[u32; 2]>,
    pub canvas_size: [u32; 2],
}

#[derive(Debug, Clone)]
pub enum GraphicsRequest {
    Draw(ImageRequest),
    Fill {
        window: u32,
        color: u32,
        rect: [i32; 4],
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

#[derive(Debug, Clone, Copy)]
enum Width {
    Byte,
    Short,
    Word,
}

#[derive(Debug, Clone)]
enum Operand {
    Zero,
    Constant(u32),
    Memory(u32),
    Stack,
    Local(u32),
}

#[derive(Debug, Clone)]
enum Destination {
    Discard,
    Memory(u32),
    Stack,
    Local(u32),
}

#[derive(Debug, Clone)]
struct LineRequest {
    buffer: u32,
    max_len: u32,
    unicode: bool,
    window: u32,
}

#[derive(Debug, Clone)]
struct PendingSelect {
    event_address: u32,
    destination: Destination,
}

#[derive(Debug, Clone)]
struct UndoState {
    memory: Memory,
    stack: Stack,
    pc: u32,
    io_system: u32,
    io_rock: u32,
    random_state: u32,
    decoding_table: u32,
    destination: Destination,
    heap_next: u32,
    heap_blocks: BTreeMap<u32, u32>,
}

const WINTYPE_TEXT_BUFFER: u32 = 3;
const WINTYPE_TEXT_GRID: u32 = 4;
const WINTYPE_GRAPHICS: u32 = 5;

#[derive(Debug, Clone)]
struct GlkWindow {
    rock: u32,
    kind: u32,
    stream: u32,
    width: u32,
    height: u32,
    cursor_x: u32,
    cursor_y: u32,
    grid: Vec<char>,
    background_color: u32,
}

#[derive(Debug, Clone)]
struct GlkStream {
    rock: u32,
    target: GlkStreamTarget,
}

#[derive(Debug, Clone)]
enum GlkStreamTarget {
    Window(u32),
    Memory {
        address: u32,
        length: u32,
        position: u32,
        write_count: u32,
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
            grid,
            background_color: 0x00ff_ffff,
        }
    }

    fn put_grid_char(&mut self, character: char) {
        if character == '\n' {
            self.cursor_x = 0;
            self.cursor_y = (self.cursor_y + 1).min(self.height.saturating_sub(1));
            return;
        }
        if self.cursor_x >= self.width || self.cursor_y >= self.height {
            return;
        }
        let index = self.cursor_y.saturating_mul(self.width) + self.cursor_x;
        if let Some(cell) = self.grid.get_mut(index as usize) {
            *cell = character;
        }
        self.cursor_x += 1;
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

#[derive(Debug, Clone)]
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

    fn read_local(&self, offset: u32, width: Width) -> Result<u32, VmError> {
        let address = self.local_address(offset)?;
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
        let address = self.local_address(offset)? as usize;
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
            .checked_sub((depth + 1).saturating_mul(4))
            .filter(|address| *address >= self.frame_end().unwrap_or(u32::MAX))
            .ok_or(VmError::StackUnderflow)?;
        self.read_raw_u32(address)
    }
}

pub struct Vm {
    story: Story,
    memory: Memory,
    stack: Stack,
    pc: u32,
    state: RunState,
    io_system: u32,
    io_rock: u32,
    output: String,
    line_request: Option<LineRequest>,
    char_requested: bool,
    pending_select: Option<PendingSelect>,
    random_state: u32,
    unsupported_glk: HashSet<u32>,
    protection: Option<(u32, u32)>,
    undo: Option<UndoState>,
    glk_windows: BTreeMap<u32, GlkWindow>,
    glk_streams: BTreeMap<u32, GlkStream>,
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
    pub fn new(story: Story) -> Result<Self, VmError> {
        let memory = Memory::new(&story);
        let heap_next = memory.len();
        let stack_size = story.header.stack_size;
        let start_func = story.header.start_func;
        let mut vm = Self {
            story,
            memory,
            stack: Stack::new(stack_size),
            pc: 0,
            state: RunState::Running,
            io_system: 0,
            io_rock: 0,
            output: String::new(),
            line_request: None,
            char_requested: false,
            pending_select: None,
            random_state: 0x6d2b_79f5,
            unsupported_glk: HashSet::new(),
            protection: None,
            undo: None,
            glk_windows: BTreeMap::new(),
            glk_streams: BTreeMap::new(),
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
            RunState::WaitingForLine => {
                self.line_request
                    .as_ref()
                    .map(|request| InputRequest::Line {
                        maximum_length: request.max_len,
                    })
            }
            RunState::WaitingForChar => Some(InputRequest::Character),
            _ => None,
        }
    }

    pub fn take_output(&mut self) -> String {
        std::mem::take(&mut self.output)
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
        for _ in 0..budget {
            if self.state != RunState::Running {
                break;
            }
            self.step()?;
        }
        Ok(self.state)
    }

    pub fn provide_input(&mut self, text: &str) -> Result<(), VmError> {
        let pending = self.pending_select.take().ok_or(VmError::UnexpectedInput)?;
        let (event_type, value) = match self.state {
            RunState::WaitingForLine => {
                let request = self.line_request.take().ok_or(VmError::UnexpectedInput)?;
                let length = if request.unicode {
                    let characters = text.chars().take(request.max_len as usize);
                    let mut length = 0;
                    for (index, character) in characters.enumerate() {
                        self.memory
                            .write32(request.buffer + index as u32 * 4, character as u32)?;
                        length += 1;
                    }
                    length
                } else {
                    let bytes = text.as_bytes();
                    let length = (bytes.len() as u32).min(request.max_len);
                    for (index, byte) in bytes.iter().take(length as usize).enumerate() {
                        self.memory.write8(request.buffer + index as u32, *byte)?;
                    }
                    length
                };
                (3, length)
            }
            RunState::WaitingForChar => {
                self.char_requested = false;
                (2, text.chars().next().unwrap_or('\n') as u32)
            }
            _ => return Err(VmError::UnexpectedInput),
        };
        self.memory.write32(pending.event_address, event_type)?;
        self.memory
            .write32(pending.event_address + 4, self.input_window)?;
        self.memory.write32(pending.event_address + 8, value)?;
        self.memory.write32(pending.event_address + 12, 0)?;
        self.store_destination(&pending.destination, 0, Width::Word)?;
        self.state = RunState::Running;
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), VmError> {
        self.memory.restart(self.protection);
        self.stack.clear();
        self.pc = 0;
        self.state = RunState::Running;
        self.io_system = 0;
        self.io_rock = 0;
        self.output.clear();
        self.line_request = None;
        self.char_requested = false;
        self.pending_select = None;
        self.input_window = 0;
        self.undo = None;
        self.graphics.clear();
        self.heap_next = self.memory.len();
        self.heap_blocks.clear();
        self.enter_function(self.story.header.start_func, &[])
    }

    pub fn stop(&mut self) {
        self.state = RunState::Halted;
    }

    fn step(&mut self) -> Result<(), VmError> {
        let instruction_address = self.pc;
        let opcode = self.fetch_opcode()?;
        let count = operand_count(opcode).ok_or(VmError::UnsupportedOpcode {
            opcode,
            address: instruction_address,
        })?;
        let operands = self.fetch_operands(count)?;
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
                let mut arguments = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    arguments.push(self.stack.pop_u32()?);
                }
                self.call(address, &arguments, destination)?;
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
                let mut arguments = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    arguments.push(self.stack.pop_u32()?);
                }
                let frame = self.stack.frame_ptr;
                self.stack.truncate(frame)?;
                self.enter_function(address, &arguments)?;
            }
            0x40 => {
                let value = load!(0);
                store!(1, value);
            }
            0x41 => {
                let value = load!(0, Width::Short);
                store!(1, value, Width::Short);
            }
            0x42 => {
                let value = load!(0, Width::Byte);
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
                let values = (0..count)
                    .rev()
                    .map(|depth| self.stack.peek(depth))
                    .collect::<Result<Vec<_>, _>>()?;
                for value in values {
                    self.stack.push_u32(value)?;
                }
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
                self.random_state = load!(0).max(1);
            }
            0x120 => self.state = RunState::Halted,
            0x121 => store!(0, 0),
            0x122 => self.restart()?,
            0x125 => {
                let destination = self.destination(&operands[0])?;
                self.undo = Some(UndoState {
                    memory: self.memory.clone(),
                    stack: self.stack.clone(),
                    pc: self.pc,
                    io_system: self.io_system,
                    io_rock: self.io_rock,
                    random_state: self.random_state,
                    decoding_table: self.story.header.decoding_table,
                    destination: destination.clone(),
                    heap_next: self.heap_next,
                    heap_blocks: self.heap_blocks.clone(),
                });
                self.store_destination(&destination, 0, Width::Word)?;
            }
            0x126 => {
                let failure_destination = self.destination(&operands[0])?;
                if let Some(undo) = self.undo.take() {
                    self.memory.restore(&undo.memory, self.protection);
                    self.stack = undo.stack;
                    self.pc = undo.pc;
                    self.io_system = undo.io_system;
                    self.io_rock = undo.io_rock;
                    self.random_state = undo.random_state;
                    self.story.header.decoding_table = undo.decoding_table;
                    self.line_request = None;
                    self.char_requested = false;
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
            0x128 => store!(0, u32::from(self.undo.is_some())),
            0x129 => self.undo = None,
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
                let mut arguments = Vec::with_capacity(argument_count);
                for index in 0..argument_count {
                    arguments.push(load!(index + 1));
                }
                let destination = self.destination(&operands[argument_count + 1])?;
                self.call(address, &arguments, destination)?;
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
            0x190 => {
                let value = (load!(0) as i32 as f32).to_bits();
                store!(1, value);
            }
            0x191 => {
                let value = f32::from_bits(load!(0)).trunc() as i32 as u32;
                store!(1, value);
            }
            0x192 => {
                let value = f32::from_bits(load!(0)).round() as i32 as u32;
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
                let quotient = (a / b).trunc();
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
                    _ => a.powf(f32::from_bits(load!(1))),
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
                if (opcode == 0x1c0 && equal) || (opcode == 0x1c1 && !equal) {
                    let offset = load!(3);
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
                if condition {
                    let offset = load!(2);
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
                if condition {
                    let offset = load!(1);
                    self.branch(offset)?;
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

    fn fetch_operands(&mut self, count: usize) -> Result<Vec<Operand>, VmError> {
        let mut modes = Vec::with_capacity(count);
        for index in 0..count.div_ceil(2) {
            let byte = self.fetch8()?;
            modes.push(byte & 0x0f);
            if index * 2 + 1 < count {
                modes.push(byte >> 4);
            }
        }
        modes
            .into_iter()
            .map(|mode| self.fetch_operand(mode))
            .collect()
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
        self.stack.frame_ptr = frame_ptr;
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
        self.stack.push_u32(frame_len)?;
        self.stack.push_u32(locals_pos)?;
        if self
            .stack
            .len()
            .checked_add(format.len() as u32)
            .is_none_or(|end| end > self.stack.maximum)
        {
            return Err(VmError::StackOverflow);
        }
        self.stack.bytes.extend_from_slice(&format);
        self.stack.bytes.resize((frame_ptr + frame_len) as usize, 0);

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
            self.state = RunState::Halted;
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
        let mut values = Vec::with_capacity(count as usize);
        for _ in 0..count {
            values.push(self.stack.pop_u32()?);
        }
        values.reverse();
        let rotation = places.rem_euclid(count as i32) as usize;
        values.rotate_right(rotation);
        for value in values {
            self.stack.push_u32(value)?;
        }
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

    fn stream_string(&mut self, address: u32) -> Result<(), VmError> {
        let kind = self.memory.read8(address)?;
        if !matches!(kind, 0xe0..=0xe2) {
            return Err(VmError::InvalidString { address, kind });
        }
        self.push_call_stub(11, 0, self.pc)?;
        match kind {
            0xe0 => self.resume_c_string(address + 1),
            0xe1 => self.resume_compressed(address + 1, 0),
            0xe2 => self.resume_unicode_string(address + 4),
            _ => unreachable!("string type was validated"),
        }
    }

    fn stream_character(&mut self, value: u32) -> Result<(), VmError> {
        match self.io_system {
            0 => Ok(()),
            1 => self.call(self.io_rock, &[value], Destination::Discard),
            2 => {
                self.glk_write_char(0, char::from_u32(value).unwrap_or('\u{fffd}'));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn enter_filter(&mut self, value: u32) -> Result<(), VmError> {
        self.enter_function(self.io_rock, &[value])
    }

    fn resume_number(&mut self, value: u32, next: u32) -> Result<(), VmError> {
        if self.io_system == 0 {
            return self.finish_string();
        }
        let text = (value as i32).to_string();
        let Some(character) = text.as_bytes().get(next as usize).copied() else {
            return self.finish_string();
        };
        if self.io_system == 1 {
            self.push_call_stub(12, next + 1, value)?;
            return self.enter_filter(u32::from(character));
        }
        self.glk_write_text(0, &text[next as usize..]);
        self.finish_string()
    }

    fn resume_c_string(&mut self, mut cursor: u32) -> Result<(), VmError> {
        let mut character = self.memory.read8(cursor)?;
        cursor = cursor.wrapping_add(1);
        if character == 0 || self.io_system == 0 {
            return self.finish_string();
        }
        if self.io_system == 1 {
            self.push_call_stub(13, 0, cursor)?;
            return self.enter_filter(u32::from(character));
        }
        while character != 0 {
            self.glk_write_char(0, char::from(character));
            character = self.memory.read8(cursor)?;
            cursor = cursor.wrapping_add(1);
        }
        self.finish_string()
    }

    fn resume_unicode_string(&mut self, mut cursor: u32) -> Result<(), VmError> {
        let character = self.memory.read32(cursor)?;
        cursor = cursor.wrapping_add(4);
        if character == 0 || self.io_system == 0 {
            return self.finish_string();
        }
        if self.io_system == 1 {
            self.push_call_stub(14, 0, cursor)?;
            return self.enter_filter(character);
        }
        let mut character = character;
        while character != 0 {
            self.glk_write_char(0, char::from_u32(character).unwrap_or('\u{fffd}'));
            character = self.memory.read32(cursor)?;
            cursor = cursor.wrapping_add(4);
        }
        self.finish_string()
    }

    fn resume_compressed(&mut self, mut byte_address: u32, mut bit: u8) -> Result<(), VmError> {
        let table = self.story.header.decoding_table;
        if table == 0 {
            return Err(VmError::InvalidDecodingTable(
                "no decoding table is selected",
            ));
        }
        let root = self.memory.read32(table + 8)?;
        let root_type = self.memory.read8(root)?;
        if root_type == 1 {
            return self.finish_string();
        }
        if root_type != 0 {
            return Err(VmError::InvalidDecodingTable("root node is not a branch"));
        }

        loop {
            let mut node = root;
            while self.memory.read8(node)? == 0 {
                let packed = self.memory.read8(byte_address)?;
                let direction = (packed >> bit) & 1;
                bit += 1;
                if bit == 8 {
                    bit = 0;
                    byte_address = byte_address.wrapping_add(1);
                }
                node = self.memory.read32(node + 1 + u32::from(direction) * 4)?;
            }

            match self.memory.read8(node)? {
                1 => return self.finish_string(),
                2 => {
                    let character = u32::from(self.memory.read8(node + 1)?);
                    match self.io_system {
                        0 => {}
                        1 => {
                            self.push_call_stub(10, u32::from(bit), byte_address)?;
                            return self.enter_filter(character);
                        }
                        2 => self.glk_write_char(0, char::from(character as u8)),
                        _ => {}
                    }
                }
                3 => {
                    self.push_call_stub(10, u32::from(bit), byte_address)?;
                    return self.resume_c_string(node + 1);
                }
                4 => {
                    let value = self.memory.read32(node + 1)?;
                    match self.io_system {
                        0 => {}
                        1 => {
                            self.push_call_stub(10, u32::from(bit), byte_address)?;
                            return self.enter_filter(value);
                        }
                        2 => self.glk_write_char(0, char::from_u32(value).unwrap_or('\u{fffd}')),
                        _ => {}
                    }
                }
                5 => {
                    self.push_call_stub(10, u32::from(bit), byte_address)?;
                    return self.resume_unicode_string(node + 1);
                }
                kind @ 8..=11 => {
                    let indirect = matches!(kind, 9 | 11);
                    let has_arguments = matches!(kind, 10 | 11);
                    let mut object = self.memory.read32(node + 1)?;
                    if indirect {
                        object = self.memory.read32(object)?;
                    }
                    let arguments = if has_arguments {
                        let count = self.memory.read32(node + 5)?;
                        let mut values = Vec::with_capacity(count as usize);
                        for index in 0..count {
                            values.push(self.memory.read32(node + 9 + index * 4)?);
                        }
                        values
                    } else {
                        Vec::new()
                    };

                    self.push_call_stub(10, u32::from(bit), byte_address)?;
                    return match self.memory.read8(object)? {
                        0xe0 => self.resume_c_string(object + 1),
                        0xe1 => self.resume_compressed(object + 1, 0),
                        0xe2 => self.resume_unicode_string(object + 4),
                        0xc0 | 0xc1 => self.enter_function(object, &arguments),
                        object_type => Err(VmError::InvalidString {
                            address: object,
                            kind: object_type,
                        }),
                    };
                }
                kind => return Err(VmError::UnsupportedStringNode(kind)),
            }
        }
    }

    fn push_call_stub(
        &mut self,
        destination_type: u32,
        destination_address: u32,
        pc: u32,
    ) -> Result<(), VmError> {
        self.stack.push_u32(destination_type)?;
        self.stack.push_u32(destination_address)?;
        self.stack.push_u32(pc)?;
        self.stack.push_u32(self.stack.frame_ptr)
    }

    fn finish_string(&mut self) -> Result<(), VmError> {
        let frame_ptr = self.stack.pop_raw_u32()?;
        let pc = self.stack.pop_raw_u32()?;
        let destination_address = self.stack.pop_raw_u32()?;
        let destination_type = self.stack.pop_raw_u32()?;
        match destination_type {
            10 => {
                self.stack.frame_ptr = frame_ptr;
                self.resume_compressed(pc, destination_address as u8)
            }
            11 => {
                self.pc = pc;
                Ok(())
            }
            _ => Err(VmError::InvalidCallStub),
        }
    }

    fn glk_write_char(&mut self, stream: u32, character: char) {
        let stream = if stream == 0 {
            self.glk_current_stream
        } else {
            stream
        };
        let Some(target) = self
            .glk_streams
            .get(&stream)
            .map(|stream| stream.target.clone())
        else {
            self.output.push(character);
            return;
        };
        match target {
            GlkStreamTarget::Window(window_id) => {
                let kind = self
                    .glk_windows
                    .get(&window_id)
                    .map(|window| window.kind)
                    .unwrap_or(WINTYPE_TEXT_BUFFER);
                if kind == WINTYPE_TEXT_GRID {
                    if let Some(window) = self.glk_windows.get_mut(&window_id) {
                        window.put_grid_char(character);
                    }
                } else if kind == WINTYPE_TEXT_BUFFER {
                    self.output.push(character);
                }
            }
            GlkStreamTarget::Memory { .. } => self.write_memory_stream(stream, character),
        }
    }

    fn write_memory_stream(&mut self, stream_id: u32, character: char) {
        let Some(stream) = self.glk_streams.get_mut(&stream_id) else {
            return;
        };
        let GlkStreamTarget::Memory {
            address,
            length,
            position,
            write_count,
            unicode,
        } = &mut stream.target
        else {
            return;
        };
        if *position >= *length {
            return;
        }
        let width = if *unicode { 4 } else { 1 };
        let destination = address.saturating_add(position.saturating_mul(width));
        let result = if *unicode {
            self.memory.write32(destination, character as u32)
        } else {
            self.memory.write8(destination, character as u8)
        };
        if result.is_ok() {
            *position = position.saturating_add(1);
            *write_count = write_count.saturating_add(1);
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
            9..=10 => 0,
            11 => 1,
            _ => 0,
        }
    }

    fn glk(
        &mut self,
        selector: u32,
        argument_count: u32,
        destination: Destination,
    ) -> Result<(), VmError> {
        let mut arguments = Vec::with_capacity(argument_count as usize);
        for _ in 0..argument_count {
            arguments.push(self.stack.pop_u32()?);
        }
        let result = match selector {
            0x0001 => {
                self.state = RunState::Halted;
                0
            }
            0x0002 | 0x0003 => 0,
            0x0004 => self.glk_gestalt(
                arguments.first().copied().unwrap_or(0),
                arguments.get(1).copied().unwrap_or(0),
            ),
            0x0005 => self.glk_gestalt(
                arguments.first().copied().unwrap_or(0),
                arguments.get(1).copied().unwrap_or(0),
            ),
            0x0020 => {
                let previous = arguments.first().copied().unwrap_or(0);
                if let Some(rock_address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    let rock = self
                        .glk_windows
                        .range((previous.saturating_add(1))..)
                        .next()
                        .map(|(_, window)| window.rock)
                        .unwrap_or(0);
                    self.memory.write32(rock_address, rock)?;
                }
                self.glk_windows
                    .range((previous.saturating_add(1))..)
                    .next()
                    .map(|(id, _)| *id)
                    .unwrap_or(0)
            }
            0x0021 => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|window| window.rock)
                .unwrap_or(0),
            0x0022 => self.glk_root,
            0x0023 => {
                let split = arguments.first().copied().unwrap_or(0);
                let method = arguments.get(1).copied().unwrap_or(0);
                let size = arguments.get(2).copied().unwrap_or(0);
                let kind = arguments.get(3).copied().unwrap_or(0);
                let rock = arguments.get(4).copied().unwrap_or(0);
                if split != 0 && !self.glk_windows.contains_key(&split) {
                    0
                } else {
                    let window_id = self.glk_next_window;
                    self.glk_next_window = self.glk_next_window.wrapping_add(1).max(1);
                    let stream_id = self.glk_next_stream;
                    self.glk_next_stream = self.glk_next_stream.wrapping_add(1).max(1);
                    let fixed = method & 0x30 == 0x10;
                    let (width, height) = match kind {
                        WINTYPE_TEXT_GRID if fixed => (80, size.max(1)),
                        WINTYPE_TEXT_GRID => (80, 25),
                        WINTYPE_GRAPHICS => (640, 480),
                        _ => (80, 25),
                    };
                    self.glk_windows.insert(
                        window_id,
                        GlkWindow::new(rock, kind, stream_id, width, height),
                    );
                    self.glk_streams.insert(
                        stream_id,
                        GlkStream {
                            rock,
                            target: GlkStreamTarget::Window(window_id),
                        },
                    );
                    if self.glk_root == 0 {
                        self.glk_root = window_id;
                    }
                    window_id
                }
            }
            0x0024 => {
                let window_id = arguments.first().copied().unwrap_or(0);
                if let Some(result_address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| *address != 0 && *address != u32::MAX)
                {
                    self.memory.write32(result_address, 0)?;
                    self.memory.write32(result_address + 4, 0)?;
                }
                if let Some(window) = self.glk_windows.remove(&window_id) {
                    if window.kind == WINTYPE_GRAPHICS {
                        self.graphics
                            .push(GraphicsRequest::Close { window: window_id });
                    }
                    self.glk_streams.remove(&window.stream);
                    if self.glk_current_stream == window.stream {
                        self.glk_current_stream = 0;
                    }
                    if self.glk_root == window_id {
                        self.glk_root = self.glk_windows.keys().next().copied().unwrap_or(0);
                    }
                }
                0
            }
            0x0025 => {
                let window = self
                    .glk_windows
                    .get(&arguments.first().copied().unwrap_or(0));
                if let Some(address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    self.memory
                        .write32(address, window.map(|window| window.width).unwrap_or(0))?;
                }
                if let Some(address) = arguments
                    .get(2)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    self.memory
                        .write32(address, window.map(|window| window.height).unwrap_or(0))?;
                }
                0
            }
            0x0026..=0x0027 => 0,
            0x0028 => self
                .glk_windows
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|window| window.kind)
                .unwrap_or(0),
            0x0029 | 0x0030 => 0,
            0x002a => {
                let window_id = arguments.first().copied().unwrap_or(0);
                if let Some(window) = self.glk_windows.get_mut(&window_id) {
                    window.grid.fill(' ');
                    window.cursor_x = 0;
                    window.cursor_y = 0;
                    if window.kind == WINTYPE_GRAPHICS {
                        self.graphics.push(GraphicsRequest::Clear {
                            window: window_id,
                            color: window.background_color,
                            canvas_size: [window.width, window.height],
                        });
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
            0x002d => 0,
            0x002e => 0,
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
                    .range((previous.saturating_add(1))..)
                    .next()
                    .map(|(id, stream)| (*id, stream.rock));
                if let Some(rock_address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    let rock = next.map(|(_, rock)| rock).unwrap_or(0);
                    self.memory.write32(rock_address, rock)?;
                }
                next.map(|(stream, _)| stream).unwrap_or(0)
            }
            0x0041 => self
                .glk_streams
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|stream| stream.rock)
                .unwrap_or(0),
            0x0042 | 0x0049 => 0,
            0x0043 | 0x0139 => {
                let address = arguments.first().copied().unwrap_or(0);
                let length = arguments.get(1).copied().unwrap_or(0);
                let rock = arguments.get(3).copied().unwrap_or(0);
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
                            write_count: 0,
                            unicode: selector == 0x0139,
                        },
                    },
                );
                stream_id
            }
            0x0044 => {
                let stream_id = arguments.first().copied().unwrap_or(0);
                let write_count = self
                    .glk_streams
                    .remove(&stream_id)
                    .and_then(|stream| match stream.target {
                        GlkStreamTarget::Memory { write_count, .. } => Some(write_count),
                        GlkStreamTarget::Window(_) => None,
                    })
                    .unwrap_or(0);
                if self.glk_current_stream == stream_id {
                    self.glk_current_stream = 0;
                }
                if arguments.get(1) == Some(&u32::MAX) {
                    // Glulx's -1 reference writes each struct field to the stack.
                    self.stack.push_u32(0)?;
                    self.stack.push_u32(write_count)?;
                }
                if let Some(result_address) = arguments
                    .get(1)
                    .copied()
                    .filter(|address| !matches!(*address, 0 | u32::MAX))
                {
                    self.memory.write32(result_address, 0)?;
                    self.memory.write32(result_address + 4, write_count)?;
                }
                0
            }
            0x0045 => {
                let stream_id = arguments.first().copied().unwrap_or(0);
                let offset = arguments.get(1).copied().unwrap_or(0) as i32;
                let seek_mode = arguments.get(2).copied().unwrap_or(0);
                if let Some(stream) = self.glk_streams.get_mut(&stream_id)
                    && let GlkStreamTarget::Memory {
                        position, length, ..
                    } = &mut stream.target
                {
                    let base = match seek_mode {
                        0 => 0,
                        1 => *position as i32,
                        2 => *length as i32,
                        _ => *position as i32,
                    };
                    *position = base.saturating_add(offset).max(0) as u32;
                }
                0
            }
            0x0046 => self
                .glk_streams
                .get(&arguments.first().copied().unwrap_or(0))
                .map(|stream| match stream.target {
                    GlkStreamTarget::Memory { position, .. } => position,
                    GlkStreamTarget::Window(_) => 0,
                })
                .unwrap_or(0),
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
                let text = self
                    .memory
                    .c_string(arguments.first().copied().unwrap_or(0))?;
                self.glk_write_text(0, &text);
                0
            }
            0x0083 => {
                let text = self
                    .memory
                    .c_string(arguments.get(1).copied().unwrap_or(0))?;
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
            0x00a0 => (arguments.first().copied().unwrap_or(0) as u8).to_ascii_lowercase() as u32,
            0x00a1 => (arguments.first().copied().unwrap_or(0) as u8).to_ascii_uppercase() as u32,
            0x0086 | 0x0087 | 0x00b0 | 0x00b1 => 0,
            0x00c0 => {
                let event_address = arguments.first().copied().unwrap_or(0);
                self.pending_select = Some(PendingSelect {
                    event_address,
                    destination,
                });
                if self.line_request.is_some() {
                    self.input_window = self
                        .line_request
                        .as_ref()
                        .map(|request| request.window)
                        .unwrap_or(0);
                    self.state = RunState::WaitingForLine;
                } else {
                    self.state = RunState::WaitingForChar;
                }
                return Ok(());
            }
            0x00c1 => {
                let address = arguments.first().copied().unwrap_or(0);
                if address != 0 {
                    self.memory.write32(address, 0)?;
                    self.memory.write32(address + 4, 0)?;
                    self.memory.write32(address + 8, 0)?;
                    self.memory.write32(address + 12, 0)?;
                }
                0
            }
            0x00d0 => {
                self.line_request = Some(LineRequest {
                    buffer: arguments.get(1).copied().unwrap_or(0),
                    max_len: arguments.get(2).copied().unwrap_or(0),
                    unicode: false,
                    window: arguments.first().copied().unwrap_or(0),
                });
                0
            }
            0x00d1 => {
                self.line_request = None;
                0
            }
            0x00d2 => {
                self.char_requested = true;
                self.input_window = arguments.first().copied().unwrap_or(0);
                0
            }
            0x00d3 => {
                self.char_requested = false;
                0
            }
            0x00e0 => {
                let resource = arguments.first().copied().unwrap_or(0);
                let dimensions = self
                    .story
                    .resource(*b"Pict", resource)
                    .and_then(image_dimensions);
                if let Some([width, height]) = dimensions {
                    if let Some(address) = arguments
                        .get(1)
                        .copied()
                        .filter(|address| !matches!(*address, 0 | u32::MAX))
                    {
                        self.memory.write32(address, width)?;
                    }
                    if let Some(address) = arguments
                        .get(2)
                        .copied()
                        .filter(|address| !matches!(*address, 0 | u32::MAX))
                    {
                        self.memory.write32(address, height)?;
                    }
                    1
                } else {
                    0
                }
            }
            0x00e1 | 0x00e2 => {
                let window = arguments.first().copied().unwrap_or(0);
                let resource = arguments.get(1).copied().unwrap_or(0);
                let requested_size = (selector == 0x00e2).then(|| {
                    [
                        arguments.get(4).copied().unwrap_or(0),
                        arguments.get(5).copied().unwrap_or(0),
                    ]
                });
                if let Some(data) = self.story.resource(*b"Pict", resource) {
                    let graphics_window = self
                        .glk_windows
                        .get(&window)
                        .filter(|candidate| candidate.kind == WINTYPE_GRAPHICS);
                    let position = graphics_window
                        .map(|_| {
                            [
                                arguments.get(2).copied().unwrap_or(0) as i32,
                                arguments.get(3).copied().unwrap_or(0) as i32,
                            ]
                        })
                        .unwrap_or([0, 0]);
                    let canvas_size = graphics_window
                        .map(|candidate| [candidate.width, candidate.height])
                        .or_else(|| requested_size.filter(|size| size[0] != 0 && size[1] != 0))
                        .or_else(|| image_dimensions(data))
                        .unwrap_or([640, 480]);
                    self.graphics.push(GraphicsRequest::Draw(ImageRequest {
                        window,
                        resource,
                        data: data.to_vec(),
                        position,
                        requested_size,
                        canvas_size,
                    }));
                    1
                } else {
                    0
                }
            }
            0x00e8 => 0,
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
            0x0120..=0x0124 => 0,
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
            0x0140 => {
                self.char_requested = true;
                self.input_window = arguments.first().copied().unwrap_or(0);
                0
            }
            0x0141 => {
                self.line_request = Some(LineRequest {
                    buffer: arguments.get(1).copied().unwrap_or(0),
                    max_len: arguments.get(2).copied().unwrap_or(0),
                    unicode: true,
                    window: arguments.first().copied().unwrap_or(0),
                });
                0
            }
            _ => {
                self.unsupported_glk.insert(selector);
                0
            }
        };
        self.store_destination(&destination, result, Width::Word)
    }

    fn glk_gestalt(&self, selector: u32, _argument: u32) -> u32 {
        match selector {
            0 => 0x0000_0705,
            1..=2 | 6..=7 | 15 => 1,
            3 => 2,
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

    fn glk_put_unicode_string(&mut self, stream: u32, address: u32) -> Result<(), VmError> {
        let mut cursor = address;
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
        let mut value = self.random_state;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.random_state = value;
        value
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
        let key = self.search_key(key, key_size, options)?;
        let mut index = 0u32;
        while structure_count == u32::MAX || index < structure_count {
            let structure = start.wrapping_add(index.wrapping_mul(structure_size));
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            if candidate == key {
                return Ok(search_result(structure, index, options));
            }
            if options & 0x02 != 0 && candidate.iter().all(|byte| *byte == 0) {
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
        let key = self.search_key(key, key_size, options)?;
        let mut low = 0u32;
        let mut high = structure_count;
        while low < high {
            let index = low + (high - low) / 2;
            let structure = start.wrapping_add(index.wrapping_mul(structure_size));
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            match candidate.cmp(&key) {
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
        let key = self.search_key(key, key_size, options)?;
        let mut remaining = self.memory.len() / 4 + 1;
        while structure != 0 && remaining > 0 {
            let candidate = self.memory_key(structure.wrapping_add(key_offset), key_size)?;
            if candidate == key {
                return Ok(structure);
            }
            if options & 0x02 != 0 && candidate.iter().all(|byte| *byte == 0) {
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

    fn search_key(&self, key: u32, key_size: u32, options: u32) -> Result<Vec<u8>, VmError> {
        if options & 0x01 != 0 {
            return self.memory_key(key, key_size);
        }
        if !matches!(key_size, 1 | 2 | 4) {
            return Err(VmError::InvalidSearchKeySize(key_size));
        }
        let bytes = key.to_be_bytes();
        Ok(bytes[4 - key_size as usize..].to_vec())
    }

    fn memory_key(&self, address: u32, key_size: u32) -> Result<Vec<u8>, VmError> {
        (0..key_size)
            .map(|offset| self.memory.read8(address.wrapping_add(offset)))
            .collect()
    }
}

fn align(value: u32, alignment: u32) -> u32 {
    (value + alignment - 1) & !(alignment - 1)
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

fn floats_equal(a: f32, b: f32, tolerance: f32) -> bool {
    if a.is_nan() || b.is_nan() || tolerance.is_nan() {
        return false;
    }
    if a.is_infinite() && b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= tolerance.abs()
}

fn image_dimensions(data: &[u8]) -> Option<[u32; 2]> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") && data.len() >= 24 {
        return Some([
            u32::from_be_bytes(data[16..20].try_into().ok()?),
            u32::from_be_bytes(data[20..24].try_into().ok()?),
        ]);
    }
    if !data.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut cursor = 2usize;
    while cursor + 4 <= data.len() {
        while data.get(cursor) == Some(&0xff) {
            cursor += 1;
        }
        let marker = *data.get(cursor)?;
        cursor += 1;
        if matches!(marker, 0xd8 | 0xd9) {
            continue;
        }
        let length = u16::from_be_bytes(data.get(cursor..cursor + 2)?.try_into().ok()?) as usize;
        if length < 2 || cursor + length > data.len() {
            return None;
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            if length < 7 {
                return None;
            }
            let height = u16::from_be_bytes(data[cursor + 3..cursor + 5].try_into().ok()?) as u32;
            let width = u16::from_be_bytes(data[cursor + 5..cursor + 7].try_into().ok()?) as u32;
            return Some([width, height]);
        }
        if marker == 0xda {
            return None;
        }
        cursor += length;
    }
    None
}

fn operand_count(opcode: u32) -> Option<usize> {
    Some(match opcode {
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
        | 0x127
        | 0x148..=0x149
        | 0x160
        | 0x170
        | 0x178
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
mod tests {
    use super::*;

    fn push_glk_arguments(vm: &mut Vm, arguments: &[u32]) {
        for argument in arguments.iter().rev() {
            vm.stack.push_u32(*argument).unwrap();
        }
    }

    fn image_with_program(program: &[u8]) -> Vec<u8> {
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
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
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
            0x40, 0x82, 0x00, 0xc0, // push C string address
            0x81, 0x30, 0x12, 0x00, 0x00, 0x82, 0x01, // glk put_string
            0x81, 0x20, // quit
        ];
        let mut image = image_with_program(&program);
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
        image[0xc0..0xc4].copy_from_slice(b"Hex\0");
        update_checksum(&mut image);
        let story = Story::from_bytes(&image, None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        push_glk_arguments(&mut vm, &[0x140, 8, 1, 99]);
        vm.glk(0x0043, 4, Destination::Memory(0x120)).unwrap();
        let stream = vm.memory.read32(0x120).unwrap();
        push_glk_arguments(&mut vm, &[stream]);
        vm.glk(0x0047, 1, Destination::Discard).unwrap();
        push_glk_arguments(&mut vm, &[0xc0]);
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
        push_glk_arguments(&mut vm, &[0xc0]);
        vm.glk(0x0082, 1, Destination::Discard).unwrap();

        assert_eq!(vm.status_text(), "Score: 7");
        assert_eq!(vm.take_output(), "");
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
        assert_eq!(vm.memory.read32(0x124).unwrap(), 1);
        assert_eq!(vm.memory.read32(0x128).unwrap(), 0);
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
