use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    mem::{align_of, offset_of, size_of},
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    ptr::{self, NonNull},
    sync::Arc,
};

use cranelift_codegen::{
    Context,
    control::ControlPlane,
    ir::{
        self, AbiParam, BlockArg, InstBuilder, MemFlagsData, Signature,
        condcodes::{FloatCC, IntCC},
        types,
    },
    isa::{OwnedTargetIsa, TargetFrontendConfig, TargetIsa},
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

use crate::{memory::Memory, story::StoryImage};

use super::{DecodedInstruction, Operand, Stack, Width, jit::TraceInstruction};

const AOT_MAGIC: &[u8; 8] = b"GLXAOT02";
const AOT_VERSION: u32 = 7;
const NATIVE_CODEGEN_VERSION: &str = "cranelift-0.135.2-native-traces-v6";
const MAX_EXECUTOR_BYTES: usize = 4 * 1024 * 1024;
const AOT_FLUSH_BLOCKS: usize = 64;

pub(super) const NATIVE_SLOW_FLAG: u32 = 0x8000_0000;
pub(super) const NATIVE_COUNT_MASK: u32 = !NATIVE_SLOW_FLAG;

/// The C-layout state surface exposed to generated code.
#[repr(C)]
pub(super) struct NativeAbiState {
    pub(super) rom_ptr: *const u8,
    pub(super) ram_ptr: *mut u8,
    pub(super) memory_len: u32,
    pub(super) ram_start: u32,
    pub(super) ram_len: u32,
    pub(super) stack_ptr: *mut u8,
    pub(super) stack_len: u32,
    pub(super) stack_capacity: u32,
    pub(super) stack_maximum: u32,
    pub(super) stack_frame_end: u32,
    pub(super) locals_base: u32,
    pub(super) pc: u32,
    pub(super) random_state: u32,
    pub(super) dirty_min: u32,
    pub(super) dirty_max: u32,
    pub(super) stack_dirty: u32,
    pub(super) rollback_stack_len: u32,
    pub(super) rollback_pc: u32,
    pub(super) rollback_dirty_min: u32,
    pub(super) rollback_dirty_max: u32,
    pub(super) rollback_stack_dirty: u32,
    pub(super) rollback_random_state: u32,
}

/// Host-owned native state. The ABI-visible fields are kept separate from the
/// storage that owns their backing allocations.
pub(super) struct NativeState {
    pub(super) abi: NativeAbiState,
    ram_storage: Vec<u8>,
    rom_storage: Option<Arc<StoryImage>>,
    stack_storage: Vec<u8>,
}

impl Default for NativeState {
    fn default() -> Self {
        Self {
            abi: NativeAbiState {
                rom_ptr: ptr::null(),
                ram_ptr: ptr::null_mut(),
                memory_len: 0,
                ram_start: 0,
                ram_len: 0,
                stack_ptr: ptr::null_mut(),
                stack_len: 0,
                stack_capacity: 0,
                stack_maximum: 0,
                stack_frame_end: 0,
                locals_base: 0,
                pc: 0,
                random_state: 0,
                dirty_min: u32::MAX,
                dirty_max: 0,
                stack_dirty: 0,
                rollback_stack_len: 0,
                rollback_pc: 0,
                rollback_dirty_min: u32::MAX,
                rollback_dirty_max: 0,
                rollback_stack_dirty: 0,
                rollback_random_state: 0,
            },
            ram_storage: Vec::new(),
            rom_storage: None,
            stack_storage: Vec::new(),
        }
    }
}

// The raw pointers are rebuilt from the owned vectors before every native run.
// A VM can be moved between host worker threads while it is not executing.
unsafe impl Send for NativeState {}

impl NativeState {
    pub(super) fn sync_from_vm(
        &mut self,
        memory: &mut Memory,
        stack: &Stack,
        pc: u32,
        random_state: u32,
    ) -> Result<(), super::VmError> {
        self.rom_storage = Some(memory.native_rom());
        let ram_len = (memory.len() - memory.ram_start()) as usize;
        if self.ram_storage.len() != ram_len {
            self.ram_storage = memory.native_ram_snapshot();
            memory.clear_native_dirty_pages();
        } else {
            memory.sync_native_ram_delta(&mut self.ram_storage)?;
        }
        self.stack_storage.clear();
        self.stack_storage
            .try_reserve(
                (stack.maximum.min(1024 * 1024) as usize).saturating_sub(stack.bytes.len()),
            )
            .map_err(|_| super::VmError::MemoryAllocation(stack.bytes.len() as u32))?;
        self.stack_storage.extend_from_slice(&stack.bytes);
        let stack_capacity = self.stack_storage.capacity().min(u32::MAX as usize);
        // Native code writes directly through stack_ptr while stack_len remains
        // the guest-visible logical length. Keep the whole writable range
        // initialized so committing it does not require Vec::set_len.
        self.stack_storage.resize(stack_capacity, 0);

        self.abi.rom_ptr = self
            .rom_storage
            .as_ref()
            .expect("ROM backing installed above")
            .as_slice()
            .as_ptr();
        self.abi.ram_ptr = self.ram_storage.as_mut_ptr();
        self.abi.memory_len = memory.len();
        self.abi.ram_start = memory.ram_start();
        self.abi.ram_len = self.ram_storage.len() as u32;
        self.abi.stack_ptr = self.stack_storage.as_mut_ptr();
        self.abi.stack_len = stack.bytes.len() as u32;
        self.abi.stack_capacity = stack_capacity as u32;
        self.abi.stack_maximum = stack.maximum;
        self.abi.stack_frame_end = stack.current_frame_end;
        self.abi.locals_base = stack.locals_base;
        self.abi.pc = pc;
        self.abi.random_state = random_state;
        self.abi.dirty_min = u32::MAX;
        self.abi.dirty_max = 0;
        self.abi.stack_dirty = 0;
        Ok(())
    }

    pub(super) fn commit_to_vm(
        &mut self,
        memory: &mut Memory,
        stack: &mut Stack,
        pc: &mut u32,
        random_state: &mut u32,
    ) -> Result<(), super::VmError> {
        if self.abi.dirty_min != u32::MAX {
            memory.sync_native_ram_range(
                &self.ram_storage,
                self.abi.dirty_min,
                self.abi.dirty_max,
            )?;
        }

        let stack_len = usize::try_from(self.abi.stack_len)
            .ok()
            .filter(|length| *length <= self.stack_storage.len())
            .ok_or(super::VmError::StackOverflow)?;
        self.stack_storage.truncate(stack_len);
        if self.abi.stack_dirty != 0 || stack.bytes.len() != stack_len {
            stack.bytes.clear();
            stack.bytes.extend_from_slice(&self.stack_storage);
        }
        stack.current_frame_end = self.abi.stack_frame_end;
        stack.locals_base = self.abi.locals_base;
        *pc = self.abi.pc;
        *random_state = self.abi.random_state;

        self.abi.dirty_min = u32::MAX;
        self.abi.dirty_max = 0;
        self.abi.stack_dirty = 0;
        Ok(())
    }
}

pub(super) type NativeBlockFn = unsafe extern "C" fn(*mut NativeAbiState, u32) -> u32;

struct ExecutableMemory {
    ptr: NonNull<u8>,
    len: usize,
}

impl ExecutableMemory {
    fn new(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("native code buffer is empty".to_owned());
        }
        let ptr = allocate_executable(bytes.len())?;
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.as_ptr(), bytes.len());
        }
        if let Err(error) = protect_executable(ptr, bytes.len()) {
            free_executable(ptr, bytes.len());
            return Err(error);
        }
        Ok(Self {
            ptr,
            len: bytes.len(),
        })
    }

    fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        free_executable(self.ptr, self.len);
    }
}

// ExecutableMemory is immutable after construction and can be called by any
// thread. Its owner controls the lifetime of the mapped pages.
unsafe impl Send for ExecutableMemory {}
unsafe impl Sync for ExecutableMemory {}

pub(super) struct NativeExecutor {
    _memory: ExecutableMemory,
    function: NativeBlockFn,
}

impl NativeExecutor {
    pub(super) fn from_bytes(bytes: Vec<u8>) -> Result<Arc<Self>, String> {
        if bytes.len() > MAX_EXECUTOR_BYTES {
            return Err("native code buffer exceeds the AOT limit".to_owned());
        }
        let memory = ExecutableMemory::new(&bytes)?;
        let function = unsafe { std::mem::transmute::<*const u8, NativeBlockFn>(memory.as_ptr()) };
        Ok(Arc::new(Self {
            _memory: memory,
            function,
        }))
    }

    pub(super) fn run(&self, state: &mut NativeState, budget: usize) -> u32 {
        let budget = u32::try_from(budget).unwrap_or(u32::MAX);
        // SAFETY: the executor owns the generated function's executable memory,
        // and this wrapper supplies a live native state for the duration of the call.
        unsafe { (self.function)(&mut state.abi, budget) }
    }
}

// NativeExecutor only owns immutable executable memory and a function pointer.
unsafe impl Send for NativeExecutor {}
unsafe impl Sync for NativeExecutor {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BlockKey {
    address: u32,
    fingerprint: u64,
}

// Cranelift's instruction builder borrows FunctionBuilder mutably. The proxy
// keeps each borrow inside one instruction operation, so nested lowering calls
// remain safe without manufacturing overlapping Rust references.
struct IrBuilder<'function> {
    builder: RefCell<FunctionBuilder<'function>>,
}

struct IrInstBuilder<'builder, 'function> {
    builder: &'builder RefCell<FunctionBuilder<'function>>,
}

#[derive(Clone, Copy)]
struct TracePath {
    done: ir::Block,
    next: ir::Block,
    index: u32,
    continuation: Option<u32>,
    branch_target: Option<u32>,
}

#[derive(Clone, Copy)]
struct TraceBranch {
    condition: ir::Value,
    expected_target: u32,
    taken_target: u32,
    fallthrough: u32,
}

impl<'function> IrBuilder<'function> {
    fn new(builder: FunctionBuilder<'function>) -> Self {
        Self {
            builder: RefCell::new(builder),
        }
    }

    fn ins(&self) -> IrInstBuilder<'_, 'function> {
        IrInstBuilder {
            builder: &self.builder,
        }
    }

    fn create_block(&self) -> ir::Block {
        self.builder.borrow_mut().create_block()
    }

    fn switch_to_block(&self, block: ir::Block) {
        self.builder.borrow_mut().switch_to_block(block)
    }

    fn block_param(&self, block: ir::Block, index: usize) -> ir::Value {
        self.builder.borrow().block_params(block)[index]
    }

    fn append_block_param(&self, block: ir::Block, ty: ir::Type) -> ir::Value {
        self.builder.borrow_mut().append_block_param(block, ty)
    }

    fn append_block_params_for_function_params(&self, block: ir::Block) {
        self.builder
            .borrow_mut()
            .append_block_params_for_function_params(block);
    }

    fn seal_all_blocks(&self) {
        self.builder.borrow_mut().seal_all_blocks();
    }

    fn finalize(self, config: TargetFrontendConfig) {
        self.builder.into_inner().finalize(config);
    }
}

impl<'builder, 'function> IrInstBuilder<'builder, 'function> {
    fn jump(&self, block: ir::Block, args: &[BlockArg]) -> ir::Inst {
        self.builder.borrow_mut().ins().jump(block, args)
    }

    fn brif(
        &self,
        condition: ir::Value,
        then_block: ir::Block,
        then_args: &[BlockArg],
        else_block: ir::Block,
        else_args: &[BlockArg],
    ) -> ir::Inst {
        self.builder
            .borrow_mut()
            .ins()
            .brif(condition, then_block, then_args, else_block, else_args)
    }

    fn return_(&self, values: &[ir::Value]) -> ir::Inst {
        self.builder.borrow_mut().ins().return_(values)
    }

    fn iconst<T: Into<ir::immediates::Imm64>>(&self, ty: ir::Type, value: T) -> ir::Value {
        self.builder.borrow_mut().ins().iconst(ty, value)
    }

    fn icmp<T: Into<ir::condcodes::IntCC>>(
        &self,
        condition: T,
        left: ir::Value,
        right: ir::Value,
    ) -> ir::Value {
        self.builder.borrow_mut().ins().icmp(condition, left, right)
    }

    fn fcmp<T: Into<ir::condcodes::FloatCC>>(
        &self,
        condition: T,
        left: ir::Value,
        right: ir::Value,
    ) -> ir::Value {
        self.builder.borrow_mut().ins().fcmp(condition, left, right)
    }

    fn load<T1: Into<ir::MemFlagsData>, T2: Into<ir::immediates::Offset32>>(
        &self,
        ty: ir::Type,
        flags: T1,
        pointer: ir::Value,
        offset: T2,
    ) -> ir::Value {
        self.builder
            .borrow_mut()
            .ins()
            .load(ty, flags, pointer, offset)
    }

    fn store<T1: Into<ir::MemFlagsData>, T2: Into<ir::immediates::Offset32>>(
        &self,
        flags: T1,
        value: ir::Value,
        pointer: ir::Value,
        offset: T2,
    ) -> ir::Inst {
        self.builder
            .borrow_mut()
            .ins()
            .store(flags, value, pointer, offset)
    }
}

macro_rules! value_ops {
    ($(fn $name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&self, $($arg: $ty),*) -> ir::Value {
                self.builder.borrow_mut().ins().$name($($arg),*)
            }
        )*
    };
}

impl<'builder, 'function> IrInstBuilder<'builder, 'function> {
    value_ops! {
        fn iadd(left: ir::Value, right: ir::Value);
        fn isub(left: ir::Value, right: ir::Value);
        fn imul(left: ir::Value, right: ir::Value);
        fn sdiv(left: ir::Value, right: ir::Value);
        fn srem(left: ir::Value, right: ir::Value);
        fn urem(left: ir::Value, right: ir::Value);
        fn band(left: ir::Value, right: ir::Value);
        fn bor(left: ir::Value, right: ir::Value);
        fn bxor(left: ir::Value, right: ir::Value);
        fn bnot(value: ir::Value);
        fn ineg(value: ir::Value);
        fn ishl(left: ir::Value, right: ir::Value);
        fn ushr(left: ir::Value, right: ir::Value);
        fn sshr(left: ir::Value, right: ir::Value);
        fn select(condition: ir::Value, when_true: ir::Value, when_false: ir::Value);
        fn bswap(value: ir::Value);
        fn fadd(left: ir::Value, right: ir::Value);
        fn fsub(left: ir::Value, right: ir::Value);
        fn fmul(left: ir::Value, right: ir::Value);
        fn fdiv(left: ir::Value, right: ir::Value);
        fn fneg(value: ir::Value);
        fn floor(value: ir::Value);
        fn ireduce(ty: ir::Type, value: ir::Value);
        fn uextend(ty: ir::Type, value: ir::Value);
        fn sextend(ty: ir::Type, value: ir::Value);
        fn fpromote(ty: ir::Type, value: ir::Value);
        fn fdemote(ty: ir::Type, value: ir::Value);
        fn fcvt_from_sint(ty: ir::Type, value: ir::Value);
    }

    fn bitcast<T: Into<ir::MemFlagsData>>(
        &self,
        ty: ir::Type,
        flags: T,
        value: ir::Value,
    ) -> ir::Value {
        self.builder.borrow_mut().ins().bitcast(ty, flags, value)
    }
}

pub(super) struct NativeCompiler {
    isa: OwnedTargetIsa,
    target_key: String,
    executors: HashMap<BlockKey, Arc<NativeExecutor>>,
    aot_path: Option<PathBuf>,
    aot_story_fingerprint: u64,
    aot_entries: HashMap<BlockKey, Vec<u8>>,
    aot_index: Vec<BlockKey>,
    aot_dirty_blocks: usize,
}

impl NativeCompiler {
    pub(super) fn new() -> Result<Self, String> {
        if size_of::<*const u8>() != size_of::<u64>() {
            return Err("native JIT requires a 64-bit target".to_owned());
        }
        let mut flags = settings::builder();
        flags
            .set("is_pic", "false")
            .map_err(|error| format!("native JIT flags: {error}"))?;
        flags
            .set("use_colocated_libcalls", "false")
            .map_err(|error| format!("native JIT flags: {error}"))?;
        flags
            .set("opt_level", "speed")
            .map_err(|error| format!("native JIT flags: {error}"))?;
        let builder =
            cranelift_native::builder().map_err(|error| format!("native JIT target: {error}"))?;
        let isa = builder
            .finish(settings::Flags::new(flags))
            .map_err(|error| format!("native JIT ISA: {error}"))?;
        let target_key = format!(
            "{}|{}|{}|state-size={}|state-align={}|instruction-size={}|instruction-align={}",
            isa,
            NATIVE_CODEGEN_VERSION,
            AOT_VERSION,
            size_of::<NativeState>(),
            align_of::<NativeState>(),
            size_of::<TraceInstruction>(),
            align_of::<TraceInstruction>()
        );
        Ok(Self {
            isa,
            target_key,
            executors: HashMap::new(),
            aot_path: None,
            aot_story_fingerprint: 0,
            aot_entries: HashMap::new(),
            aot_index: Vec::new(),
            aot_dirty_blocks: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn executor(
        &mut self,
        address: u32,
        instructions: &[DecodedInstruction],
    ) -> Result<Arc<NativeExecutor>, String> {
        let trace = instructions
            .iter()
            .copied()
            .map(TraceInstruction::plain)
            .collect::<Vec<_>>();
        self.executor_trace(address, &trace)
    }

    pub(super) fn executor_trace(
        &mut self,
        address: u32,
        instructions: &[TraceInstruction],
    ) -> Result<Arc<NativeExecutor>, String> {
        if instructions.is_empty() || instructions.len() > super::jit::MAX_TRACE_INSTRUCTIONS {
            return Err(format!(
                "invalid native block length {}",
                instructions.len()
            ));
        }
        let key = BlockKey {
            address,
            fingerprint: block_fingerprint(address, instructions),
        };
        if let Some(executor) = self.executors.get(&key) {
            return Ok(executor.clone());
        }
        let code = if let Ok(index) = self
            .aot_index
            .binary_search_by_key(&(key.address, key.fingerprint), |candidate| {
                (candidate.address, candidate.fingerprint)
            }) {
            self.aot_entries
                .get(&self.aot_index[index])
                .cloned()
                .expect("AOT index entry must have a payload")
        } else if let Some(code) = self.aot_entries.get(&key) {
            code.clone()
        } else {
            let code = catch_unwind(AssertUnwindSafe(|| {
                compile_native_trace(self.isa.as_ref(), address, instructions)
            }))
            .map_err(|_| "native block compiler panicked".to_owned())??;
            self.aot_entries.insert(key, code.clone());
            if self.aot_path.is_some() {
                self.aot_dirty_blocks += 1;
            }
            if self.aot_dirty_blocks >= AOT_FLUSH_BLOCKS {
                let _ = self.persist_aot();
                self.aot_dirty_blocks = 0;
            }
            code
        };
        let executor = NativeExecutor::from_bytes(code)?;
        self.executors.insert(key, executor.clone());
        Ok(executor)
    }

    pub(super) fn prepare_aot(
        &mut self,
        directory: &Path,
        story_fingerprint: u64,
    ) -> Result<(), String> {
        self.aot_story_fingerprint = story_fingerprint;
        let path = aot_path(directory, story_fingerprint, &self.target_key);
        self.aot_entries = fs::read(&path)
            .ok()
            .and_then(|bytes| load_aot(&bytes, story_fingerprint, &self.target_key).ok())
            .unwrap_or_default();
        self.aot_index = self.aot_entries.keys().copied().collect();
        self.aot_index
            .sort_unstable_by_key(|key| (key.address, key.fingerprint));
        self.aot_path = Some(path);
        self.aot_dirty_blocks = 0;
        self.persist_aot()
    }

    pub(super) fn finish_aot(&mut self) {
        if self.aot_dirty_blocks != 0 {
            let _ = self.persist_aot();
            self.aot_dirty_blocks = 0;
        }
    }

    fn persist_aot(&self) -> Result<(), String> {
        let Some(path) = &self.aot_path else {
            return Ok(());
        };
        let mut code = Vec::new();
        code.extend_from_slice(AOT_MAGIC);
        push_u32(&mut code, AOT_VERSION);
        push_u64(&mut code, self.aot_story_fingerprint);
        let target = self.target_key.as_bytes();
        push_u32(
            &mut code,
            u32::try_from(target.len()).map_err(|_| "AOT target key is too long")?,
        );
        code.extend_from_slice(target);
        let mut entries = self.aot_entries.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(key, _)| (key.address, key.fingerprint));
        push_u32(
            &mut code,
            u32::try_from(entries.len()).map_err(|_| "too many AOT blocks")?,
        );
        for (key, bytes) in entries {
            push_u32(&mut code, key.address);
            push_u64(&mut code, key.fingerprint);
            push_u32(
                &mut code,
                u32::try_from(bytes.len()).map_err(|_| "AOT native function is too large")?,
            );
            code.extend_from_slice(bytes);
        }
        let checksum = fnv1a(&code);
        push_u64(&mut code, checksum);

        let directory = path
            .parent()
            .ok_or_else(|| "AOT cache path has no parent".to_owned())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("create AOT cache directory: {error}"))?;
        let temporary = directory.join(format!(
            ".glulx-aot-{}-{}.tmp",
            std::process::id(),
            key_hash(path)
        ));
        fs::write(&temporary, code).map_err(|error| format!("write AOT cache: {error}"))?;
        if let Err(error) = fs::rename(&temporary, path) {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                fs::remove_file(path)
                    .and_then(|()| fs::rename(&temporary, path))
                    .map_err(|replace_error| format!("install AOT cache: {replace_error}"))?;
            } else {
                let _ = fs::remove_file(&temporary);
                return Err(format!("install AOT cache: {error}"));
            }
        }
        Ok(())
    }
}

const ROM_PTR: i32 = offset_of!(NativeAbiState, rom_ptr) as i32;
const RAM_PTR: i32 = offset_of!(NativeAbiState, ram_ptr) as i32;
const MEMORY_LEN: i32 = offset_of!(NativeAbiState, memory_len) as i32;
const RAM_START: i32 = offset_of!(NativeAbiState, ram_start) as i32;
const STACK_PTR: i32 = offset_of!(NativeAbiState, stack_ptr) as i32;
const STACK_LEN: i32 = offset_of!(NativeAbiState, stack_len) as i32;
const STACK_CAPACITY: i32 = offset_of!(NativeAbiState, stack_capacity) as i32;
const STACK_MAXIMUM: i32 = offset_of!(NativeAbiState, stack_maximum) as i32;
const STACK_FRAME_END: i32 = offset_of!(NativeAbiState, stack_frame_end) as i32;
const LOCALS_BASE: i32 = offset_of!(NativeAbiState, locals_base) as i32;
const PC: i32 = offset_of!(NativeAbiState, pc) as i32;
const RANDOM_STATE: i32 = offset_of!(NativeAbiState, random_state) as i32;
const DIRTY_MIN: i32 = offset_of!(NativeAbiState, dirty_min) as i32;
const DIRTY_MAX: i32 = offset_of!(NativeAbiState, dirty_max) as i32;
const STACK_DIRTY: i32 = offset_of!(NativeAbiState, stack_dirty) as i32;
const ROLLBACK_STACK_LEN: i32 = offset_of!(NativeAbiState, rollback_stack_len) as i32;
const ROLLBACK_PC: i32 = offset_of!(NativeAbiState, rollback_pc) as i32;
const ROLLBACK_DIRTY_MIN: i32 = offset_of!(NativeAbiState, rollback_dirty_min) as i32;
const ROLLBACK_DIRTY_MAX: i32 = offset_of!(NativeAbiState, rollback_dirty_max) as i32;
const ROLLBACK_STACK_DIRTY: i32 = offset_of!(NativeAbiState, rollback_stack_dirty) as i32;
const ROLLBACK_RANDOM_STATE: i32 = offset_of!(NativeAbiState, rollback_random_state) as i32;

fn compile_native_trace(
    isa: &dyn TargetIsa,
    address: u32,
    instructions: &[TraceInstruction],
) -> Result<Vec<u8>, String> {
    let pointer_type = isa.frontend_config().pointer_type();
    if pointer_type != types::I64 {
        return Err("native JIT currently requires 64-bit pointers".to_owned());
    }
    let mut context = Context::new();
    context.func.signature = Signature::new(isa.default_call_conv());
    context
        .func
        .signature
        .params
        .extend([AbiParam::new(pointer_type), AbiParam::new(types::I32)]);
    context
        .func
        .signature
        .returns
        .push(AbiParam::new(types::I32));
    context.func.name = ir::UserFuncName::user(0, address);

    let mut builder_context = FunctionBuilderContext::new();
    let builder = IrBuilder::new(FunctionBuilder::new(
        &mut context.func,
        &mut builder_context,
    ));
    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    let state = builder.block_param(entry, 0);
    let budget = builder.block_param(entry, 1);

    let done = builder.create_block();
    builder.append_block_param(done, types::I32);
    let slow = builder.create_block();
    builder.append_block_param(slow, types::I32);

    let mut current = entry;
    let mut terminated = false;
    for (index, trace_instruction) in instructions.iter().enumerate() {
        builder.switch_to_block(current);
        let execute = builder.create_block();
        let index_value = builder.ins().iconst(types::I32, index as i64);
        let enough = builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThan, budget, index_value);
        builder
            .ins()
            .brif(enough, execute, &[], done, &[BlockArg::Value(index_value)]);
        builder.switch_to_block(execute);

        // A failed native guard returns to the interpreter. These values make
        // that transition transactional for stack and memory metadata.
        let next = builder.create_block();
        copy_field(&builder, state, STACK_LEN, ROLLBACK_STACK_LEN);
        copy_field(&builder, state, PC, ROLLBACK_PC);
        copy_field(&builder, state, DIRTY_MIN, ROLLBACK_DIRTY_MIN);
        copy_field(&builder, state, DIRTY_MAX, ROLLBACK_DIRTY_MAX);
        copy_field(&builder, state, STACK_DIRTY, ROLLBACK_STACK_DIRTY);
        copy_field(&builder, state, RANDOM_STATE, ROLLBACK_RANDOM_STATE);
        terminated = emit_instruction(
            &builder,
            state,
            slow,
            done,
            next,
            index as u32,
            trace_instruction,
        );
        if terminated {
            break;
        }
        current = next;
    }
    if !terminated {
        builder.switch_to_block(current);
        let count = builder.ins().iconst(
            types::I32,
            instructions.len().try_into().unwrap_or(i64::MAX),
        );
        builder.ins().jump(done, &[BlockArg::Value(count)]);
    }

    builder.switch_to_block(slow);
    copy_field(&builder, state, ROLLBACK_STACK_LEN, STACK_LEN);
    copy_field(&builder, state, ROLLBACK_PC, PC);
    copy_field(&builder, state, ROLLBACK_DIRTY_MIN, DIRTY_MIN);
    copy_field(&builder, state, ROLLBACK_DIRTY_MAX, DIRTY_MAX);
    copy_field(&builder, state, ROLLBACK_STACK_DIRTY, STACK_DIRTY);
    copy_field(&builder, state, ROLLBACK_RANDOM_STATE, RANDOM_STATE);
    let slow_count = builder.block_param(slow, 0);
    let slow_flag = builder
        .ins()
        .iconst(types::I32, i64::from(NATIVE_SLOW_FLAG as i32));
    let slow_result = builder.ins().bor(slow_flag, slow_count);
    builder.ins().return_(&[slow_result]);

    builder.switch_to_block(done);
    let count = builder.block_param(done, 0);
    builder.ins().return_(&[count]);
    builder.seal_all_blocks();
    builder.finalize(isa.frontend_config());

    let mut control = ControlPlane::default();
    context
        .compile(isa, &mut control)
        .map_err(|error| format!("native block compilation: {error:?}"))?;
    let compiled = context
        .take_compiled_code()
        .ok_or_else(|| "native compiler returned no code".to_owned())?;
    if !compiled.buffer.relocs().is_empty() {
        return Err("native block unexpectedly contains relocations".to_owned());
    }
    Ok(compiled.code_buffer().to_vec())
}

fn emit_instruction(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    done: ir::Block,
    next: ir::Block,
    index: u32,
    trace_instruction: &TraceInstruction,
) -> bool {
    let instruction = &trace_instruction.instruction;
    let continuation = trace_instruction.continuation;
    if !is_direct_instruction(instruction) {
        emit_slow(builder, slow, index);
        return true;
    }

    let next_pc = iconst(builder, types::I32, i64::from(instruction.next_pc));
    store_field(builder, state, PC, next_pc);

    match instruction.opcode {
        0x00 => {}
        0x10..=0x14 | 0x18..=0x1a => {
            let a = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let b = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                Width::Word,
            );
            let value = match instruction.opcode {
                0x10 => builder.ins().iadd(a, b),
                0x11 => builder.ins().isub(a, b),
                0x12 => builder.ins().imul(a, b),
                0x13 | 0x14 => {
                    let nonzero =
                        builder
                            .ins()
                            .icmp(IntCC::NotEqual, b, iconst(builder, types::I32, 0));
                    let min = builder.ins().icmp(
                        IntCC::Equal,
                        a,
                        iconst(builder, types::I32, i64::from(i32::MIN)),
                    );
                    let minus_one =
                        builder
                            .ins()
                            .icmp(IntCC::Equal, b, iconst(builder, types::I32, -1));
                    let overflow = builder.ins().band(min, minus_one);
                    let nonzero_block = builder.create_block();
                    let valid = builder.create_block();
                    builder.ins().brif(
                        nonzero,
                        nonzero_block,
                        &[],
                        slow,
                        &[BlockArg::Value(iconst(
                            builder,
                            types::I32,
                            i64::from(index),
                        ))],
                    );
                    builder.switch_to_block(nonzero_block);
                    builder.ins().brif(
                        overflow,
                        slow,
                        &[BlockArg::Value(iconst(
                            builder,
                            types::I32,
                            i64::from(index),
                        ))],
                        valid,
                        &[],
                    );
                    builder.switch_to_block(valid);
                    if instruction.opcode == 0x13 {
                        builder.ins().sdiv(a, b)
                    } else {
                        builder.ins().srem(a, b)
                    }
                }
                0x18 => builder.ins().band(a, b),
                0x19 => builder.ins().bor(a, b),
                _ => builder.ins().bxor(a, b),
            };
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[2],
                value,
                Width::Word,
            );
        }
        0x15 => {
            let operand = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let value = builder.ins().ineg(operand);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x1b => {
            let operand = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let value = builder.ins().bnot(operand);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x1c..=0x1e => {
            let a = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let b = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                Width::Word,
            );
            let too_large = builder.ins().icmp(
                IntCC::UnsignedGreaterThanOrEqual,
                b,
                iconst(builder, types::I32, 32),
            );
            let shifted = match instruction.opcode {
                0x1c => builder.ins().ishl(a, b),
                0x1d => builder.ins().sshr(a, b),
                _ => builder.ins().ushr(a, b),
            };
            let fallback = if instruction.opcode == 0x1d {
                let negative =
                    builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, a, iconst(builder, types::I32, 0));
                builder.ins().select(
                    negative,
                    iconst(builder, types::I32, -1),
                    iconst(builder, types::I32, 0),
                )
            } else {
                iconst(builder, types::I32, 0)
            };
            let value = builder.ins().select(too_large, fallback, shifted);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[2],
                value,
                Width::Word,
            );
        }
        0x20 => {
            if let Some(target) = continuation {
                emit_trace_jump(builder, state, next, target);
                return false;
            }
            let offset = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            emit_direct_branch(builder, state, slow, done, index, offset);
            return true;
        }
        0x22..=0x2d => {
            let comparison = match instruction.opcode {
                0x22 => IntCC::Equal,
                0x23 => IntCC::NotEqual,
                0x24 => IntCC::Equal,
                0x25 => IntCC::NotEqual,
                0x26 => IntCC::SignedLessThan,
                0x27 => IntCC::SignedGreaterThanOrEqual,
                0x28 => IntCC::SignedGreaterThan,
                0x29 => IntCC::SignedLessThanOrEqual,
                0x2a => IntCC::UnsignedLessThan,
                0x2b => IntCC::UnsignedGreaterThanOrEqual,
                0x2c => IntCC::UnsignedGreaterThan,
                _ => IntCC::UnsignedLessThanOrEqual,
            };
            let left = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let right = if instruction.opcode <= 0x23 {
                iconst(builder, types::I32, 0)
            } else {
                emit_load_operand(
                    builder,
                    state,
                    slow,
                    index,
                    &instruction.operands[1],
                    Width::Word,
                )
            };
            let condition = builder.ins().icmp(comparison, left, right);
            let offset_index = if instruction.opcode <= 0x23 { 1 } else { 2 };
            if let Some(target) = continuation {
                emit_trace_conditional_branch(
                    builder,
                    state,
                    TraceBranch {
                        condition,
                        expected_target: target,
                        taken_target: trace_instruction
                            .branch_target
                            .expect("trace branch target"),
                        fallthrough: instruction.next_pc,
                    },
                    done,
                    next,
                    index,
                );
                return false;
            }
            let offset = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[offset_index],
                Width::Word,
            );
            let taken = builder.create_block();
            let fallthrough = builder.create_block();
            builder.ins().brif(condition, taken, &[], fallthrough, &[]);
            builder.switch_to_block(fallthrough);
            builder.ins().jump(
                done,
                &[BlockArg::Value(iconst(
                    builder,
                    types::I32,
                    i64::from(index + 1),
                ))],
            );
            builder.switch_to_block(taken);
            emit_direct_branch(builder, state, slow, done, index, offset);
            return true;
        }
        0x40 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x41 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Short,
            );
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Short,
            );
        }
        0x42 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Byte,
            );
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Byte,
            );
        }
        0x44 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Short,
            );
            let value = builder.ins().sextend(types::I32, value);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x45 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Byte,
            );
            let value = builder.ins().sextend(types::I32, value);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x190 | 0x198..=0x199 | 0x1a0..=0x1a3 => {
            emit_float32_value(builder, state, slow, index, instruction);
        }
        0x1c2..=0x1c5 | 0x1c8..=0x1c9 => {
            return emit_float32_branch(
                builder,
                state,
                slow,
                instruction,
                TracePath {
                    done,
                    next,
                    index,
                    continuation,
                    branch_target: trace_instruction.branch_target,
                },
            );
        }
        0x200 | 0x203 | 0x204 | 0x208..=0x209 | 0x210..=0x213 => {
            emit_float64_value(builder, state, slow, index, instruction);
        }
        0x232..=0x235 | 0x238..=0x239 => {
            return emit_float64_branch(
                builder,
                state,
                slow,
                instruction,
                TracePath {
                    done,
                    next,
                    index,
                    continuation,
                    branch_target: trace_instruction.branch_target,
                },
            );
        }
        0x48..=0x4f => emit_array_instruction(builder, state, slow, index, instruction),
        0x50 => {
            let length = builder.ins().isub(
                load_field(builder, state, STACK_LEN),
                load_field(builder, state, STACK_FRAME_END),
            );
            let length = builder.ins().ushr(length, iconst(builder, types::I32, 2));
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                length,
                Width::Word,
            );
        }
        0x51 => {
            let depth = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let too_large = builder.ins().icmp(
                IntCC::UnsignedGreaterThan,
                depth,
                iconst(builder, types::I32, 0x3fff_fffe),
            );
            let valid = builder.create_block();
            builder.ins().brif(
                too_large,
                slow,
                &[BlockArg::Value(iconst(
                    builder,
                    types::I32,
                    i64::from(index),
                ))],
                valid,
                &[],
            );
            builder.switch_to_block(valid);
            let count = builder.ins().imul(
                builder.ins().iadd(depth, iconst(builder, types::I32, 1)),
                iconst(builder, types::I32, 4),
            );
            let ptr = guard_stack_range(builder, state, slow, index, count);
            let value = load_stack_at(builder, ptr, Width::Word);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                value,
                Width::Word,
            );
        }
        0x52 => {
            let ptr =
                guard_stack_range(builder, state, slow, index, iconst(builder, types::I32, 8));
            let top = load_stack_at(builder, ptr, Width::Word);
            let below = load_stack_at_offset(builder, ptr, 4, Width::Word);
            store_stack_at(builder, ptr, below, Width::Word);
            store_stack_at_offset(builder, ptr, 4, top, Width::Word);
            store_field(builder, state, STACK_DIRTY, iconst(builder, types::I32, 1));
        }
        0x102 => emit_store_operand(
            builder,
            state,
            slow,
            index,
            &instruction.operands[0],
            load_field(builder, state, MEMORY_LEN),
            Width::Word,
        ),
        0x110 => {
            emit_random_instruction(builder, state, slow, index, instruction);
        }
        0x104 => {
            let value = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            store_field(builder, state, PC, value);
            builder.ins().jump(
                done,
                &[BlockArg::Value(iconst(
                    builder,
                    types::I32,
                    i64::from(index + 1),
                ))],
            );
            return true;
        }
        0x111 => {
            let seed = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let valid = builder.create_block();
            let nonzero = builder
                .ins()
                .icmp(IntCC::NotEqual, seed, iconst(builder, types::I32, 0));
            builder.ins().brif(
                nonzero,
                valid,
                &[],
                slow,
                &[BlockArg::Value(iconst(
                    builder,
                    types::I32,
                    i64::from(index),
                ))],
            );
            builder.switch_to_block(valid);
            store_field(builder, state, RANDOM_STATE, seed);
        }
        _ => unreachable!("is_direct_instruction admitted an unsupported opcode"),
    }
    builder.ins().jump(next, &[]);
    false
}

pub(super) fn is_direct_instruction(instruction: &DecodedInstruction) -> bool {
    let destination = |index: usize| !matches!(instruction.operands[index], Operand::Constant(_));
    match instruction.opcode {
        0x00 => true,
        0x10..=0x14 | 0x18..=0x1a => destination(2),
        0x15 | 0x1b => destination(1),
        0x1c..=0x1e => destination(2),
        0x20 | 0x22..=0x2d => true,
        0x40..=0x42 | 0x44..=0x45 => destination(1),
        0x48..=0x4a => destination(2),
        0x4b..=0x4f => true,
        0x50 => destination(0),
        0x51 => destination(1),
        0x52 => true,
        0x190 | 0x198..=0x199 => destination(1),
        0x1a0..=0x1a3 => destination(2),
        0x1c2..=0x1c5 | 0x1c8..=0x1c9 => true,
        0x200 | 0x203 => destination(1) && destination(2),
        0x204 => destination(2),
        0x208..=0x209 => destination(2) && destination(3),
        0x210..=0x213 => destination(4) && destination(5),
        0x232..=0x235 | 0x238..=0x239 => true,
        0x102 => destination(0),
        0x104 | 0x110 | 0x111 => true,
        _ => false,
    }
}

fn emit_random_instruction(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    instruction: &DecodedInstruction,
) {
    let range = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[0],
        Width::Word,
    );
    let state_value = load_field(builder, state, RANDOM_STATE);
    let state_value = builder.ins().iadd(
        state_value,
        iconst(builder, types::I32, i64::from(0x9e37_79b9u32 as i32)),
    );
    store_field(builder, state, RANDOM_STATE, state_value);
    let mixed = builder.ins().bxor(
        state_value,
        builder
            .ins()
            .ushr(state_value, iconst(builder, types::I32, 16)),
    );
    let mixed = builder.ins().imul(
        mixed,
        iconst(builder, types::I32, i64::from(0x21f0_aaadu32 as i32)),
    );
    let mixed = builder.ins().bxor(
        mixed,
        builder.ins().ushr(mixed, iconst(builder, types::I32, 15)),
    );
    let mixed = builder.ins().imul(
        mixed,
        iconst(builder, types::I32, i64::from(0x735a_2d97u32 as i32)),
    );
    let raw = builder.ins().bxor(
        mixed,
        builder.ins().ushr(mixed, iconst(builder, types::I32, 15)),
    );

    let positive = builder.create_block();
    let non_positive = builder.create_block();
    let negative = builder.create_block();
    let zero = builder.create_block();
    let result = builder.create_block();
    builder.append_block_param(result, types::I32);
    let positive_condition = builder.ins().icmp(
        IntCC::SignedGreaterThan,
        range,
        iconst(builder, types::I32, 0),
    );
    builder
        .ins()
        .brif(positive_condition, positive, &[], non_positive, &[]);

    builder.switch_to_block(positive);
    let value = builder.ins().urem(raw, range);
    builder.ins().jump(result, &[BlockArg::Value(value)]);

    builder.switch_to_block(non_positive);
    let negative_condition =
        builder
            .ins()
            .icmp(IntCC::SignedLessThan, range, iconst(builder, types::I32, 0));
    builder
        .ins()
        .brif(negative_condition, negative, &[], zero, &[]);

    builder.switch_to_block(negative);
    let magnitude = builder.ins().ineg(range);
    let value = builder.ins().ineg(builder.ins().urem(raw, magnitude));
    builder.ins().jump(result, &[BlockArg::Value(value)]);

    builder.switch_to_block(zero);
    builder.ins().jump(result, &[BlockArg::Value(raw)]);
    builder.switch_to_block(result);
    let value = builder.block_param(result, 0);
    emit_store_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[1],
        value,
        Width::Word,
    );
}

fn emit_float32_value(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    instruction: &DecodedInstruction,
) {
    let first = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[0],
        Width::Word,
    );
    let value = match instruction.opcode {
        0x190 => builder.ins().fcvt_from_sint(types::F32, first),
        0x198 => {
            let negated = builder
                .ins()
                .fneg(bitcast_float(builder, first, types::F32));
            let rounded = builder.ins().floor(negated);
            builder.ins().fneg(rounded)
        }
        0x199 => builder
            .ins()
            .floor(bitcast_float(builder, first, types::F32)),
        0x1a0..=0x1a3 => {
            let second = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                Width::Word,
            );
            let a = bitcast_float(builder, first, types::F32);
            let b = bitcast_float(builder, second, types::F32);
            match instruction.opcode {
                0x1a0 => builder.ins().fadd(a, b),
                0x1a1 => builder.ins().fsub(a, b),
                0x1a2 => builder.ins().fmul(a, b),
                _ => builder.ins().fdiv(a, b),
            }
        }
        _ => unreachable!("is_direct_instruction admitted an unsupported float opcode"),
    };
    let bits = builder.ins().bitcast(types::I32, native_flags(), value);
    let destination = if instruction.opcode == 0x190 {
        1
    } else if instruction.opcode >= 0x1a0 {
        2
    } else {
        1
    };
    emit_store_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[destination],
        bits,
        Width::Word,
    );
}

fn emit_float32_branch(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    instruction: &DecodedInstruction,
    path: TracePath,
) -> bool {
    let TracePath {
        done,
        next,
        index,
        continuation,
        branch_target,
    } = path;
    let first = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[0],
        Width::Word,
    );
    let (condition, offset_index) = match instruction.opcode {
        0x1c2..=0x1c5 => {
            let second = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[1],
                Width::Word,
            );
            let comparison = match instruction.opcode {
                0x1c2 => FloatCC::LessThan,
                0x1c3 => FloatCC::LessThanOrEqual,
                0x1c4 => FloatCC::GreaterThan,
                _ => FloatCC::GreaterThanOrEqual,
            };
            (
                builder.ins().fcmp(
                    comparison,
                    bitcast_float(builder, first, types::F32),
                    bitcast_float(builder, second, types::F32),
                ),
                2,
            )
        }
        0x1c8 => (
            builder.ins().fcmp(
                FloatCC::Unordered,
                bitcast_float(builder, first, types::F32),
                bitcast_float(builder, first, types::F32),
            ),
            1,
        ),
        0x1c9 => {
            let absolute = builder.ins().band(
                first,
                iconst(builder, types::I32, i64::from(0x7fff_ffffu32 as i32)),
            );
            (
                builder.ins().icmp(
                    IntCC::Equal,
                    absolute,
                    iconst(builder, types::I32, i64::from(0x7f80_0000u32 as i32)),
                ),
                1,
            )
        }
        _ => unreachable!("is_direct_instruction admitted an unsupported float branch"),
    };
    if let Some(target) = continuation {
        emit_trace_conditional_branch(
            builder,
            state,
            TraceBranch {
                condition,
                expected_target: target,
                taken_target: branch_target.expect("trace branch target"),
                fallthrough: instruction.next_pc,
            },
            done,
            next,
            index,
        );
        return false;
    }
    let offset = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[offset_index],
        Width::Word,
    );
    let taken = builder.create_block();
    let fallthrough = builder.create_block();
    builder.ins().brif(condition, taken, &[], fallthrough, &[]);
    builder.switch_to_block(fallthrough);
    builder.ins().jump(
        done,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index + 1),
        ))],
    );
    builder.switch_to_block(taken);
    emit_direct_branch(builder, state, slow, done, index, offset);
    true
}

fn emit_float64_value(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    instruction: &DecodedInstruction,
) {
    let (value, low_destination) = match instruction.opcode {
        0x200 => {
            let integer = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            (builder.ins().fcvt_from_sint(types::F64, integer), 1)
        }
        0x203 => {
            let bits = emit_load_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[0],
                Width::Word,
            );
            let f32_value = bitcast_float(builder, bits, types::F32);
            (builder.ins().fpromote(types::F64, f32_value), 1)
        }
        0x204 => {
            let value = load_f64_pair(builder, state, slow, index, &instruction.operands, 0);
            let value = builder.ins().fdemote(types::F32, value);
            let bits = builder.ins().bitcast(types::I32, native_flags(), value);
            emit_store_operand(
                builder,
                state,
                slow,
                index,
                &instruction.operands[2],
                bits,
                Width::Word,
            );
            return;
        }
        0x208 | 0x209 => {
            let value = load_f64_pair(builder, state, slow, index, &instruction.operands, 0);
            let value = if instruction.opcode == 0x208 {
                let negated = builder.ins().fneg(value);
                let rounded = builder.ins().floor(negated);
                builder.ins().fneg(rounded)
            } else {
                builder.ins().floor(value)
            };
            (value, 2)
        }
        0x210..=0x213 => {
            let first = load_f64_pair(builder, state, slow, index, &instruction.operands, 0);
            let second = load_f64_pair(builder, state, slow, index, &instruction.operands, 2);
            let value = match instruction.opcode {
                0x210 => builder.ins().fadd(first, second),
                0x211 => builder.ins().fsub(first, second),
                0x212 => builder.ins().fmul(first, second),
                _ => builder.ins().fdiv(first, second),
            };
            (value, 4)
        }
        _ => unreachable!("is_direct_instruction admitted an unsupported double opcode"),
    };
    store_f64_pair(
        builder,
        state,
        slow,
        index,
        &instruction.operands,
        low_destination,
        value,
    );
}

fn emit_float64_branch(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    instruction: &DecodedInstruction,
    path: TracePath,
) -> bool {
    let TracePath {
        done,
        next,
        index,
        continuation,
        branch_target,
    } = path;
    let first = load_f64_pair(builder, state, slow, index, &instruction.operands, 0);
    let (condition, offset_index) = match instruction.opcode {
        0x232..=0x235 => {
            let second = load_f64_pair(builder, state, slow, index, &instruction.operands, 2);
            let comparison = match instruction.opcode {
                0x232 => FloatCC::LessThan,
                0x233 => FloatCC::LessThanOrEqual,
                0x234 => FloatCC::GreaterThan,
                _ => FloatCC::GreaterThanOrEqual,
            };
            (builder.ins().fcmp(comparison, first, second), 4)
        }
        0x238 => (builder.ins().fcmp(FloatCC::Unordered, first, first), 2),
        0x239 => {
            let bits = builder.ins().bitcast(types::I64, native_flags(), first);
            let absolute = builder.ins().band(
                bits,
                iconst(builder, types::I64, 0x7fff_ffff_ffff_ffffu64 as i64),
            );
            (
                builder.ins().icmp(
                    IntCC::Equal,
                    absolute,
                    iconst(builder, types::I64, 0x7ff0_0000_0000_0000u64 as i64),
                ),
                2,
            )
        }
        _ => unreachable!("is_direct_instruction admitted an unsupported double branch"),
    };
    if let Some(target) = continuation {
        emit_trace_conditional_branch(
            builder,
            state,
            TraceBranch {
                condition,
                expected_target: target,
                taken_target: branch_target.expect("trace branch target"),
                fallthrough: instruction.next_pc,
            },
            done,
            next,
            index,
        );
        return false;
    }
    let offset = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[offset_index],
        Width::Word,
    );
    let taken = builder.create_block();
    let fallthrough = builder.create_block();
    builder.ins().brif(condition, taken, &[], fallthrough, &[]);
    builder.switch_to_block(fallthrough);
    builder.ins().jump(
        done,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index + 1),
        ))],
    );
    builder.switch_to_block(taken);
    emit_direct_branch(builder, state, slow, done, index, offset);
    true
}

fn load_f64_pair(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    operands: &[Operand; 8],
    first: usize,
) -> ir::Value {
    let high = emit_load_operand(builder, state, slow, index, &operands[first], Width::Word);
    let low = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &operands[first + 1],
        Width::Word,
    );
    let high = builder.ins().uextend(types::I64, high);
    let low = builder.ins().uextend(types::I64, low);
    let high = builder.ins().ishl(high, iconst(builder, types::I64, 32));
    let bits = builder.ins().bor(high, low);
    builder.ins().bitcast(types::F64, native_flags(), bits)
}

fn store_f64_pair(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    operands: &[Operand; 8],
    first: usize,
    value: ir::Value,
) {
    let bits = builder.ins().bitcast(types::I64, native_flags(), value);
    let low = builder.ins().ireduce(types::I32, bits);
    let high = builder.ins().ireduce(
        types::I32,
        builder.ins().ushr(bits, iconst(builder, types::I64, 32)),
    );
    emit_store_operand(
        builder,
        state,
        slow,
        index,
        &operands[first],
        low,
        Width::Word,
    );
    emit_store_operand(
        builder,
        state,
        slow,
        index,
        &operands[first + 1],
        high,
        Width::Word,
    );
}

fn bitcast_float(builder: &IrBuilder<'_>, value: ir::Value, ty: ir::Type) -> ir::Value {
    builder.ins().bitcast(ty, native_flags(), value)
}

fn emit_array_instruction(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    instruction: &DecodedInstruction,
) {
    let base = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[0],
        Width::Word,
    );
    let element = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[1],
        Width::Word,
    );
    let is_bit = instruction.opcode == 0x4b || instruction.opcode == 0x4f;
    let address = if is_bit {
        let byte_index = builder.ins().sshr(element, iconst(builder, types::I32, 3));
        builder.ins().iadd(base, byte_index)
    } else {
        let scale = match instruction.opcode {
            0x48 | 0x4c => 4,
            0x49 | 0x4d => 2,
            _ => 1,
        };
        let scaled = builder
            .ins()
            .imul(element, iconst(builder, types::I32, i64::from(scale)));
        builder.ins().iadd(base, scaled)
    };
    let width = if is_bit {
        Width::Byte
    } else {
        match instruction.opcode {
            0x48 | 0x4c => Width::Word,
            0x49 | 0x4d => Width::Short,
            _ => Width::Byte,
        }
    };
    if instruction.opcode <= 0x4b {
        let value = emit_load_memory(builder, state, slow, index, address, width);
        let value = if instruction.opcode == 0x4b {
            let bit = builder.ins().band(element, iconst(builder, types::I32, 7));
            let shifted = builder.ins().ushr(value, bit);
            builder.ins().band(shifted, iconst(builder, types::I32, 1))
        } else {
            value
        };
        emit_store_operand(
            builder,
            state,
            slow,
            index,
            &instruction.operands[2],
            value,
            Width::Word,
        );
        return;
    }

    let value = emit_load_operand(
        builder,
        state,
        slow,
        index,
        &instruction.operands[2],
        Width::Word,
    );
    if instruction.opcode == 0x4f {
        let ptr = guard_memory(builder, state, slow, index, address, Width::Byte, true);
        let old = load_memory_at(builder, ptr, Width::Byte);
        let bit = builder.ins().band(element, iconst(builder, types::I32, 7));
        let mask = builder.ins().ishl(iconst(builder, types::I32, 1), bit);
        let clear = builder
            .ins()
            .icmp(IntCC::Equal, value, iconst(builder, types::I32, 0));
        let inverse = builder.ins().bxor(mask, iconst(builder, types::I32, 0xff));
        let cleared = builder.ins().band(old, inverse);
        let set = builder.ins().bor(old, mask);
        let updated = builder.ins().select(clear, cleared, set);
        store_memory_at(builder, ptr, updated, Width::Byte);
        mark_dirty(builder, state, address, 1);
    } else {
        emit_store_memory(builder, state, slow, index, address, value, width);
    }
}

fn emit_trace_jump(builder: &IrBuilder<'_>, state: ir::Value, next: ir::Block, target: u32) {
    store_field(
        builder,
        state,
        PC,
        iconst(builder, types::I32, i64::from(target)),
    );
    builder.ins().jump(next, &[]);
}

fn emit_trace_conditional_branch(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    branch: TraceBranch,
    done: ir::Block,
    next: ir::Block,
    index: u32,
) {
    if branch.expected_target == branch.taken_target && branch.expected_target == branch.fallthrough
    {
        builder.ins().jump(next, &[]);
        return;
    }
    let exit = builder.create_block();
    if branch.expected_target == branch.taken_target {
        builder.ins().brif(branch.condition, next, &[], exit, &[]);
        builder.switch_to_block(exit);
        // The instruction's default PC is already the fallthrough address.
    } else {
        builder.ins().brif(branch.condition, exit, &[], next, &[]);
        builder.switch_to_block(exit);
        store_field(
            builder,
            state,
            PC,
            iconst(builder, types::I32, i64::from(branch.taken_target)),
        );
    }
    builder.ins().jump(
        done,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index + 1),
        ))],
    );
}

fn emit_direct_branch(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    done: ir::Block,
    index: u32,
    offset: ir::Value,
) {
    let zero = builder
        .ins()
        .icmp(IntCC::Equal, offset, iconst(builder, types::I32, 0));
    let one = builder
        .ins()
        .icmp(IntCC::Equal, offset, iconst(builder, types::I32, 1));
    let invalid = builder.ins().bor(zero, one);
    let valid = builder.create_block();
    builder.ins().brif(
        invalid,
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
        valid,
        &[],
    );
    builder.switch_to_block(valid);
    let pc = load_field(builder, state, PC);
    let target = builder.ins().isub(
        builder.ins().iadd(pc, offset),
        iconst(builder, types::I32, 2),
    );
    store_field(builder, state, PC, target);
    builder.ins().jump(
        done,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index + 1),
        ))],
    );
}

fn emit_load_operand(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    operand: &Operand,
    width: Width,
) -> ir::Value {
    match operand {
        Operand::Zero => iconst(builder, types::I32, 0),
        Operand::Constant(value) => iconst(builder, types::I32, i64::from(*value as i32)),
        Operand::Memory(address) => emit_load_memory(
            builder,
            state,
            slow,
            index,
            iconst(builder, types::I32, i64::from(*address as i32)),
            width,
        ),
        Operand::Stack => emit_pop_stack(builder, state, slow, index),
        Operand::Local(offset) => emit_load_local(builder, state, slow, index, *offset, width),
    }
}

fn emit_store_operand(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    operand: &Operand,
    value: ir::Value,
    width: Width,
) {
    match operand {
        Operand::Zero => {}
        Operand::Memory(address) => emit_store_memory(
            builder,
            state,
            slow,
            index,
            iconst(builder, types::I32, i64::from(*address as i32)),
            value,
            width,
        ),
        Operand::Stack => emit_push_stack(builder, state, slow, index, value),
        Operand::Local(offset) => {
            emit_store_local(builder, state, slow, index, *offset, value, width)
        }
        Operand::Constant(_) => emit_slow(builder, slow, index),
    }
}

fn emit_load_memory(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    address: ir::Value,
    width: Width,
) -> ir::Value {
    let ptr = guard_memory(builder, state, slow, index, address, width, false);
    load_memory_at(builder, ptr, width)
}

fn emit_store_memory(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    address: ir::Value,
    value: ir::Value,
    width: Width,
) {
    let ptr = guard_memory(builder, state, slow, index, address, width, true);
    store_memory_at(builder, ptr, value, width);
    mark_dirty(builder, state, address, width_bytes(width));
}

fn guard_memory(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    address: ir::Value,
    width: Width,
    write: bool,
) -> ir::Value {
    let width = width_bytes(width);
    let length = load_field(builder, state, MEMORY_LEN);
    let enough_memory = builder.ins().icmp(
        IntCC::UnsignedGreaterThanOrEqual,
        length,
        iconst(builder, types::I32, i64::from(width)),
    );
    let maximum = builder
        .ins()
        .isub(length, iconst(builder, types::I32, i64::from(width)));
    let in_range = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, address, maximum);
    let in_bounds = builder.ins().band(enough_memory, in_range);
    let ram_start = load_field(builder, state, RAM_START);
    if write {
        let writable = builder
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, address, ram_start);
        let valid = builder.ins().band(in_bounds, writable);
        let block = builder.create_block();
        builder.ins().brif(
            valid,
            block,
            &[],
            slow,
            &[BlockArg::Value(iconst(
                builder,
                types::I32,
                i64::from(index),
            ))],
        );
        builder.switch_to_block(block);
        let base = load_field_ptr(builder, state, RAM_PTR);
        let offset = builder.ins().isub(address, ram_start);
        let offset = builder.ins().uextend(types::I64, offset);
        return builder.ins().iadd(base, offset);
    }

    let valid = builder.create_block();
    builder.ins().brif(
        in_bounds,
        valid,
        &[],
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
    );
    builder.switch_to_block(valid);
    let rom = builder.create_block();
    let ram = builder.create_block();
    let join = builder.create_block();
    builder.append_block_param(join, types::I64);
    let is_rom = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, address, ram_start);
    builder.ins().brif(is_rom, rom, &[], ram, &[]);

    builder.switch_to_block(rom);
    let base = load_field_ptr(builder, state, ROM_PTR);
    let offset = builder.ins().uextend(types::I64, address);
    let pointer = builder.ins().iadd(base, offset);
    builder.ins().jump(join, &[BlockArg::Value(pointer)]);

    builder.switch_to_block(ram);
    let base = load_field_ptr(builder, state, RAM_PTR);
    let offset = builder.ins().isub(address, ram_start);
    let offset = builder.ins().uextend(types::I64, offset);
    let pointer = builder.ins().iadd(base, offset);
    builder.ins().jump(join, &[BlockArg::Value(pointer)]);

    builder.switch_to_block(join);
    builder.block_param(join, 0)
}

fn load_memory_at(builder: &IrBuilder<'_>, ptr: ir::Value, width: Width) -> ir::Value {
    let (ty, extend) = match width {
        Width::Byte => (types::I8, true),
        Width::Short => (types::I16, true),
        Width::Word => (types::I32, false),
    };
    let value = builder.ins().load(ty, native_flags(), ptr, 0);
    let value = if matches!(width, Width::Short | Width::Word) {
        builder.ins().bswap(value)
    } else {
        value
    };
    if extend {
        builder.ins().uextend(types::I32, value)
    } else {
        value
    }
}

fn store_memory_at(builder: &IrBuilder<'_>, ptr: ir::Value, value: ir::Value, width: Width) {
    let value = match width {
        Width::Byte => builder.ins().ireduce(types::I8, value),
        Width::Short => {
            let value = builder.ins().ireduce(types::I16, value);
            builder.ins().bswap(value)
        }
        Width::Word => builder.ins().bswap(value),
    };
    builder.ins().store(native_flags(), value, ptr, 0);
}

fn emit_load_local(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    offset: u32,
    width: Width,
) -> ir::Value {
    let ptr = guard_local(builder, state, slow, index, offset, width);
    load_memory_at(builder, ptr, width)
}

fn emit_store_local(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    offset: u32,
    value: ir::Value,
    width: Width,
) {
    let ptr = guard_local(builder, state, slow, index, offset, width);
    store_memory_at(builder, ptr, value, width);
    store_field(builder, state, STACK_DIRTY, iconst(builder, types::I32, 1));
}

fn guard_local(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    offset: u32,
    width: Width,
) -> ir::Value {
    let base = load_field(builder, state, LOCALS_BASE);
    let address = builder
        .ins()
        .iadd(base, iconst(builder, types::I32, i64::from(offset)));
    let width = width_bytes(width);
    let frame_end = load_field(builder, state, STACK_FRAME_END);
    let no_overflow = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, address, base);
    let enough = builder.ins().icmp(
        IntCC::UnsignedGreaterThanOrEqual,
        frame_end,
        iconst(builder, types::I32, i64::from(width)),
    );
    let maximum = builder
        .ins()
        .isub(frame_end, iconst(builder, types::I32, i64::from(width)));
    let in_range = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, address, maximum);
    let valid = builder
        .ins()
        .band(no_overflow, builder.ins().band(enough, in_range));
    let block = builder.create_block();
    builder.ins().brif(
        valid,
        block,
        &[],
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
    );
    builder.switch_to_block(block);
    let base_ptr = load_field_ptr(builder, state, STACK_PTR);
    builder
        .ins()
        .iadd(base_ptr, builder.ins().uextend(types::I64, address))
}

fn emit_pop_stack(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
) -> ir::Value {
    let ptr = guard_stack_range(builder, state, slow, index, iconst(builder, types::I32, 4));
    let value = load_stack_at(builder, ptr, Width::Word);
    let length = load_field(builder, state, STACK_LEN);
    let next = builder.ins().isub(length, iconst(builder, types::I32, 4));
    store_field(builder, state, STACK_LEN, next);
    store_field(builder, state, STACK_DIRTY, iconst(builder, types::I32, 1));
    value
}

fn emit_push_stack(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    value: ir::Value,
) {
    let length = load_field(builder, state, STACK_LEN);
    let capacity = load_field(builder, state, STACK_CAPACITY);
    let maximum = load_field(builder, state, STACK_MAXIMUM);
    let end = builder.ins().iadd(length, iconst(builder, types::I32, 4));
    let enough_capacity = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, end, capacity);
    let enough_maximum = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, end, maximum);
    let valid = builder.ins().band(enough_capacity, enough_maximum);
    let block = builder.create_block();
    builder.ins().brif(
        valid,
        block,
        &[],
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
    );
    builder.switch_to_block(block);
    let base = load_field_ptr(builder, state, STACK_PTR);
    let ptr = builder
        .ins()
        .iadd(base, builder.ins().uextend(types::I64, length));
    store_memory_at(builder, ptr, value, Width::Word);
    store_field(builder, state, STACK_LEN, end);
    store_field(builder, state, STACK_DIRTY, iconst(builder, types::I32, 1));
}

fn guard_stack_range(
    builder: &IrBuilder<'_>,
    state: ir::Value,
    slow: ir::Block,
    index: u32,
    count: ir::Value,
) -> ir::Value {
    let length = load_field(builder, state, STACK_LEN);
    let frame_end = load_field(builder, state, STACK_FRAME_END);
    let max = builder.ins().isub(iconst(builder, types::I32, -1), count);
    let frame_ok = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, frame_end, max);
    let required = builder.ins().iadd(frame_end, count);
    let length_ok = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, length, required);
    let valid = builder.ins().band(frame_ok, length_ok);
    let block = builder.create_block();
    builder.ins().brif(
        valid,
        block,
        &[],
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
    );
    builder.switch_to_block(block);
    let start = builder.ins().isub(length, count);
    let base = load_field_ptr(builder, state, STACK_PTR);
    builder
        .ins()
        .iadd(base, builder.ins().uextend(types::I64, start))
}

fn load_stack_at(builder: &IrBuilder<'_>, ptr: ir::Value, width: Width) -> ir::Value {
    load_memory_at(builder, ptr, width)
}

fn load_stack_at_offset(
    builder: &IrBuilder<'_>,
    ptr: ir::Value,
    offset: i32,
    width: Width,
) -> ir::Value {
    let ptr = builder
        .ins()
        .iadd(ptr, iconst(builder, types::I64, i64::from(offset)));
    load_memory_at(builder, ptr, width)
}

fn store_stack_at(builder: &IrBuilder<'_>, ptr: ir::Value, value: ir::Value, width: Width) {
    store_memory_at(builder, ptr, value, width);
}

fn store_stack_at_offset(
    builder: &IrBuilder<'_>,
    ptr: ir::Value,
    offset: i32,
    value: ir::Value,
    width: Width,
) {
    let ptr = builder
        .ins()
        .iadd(ptr, iconst(builder, types::I64, i64::from(offset)));
    store_memory_at(builder, ptr, value, width);
}

fn mark_dirty(builder: &IrBuilder<'_>, state: ir::Value, address: ir::Value, width: u32) {
    let old_min = load_field(builder, state, DIRTY_MIN);
    let old_max = load_field(builder, state, DIRTY_MAX);
    let smaller = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, address, old_min);
    let minimum = builder.ins().select(smaller, address, old_min);
    let end = builder
        .ins()
        .iadd(address, iconst(builder, types::I32, i64::from(width)));
    let larger = builder.ins().icmp(IntCC::UnsignedGreaterThan, end, old_max);
    let maximum = builder.ins().select(larger, end, old_max);
    store_field(builder, state, DIRTY_MIN, minimum);
    store_field(builder, state, DIRTY_MAX, maximum);
}

fn emit_slow(builder: &IrBuilder<'_>, slow: ir::Block, index: u32) {
    builder.ins().jump(
        slow,
        &[BlockArg::Value(iconst(
            builder,
            types::I32,
            i64::from(index),
        ))],
    );
}

fn load_field(builder: &IrBuilder<'_>, state: ir::Value, offset: i32) -> ir::Value {
    builder
        .ins()
        .load(types::I32, native_flags(), state, offset)
}

fn load_field_ptr(builder: &IrBuilder<'_>, state: ir::Value, offset: i32) -> ir::Value {
    builder
        .ins()
        .load(types::I64, native_flags(), state, offset)
}

fn store_field(builder: &IrBuilder<'_>, state: ir::Value, offset: i32, value: ir::Value) {
    builder.ins().store(native_flags(), value, state, offset);
}

fn copy_field(builder: &IrBuilder<'_>, state: ir::Value, source: i32, destination: i32) {
    let value = load_field(builder, state, source);
    store_field(builder, state, destination, value);
}

fn iconst(builder: &IrBuilder<'_>, ty: ir::Type, value: i64) -> ir::Value {
    builder.ins().iconst(ty, value)
}

fn native_flags() -> MemFlagsData {
    MemFlagsData::new().with_endianness(ir::Endianness::Little)
}

fn width_bytes(width: Width) -> u32 {
    match width {
        Width::Byte => 1,
        Width::Short => 2,
        Width::Word => 4,
    }
}

fn block_fingerprint(address: u32, instructions: &[TraceInstruction]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    hash_word(&mut hash, address);
    for trace_instruction in instructions {
        let instruction = trace_instruction.instruction;
        hash_word(&mut hash, instruction.address);
        hash_word(&mut hash, instruction.next_pc);
        hash_word(&mut hash, instruction.opcode);
        for operand in instruction.operands {
            match operand {
                Operand::Zero => hash_byte(&mut hash, 0),
                Operand::Constant(value) => {
                    hash_byte(&mut hash, 1);
                    hash_word(&mut hash, value);
                }
                Operand::Memory(value) => {
                    hash_byte(&mut hash, 2);
                    hash_word(&mut hash, value);
                }
                Operand::Stack => hash_byte(&mut hash, 3),
                Operand::Local(value) => {
                    hash_byte(&mut hash, 4);
                    hash_word(&mut hash, value);
                }
            }
        }
        match trace_instruction.continuation {
            Some(value) => {
                hash_byte(&mut hash, 5);
                hash_word(&mut hash, value);
            }
            None => hash_byte(&mut hash, 6),
        }
        if let Some(value) = trace_instruction.branch_target {
            hash_byte(&mut hash, 7);
            hash_word(&mut hash, value);
        } else {
            hash_byte(&mut hash, 8);
        }
    }
    hash
}

fn hash_byte(hash: &mut u64, value: u8) {
    *hash = (*hash ^ u64::from(value)).wrapping_mul(0x1000_0000_01b3);
}

fn hash_word(hash: &mut u64, value: u32) {
    for byte in value.to_le_bytes() {
        hash_byte(hash, byte);
    }
}

pub(super) fn story_fingerprint(image: &[u8]) -> u64 {
    fnv1a(image)
}

fn aot_path(directory: &Path, story_fingerprint: u64, target_key: &str) -> PathBuf {
    directory.join(format!(
        "glulx-rs-aot-{story_fingerprint:016x}-{:016x}.bin",
        fnv1a(target_key.as_bytes())
    ))
}

fn load_aot(
    bytes: &[u8],
    story_fingerprint: u64,
    target_key: &str,
) -> Result<HashMap<BlockKey, Vec<u8>>, String> {
    let mut cursor = 0;
    if take(bytes, &mut cursor, AOT_MAGIC.len())? != AOT_MAGIC {
        return Err("AOT magic mismatch".to_owned());
    }
    if take_u32(bytes, &mut cursor)? != AOT_VERSION {
        return Err("AOT version mismatch".to_owned());
    }
    if take_u64(bytes, &mut cursor)? != story_fingerprint {
        return Err("AOT story mismatch".to_owned());
    }
    let target_len = take_u32(bytes, &mut cursor)? as usize;
    if take(bytes, &mut cursor, target_len)? != target_key.as_bytes() {
        return Err("AOT target mismatch".to_owned());
    }
    let count = take_u32(bytes, &mut cursor)? as usize;
    let mut entries = HashMap::with_capacity(count.min(4096));
    for _ in 0..count {
        let key = BlockKey {
            address: take_u32(bytes, &mut cursor)?,
            fingerprint: take_u64(bytes, &mut cursor)?,
        };
        let length = take_u32(bytes, &mut cursor)? as usize;
        if length == 0 || length > MAX_EXECUTOR_BYTES {
            return Err("AOT native function length is invalid".to_owned());
        }
        if entries
            .insert(key, take(bytes, &mut cursor, length)?.to_vec())
            .is_some()
        {
            return Err("AOT contains duplicate blocks".to_owned());
        }
    }
    let checksum_position = cursor;
    let checksum = take_u64(bytes, &mut cursor)?;
    if fnv1a(&bytes[..checksum_position]) != checksum {
        return Err("AOT checksum mismatch".to_owned());
    }
    if cursor != bytes.len() {
        return Err("AOT cache has trailing bytes".to_owned());
    }
    Ok(entries)
}

fn key_hash(path: &Path) -> u64 {
    fnv1a(path.to_string_lossy().as_bytes())
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn take<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| "AOT cache length overflow".to_owned())?;
    let result = bytes
        .get(*cursor..end)
        .ok_or_else(|| "AOT cache is truncated".to_owned())?;
    *cursor = end;
    Ok(result)
}

fn take_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        take(bytes, cursor, 4)?
            .try_into()
            .expect("AOT word length checked"),
    ))
}

fn take_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        take(bytes, cursor, 8)?
            .try_into()
            .expect("AOT double-word length checked"),
    ))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x1000_0000_01b3)
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn allocate_executable(length: usize) -> Result<NonNull<u8>, String> {
    let ptr = unsafe {
        libc::mmap(
            ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        return Err(format!(
            "allocate executable memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    NonNull::new(ptr.cast()).ok_or_else(|| "executable memory address is null".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn protect_executable(ptr: NonNull<u8>, length: usize) -> Result<(), String> {
    if unsafe {
        libc::mprotect(
            ptr.as_ptr().cast(),
            length,
            libc::PROT_READ | libc::PROT_EXEC,
        )
    } != 0
    {
        return Err(format!(
            "make executable memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn allocate_executable(length: usize) -> Result<NonNull<u8>, String> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
    };
    let ptr = unsafe {
        VirtualAlloc(
            ptr::null_mut(),
            length,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    let Some(ptr) = NonNull::new(ptr.cast()) else {
        return Err(format!(
            "allocate executable memory: {}",
            std::io::Error::last_os_error()
        ));
    };
    Ok(ptr)
}

#[cfg(target_os = "windows")]
fn protect_executable(ptr: NonNull<u8>, length: usize) -> Result<(), String> {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READ, VirtualProtect};
    let mut old_protection = 0;
    if unsafe {
        VirtualProtect(
            ptr.as_ptr().cast(),
            length,
            PAGE_EXECUTE_READ,
            &mut old_protection,
        )
    } == 0
    {
        return Err(format!(
            "make executable memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn allocate_executable(_length: usize) -> Result<NonNull<u8>, String> {
    Err("native JIT executable memory is unsupported on this platform".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn protect_executable(_ptr: NonNull<u8>, _length: usize) -> Result<(), String> {
    Err("native JIT executable memory is unsupported on this platform".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn free_executable(ptr: NonNull<u8>, length: usize) {
    let _ = unsafe { libc::munmap(ptr.as_ptr().cast(), length) };
}

#[cfg(target_os = "windows")]
fn free_executable(ptr: NonNull<u8>, _length: usize) {
    unsafe {
        windows_sys::Win32::System::Memory::VirtualFree(
            ptr.as_ptr().cast(),
            0,
            windows_sys::Win32::System::Memory::MEM_RELEASE,
        );
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn free_executable(_ptr: NonNull<u8>, _length: usize) {}

#[cfg(test)]
mod tests {
    use super::super::Operand;
    use super::super::jit::TraceInstruction;
    use super::*;

    fn nop() -> DecodedInstruction {
        DecodedInstruction {
            address: 0x40,
            next_pc: 0x41,
            opcode: 0,
            operands: [const { Operand::Zero }; 8],
        }
    }

    fn state_for(memory: &mut [u8], stack: &mut Vec<u8>) -> NativeState {
        let memory_len = memory.len();
        let (rom, ram) = memory.split_at_mut(0x100);
        NativeState {
            abi: NativeAbiState {
                rom_ptr: rom.as_ptr(),
                ram_ptr: ram.as_mut_ptr(),
                memory_len: memory_len as u32,
                ram_start: 0x100,
                ram_len: ram.len() as u32,
                stack_ptr: stack.as_mut_ptr(),
                stack_len: 0,
                stack_capacity: stack.capacity() as u32,
                stack_maximum: stack.capacity() as u32,
                stack_frame_end: 0,
                locals_base: 0,
                pc: 0x40,
                random_state: 1,
                dirty_min: u32::MAX,
                dirty_max: 0,
                stack_dirty: 0,
                rollback_stack_len: 0,
                rollback_pc: 0,
                rollback_dirty_min: u32::MAX,
                rollback_dirty_max: 0,
                rollback_stack_dirty: 0,
                rollback_random_state: 0,
            },
            ram_storage: Vec::new(),
            rom_storage: None,
            stack_storage: Vec::new(),
        }
    }

    #[test]
    fn native_executor_runs_a_block_without_a_helper_call() {
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let instruction = nop();
        let executor = compiler
            .executor(instruction.address, &[instruction])
            .expect("native block");
        let mut state = NativeState::default();
        let result = executor.run(&mut state, 1);
        assert_eq!(result, 1);
    }

    #[test]
    fn native_block_lowers_arithmetic_memory_and_stack_without_dispatch_helpers() {
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let instructions = [
            DecodedInstruction {
                address: 0x40,
                next_pc: 0x48,
                opcode: 0x10,
                operands: [
                    Operand::Constant(7),
                    Operand::Constant(9),
                    Operand::Stack,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                ],
            },
            DecodedInstruction {
                address: 0x48,
                next_pc: 0x4b,
                opcode: 0x40,
                operands: [
                    Operand::Stack,
                    Operand::Memory(0x120),
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                    Operand::Zero,
                ],
            },
        ];
        let executor = compiler
            .executor(0x40, &instructions)
            .expect("native block");
        let mut memory = vec![0; 0x200];
        let mut stack = Vec::with_capacity(64);
        let mut state = state_for(&mut memory, &mut stack);
        let result = executor.run(&mut state, 2);
        assert_eq!(result, 2);
        assert_eq!(state.abi.stack_len, 0);
        assert_eq!(&memory[0x120..0x124], &16u32.to_be_bytes());
    }

    #[test]
    fn native_conditional_branch_returns_the_native_pc_for_both_paths() {
        let instruction = DecodedInstruction {
            address: 0x40,
            next_pc: 0x44,
            opcode: 0x24,
            operands: [
                Operand::Constant(1),
                Operand::Constant(1),
                Operand::Constant(0x10),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let executor = compiler
            .executor(instruction.address, &[instruction])
            .expect("native branch");
        let mut memory = vec![0; 0x200];
        let mut stack = Vec::with_capacity(64);
        let mut state = state_for(&mut memory, &mut stack);
        assert_eq!(executor.run(&mut state, 1), 1);
        assert_eq!(state.abi.pc, 0x52);

        let false_instruction = DecodedInstruction {
            operands: [
                Operand::Constant(1),
                Operand::Constant(2),
                Operand::Constant(0x10),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
            ..instruction
        };
        let executor = compiler
            .executor(false_instruction.address, &[false_instruction])
            .expect("native false branch");
        state.abi.pc = false_instruction.address;
        assert_eq!(executor.run(&mut state, 1), 1);
        assert_eq!(state.abi.pc, false_instruction.next_pc);

        let special_instruction = DecodedInstruction {
            operands: [
                Operand::Constant(1),
                Operand::Constant(1),
                Operand::Constant(1),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
            ..instruction
        };
        let executor = compiler
            .executor(special_instruction.address, &[special_instruction])
            .expect("native special branch");
        state.abi.pc = special_instruction.address;
        assert_eq!(executor.run(&mut state, 1), NATIVE_SLOW_FLAG);
        assert_eq!(state.abi.pc, special_instruction.address);
    }

    #[test]
    fn native_trace_can_continue_after_a_conditional_branch() {
        let branch = DecodedInstruction {
            address: 0x40,
            next_pc: 0x44,
            opcode: 0x27,
            operands: [
                Operand::Constant(1),
                Operand::Constant(2),
                Operand::Constant(0x20),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let invert = DecodedInstruction {
            address: 0x44,
            next_pc: 0x48,
            opcode: 0x1b,
            operands: [
                Operand::Local(8),
                Operand::Local(8),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let trace = [
            TraceInstruction {
                instruction: branch,
                continuation: Some(0x44),
                branch_target: Some(0x62),
            },
            TraceInstruction {
                instruction: invert,
                continuation: None,
                branch_target: None,
            },
        ];
        let mut compiler = NativeCompiler::new().expect("native ISA");
        compiler.executor_trace(0x40, &trace).expect("native trace");
    }

    #[test]
    fn native_random_lowering_preserves_the_guest_mixer() {
        let instruction = DecodedInstruction {
            address: 0x40,
            next_pc: 0x48,
            opcode: 0x110,
            operands: [
                Operand::Constant(3),
                Operand::Memory(0x120),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let executor = compiler
            .executor(instruction.address, &[instruction])
            .expect("native random");
        let mut memory = vec![0; 0x200];
        let mut stack = Vec::with_capacity(64);
        let mut state = state_for(&mut memory, &mut stack);
        let result = executor.run(&mut state, 1);
        assert_eq!(result, 1);
        let mut expected = 1u32.wrapping_add(0x9e37_79b9);
        expected = (expected ^ (expected >> 16)).wrapping_mul(0x21f0_aaad);
        expected = (expected ^ (expected >> 15)).wrapping_mul(0x735a_2d97);
        expected ^= expected >> 15;
        assert_eq!(
            u32::from_be_bytes(memory[0x120..0x124].try_into().unwrap()),
            expected % 3
        );
        assert_eq!(state.abi.random_state, 1u32.wrapping_add(0x9e37_79b9));
    }

    #[test]
    fn native_float32_lowering_preserves_bits_and_ordered_branches() {
        let add = DecodedInstruction {
            address: 0x40,
            next_pc: 0x4c,
            opcode: 0x1a0,
            operands: [
                Operand::Constant(1.5f32.to_bits()),
                Operand::Constant(2.25f32.to_bits()),
                Operand::Memory(0x120),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let branch = DecodedInstruction {
            address: 0x4c,
            next_pc: 0x50,
            opcode: 0x1c2,
            operands: [
                Operand::Constant(1.0f32.to_bits()),
                Operand::Constant(2.0f32.to_bits()),
                Operand::Constant(0x10),
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let executor = compiler
            .executor(add.address, &[add])
            .expect("native float add");
        let mut memory = vec![0; 0x200];
        let mut stack = Vec::with_capacity(64);
        let mut state = state_for(&mut memory, &mut stack);
        assert_eq!(executor.run(&mut state, 1), 1);
        assert_eq!(
            u32::from_be_bytes(memory[0x120..0x124].try_into().unwrap()),
            3.75f32.to_bits()
        );

        let executor = compiler
            .executor(branch.address, &[branch])
            .expect("native float branch");
        state.abi.pc = branch.address;
        assert_eq!(executor.run(&mut state, 1), 1);
        assert_eq!(state.abi.pc, 0x5e);
    }

    #[test]
    fn native_float64_lowering_keeps_glulx_low_high_store_order() {
        let left = 7.5f64.to_bits();
        let right = 2.0f64.to_bits();
        let add = DecodedInstruction {
            address: 0x40,
            next_pc: 0x50,
            opcode: 0x210,
            operands: [
                Operand::Constant((left >> 32) as u32),
                Operand::Constant(left as u32),
                Operand::Constant((right >> 32) as u32),
                Operand::Constant(right as u32),
                Operand::Memory(0x120),
                Operand::Memory(0x124),
                Operand::Zero,
                Operand::Zero,
            ],
        };
        let mut compiler = NativeCompiler::new().expect("native ISA");
        let executor = compiler
            .executor(add.address, &[add])
            .expect("native double add");
        let mut memory = vec![0; 0x200];
        let mut stack = Vec::with_capacity(64);
        let mut state = state_for(&mut memory, &mut stack);
        assert_eq!(executor.run(&mut state, 1), 1);
        let expected = 9.5f64.to_bits();
        assert_eq!(
            u32::from_be_bytes(memory[0x120..0x124].try_into().unwrap()),
            expected as u32
        );
        assert_eq!(
            u32::from_be_bytes(memory[0x124..0x128].try_into().unwrap()),
            (expected >> 32) as u32
        );
    }

    #[test]
    fn aot_cache_round_trips_story_specific_blocks_and_rebuilds_invalid_artifacts() {
        let directory =
            std::env::temp_dir().join(format!("glulx-rs-native-aot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);

        let instruction = nop();
        let mut compiler = NativeCompiler::new().expect("native ISA");
        compiler
            .prepare_aot(&directory, 0xfeed_face)
            .expect("write AOT cache");
        let path = aot_path(&directory, 0xfeed_face, &compiler.target_key);
        let original_empty = std::fs::read(&path).expect("AOT artifact");
        compiler
            .executor(instruction.address, &[instruction])
            .expect("compile block");
        compiler.finish_aot();
        let original = std::fs::read(&path).expect("updated AOT artifact");

        let mut loaded = NativeCompiler::new().expect("native ISA");
        loaded
            .prepare_aot(&directory, 0xfeed_face)
            .expect("load AOT cache");
        assert_eq!(loaded.aot_entries.len(), 1);
        loaded
            .executor(instruction.address, &[instruction])
            .expect("load block");
        assert_eq!(std::fs::read(&path).unwrap(), original);

        std::fs::write(&path, b"corrupted AOT").expect("corrupt AOT artifact");
        let mut rebuilt = NativeCompiler::new().expect("native ISA");
        rebuilt
            .prepare_aot(&directory, 0xfeed_face)
            .expect("rebuild AOT cache");
        assert_eq!(std::fs::read(&path).unwrap(), original_empty);

        std::fs::write(&path, original).expect("restore AOT artifact");
        let mut bit_corrupted = std::fs::read(&path).unwrap();
        let checksum_byte = bit_corrupted.len() - 9;
        bit_corrupted[checksum_byte] ^= 1;
        std::fs::write(&path, bit_corrupted).expect("corrupt checksum");
        let mut rebuilt = NativeCompiler::new().expect("native ISA");
        rebuilt
            .prepare_aot(&directory, 0xfeed_face)
            .expect("rebuild checksum-invalid AOT cache");
        assert_eq!(std::fs::read(&path).unwrap(), original_empty);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
