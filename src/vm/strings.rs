use super::*;

enum Continuation {
    Number(u32, u32),
    Bytes(u32),
    Unicode(u32),
    Compressed(u32, u8),
    Finish,
}

impl Vm {
    pub(super) fn stream_string(&mut self, address: u32) -> Result<(), VmError> {
        let continuation = self.string_continuation(address)?;
        self.push_call_stub(11, 0, self.pc)?;
        self.resume_string(continuation)
    }

    fn string_continuation(&self, address: u32) -> Result<Continuation, VmError> {
        match self.memory.read8(address)? {
            0xe0 => Ok(Continuation::Bytes(address + 1)),
            0xe1 => Ok(Continuation::Compressed(address + 1, 0)),
            0xe2 => Ok(Continuation::Unicode(address + 4)),
            kind => Err(VmError::InvalidString { address, kind }),
        }
    }

    pub(super) fn stream_character(&mut self, value: u32) -> Result<(), VmError> {
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

    pub(super) fn resume_number(&mut self, value: u32, next: u32) -> Result<(), VmError> {
        self.resume_string(Continuation::Number(value, next))
    }

    pub(super) fn resume_c_string(&mut self, cursor: u32) -> Result<(), VmError> {
        self.resume_string(Continuation::Bytes(cursor))
    }

    pub(super) fn resume_unicode_string(&mut self, cursor: u32) -> Result<(), VmError> {
        self.resume_string(Continuation::Unicode(cursor))
    }

    pub(super) fn resume_compressed(&mut self, cursor: u32, bit: u8) -> Result<(), VmError> {
        self.resume_string(Continuation::Compressed(cursor, bit))
    }

    // Decoding can contain arbitrarily many substring leaves or nested strings.
    // Their continuations live on the Glulx stack, never on the Rust call stack.
    // Calling a filter or indirect function yields back to the instruction loop;
    // return_from_function resumes this loop from the portable call stub.
    fn resume_string(&mut self, mut continuation: Continuation) -> Result<(), VmError> {
        loop {
            continuation = match continuation {
                Continuation::Number(value, next) => {
                    if self.io_system != 0 {
                        let text = (value as i32).to_string();
                        if let Some(character) = text.as_bytes().get(next as usize).copied() {
                            if self.io_system == 1 {
                                self.push_call_stub(12, next + 1, value)?;
                                return self.enter_function(self.io_rock, &[u32::from(character)]);
                            }
                            self.glk_write_text(0, &text[next as usize..]);
                        }
                    }
                    Continuation::Finish
                }
                Continuation::Bytes(mut cursor) => {
                    if self.io_system != 0 {
                        loop {
                            let character = self.memory.read8(cursor)?;
                            cursor = cursor.wrapping_add(1);
                            if character == 0 {
                                break;
                            }
                            if self.io_system == 1 {
                                self.push_call_stub(13, 0, cursor)?;
                                return self.enter_function(self.io_rock, &[u32::from(character)]);
                            }
                            self.glk_write_char(0, char::from(character));
                        }
                    }
                    Continuation::Finish
                }
                Continuation::Unicode(mut cursor) => {
                    if self.io_system != 0 {
                        loop {
                            let character = self.memory.read32(cursor)?;
                            cursor = cursor.wrapping_add(4);
                            if character == 0 {
                                break;
                            }
                            if self.io_system == 1 {
                                self.push_call_stub(14, 0, cursor)?;
                                return self.enter_function(self.io_rock, &[character]);
                            }
                            self.glk_write_char(0, char::from_u32(character).unwrap_or('\u{fffd}'));
                        }
                    }
                    Continuation::Finish
                }
                Continuation::Compressed(mut byte_address, mut bit) => {
                    let table = self.story.header.decoding_table;
                    if table == 0 {
                        return Err(VmError::InvalidDecodingTable(
                            "no decoding table is selected",
                        ));
                    }
                    let root = self.memory.read32(table + 8)?;
                    let root_type = self.memory.read8(root)?;
                    if root_type == 1 {
                        Continuation::Finish
                    } else {
                        if root_type != 0 {
                            return Err(VmError::InvalidDecodingTable("root node is not a branch"));
                        }
                        loop {
                            let mut node = root;
                            while self.memory.read8(node)? == 0 {
                                let direction = (self.memory.read8(byte_address)? >> bit) & 1;
                                bit += 1;
                                if bit == 8 {
                                    bit = 0;
                                    byte_address = byte_address.wrapping_add(1);
                                }
                                node = self.memory.read32(node + 1 + u32::from(direction) * 4)?;
                            }
                            match self.memory.read8(node)? {
                                1 => break Continuation::Finish,
                                kind @ (2 | 4) => {
                                    let value = if kind == 2 {
                                        u32::from(self.memory.read8(node + 1)?)
                                    } else {
                                        self.memory.read32(node + 1)?
                                    };
                                    match self.io_system {
                                        1 => {
                                            self.push_call_stub(10, u32::from(bit), byte_address)?;
                                            return self.enter_function(self.io_rock, &[value]);
                                        }
                                        2 => self.glk_write_char(
                                            0,
                                            char::from_u32(value).unwrap_or('\u{fffd}'),
                                        ),
                                        _ => {}
                                    }
                                }
                                kind @ (3 | 5) => {
                                    self.push_call_stub(10, u32::from(bit), byte_address)?;
                                    break if kind == 3 {
                                        Continuation::Bytes(node + 1)
                                    } else {
                                        Continuation::Unicode(node + 1)
                                    };
                                }
                                kind @ 8..=11 => {
                                    let mut object = self.memory.read32(node + 1)?;
                                    if matches!(kind, 9 | 11) {
                                        object = self.memory.read32(object)?;
                                    }
                                    let object_type = self.memory.read8(object)?;
                                    self.push_call_stub(10, u32::from(bit), byte_address)?;
                                    if matches!(object_type, 0xc0 | 0xc1) {
                                        let mut arguments = Vec::new();
                                        if matches!(kind, 10 | 11) {
                                            let count = self.memory.read32(node + 5)?;
                                            for index in 0..count {
                                                arguments.push(
                                                    self.memory.read32(node + 9 + index * 4)?,
                                                );
                                            }
                                        }
                                        return self.enter_function(object, &arguments);
                                    }
                                    // Arguments apply only to functions. Referenced strings
                                    // retain the same continuation regardless of the list.
                                    break self.string_continuation(object)?;
                                }
                                kind => return Err(VmError::UnsupportedStringNode(kind)),
                            }
                        }
                    }
                }
                Continuation::Finish => {
                    let [kind, address, pc, frame_ptr] = self.stack.pop_stub()?;
                    match kind {
                        10 => {
                            self.stack.set_frame_ptr(frame_ptr)?;
                            Continuation::Compressed(pc, address as u8)
                        }
                        11 => {
                            self.pc = pc;
                            return Ok(());
                        }
                        _ => return Err(VmError::InvalidCallStub),
                    }
                }
            };
        }
    }

    pub(super) fn push_call_stub(
        &mut self,
        kind: u32,
        address: u32,
        pc: u32,
    ) -> Result<(), VmError> {
        let frame_ptr = self.stack.frame_ptr;
        self.stack.push_stub(kind, address, pc, frame_ptr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::image_with_program;

    #[test]
    fn repeated_huffman_substrings_keep_the_host_and_vm_stacks_bounded() {
        const REPETITIONS: usize = 40_000;
        for kind in [3, 5, 8, 9, 10, 11] {
            let mut vm =
                Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap())
                    .unwrap();
            vm.memory.resize(0x2000).unwrap();
            vm.io_system = 2;
            vm.story.header.decoding_table = 0x100;
            vm.memory.write32(0x100, 0x61).unwrap();
            vm.memory.write32(0x104, 3).unwrap();
            vm.memory.write32(0x108, 0x110).unwrap();
            vm.memory.write8(0x110, 0).unwrap();
            vm.memory.write32(0x111, 0x120).unwrap();
            vm.memory.write32(0x115, 0x160).unwrap();
            vm.memory.write8(0x160, 1).unwrap();
            vm.memory.write8(0x120, kind).unwrap();
            let expected = match kind {
                3 => {
                    vm.memory.write8(0x121, b'A').unwrap();
                    "A"
                }
                5 => {
                    vm.memory.write32(0x121, '中' as u32).unwrap();
                    "中"
                }
                _ => {
                    vm.memory
                        .write32(0x121, if matches!(kind, 9 | 11) { 0x150 } else { 0x170 })
                        .unwrap();
                    vm.memory.write32(0x150, 0x170).unwrap();
                    // The referenced compressed string immediately terminates.
                    vm.memory.write8(0x170, 0xe1).unwrap();
                    vm.memory.write8(0x171, 1).unwrap();
                    ""
                }
            };
            vm.memory.write8(0x200, 0xe1).unwrap();
            vm.memory
                .write8(0x201 + (REPETITIONS / 8) as u32, 1)
                .unwrap();
            let stack_length = vm.stack.len();
            let pc = vm.pc;
            vm.stream_string(0x200).unwrap();
            assert_eq!(
                vm.take_output(),
                expected.repeat(REPETITIONS),
                "node {kind}"
            );
            assert_eq!(vm.stack.len(), stack_length, "node {kind}");
            assert_eq!(vm.pc, pc, "node {kind}");
        }
    }

    #[test]
    fn portable_save_resumes_a_filter_interrupted_number() {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        // A character filter with one local. The first character has entered
        // this function when a save is requested from inside its execution.
        for (offset, value) in [0xc1, 4, 1, 0, 0, 0x31, 0].into_iter().enumerate() {
            vm.memory.write8(0x100 + offset as u32, value).unwrap();
        }
        vm.io_system = 1;
        vm.io_rock = 0x100;
        let pc = vm.pc;
        let stack = vm.stack.bytes.clone();
        vm.push_call_stub(11, 0, pc).unwrap();
        vm.resume_number(i32::MIN as u32, 0).unwrap();
        assert_eq!(
            vm.stack.read_local(0, Width::Word).unwrap(),
            u32::from(b'-')
        );
        let save = vm.encode_save(&Destination::Discard).unwrap();
        vm.restart().unwrap();
        // I/O state does not belong to portable saves. The remainder must be
        // printed to the system current at restore time, preserving the index.
        vm.io_system = 2;
        vm.decode_save(&save).unwrap();
        vm.return_from_function(0).unwrap();
        assert_eq!(vm.take_output(), "2147483648");
        assert_eq!(vm.pc, pc);
        assert_eq!(vm.stack.bytes, stack);
    }
}
