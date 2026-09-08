use std::{cmp::Ordering, collections::HashSet};

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
}

#[derive(Debug, Clone)]
struct PendingSelect {
    event_address: u32,
    destination: Destination,
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
}

impl Vm {
    pub fn new(story: Story) -> Result<Self, VmError> {
        let memory = Memory::new(&story);
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
        self.memory.write32(pending.event_address + 4, 1)?;
        self.memory.write32(pending.event_address + 8, value)?;
        self.memory.write32(pending.event_address + 12, 0)?;
        self.store_destination(&pending.destination, 0, Width::Word)?;
        self.state = RunState::Running;
        Ok(())
    }

    pub fn restart(&mut self) -> Result<(), VmError> {
        self.memory.restart();
        self.stack.clear();
        self.pc = 0;
        self.state = RunState::Running;
        self.io_system = 0;
        self.io_rock = 0;
        self.output.clear();
        self.line_request = None;
        self.char_requested = false;
        self.pending_select = None;
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
                self.emit(char::from(value as u8));
            }
            0x71 => {
                let value = load!(0) as i32;
                self.emit_text(&value.to_string());
            }
            0x72 => {
                let address = load!(0);
                self.stream_string(address)?;
            }
            0x73 => {
                let value = load!(0);
                self.emit(char::from_u32(value).unwrap_or('\u{fffd}'));
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
                let result = if self.memory.resize(size)? { 0 } else { 1 };
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
            0x125 | 0x126 => {
                // Undo is optional. Report failure so stories can disable it cleanly.
                store!(0, 1);
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
                self.io_system = load!(0);
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
        let (destination_type, destination_address) = match destination {
            Destination::Discard => (0, 0),
            Destination::Memory(address) => (1, address),
            Destination::Local(offset) => (2, offset),
            Destination::Stack => (3, 0),
        };
        self.stack.push_u32(destination_type)?;
        self.stack.push_u32(destination_address)?;
        self.stack.push_u32(self.pc)?;
        self.stack.push_u32(self.stack.frame_ptr)?;
        self.enter_function(address, arguments)
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

    fn stream_string(&mut self, address: u32) -> Result<(), VmError> {
        match self.memory.read8(address)? {
            0xe0 => {
                let text = self.memory.c_string(address + 1)?;
                self.emit_text(&text);
                Ok(())
            }
            0xe1 => {
                self.push_call_stub(11, 0, self.pc)?;
                self.resume_compressed(address + 1, 0)
            }
            0xe2 => {
                let mut cursor = address + 4;
                loop {
                    let value = self.memory.read32(cursor)?;
                    if value == 0 {
                        break;
                    }
                    self.emit(char::from_u32(value).unwrap_or('\u{fffd}'));
                    cursor += 4;
                }
                Ok(())
            }
            kind => Err(VmError::InvalidString { address, kind }),
        }
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
            return self.finish_compressed();
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
                1 => return self.finish_compressed(),
                2 => self.emit(char::from(self.memory.read8(node + 1)?)),
                3 => {
                    let text = self.memory.c_string(node + 1)?;
                    self.emit_text(&text);
                }
                4 => {
                    let value = self.memory.read32(node + 1)?;
                    self.emit(char::from_u32(value).unwrap_or('\u{fffd}'));
                }
                5 => {
                    let mut cursor = node + 1;
                    loop {
                        let value = self.memory.read32(cursor)?;
                        if value == 0 {
                            break;
                        }
                        self.emit(char::from_u32(value).unwrap_or('\u{fffd}'));
                        cursor += 4;
                    }
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

                    match self.memory.read8(object)? {
                        0xe0 | 0xe2 => self.stream_string(object)?,
                        0xe1 => {
                            self.push_call_stub(10, u32::from(bit), byte_address)?;
                            return self.resume_compressed(object + 1, 0);
                        }
                        0xc0 | 0xc1 => {
                            self.push_call_stub(10, u32::from(bit), byte_address)?;
                            return self.enter_function(object, &arguments);
                        }
                        object_type => {
                            return Err(VmError::InvalidString {
                                address: object,
                                kind: object_type,
                            });
                        }
                    }
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

    fn finish_compressed(&mut self) -> Result<(), VmError> {
        let frame_ptr = self.stack.pop_raw_u32()?;
        let pc = self.stack.pop_raw_u32()?;
        let destination_address = self.stack.pop_raw_u32()?;
        let destination_type = self.stack.pop_raw_u32()?;
        self.stack.frame_ptr = frame_ptr;
        match destination_type {
            10 => self.resume_compressed(pc, destination_address as u8),
            11 => {
                self.pc = pc;
                Ok(())
            }
            _ => Err(VmError::InvalidCallStub),
        }
    }

    fn emit(&mut self, character: char) {
        if self.io_system == 2 {
            self.output.push(character);
        }
    }

    fn emit_text(&mut self, text: &str) {
        if self.io_system == 2 {
            self.output.push_str(text);
        }
    }

    fn gestalt(&self, selector: u32, argument: u32) -> u32 {
        match selector {
            0 => 0x0003_0103,
            1 => 0x0000_0100,
            2 => 1,
            3 => 0,
            4 => u32::from(matches!(argument, 0 | 2)),
            5 => 1,
            6 => 1,
            7..=10 => 0,
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
                let window = arguments.first().copied().unwrap_or(0);
                if let Some(rock_address) =
                    arguments.get(1).copied().filter(|address| *address != 0)
                {
                    self.memory.write32(rock_address, 0)?;
                }
                u32::from(window == 0)
            }
            0x0021 => 0,
            0x0022 => 1,
            0x0023 => 1,
            0x0024 => {
                if let Some(result_address) =
                    arguments.get(1).copied().filter(|address| *address != 0)
                {
                    self.memory.write32(result_address, 0)?;
                    self.memory.write32(result_address + 4, 0)?;
                }
                0
            }
            0x0025 => {
                if let Some(address) = arguments.get(1).copied().filter(|address| *address != 0) {
                    self.memory.write32(address, 80)?;
                }
                if let Some(address) = arguments.get(2).copied().filter(|address| *address != 0) {
                    self.memory.write32(address, 25)?;
                }
                0
            }
            0x0026..=0x0027 => 0,
            0x0028 => 3,
            0x0029 | 0x0030 => 0,
            0x002a..=0x002b | 0x002d | 0x002f => 0,
            0x002c => 1,
            0x002e => 0,
            0x0040 => 0,
            0x0041 => 0,
            0x0042..=0x0043 | 0x0049 => 0,
            0x0044 => {
                if let Some(result_address) =
                    arguments.get(1).copied().filter(|address| *address != 0)
                {
                    self.memory.write32(result_address, 0)?;
                    self.memory.write32(result_address + 4, 0)?;
                }
                0
            }
            0x0045..=0x0048 => 0,
            0x0080 | 0x0081 => {
                self.output
                    .push(char::from(arguments.last().copied().unwrap_or(0) as u8));
                0
            }
            0x0082 => {
                let text = self
                    .memory
                    .c_string(arguments.first().copied().unwrap_or(0))?;
                self.output.push_str(&text);
                0
            }
            0x0083 => {
                let text = self
                    .memory
                    .c_string(arguments.get(1).copied().unwrap_or(0))?;
                self.output.push_str(&text);
                0
            }
            0x0084 => {
                self.glk_put_buffer(
                    arguments.first().copied().unwrap_or(0),
                    arguments.get(1).copied().unwrap_or(0),
                )?;
                0
            }
            0x0085 => {
                self.glk_put_buffer(
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
                    self.state = RunState::WaitingForLine;
                } else {
                    self.char_requested = true;
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
                });
                0
            }
            0x00d1 => {
                self.line_request = None;
                0
            }
            0x00d2 => {
                self.char_requested = true;
                0
            }
            0x00d3 => {
                self.char_requested = false;
                0
            }
            0x0120..=0x0124 => 0,
            0x0128 | 0x012b => {
                let value = arguments.last().copied().unwrap_or(0);
                self.output
                    .push(char::from_u32(value).unwrap_or('\u{fffd}'));
                0
            }
            0x0129 | 0x012c => {
                let offset = usize::from(selector == 0x012c);
                self.glk_put_unicode_string(arguments.get(offset).copied().unwrap_or(0))?;
                0
            }
            0x012a | 0x012d => {
                let offset = usize::from(selector == 0x012d);
                self.glk_put_unicode_buffer(
                    arguments.get(offset).copied().unwrap_or(0),
                    arguments.get(offset + 1).copied().unwrap_or(0),
                )?;
                0
            }
            0x0140 => {
                self.char_requested = true;
                0
            }
            0x0141 => {
                self.line_request = Some(LineRequest {
                    buffer: arguments.get(1).copied().unwrap_or(0),
                    max_len: arguments.get(2).copied().unwrap_or(0),
                    unicode: true,
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
            1..=3 | 15 => 1,
            _ => 0,
        }
    }

    fn glk_put_buffer(&mut self, address: u32, length: u32) -> Result<(), VmError> {
        for index in 0..length {
            self.output
                .push(char::from(self.memory.read8(address + index)?));
        }
        Ok(())
    }

    fn glk_put_unicode_buffer(&mut self, address: u32, length: u32) -> Result<(), VmError> {
        for index in 0..length {
            let value = self.memory.read32(address + index * 4)?;
            self.output
                .push(char::from_u32(value).unwrap_or('\u{fffd}'));
        }
        Ok(())
    }

    fn glk_put_unicode_string(&mut self, address: u32) -> Result<(), VmError> {
        let mut cursor = address;
        loop {
            let value = self.memory.read32(cursor)?;
            if value == 0 {
                return Ok(());
            }
            self.output
                .push(char::from_u32(value).unwrap_or('\u{fffd}'));
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

fn search_result(address: u32, index: u32, options: u32) -> u32 {
    if options & 0x04 != 0 { index } else { address }
}

fn search_failure(options: u32) -> u32 {
    if options & 0x04 != 0 { u32::MAX } else { 0 }
}

fn operand_count(opcode: u32) -> Option<usize> {
    Some(match opcode {
        0x00 | 0x52 | 0x120 | 0x122 => 0,
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
        | 0x140..=0x141 => 1,
        0x15
        | 0x1b
        | 0x22..=0x23
        | 0x34
        | 0x40..=0x42
        | 0x44..=0x45
        | 0x51
        | 0x53
        | 0x103
        | 0x110
        | 0x148..=0x149
        | 0x160
        | 0x170
        | 0x190..=0x192
        | 0x198..=0x199
        | 0x1a8..=0x1aa
        | 0x1b0..=0x1b5 => 2,
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
        | 0x1b6 => 3,
        0x162 => 4,
        0x163 => 5,
        0x152 => 7,
        0x150..=0x151 => 8,
        _ => return None,
    })
}

#[derive(Debug, Error)]
pub enum VmError {
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
    fn unsupported_undo_reports_failure_without_stopping_the_story() {
        // saveundo to RAM+0x20; restoreundo to RAM+0x24; quit
        let program = [0x81, 0x25, 0x0d, 0x20, 0x81, 0x26, 0x0d, 0x24, 0x81, 0x20];
        let story = Story::from_bytes(&image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();

        assert_eq!(vm.run_steps(8).unwrap(), RunState::Halted);
        assert_eq!(vm.memory.read32(0x120).unwrap(), 1);
        assert_eq!(vm.memory.read32(0x124).unwrap(), 1);
    }
}
