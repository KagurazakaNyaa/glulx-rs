use super::tests::{image_with_program, push_glk_arguments};
use super::*;
use std::sync::Arc;

fn vm() -> Vm {
    Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap()
}
fn instruction(opcode: u32, loads: &[u32], stores: &[u8]) -> Vec<u8> {
    let mut bytes = if opcode < 128 {
        vec![opcode as u8]
    } else {
        vec![0x80 | (opcode >> 8) as u8, opcode as u8]
    };
    let modes: Vec<u8> = std::iter::repeat_n(3, loads.len())
        .chain(stores.iter().copied())
        .collect();
    for pair in modes.chunks(2) {
        bytes.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
    }
    for value in loads {
        bytes.extend_from_slice(&value.to_be_bytes());
    }
    bytes
}
fn step(vm: &mut Vm, opcode: u32, loads: &[u32], stores: &[u8]) {
    let bytes = instruction(opcode, loads, stores);
    for (i, byte) in bytes.iter().enumerate() {
        vm.memory.write8(0x180 + i as u32, *byte).unwrap();
    }
    vm.pc = 0x180;
    vm.step().unwrap();
}
fn words(value: f64) -> [u32; 2] {
    [(value.to_bits() >> 32) as u32, value.to_bits() as u32]
}
fn pop_double(vm: &mut Vm) -> f64 {
    let hi = vm.stack.pop_u32().unwrap();
    let lo = vm.stack.pop_u32().unwrap();
    f64::from_bits((hi as u64) << 32 | lo as u64)
}
fn glk(vm: &mut Vm, selector: u32, args: &[u32]) -> u32 {
    push_glk_arguments(vm, args);
    vm.glk(selector, args.len() as u32, Destination::Stack)
        .unwrap();
    vm.stack.pop_u32().unwrap()
}

#[test]
fn double_arithmetic_opcodes_store_low_then_high() {
    let mut vm = vm();
    for (opcode, a, b, expected) in [
        (0x210, 7.5, 2.0, 9.5),
        (0x211, 7.5, 2.0, 5.5),
        (0x212, -3.0, 2.0, -6.0),
        (0x213, 7.0, 2.0, 3.5),
        (0x214, -7.5, 2.0, -1.5),
        (0x215, -7.5, 2.0, -3.0),
        (0x21b, 2.0, 3.0, 8.0),
        (0x226, 0.0, 1.0, 0.0),
    ] {
        let args = [words(a), words(b)].concat();
        step(&mut vm, opcode, &args, &[8, 8]);
        assert_eq!(pop_double(&mut vm), expected, "{opcode:x}");
    }
    // Output stack pair is consumed directly by the next double instruction.
    step(&mut vm, 0x200, &[42], &[8, 8]);
    for (i, byte) in [0x82, 0x04, 0x88, 0x08].iter().enumerate() {
        vm.memory.write8(0x180 + i as u32, *byte).unwrap();
    }
    vm.pc = 0x180;
    vm.step().unwrap();
    assert_eq!(f32::from_bits(vm.stack.pop_u32().unwrap()), 42.0);
}

#[test]
fn double_unary_and_conversion_matrix() {
    let mut vm = vm();
    for (opcode, input, expected) in [
        (0x208, -0.5, -0.0f64),
        (0x209, 0.5, 0.0),
        (0x218, 4.0, 2.0),
        (0x219, 0.0, 1.0),
        (0x21a, 1.0, 0.0),
        (0x220, 0.0, 0.0),
        (0x221, 0.0, 1.0),
        (0x222, 0.0, 0.0),
        (0x223, 0.0, 0.0),
        (0x224, 1.0, 0.0),
        (0x225, 0.0, 0.0),
    ] {
        step(&mut vm, opcode, &words(input), &[8, 8]);
        assert_eq!(
            pop_double(&mut vm).to_bits(),
            expected.to_bits(),
            "{opcode:x}"
        );
    }
    for (value, expected) in [
        (f64::INFINITY, 0x7fff_ffff),
        (f64::NEG_INFINITY, 0x8000_0000),
        (f64::NAN, 0x7fff_ffff),
        (-f64::NAN, 0x8000_0000),
        (2147483648.0, 0x7fff_ffff),
    ] {
        for opcode in [0x201, 0x202] {
            step(&mut vm, opcode, &words(value), &[8]);
            assert_eq!(vm.stack.pop_u32().unwrap(), expected);
        }
    }
    step(&mut vm, 0x203, &[(-0.0f32).to_bits()], &[8, 8]);
    assert_eq!(pop_double(&mut vm).to_bits(), (-0.0f64).to_bits());
    step(&mut vm, 0x215, &[words(0.0), words(-2.0)].concat(), &[8, 8]);
    assert_eq!(pop_double(&mut vm).to_bits(), (-0.0f64).to_bits());
    step(&mut vm, 0x215, &[words(1.0), words(0.0)].concat(), &[8, 8]);
    assert!(pop_double(&mut vm).is_nan());
}

#[test]
fn floating_branches_consume_stack_offset_even_when_false() {
    let mut vm = vm();
    for opcode in [
        0x1c0, 0x1c1, 0x1c2, 0x1c3, 0x1c4, 0x1c5, 0x1c8, 0x1c9, 0x230, 0x231, 0x232, 0x233, 0x234,
        0x235, 0x238, 0x239,
    ] {
        let count = operand_count(opcode).unwrap();
        let mut operands = vec![0; count];
        if matches!(opcode, 0x1c0 | 0x1c1) {
            operands[0] = f32::NAN.to_bits();
        }
        if matches!(opcode, 0x230 | 0x231) {
            operands[0] = (f64::NAN.to_bits() >> 32) as u32;
        }
        // Branch offset two continues at next instruction for either outcome.
        operands[count - 1] = 2;
        for &value in operands.iter().rev() {
            vm.stack.push_u32(value).unwrap();
        }
        let mut program = vec![0x80 | (opcode >> 8) as u8, opcode as u8];
        program.extend(std::iter::repeat_n(0x88, count / 2));
        if !count.is_multiple_of(2) {
            program.push(8);
        }
        let before = vm.stack.len() - count as u32 * 4;
        for (i, byte) in program.iter().enumerate() {
            vm.memory.write8(0x180 + i as u32, *byte).unwrap();
        }
        vm.pc = 0x180;
        vm.step().unwrap();
        assert_eq!(vm.stack.len(), before, "{opcode:x}");
    }
}

#[test]
fn undo_keeps_external_state_and_reports_spec_result() {
    let mut vm = vm();
    step(&mut vm, 0x128, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 1);
    step(&mut vm, 0x125, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    vm.io_system = 1;
    vm.io_rock = 99;
    vm.random_state = 123;
    vm.story.header.decoding_table = 0x150;
    step(&mut vm, 0x128, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    step(&mut vm, 0x126, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
    assert_eq!(
        (
            vm.io_system,
            vm.io_rock,
            vm.random_state,
            vm.story.header.decoding_table
        ),
        (1, 99, 123, 0x150)
    );
    vm.restart().unwrap();
    assert_eq!(
        (
            vm.io_system,
            vm.io_rock,
            vm.random_state,
            vm.story.header.decoding_table
        ),
        (1, 99, 123, 0x150)
    );
}

#[test]
fn save_round_trip_preserves_heap_protect_and_continuation() {
    let mut vm = vm();
    let allocation = vm.malloc(128).unwrap();
    vm.memory.write32(allocation, 0x1234).unwrap();
    vm.memory.write32(0x100, 10).unwrap();
    let pc = vm.pc;
    let data = vm.encode_save(&Destination::Stack).unwrap();
    vm.memory.write32(allocation, 999).unwrap();
    vm.memory.write32(0x100, 20).unwrap();
    vm.protection = Some((0x100, 4));
    vm.io_system = 2;
    vm.decode_save(&data).unwrap();
    assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
    assert_eq!(vm.pc, pc);
    assert_eq!(vm.memory.read32(allocation).unwrap(), 0x1234);
    assert_eq!(vm.memory.read32(0x100).unwrap(), 20);
    assert_eq!(vm.io_system, 2);
    vm.mfree(allocation).unwrap();
    assert_eq!(vm.memory.len(), 0x300);
    // A protected range entirely beyond restored memory must not panic.
    vm.memory.resize(0x500).unwrap();
    vm.protection = Some((0x400, 16));
    vm.restart().unwrap();
}

#[test]
fn corrupt_saves_fail_without_changing_vm_state() {
    let mut vm = vm();
    let data = vm.encode_save(&Destination::Stack).unwrap();
    for length in 0..data.len() {
        assert!(vm.decode_save(&data[..length]).is_err());
    }
    let mut wrong = data.clone();
    wrong[21] ^= 1;
    assert!(vm.decode_save(&wrong).is_err());
    let stack_chunk = data.windows(4).position(|w| w == b"Stks").unwrap();
    wrong = data.clone();
    wrong[stack_chunk + 8..stack_chunk + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    let stack = vm.stack.bytes.clone();
    let pc = vm.pc;
    assert!(vm.decode_save(&wrong).is_err());
    assert_eq!(vm.stack.bytes, stack);
    assert_eq!(vm.pc, pc);
}

#[test]
fn file_prompt_save_restore_opcodes_round_trip() {
    let mut vm = vm();
    vm.io_system = 2;
    let path =
        std::env::temp_dir().join(format!("glulx-test-{:08x}.glksave", unpredictable_seed()));
    push_glk_arguments(&mut vm, &[1, 1, 42]);
    vm.glk(0x0062, 3, Destination::Stack).unwrap();
    assert_eq!(vm.state(), RunState::WaitingForFile);
    vm.provide_input(path.to_str().unwrap()).unwrap();
    let fileref = vm.stack.pop_u32().unwrap();
    let stream = glk(&mut vm, 0x0042, &[fileref, 1, 0]);
    assert_ne!(stream, 0);
    step(&mut vm, 0x123, &[stream], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    glk(&mut vm, 0x0044, &[stream, 0]);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"FORM");
    let stream = glk(&mut vm, 0x0042, &[fileref, 2, 0]);
    step(&mut vm, 0x124, &[stream], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
    glk(&mut vm, 0x0044, &[stream, 0]);
    std::fs::remove_file(path).unwrap();
    step(&mut vm, 0x124, &[9999], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 1);
}

#[test]
fn stream_read_seek_unicode_and_overflow_counts() {
    let mut vm = vm();
    let stream = glk(&mut vm, 0x0139, &[0x100, 3, 3, 0]);
    for value in [0x4e2d, 10, 65, 66] {
        vm.write_stream_value(stream, value).unwrap();
    }
    glk(&mut vm, 0x0045, &[stream, 0, 0]);
    assert_eq!(glk(&mut vm, 0x0132, &[stream, 0x120, 4]), 2);
    assert_eq!(vm.memory.read32(0x120).unwrap(), 0x4e2d);
    assert_eq!(vm.memory.read32(0x128).unwrap(), 0);
    assert_eq!(glk(&mut vm, 0x0090, &[stream]), 65);
    assert_eq!(glk(&mut vm, 0x0090, &[stream]), u32::MAX);
    glk(&mut vm, 0x0044, &[stream, 0x130]);
    assert_eq!(vm.memory.read32(0x130).unwrap(), 3);
    assert_eq!(vm.memory.read32(0x134).unwrap(), 4);
}

#[test]
fn random_ranges_determinism_verify_and_capabilities() {
    let mut vm = vm();
    let mut sequences = Vec::new();
    for _ in 0..2 {
        step(&mut vm, 0x111, &[1234], &[]);
        let mut sequence = Vec::new();
        for range in [0, 1, 10, (-10i32) as u32, i32::MIN as u32] {
            step(&mut vm, 0x110, &[range], &[8]);
            let value = vm.stack.pop_u32().unwrap();
            if range as i32 > 0 {
                assert!(value < range);
            }
            if (range as i32) < 0 {
                assert!(value as i32 <= 0 && (value as i32) > range as i32);
            }
            sequence.push(value);
        }
        sequences.push(sequence);
    }
    assert_eq!(sequences[0], sequences[1]);
    step(&mut vm, 0x121, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    Arc::make_mut(&mut vm.story.image)[40] ^= 1;
    step(&mut vm, 0x121, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 1);
    for selector in [9, 11, 12, 13] {
        assert_eq!(vm.gestalt(selector, 0), 1);
    }
    for opcode in [0x180, 0x181] {
        step(&mut vm, opcode, &[999, 123], &[]);
    }
}

#[test]
fn nested_window_tree_arrangement_close_and_resize() {
    let mut vm = vm();
    let main = glk(&mut vm, 0x23, &[0, 0, 0, 3, 11]);
    let grid = glk(&mut vm, 0x23, &[main, 0x12, 2, 4, 22]);
    let outer = glk(&mut vm, 0x29, &[main]);
    assert_eq!(glk(&mut vm, 0x22, &[]), outer);
    assert_eq!(glk(&mut vm, 0x30, &[main]), grid);
    let graphics = glk(&mut vm, 0x23, &[main, 0x21, 25, 5, 33]);
    let inner = glk(&mut vm, 0x29, &[main]);
    assert_eq!(glk(&mut vm, 0x29, &[inner]), outer);
    glk(&mut vm, 0x27, &[inner, 0x100, 0x104, 0x108]);
    assert_eq!(vm.memory.read32(0x100).unwrap(), 0x21);
    assert_eq!(vm.memory.read32(0x108).unwrap(), graphics);
    glk(&mut vm, 0x26, &[outer, 0x12, 3, grid]);
    glk(&mut vm, 0x25, &[grid, 0x100, 0x104]);
    assert_eq!(vm.memory.read32(0x104).unwrap(), 3);
    vm.resize_windows(800, 640);
    assert_eq!(vm.events.front(), Some(&[5, 0, 0, 0]));
    glk(&mut vm, 0x24, &[inner, 0]);
    assert_eq!(glk(&mut vm, 0x22, &[]), grid);
    assert_eq!(vm.glk_windows.len(), 1);
    assert_eq!(vm.glk_streams.len(), 1);
}

#[test]
fn event_queue_multiwindow_initial_line_cancel_and_timer() {
    let mut vm = vm();
    let first = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
    let second = glk(&mut vm, 0x23, &[first, 0x12, 1, 3, 0]);
    vm.memory.write8(0x100, 0xe9).unwrap();
    glk(&mut vm, 0xd0, &[first, 0x100, 8, 1]);
    glk(&mut vm, 0x140, &[second]);
    push_glk_arguments(&mut vm, &[u32::MAX]);
    vm.glk(0xc0, 1, Destination::Discard).unwrap();
    assert_eq!(vm.initial_input(), "é");
    vm.provide_window_input(second, "中").unwrap();
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0x4e2d);
    assert_eq!(vm.stack.pop_u32().unwrap(), 2);
    assert_eq!(vm.stack.pop_u32().unwrap(), 2);
    assert!(vm.requests.contains_key(&first));
    glk(&mut vm, 0xd1, &[first, 0x120]);
    assert_eq!(vm.memory.read32(0x120).unwrap(), 3);
    assert_eq!(vm.memory.read32(0x128).unwrap(), 1);
    vm.timer = Some((std::time::Duration::from_secs(1), std::time::Instant::now()));
    glk(&mut vm, 0xc1, &[0x120]);
    assert_eq!(vm.memory.read32(0x120).unwrap(), 1);
    glk(&mut vm, 0xd0, &[first, 0x100, 8, 0]);
    vm.select_event(0x120, Destination::Discard).unwrap();
    vm.provide_input("é中").unwrap();
    assert_eq!(vm.memory.read8(0x100).unwrap(), 0xe9);
    assert_eq!(vm.memory.read8(0x101).unwrap(), b'?');
    assert_eq!(vm.memory.read32(0x128).unwrap(), 2);
}

#[test]
fn unicode_transform_expansion_titlecase_normalization_and_capability_arguments() {
    let mut vm = vm();
    assert_eq!(glk(&mut vm, 0xa0, &[0xc9]), 0xe9);
    assert_eq!(glk(&mut vm, 0xa1, &[0xff]), 0xff);
    vm.memory.write32(0x100, 'ß' as u32).unwrap();
    assert_eq!(glk(&mut vm, 0x121, &[0x100, 1, 1]), 2);
    assert_eq!(vm.memory.read32(0x100).unwrap(), 'S' as u32);
    vm.memory.write32(0x100, 'ß' as u32).unwrap();
    assert_eq!(glk(&mut vm, 0x122, &[0x100, 4, 1, 1]), 2);
    assert_eq!(vm.memory.read32(0x104).unwrap(), 's' as u32);
    vm.memory.write32(0x100, 0xe9).unwrap();
    assert_eq!(glk(&mut vm, 0x123, &[0x100, 4, 1]), 2);
    assert_eq!(vm.memory.read32(0x104).unwrap(), 0x301);
    assert_eq!(glk(&mut vm, 0x124, &[0x100, 4, 2]), 1);
    assert_eq!(vm.memory.read32(0x100).unwrap(), 0xe9);
    assert_eq!(vm.glk_gestalt(7, 5), 1);
    assert_eq!(vm.glk_gestalt(7, 3), 1);
    assert_eq!(vm.glk_gestalt(2, 0xd800), 0);
    assert_eq!(vm.glk_gestalt(3, 0x4e2d), 0);
    vm.set_glyph_support(std::sync::Arc::new(|c| c.is_ascii() || c == '中'));
    assert_eq!(vm.glk_gestalt(3, 0x4e2d), 2);
    assert_eq!(vm.glk_gestalt(16, 0), 1);
}

#[test]
fn narrow_copy_integer_extremes_and_stack_bounds() {
    let mut vm = vm();
    for (opcode, expected) in [(0x41, 0xcdef), (0x42, 0xef)] {
        step(&mut vm, opcode, &[0x89ab_cdef], &[8]);
        assert_eq!(vm.stack.pop_u32().unwrap(), expected);
    }
    step(&mut vm, 0x13, &[i32::MIN as u32, u32::MAX], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), i32::MIN as u32);
    step(&mut vm, 0x14, &[i32::MIN as u32, u32::MAX], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    for shift in [32, 33, u32::MAX] {
        step(&mut vm, 0x1c, &[1, shift], &[8]);
        assert_eq!(vm.stack.pop_u32().unwrap(), 0);
        step(&mut vm, 0x1d, &[u32::MAX, shift], &[8]);
        assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
        step(&mut vm, 0x1e, &[u32::MAX, shift], &[8]);
        assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    }
    assert!(vm.stack.peek(u32::MAX).is_err());
    assert!(vm.stack.write_local(0, 1, Width::Word).is_err());
}

#[test]
fn echo_streams_write_counts_and_cycles() {
    let mut vm = vm();
    let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
    let stream = glk(&mut vm, 0x2c, &[window]);
    let echo = glk(&mut vm, 0x43, &[0x100, 16, 1, 0]);
    glk(&mut vm, 0x2d, &[window, echo]);
    glk(&mut vm, 0x81, &[stream, 65]);
    assert_eq!(vm.memory.read8(0x100).unwrap(), 65);
    glk(&mut vm, 0x2d, &[window, stream]);
    assert_eq!(glk(&mut vm, 0x2e, &[window]), echo);
    glk(&mut vm, 0x24, &[window, 0x120]);
    assert_eq!(vm.memory.read32(0x124).unwrap(), 1);
}

#[test]
fn datetime_epoch_negative_normalization_and_roundtrip() {
    let mut vm = vm();
    vm.memory.write32(0x100, u32::MAX).unwrap();
    vm.memory.write32(0x104, u32::MAX).unwrap();
    vm.memory.write32(0x108, 999_999).unwrap();
    glk(&mut vm, 0x168, &[0x100, 0x120]);
    assert_eq!(vm.memory.read32(0x120).unwrap(), 1969);
    assert_eq!(vm.memory.read32(0x124).unwrap(), 12);
    assert_eq!(vm.memory.read32(0x128).unwrap(), 31);
    glk(&mut vm, 0x16c, &[0x120, 0x140]);
    for i in 0..3 {
        assert_eq!(
            vm.memory.read32(0x140 + i * 4).unwrap(),
            vm.memory.read32(0x100 + i * 4).unwrap()
        );
    }
    assert_eq!(glk(&mut vm, 0x16e, &[0x120, 60]), u32::MAX);
    // Month 13 normalizes to January of the following year.
    vm.memory.write32(0x124, 13).unwrap();
    vm.memory.write32(0x128, 1).unwrap();
    for i in 4..8 {
        vm.memory.write32(0x120 + i * 4, 0).unwrap();
    }
    assert_eq!(glk(&mut vm, 0x16e, &[0x120, 1]), 0);
}

#[test]
fn all_load_address_modes_and_opcode_encodings() {
    let mut vm = vm();
    vm.memory.write32(0x100, 0x1234_5678).unwrap();
    // A function with one word local, initialized via C1 arguments.
    for (i, byte) in [0xc1, 4, 1, 0, 0, 0].into_iter().enumerate() {
        vm.memory.write8(0x140 + i as u32, byte).unwrap();
    }
    vm.call(0x140, &[0x8765_4321], Destination::Discard)
        .unwrap();
    for (mode, data, expected) in [
        (0, vec![], 0),
        (1, vec![0x80], 0xffff_ff80),
        (2, vec![0x80, 0], 0xffff_8000),
        (3, vec![0x12, 0x34, 0x56, 0x78], 0x1234_5678),
        (5, vec![0x20], vm.memory.read32(0x20).unwrap()),
        (6, vec![1, 0], 0x1234_5678),
        (7, vec![0, 0, 1, 0], 0x1234_5678),
        (8, vec![], 0xabcdef01),
        (9, vec![0], 0x8765_4321),
        (10, vec![0, 0], 0x8765_4321),
        (11, vec![0, 0, 0, 0], 0x8765_4321),
        (13, vec![0], 0x1234_5678),
        (14, vec![0, 0], 0x1234_5678),
        (15, vec![0, 0, 0, 0], 0x1234_5678),
    ] {
        if mode == 8 {
            vm.stack.push_u32(expected).unwrap();
        }
        let mut program = vec![0x40, mode | 0x80];
        program.extend(data);
        for (i, byte) in program.into_iter().enumerate() {
            vm.memory.write8(0x180 + i as u32, byte).unwrap();
        }
        vm.pc = 0x180;
        vm.step().unwrap();
        assert_eq!(vm.stack.pop_u32().unwrap(), expected, "mode {mode}");
    }
    for encoding in [vec![0], vec![0x80, 0], vec![0xc0, 0, 0, 0]] {
        for (i, byte) in encoding.iter().enumerate() {
            vm.memory.write8(0x180 + i as u32, *byte).unwrap();
        }
        vm.pc = 0x180;
        vm.step().unwrap();
        assert_eq!(vm.pc, 0x180 + encoding.len() as u32);
    }
}

#[test]
fn huffman_leaf_and_indirection_matrix() {
    for kind in [2u8, 3, 4, 5, 8, 9, 10, 11] {
        let mut vm = vm();
        vm.io_system = 2;
        vm.story.header.decoding_table = 0x100;
        vm.memory.write32(0x108, 0x110).unwrap();
        vm.memory.write8(0x110, 0).unwrap();
        vm.memory.write32(0x111, 0x120).unwrap();
        vm.memory.write32(0x115, 0x160).unwrap();
        vm.memory.write8(0x160, 1).unwrap();
        vm.memory.write8(0x120, kind).unwrap();
        let expected = match kind {
            2 => {
                vm.memory.write8(0x121, b'A').unwrap();
                "A"
            }
            3 => {
                vm.memory.write8(0x121, b'B').unwrap();
                vm.memory.write8(0x122, 0).unwrap();
                "B"
            }
            4 => {
                vm.memory.write32(0x121, 0x4e2d).unwrap();
                "中"
            }
            5 => {
                vm.memory.write32(0x121, 0x6587).unwrap();
                vm.memory.write32(0x125, 0).unwrap();
                "文"
            }
            _ => {
                let indirect = matches!(kind, 9 | 11);
                vm.memory
                    .write32(0x121, if indirect { 0x150 } else { 0x170 })
                    .unwrap();
                vm.memory.write32(0x150, 0x170).unwrap();
                if kind < 10 {
                    vm.memory.write8(0x170, 0xe0).unwrap();
                    vm.memory.write8(0x171, b'C').unwrap();
                    vm.memory.write8(0x172, 0).unwrap();
                    "C"
                } else {
                    vm.memory.write32(0x125, 1).unwrap();
                    vm.memory.write32(0x129, b'D' as u32).unwrap();
                    // C1 function: streamchar local 0, return 0.
                    for (i, byte) in [0xc1, 4, 1, 0, 0, 0x70, 9, 0, 0x31, 0]
                        .into_iter()
                        .enumerate()
                    {
                        vm.memory.write8(0x170 + i as u32, byte).unwrap();
                    }
                    "D"
                }
            }
        };
        vm.memory.write8(0x190, 0xe1).unwrap();
        vm.memory.write8(0x191, 2).unwrap();
        vm.pc = 0x43;
        vm.stream_string(0x190).unwrap();
        vm.run_steps(10).unwrap();
        assert_eq!(vm.take_output(), expected, "node {kind}");
    }
}

#[test]
fn style_hints_links_mouse_and_terminator_events() {
    let mut vm = vm();
    glk(&mut vm, 0xb0, &[3, 1, 7, 0x123456]);
    let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 0]);
    let stream = glk(&mut vm, 0x2c, &[window]);
    glk(&mut vm, 0x87, &[stream, 1]);
    glk(&mut vm, 0x101, &[stream, 99]);
    glk(&mut vm, 0x81, &[stream, 65]);
    let view = vm.window_views().pop().unwrap();
    assert_eq!(view.runs[0].style, 1);
    assert_eq!(view.runs[0].hyperlink, 99);
    assert_eq!(view.hints.get(&(1, 7)), Some(&0x123456));
    glk(&mut vm, 0x102, &[window]);
    vm.hyperlink_input(window, 99).unwrap();
    vm.select_event(0x100, Destination::Discard).unwrap();
    assert_eq!(vm.memory.read32(0x100).unwrap(), 8);
    assert_eq!(vm.memory.read32(0x108).unwrap(), 99);
    vm.memory.write32(0x100, 0xffff_fff8).unwrap();
    glk(&mut vm, 0x151, &[window, 0x100, 1]);
    glk(&mut vm, 0x150, &[window, 0]);
    glk(&mut vm, 0xd0, &[window, 0x100, 8, 0]);
    vm.select_event(0x120, Destination::Discard).unwrap();
    vm.provide_terminated_input("abc", 0xffff_fff8).unwrap();
    assert_eq!(vm.memory.read32(0x12c).unwrap(), 0xffff_fff8);
    assert_eq!(vm.window_views()[0].runs.len(), 1); // echo was disabled
    let graphics = glk(&mut vm, 0x23, &[window, 0x21, 50, 5, 0]);
    glk(&mut vm, 0xd4, &[graphics]);
    vm.mouse_input(graphics, 12, 34).unwrap();
    vm.select_event(0x120, Destination::Discard).unwrap();
    assert_eq!(vm.memory.read32(0x120).unwrap(), 4);
    assert_eq!(vm.memory.read32(0x128).unwrap(), 12);
}

#[test]
fn session_serialization_preserves_glk_objects_and_pending_input() {
    #[derive(Default)]
    struct Storage(BTreeMap<String, String>);
    impl eframe::Storage for Storage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }
    let mut vm = vm();
    glk(&mut vm, 0xb0, &[3, 1, 7, 0x123456]);
    let window = glk(&mut vm, 0x23, &[0, 0, 0, 3, 42]);
    glk(&mut vm, 0xd0, &[window, 0x100, 8, 0]);
    vm.select_event(0x120, Destination::Discard).unwrap();
    let mut storage = Storage::default();
    eframe::set_value(&mut storage, "vm", &vm);
    let restored: Vm = eframe::get_value(&storage, "vm").unwrap();
    let mut restored = restored.validate_session().unwrap();
    assert_eq!(restored.state(), RunState::WaitingForLine);
    assert_eq!(glk(&mut restored, 0x21, &[window]), 42);
    restored.provide_input("resume").unwrap();
    assert_eq!(restored.memory.read32(0x128).unwrap(), 6);
    assert_eq!(restored.memory.read8(0x100).unwrap(), b'r');
}

#[test]
fn accepts_glulxe_empty_heap_chunk() {
    let mut vm = vm();
    let mut data = vm.encode_save(&Destination::Stack).unwrap();
    data.extend_from_slice(b"MAll\0\0\0\0");
    let length = (data.len() - 8) as u32;
    data[4..8].copy_from_slice(&length.to_be_bytes());
    vm.decode_save(&data).unwrap();
    assert!(vm.heap_blocks.is_empty());
}

#[test]
fn allocation_limits_fail_without_mutating_state() {
    let mut vm = vm();
    let before = vm.memory.len();
    step(
        &mut vm,
        0x103,
        &[crate::memory::MAX_MEMORY_SIZE + 256],
        &[8],
    );
    assert_eq!(vm.stack.pop_u32().unwrap(), 1);
    step(&mut vm, 0x178, &[crate::memory::MAX_MEMORY_SIZE], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), 0);
    assert_eq!(vm.memory.len(), before);
    assert!(vm.heap_blocks.is_empty());
}
#[test]
fn undo_survives_restart_and_protected_absent_bytes_restore_as_zero() {
    let mut vm = vm();
    vm.memory.resize(0x400).unwrap();
    vm.memory.write32(0x380, 123).unwrap();
    step(&mut vm, 0x125, &[], &[8]);
    vm.stack.pop_u32().unwrap();
    vm.protection = Some((0x380, 4));
    vm.restart().unwrap();
    assert_eq!(vm.undo.len(), 1);
    step(&mut vm, 0x126, &[], &[8]);
    assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
    assert_eq!(vm.memory.read32(0x380).unwrap(), 0);
}
#[test]
fn float_nan_modulo_and_power_identities() {
    let mut vm = vm();
    for (a, b) in [(1.0f32, f32::NAN), (f32::NAN, 0.0)] {
        step(&mut vm, 0x1ab, &[a.to_bits(), b.to_bits()], &[8]);
        assert_eq!(f32::from_bits(vm.stack.pop_u32().unwrap()), 1.0);
    }
    step(
        &mut vm,
        0x1a4,
        &[f32::NAN.to_bits(), f32::INFINITY.to_bits()],
        &[8, 8],
    );
    assert!(f32::from_bits(vm.stack.pop_u32().unwrap()).is_nan());
    assert!(f32::from_bits(vm.stack.pop_u32().unwrap()).is_nan());
}

#[test]
fn function_locals_cannot_exceed_the_declared_stack_size() {
    let mut vm = vm();
    for (i, byte) in [0xc1, 4, 255, 0, 0, 0x31, 0].into_iter().enumerate() {
        vm.memory.write8(0x140 + i as u32, byte).unwrap();
    }
    let frame = vm.stack.frame_ptr;
    let before = vm.stack.len();
    assert!(matches!(
        vm.enter_function(0x140, &[]),
        Err(VmError::StackOverflow)
    ));
    assert_eq!(vm.stack.frame_ptr, frame);
    assert_eq!(vm.stack.len(), before);
}
