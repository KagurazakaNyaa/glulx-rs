use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
};

use crate::story::StoryImage;

use super::{DecodedInstruction, Operand, VmError, native, operand_count};

/// Selects the VM instruction execution backend.
///
/// Jit asynchronously decodes ROM blocks and executes them through native
/// code. Generated code is runtime-only and never enters serialized VM state.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionBackend {
    #[default]
    #[serde(rename = "interpreter", alias = "Interpreter")]
    Interpreter,
    #[serde(rename = "jit", alias = "Jit")]
    Jit,
}

impl std::str::FromStr for ExecutionBackend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "interpreter" | "interp" => Ok(Self::Interpreter),
            "jit" => Ok(Self::Jit),
            _ => Err(format!(
                "unknown execution backend {value:?}; expected interpreter or jit"
            )),
        }
    }
}

const COMPILE_QUEUE_CAPACITY: usize = 128;
const HOT_BLOCK_THRESHOLD: u8 = 8;
pub(super) const MAX_TRACE_INSTRUCTIONS: usize = 256;
const MAX_BLOCKS: usize = 4096;
const MAX_TRACE_CACHE_BYTES: usize = 64 * 1024 * 1024;
const PC_PAGE_SIZE: usize = 256;
type TracePage = Box<[Option<Arc<NativeBlock>>; PC_PAGE_SIZE]>;

#[derive(Default)]
struct TraceCache {
    pages: Vec<Option<TracePage>>,
    ram_start: u32,
    len: usize,
    bytes: usize,
}

impl TraceCache {
    fn reset(&mut self, ram_start: u32) {
        self.clear();
        let page_count = (ram_start as usize).div_ceil(PC_PAGE_SIZE);
        if self.pages.len() != page_count {
            self.pages.clear();
            self.pages.resize_with(page_count, || None);
        }
        self.ram_start = ram_start;
    }

    fn clear(&mut self) {
        for page in &mut self.pages {
            *page = None;
        }
        self.len = 0;
        self.bytes = 0;
    }

    fn get(&self, address: u32) -> Option<Arc<NativeBlock>> {
        let page = self.pages.get(address as usize / PC_PAGE_SIZE)?.as_ref()?;
        page[address as usize % PC_PAGE_SIZE].clone()
    }

    fn contains(&self, address: u32) -> bool {
        self.pages
            .get(address as usize / PC_PAGE_SIZE)
            .and_then(Option::as_deref)
            .and_then(|page| page.get(address as usize % PC_PAGE_SIZE))
            .is_some_and(Option::is_some)
    }

    fn insert(&mut self, address: u32, block: Arc<NativeBlock>) {
        self.insert_with_limit(address, block, MAX_TRACE_CACHE_BYTES);
    }

    fn insert_with_limit(&mut self, address: u32, block: Arc<NativeBlock>, maximum: usize) {
        if address >= self.ram_start {
            return;
        }
        let bytes = block.byte_len();
        if bytes > maximum {
            return;
        }
        if let Some(old) = self.get(address) {
            self.bytes -= old.byte_len();
            self.pages[address as usize / PC_PAGE_SIZE]
                .as_mut()
                .unwrap()[address as usize % PC_PAGE_SIZE] = None;
            self.len -= 1;
        }
        if self.len >= MAX_BLOCKS || self.bytes.saturating_add(bytes) > maximum {
            self.clear();
        }
        let page_index = address as usize / PC_PAGE_SIZE;
        let offset = address as usize % PC_PAGE_SIZE;
        let page =
            self.pages[page_index].get_or_insert_with(|| Box::new([const { None }; PC_PAGE_SIZE]));
        if page[offset].is_none() {
            self.len += 1;
        }
        page[offset] = Some(block);
        self.bytes += bytes;
    }

    fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = &Arc<NativeBlock>> {
        self.pages
            .iter()
            .filter_map(Option::as_deref)
            .flat_map(|page| page.iter().filter_map(Option::as_ref))
    }
}

/// One decoded instruction in a linear native trace.
///
/// `continuation` is populated only for a branch whose statically selected
/// successor is also part of this trace. Other branches return to the VM
/// dispatcher after committing their computed PC.
#[derive(Clone, Copy)]
pub(super) struct TraceInstruction {
    pub(super) instruction: DecodedInstruction,
    pub(super) continuation: Option<u32>,
    pub(super) branch_target: Option<u32>,
}

impl TraceInstruction {
    #[cfg(test)]
    pub(super) fn plain(instruction: DecodedInstruction) -> Self {
        Self {
            instruction,
            continuation: None,
            branch_target: None,
        }
    }
}

/// A decoded ROM trace and its trace-specific native function.
///
/// The function contains the decoded operands and selected branch path as
/// immediates in its machine code. A guard exits to the VM when the runtime
/// branch does not match that selected path.
pub(super) struct NativeBlock {
    pub(super) instructions: Box<[TraceInstruction]>,
    executor: Option<Arc<native::NativeExecutor>>,
}

impl NativeBlock {
    fn byte_len(&self) -> usize {
        std::mem::size_of_val(self.instructions.as_ref())
            + self
                .executor
                .as_ref()
                .map_or(0, |executor| executor.byte_len())
    }

    pub(super) fn executor(&self) -> Option<Arc<native::NativeExecutor>> {
        self.executor.clone()
    }

    #[cfg(test)]
    fn native_executor_count(&self) -> usize {
        usize::from(self.executor.is_some())
    }
}

struct DecodedTrace {
    instructions: Box<[TraceInstruction]>,
}

struct CompileResult {
    address: u32,
    block: Option<Arc<NativeBlock>>,
}

struct CompilerWorker {
    requests: SyncSender<u32>,
    results: Receiver<CompileResult>,
}

#[derive(Default)]
pub(super) struct JitState {
    blocks: TraceCache,
    unavailable: HashSet<u32>,
    hot_counts: HashMap<u32, u8>,
    pending: HashSet<u32>,
    worker: Option<CompilerWorker>,
    ram_start: u32,
    block_hits: u64,
    compiled_blocks: u64,
    compiled_instructions: u64,
    compile_requests: u64,
    compile_results: u64,
}

impl JitState {
    pub(super) fn start(
        &mut self,
        image: Arc<StoryImage>,
        ram_start: u32,
        entry: u32,
        aot_cache_dir: Option<PathBuf>,
    ) {
        if self.ram_start != ram_start || self.blocks.ram_start != ram_start {
            self.ram_start = ram_start;
            self.blocks.reset(ram_start);
        }
        if self.worker.is_none() {
            let (request_sender, request_receiver) =
                mpsc::sync_channel::<u32>(COMPILE_QUEUE_CAPACITY);
            let (result_sender, result_receiver) = mpsc::channel::<CompileResult>();
            let worker = thread::Builder::new()
                .name("glulx-native-jit".to_owned())
                .spawn(move || {
                    let story_fingerprint = native::story_fingerprint(image.as_slice());
                    let mut compiler = native::NativeCompiler::new().ok();
                    if let Some(directory) = aot_cache_dir
                        && let Some(native_compiler) = compiler.as_mut()
                    {
                        let _ = native_compiler.prepare_aot(&directory, story_fingerprint);
                    }
                    while let Ok(address) = request_receiver.recv() {
                        let block = compile_trace(&image, ram_start, address).map(|decoded| {
                            Arc::new(build_native_block(address, decoded, &mut compiler))
                        });
                        if result_sender
                            .send(CompileResult { address, block })
                            .is_err()
                        {
                            break;
                        }
                    }
                    if let Some(native_compiler) = compiler.as_mut() {
                        native_compiler.finish_aot();
                    }
                })
                .ok();
            if worker.is_some() {
                self.worker = Some(CompilerWorker {
                    requests: request_sender,
                    results: result_receiver,
                });
            }
        }
        self.request(entry);
    }

    pub(super) fn clear(&mut self) {
        // Dropping the sender lets the detached worker stop after its current
        // compilation. The VM never waits for generated code while closing.
        self.worker = None;
        self.blocks.clear();
        self.unavailable.clear();
        self.hot_counts.clear();
        self.pending.clear();
        self.ram_start = 0;
        self.block_hits = 0;
        self.compiled_blocks = 0;
        self.compiled_instructions = 0;
        self.compile_requests = 0;
        self.compile_results = 0;
    }

    pub(super) fn block(&mut self, address: u32) -> Option<Arc<NativeBlock>> {
        // Every result belongs to a pending request. Avoid touching the worker's
        // channel on the steady-state native execution path.
        if !self.pending.is_empty() {
            self.poll();
        }
        if address >= self.ram_start {
            return None;
        }
        let block = self.blocks.get(address);
        if block.is_some() {
            self.block_hits = self.block_hits.wrapping_add(1);
        } else if !self.unavailable.contains(&address) && !self.pending.contains(&address) {
            let count = self.hot_counts.entry(address).or_default();
            *count = count.saturating_add(1);
            if *count >= HOT_BLOCK_THRESHOLD {
                self.request(address);
            }
        }
        block
    }

    #[cfg(test)]
    pub(super) fn native_executor_count(&self) -> usize {
        self.blocks
            .iter()
            .map(|block| block.native_executor_count())
            .sum()
    }

    fn request(&mut self, address: u32) {
        if self.worker.is_none()
            || address >= self.ram_start
            || self.blocks.contains(address)
            || self.unavailable.contains(&address)
            || !self.pending.insert(address)
        {
            return;
        }
        let result = self
            .worker
            .as_ref()
            .expect("worker checked above")
            .requests
            .try_send(address);
        match result {
            Ok(()) => self.compile_requests = self.compile_requests.wrapping_add(1),
            Err(TrySendError::Full(_)) => {
                self.pending.remove(&address);
            }
            Err(TrySendError::Disconnected(_)) => {
                self.pending.remove(&address);
                self.worker = None;
            }
        }
    }

    fn poll(&mut self) {
        loop {
            let result = match self.worker.as_ref().map(|worker| worker.results.try_recv()) {
                Some(Ok(result)) => result,
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.worker = None;
                    break;
                }
            };
            self.pending.remove(&result.address);
            self.hot_counts.remove(&result.address);
            self.compile_results = self.compile_results.wrapping_add(1);
            if let Some(block) = result.block {
                self.compiled_blocks = self.compiled_blocks.wrapping_add(1);
                self.compiled_instructions = self
                    .compiled_instructions
                    .wrapping_add(block.instructions.len() as u64);
                self.blocks.insert(result.address, block);
            } else {
                self.unavailable.insert(result.address);
            }
        }
    }

    pub(super) fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub(super) fn cache_bytes(&self) -> usize {
        self.blocks.bytes
    }

    pub(super) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(super) fn block_hits(&self) -> u64 {
        self.block_hits
    }

    pub(super) fn compiled_instructions(&self) -> u64 {
        self.compiled_instructions
    }

    pub(super) fn compiled_blocks(&self) -> u64 {
        self.compiled_blocks
    }

    pub(super) fn compile_requests(&self) -> u64 {
        self.compile_requests
    }

    pub(super) fn compile_results(&self) -> u64 {
        self.compile_results
    }
}

fn build_native_block(
    address: u32,
    decoded: DecodedTrace,
    compiler: &mut Option<native::NativeCompiler>,
) -> NativeBlock {
    let executor = compiler.as_mut().and_then(|native_compiler| {
        native_compiler
            .executor_trace(address, &decoded.instructions)
            .ok()
    });
    NativeBlock {
        instructions: decoded.instructions,
        executor,
    }
}

fn read8(image: &StoryImage, address: u32) -> Result<u8, VmError> {
    image
        .get(address as usize)
        .copied()
        .ok_or(VmError::MemoryRead(address))
}

fn read16(image: &StoryImage, address: u32) -> Result<u16, VmError> {
    let next = address.checked_add(1).ok_or(VmError::MemoryRead(address))?;
    Ok(u16::from_be_bytes([
        read8(image, address)?,
        read8(image, next)?,
    ]))
}

fn read32(image: &StoryImage, address: u32) -> Result<u32, VmError> {
    let end = address.checked_add(3).ok_or(VmError::MemoryRead(address))?;
    Ok(u32::from_be_bytes([
        read8(image, address)?,
        read8(image, address + 1)?,
        read8(image, address + 2)?,
        read8(image, end)?,
    ]))
}

fn fetch8(image: &StoryImage, cursor: &mut u32) -> Result<u8, VmError> {
    let value = read8(image, *cursor)?;
    *cursor = cursor.wrapping_add(1);
    Ok(value)
}

fn fetch16(image: &StoryImage, cursor: &mut u32) -> Result<u16, VmError> {
    let value = read16(image, *cursor)?;
    *cursor = cursor.wrapping_add(2);
    Ok(value)
}

fn fetch32(image: &StoryImage, cursor: &mut u32) -> Result<u32, VmError> {
    let value = read32(image, *cursor)?;
    *cursor = cursor.wrapping_add(4);
    Ok(value)
}

pub(super) fn decode_rom(
    image: &StoryImage,
    ram_start: u32,
    address: u32,
) -> Result<DecodedInstruction, VmError> {
    let mut cursor = address;
    let opcode = {
        let first = fetch8(image, &mut cursor)?;
        if first < 0x80 {
            first as u32
        } else if first < 0xc0 {
            ((first as u32 & 0x7f) << 8) | fetch8(image, &mut cursor)? as u32
        } else {
            ((first as u32 & 0x3f) << 24)
                | (fetch8(image, &mut cursor)? as u32) << 16
                | (fetch8(image, &mut cursor)? as u32) << 8
                | fetch8(image, &mut cursor)? as u32
        }
    };
    let count = operand_count(opcode).ok_or(VmError::UnsupportedOpcode { opcode, address })?;
    let mut modes = [0u8; 4];
    for mode in modes.iter_mut().take(count.div_ceil(2)) {
        *mode = fetch8(image, &mut cursor)?;
    }
    let mut operands = [const { Operand::Zero }; 8];
    for (index, operand) in operands.iter_mut().enumerate().take(count) {
        let mode = (modes[index / 2] >> ((index % 2) * 4)) & 0xf;
        *operand = match mode {
            0 => Operand::Zero,
            1 => Operand::Constant(fetch8(image, &mut cursor)? as i8 as i32 as u32),
            2 => Operand::Constant(fetch16(image, &mut cursor)? as i16 as i32 as u32),
            3 => Operand::Constant(fetch32(image, &mut cursor)?),
            5 => Operand::Memory(fetch8(image, &mut cursor)? as u32),
            6 => Operand::Memory(fetch16(image, &mut cursor)? as u32),
            7 => Operand::Memory(fetch32(image, &mut cursor)?),
            8 => Operand::Stack,
            9 => Operand::Local(fetch8(image, &mut cursor)? as u32),
            10 => Operand::Local(fetch16(image, &mut cursor)? as u32),
            11 => Operand::Local(fetch32(image, &mut cursor)?),
            13 => Operand::Memory(ram_start.wrapping_add(fetch8(image, &mut cursor)? as u32)),
            14 => Operand::Memory(ram_start.wrapping_add(fetch16(image, &mut cursor)? as u32)),
            15 => Operand::Memory(ram_start.wrapping_add(fetch32(image, &mut cursor)?)),
            _ => return Err(VmError::InvalidAddressMode(mode)),
        };
    }
    Ok(DecodedInstruction {
        address,
        next_pc: cursor,
        opcode,
        operands,
    })
}

fn compile_trace(image: &StoryImage, ram_start: u32, address: u32) -> Option<DecodedTrace> {
    if address >= ram_start {
        return None;
    }
    let mut instructions = Vec::with_capacity(MAX_TRACE_INSTRUCTIONS);
    let mut seen = HashSet::new();
    let mut cursor = address;
    while instructions.len() < MAX_TRACE_INSTRUCTIONS && cursor < ram_start && seen.insert(cursor) {
        let Ok(instruction) = decode_rom(image, ram_start, cursor) else {
            break;
        };
        if !native::is_direct_instruction(&instruction) {
            break;
        }
        let next_pc = instruction.next_pc;
        if next_pc > ram_start {
            break;
        }

        let branch_target = if is_branch_opcode(instruction.opcode) {
            static_branch_target(&instruction)
        } else {
            None
        };
        let continuation = branch_target
            .and_then(|target| trace_successor(image, ram_start, &instruction, target, &seen));
        instructions.push(TraceInstruction {
            instruction,
            continuation,
            branch_target,
        });
        if let Some(next) = continuation {
            cursor = next;
            continue;
        }
        if is_block_terminator(instruction.opcode) || next_pc <= cursor {
            break;
        }
        cursor = next_pc;
    }
    (!instructions.is_empty()).then(|| DecodedTrace {
        instructions: instructions.into_boxed_slice(),
    })
}

fn is_branch_opcode(opcode: u32) -> bool {
    matches!(
        opcode,
        0x20 | 0x22..=0x2d | 0x1c2..=0x1c5 | 0x1c8..=0x1c9 | 0x232..=0x235 | 0x238..=0x239
    )
}

fn branch_offset_index(opcode: u32) -> Option<usize> {
    Some(match opcode {
        0x20 => 0,
        0x22..=0x23 => 1,
        0x24..=0x2d => 2,
        0x1c2..=0x1c5 => 2,
        0x1c8..=0x1c9 => 1,
        0x232..=0x235 => 4,
        0x238..=0x239 => 2,
        _ => return None,
    })
}

fn trace_successor(
    image: &StoryImage,
    ram_start: u32,
    instruction: &DecodedInstruction,
    target: u32,
    seen: &HashSet<u32>,
) -> Option<u32> {
    let fallthrough = instruction.next_pc;
    let candidate = if instruction.opcode == 0x20 || target < instruction.address {
        target
    } else {
        fallthrough
    };
    if candidate >= ram_start || seen.contains(&candidate) {
        return None;
    }
    let next = decode_rom(image, ram_start, candidate).ok()?;
    (native::is_direct_instruction(&next) && next.next_pc <= ram_start).then_some(candidate)
}

fn static_branch_target(instruction: &DecodedInstruction) -> Option<u32> {
    let offset = match instruction.operands[branch_offset_index(instruction.opcode)?] {
        Operand::Constant(offset) if offset != 0 && offset != 1 => offset,
        _ => return None,
    };
    Some(instruction.next_pc.wrapping_add(offset).wrapping_sub(2))
}

/// A block ends after an instruction whose effects must be visible to the host
/// or which changes the next instruction in a way that cannot be inferred from
/// sequential execution.
pub(super) fn is_block_terminator(opcode: u32) -> bool {
    matches!(
        opcode,
        0x20
            | 0x22..=0x23
            | 0x24..=0x2d
            | 0x30..=0x34
            | 0x70..=0x73
            | 0x101
            | 0x103..=0x104
            | 0x120
            | 0x122
            | 0x123..=0x129
            | 0x130
            | 0x160..=0x163
            | 0x170..=0x171
            | 0x178..=0x181
            | 0x1c0..=0x1c5
            | 0x1c8..=0x1c9
            | 0x230..=0x235
            | 0x238..=0x239
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_cache_byte_budget_releases_old_blocks_and_tracks_replacements() {
        let block = || {
            Arc::new(NativeBlock {
                instructions: vec![TraceInstruction::plain(DecodedInstruction {
                    address: 0x40,
                    next_pc: 0x41,
                    opcode: 0,
                    operands: [Operand::Zero; 8],
                })]
                .into_boxed_slice(),
                executor: None,
            })
        };
        let mut cache = TraceCache::default();
        cache.reset(0x100);
        let first = block();
        let maximum = first.byte_len();
        let weak = Arc::downgrade(&first);
        cache.insert_with_limit(0x40, first, maximum);
        cache.insert_with_limit(0x80, block(), maximum);
        assert!(weak.upgrade().is_none());
        assert!(cache.get(0x40).is_none());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes, maximum);
        cache.insert_with_limit(0x80, block(), maximum);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes, maximum);
        cache.reset(0x200);
        assert_eq!((cache.len(), cache.bytes), (0, 0));
    }

    // Explicit channels let tests deliver compilation results without races or
    // depending on the host's native compiler support.
    fn worker_state() -> (JitState, Receiver<u32>, mpsc::Sender<CompileResult>) {
        let (requests, request_receiver) = mpsc::sync_channel(COMPILE_QUEUE_CAPACITY);
        let (result_sender, results) = mpsc::channel();
        let mut state = JitState {
            ram_start: 0x100,
            worker: Some(CompilerWorker { requests, results }),
            ..JitState::default()
        };
        state.blocks.reset(state.ram_start);
        (state, request_receiver, result_sender)
    }

    #[test]
    fn ram_execution_does_not_allocate_hot_counters_or_request_compilation() {
        let (mut state, requests, _results) = worker_state();
        for address in 0x100..0x1000 {
            assert!(state.block(address).is_none());
        }
        assert!(state.hot_counts.is_empty());
        assert!(state.pending.is_empty());
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn failed_compilation_stops_tracking_until_cache_reset() {
        let (mut state, requests, results) = worker_state();
        for _ in 0..HOT_BLOCK_THRESHOLD {
            assert!(state.block(0x40).is_none());
        }
        assert_eq!(requests.try_recv().unwrap(), 0x40);
        results
            .send(CompileResult {
                address: 0x40,
                block: None,
            })
            .unwrap();
        for _ in 0..32 {
            assert!(state.block(0x40).is_none());
        }
        assert!(state.hot_counts.is_empty());
        assert!(state.pending.is_empty());
        assert_eq!(state.compile_results(), 1);
        assert!(matches!(requests.try_recv(), Err(TryRecvError::Empty)));
        state.clear();
        assert!(state.unavailable.is_empty());
    }

    #[test]
    fn cached_hits_still_install_pending_results() {
        let (mut state, requests, results) = worker_state();
        let block = Arc::new(NativeBlock {
            instructions: Box::new([]),
            executor: None,
        });
        state.blocks.insert(0x40, Arc::clone(&block));
        for _ in 0..HOT_BLOCK_THRESHOLD {
            assert!(state.block(0x80).is_none());
        }
        assert_eq!(requests.try_recv().unwrap(), 0x80);
        results
            .send(CompileResult {
                address: 0x80,
                block: Some(block),
            })
            .unwrap();
        assert!(state.block(0x40).is_some());
        assert!(state.block(0x80).is_some());
        assert!(state.pending.is_empty());
        assert!(state.hot_counts.is_empty());
        assert_eq!(state.block_count(), 2);
        assert_eq!(state.compile_results(), 1);
    }
}
