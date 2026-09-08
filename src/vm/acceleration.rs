//! Inform 6 library replacements defined by the Glulx Accelerated Functions section.
//!
//! Registrations and parameters belong to the interpreter, so IFZS and undo
//! snapshots exclude them. Desktop session snapshots retain the whole process.

use super::*;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Acceleration {
    pub(super) functions: BTreeMap<u32, u32>,
    parameters: [u32; 9],
}

pub(super) fn supported(index: u32) -> bool {
    (1..=13).contains(&index)
}

impl Acceleration {
    pub(super) fn set_parameter(&mut self, index: u32, value: u32) {
        if let Some(parameter) = self.parameters.get_mut(index as usize) {
            *parameter = value;
        }
    }
}

impl Vm {
    pub(super) fn set_acceleration(&mut self, index: u32, address: u32) -> Result<(), VmError> {
        // Even an unsupported replacement cancels the previous registration.
        self.acceleration.functions.remove(&address);
        if supported(index) {
            if !matches!(self.memory.read8(address)?, 0xc0 | 0xc1) {
                return Err(VmError::InvalidFunction(address));
            }
            self.acceleration.functions.insert(address, index);
        }
        Ok(())
    }

    pub(super) fn accelerate(&mut self, index: u32, arguments: &[u32]) -> Result<u32, VmError> {
        let obj = arguments.first().copied().unwrap_or(0);
        let id = arguments.get(1).copied().unwrap_or(0);
        let modern = index >= 8;
        match index {
            1 => self.accel_region(obj),
            2 | 8 => self.accel_property_table(obj, id, modern),
            3 | 9 => self.accel_property_address(obj, id, modern),
            4 | 10 => self.accel_property_length(obj, id, modern),
            5 | 11 => self.accel_ofclass(obj, id, modern),
            6 | 12 => {
                let address = self.accel_property_address(obj, id, modern)?;
                if address != 0 {
                    return self.memory.read32(address);
                }
                if id > 0 && id < self.acceleration.parameters[1] {
                    return self.accel_word(self.acceleration.parameters[8], id);
                }
                self.accel_error("tried to read (something)");
                Ok(0)
            }
            7 | 13 => {
                let first = self.acceleration.parameters[1];
                match self.accel_region(obj)? {
                    3 => Ok(u32::from(
                        id == first.wrapping_add(6) || id == first.wrapping_add(7),
                    )),
                    2 => Ok(u32::from(id == first.wrapping_add(5))),
                    1 => {
                        if id >= first && id < first.wrapping_add(8) && self.accel_in_class(obj)? {
                            return Ok(1);
                        }
                        Ok(u32::from(
                            self.accel_property_address(obj, id, modern)? != 0,
                        ))
                    }
                    _ => Ok(0),
                }
            }
            _ => Ok(0),
        }
    }

    fn accel_error(&mut self, message: &str) {
        // Never invoke the filter I/O system from an accelerated function.
        if self.io_system == 2 {
            self.glk_write_text(
                self.glk_current_stream,
                &format!("\n[** Programming error: {message} **]\n"),
            );
        }
    }

    fn accel_word(&self, address: u32, index: u32) -> Result<u32, VmError> {
        self.memory
            .read32(address.wrapping_add(index.wrapping_mul(4)))
    }

    fn accel_region(&self, address: u32) -> Result<u32, VmError> {
        if address < 36 || address >= self.memory.len() {
            return Ok(0);
        }
        Ok(match self.memory.read8(address)? {
            0xe0..=0xff => 3,
            0xc0..=0xdf => 2,
            0x70..=0x7f if address >= self.memory.ram_start() => 1,
            _ => 0,
        })
    }

    fn accel_in_class(&self, obj: u32) -> Result<bool, VmError> {
        let parameters = &self.acceleration.parameters;
        Ok(self
            .memory
            .read32(obj.wrapping_add(13).wrapping_add(parameters[7]))?
            == parameters[2])
    }

    fn accel_property_table(&mut self, obj: u32, id: u32, modern: bool) -> Result<u32, VmError> {
        if self.accel_region(obj)? != 1 {
            self.accel_error("tried to find the ~.~ of (something)");
            return Ok(0);
        }
        let offset = if modern {
            3 + self.acceleration.parameters[7] / 4
        } else {
            4
        };
        let table = self.accel_word(obj, offset)?;
        if table == 0 {
            return Ok(0);
        }
        self.binary_search(
            id,
            2,
            table.wrapping_add(4),
            10,
            self.memory.read32(table)?,
            0,
            0,
        )
    }

    fn accel_property(&mut self, mut obj: u32, mut id: u32, modern: bool) -> Result<u32, VmError> {
        let mut class = 0;
        if id & 0xffff_0000 != 0 {
            class = self.accel_word(self.acceleration.parameters[0], id & 0xffff)?;
            if self.accel_ofclass(obj, class, modern)? == 0 {
                return Ok(0);
            }
            id >>= 16;
            obj = class;
        }
        let property = self.accel_property_table(obj, id, modern)?;
        if property == 0 {
            return Ok(0);
        }
        let first = self.acceleration.parameters[1];
        if self.accel_in_class(obj)? && class == 0 && (id < first || id >= first.wrapping_add(8)) {
            return Ok(0);
        }
        if self.memory.read32(self.acceleration.parameters[6])? != obj
            && self.memory.read8(property.wrapping_add(9))? & 1 != 0
        {
            return Ok(0);
        }
        Ok(property)
    }

    fn accel_property_address(&mut self, obj: u32, id: u32, modern: bool) -> Result<u32, VmError> {
        let property = self.accel_property(obj, id, modern)?;
        if property == 0 {
            return Ok(0);
        }
        self.accel_word(property, 1)
    }

    fn accel_property_length(&mut self, obj: u32, id: u32, modern: bool) -> Result<u32, VmError> {
        let property = self.accel_property(obj, id, modern)?;
        if property == 0 {
            return Ok(0);
        }
        Ok(u32::from(self.memory.read16(property.wrapping_add(2))?) * 4)
    }

    fn accel_ofclass(&mut self, obj: u32, class: u32, modern: bool) -> Result<u32, VmError> {
        let parameters = self.acceleration.parameters;
        match self.accel_region(obj)? {
            3 => return Ok(u32::from(class == parameters[5])),
            2 => return Ok(u32::from(class == parameters[4])),
            1 => {}
            _ => return Ok(0),
        }
        if class == parameters[2] || class == parameters[3] {
            let is_class = self.accel_in_class(obj)? || parameters[2..=5].contains(&obj);
            return Ok(u32::from(if class == parameters[2] {
                is_class
            } else {
                !is_class
            }));
        }
        if class == parameters[4] || class == parameters[5] {
            return Ok(0);
        }
        if !self.accel_in_class(class)? {
            self.accel_error("tried to apply 'ofclass' with non-class");
            return Ok(0);
        }
        let list = self.accel_property_address(obj, 2, modern)?;
        if list == 0 {
            return Ok(0);
        }
        let length = self.accel_property_length(obj, 2, modern)? / 4;
        for index in 0..length {
            if self.accel_word(list, index)? == class {
                return Ok(1);
            }
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::tests::image_with_program;

    const OBJ: u32 = 0x200;
    const CLASS: u32 = 0x300;
    const METACLASS: u32 = 0x400;
    const OBJECT: u32 = 0x440;
    const ROUTINE: u32 = 0x480;
    const STRING: u32 = 0x4c0;
    const TABLE: u32 = 0x600;
    const CLASS_TABLE: u32 = 0x700;
    const FUNC: u32 = 0x800;

    fn vm(attributes: u32) -> Vm {
        let mut vm =
            Vm::new(Story::from_bytes(&image_with_program(&[0x81, 0x20]), None).unwrap()).unwrap();
        vm.memory.resize(0x2000).unwrap();
        vm.acceleration.parameters = [
            0x900, 256, METACLASS, OBJECT, ROUTINE, STRING, 0x940, attributes, 0x1000,
        ];
        for obj in [OBJ, CLASS, METACLASS, OBJECT, ROUTINE, STRING] {
            vm.memory.write8(obj, 0x70).unwrap();
        }
        vm.memory
            .write32(CLASS + 13 + attributes, METACLASS)
            .unwrap();
        vm.memory
            .write32(OBJ + 4 * (3 + attributes / 4), TABLE)
            .unwrap();
        vm.memory
            .write32(CLASS + 4 * (3 + attributes / 4), CLASS_TABLE)
            .unwrap();
        vm.memory.write32(TABLE, 3).unwrap();
        for (index, id, length, address, private) in [
            (0, 2, 1, 0xa00, false),
            (1, 10, 2, 0xa10, false),
            (2, 20, 1, 0xa20, true),
        ] {
            property(
                &mut vm,
                TABLE + 4 + index * 10,
                id,
                length,
                address,
                private,
            );
        }
        vm.memory.write32(CLASS_TABLE, 1).unwrap();
        property(&mut vm, CLASS_TABLE + 4, 10, 1, 0xa30, false);
        vm.memory.write32(0x900, CLASS).unwrap();
        vm.memory.write32(0xa00, CLASS).unwrap();
        vm.memory.write32(0xa10, 123).unwrap();
        vm.memory.write32(0xa20, 456).unwrap();
        vm.memory.write32(0xa30, 789).unwrap();
        vm.memory.write32(0x1000 + 4 * 30, 999).unwrap();
        // Deliberately different fallback, so the tests detect missed dispatch.
        bytes(&mut vm, FUNC, &[0xc1, 0, 0, 0x31, 0x01, 99]);
        vm
    }

    fn property(vm: &mut Vm, entry: u32, id: u16, length: u16, address: u32, private: bool) {
        vm.memory.write16(entry, id).unwrap();
        vm.memory.write16(entry + 2, length).unwrap();
        vm.memory.write32(entry + 4, address).unwrap();
        vm.memory.write16(entry + 8, u16::from(private)).unwrap();
    }

    fn bytes(vm: &mut Vm, address: u32, bytes: &[u8]) {
        for (index, byte) in bytes.iter().enumerate() {
            vm.memory.write8(address + index as u32, *byte).unwrap();
        }
    }

    fn instruction(vm: &mut Vm, opcode: u32, arguments: &[u32], stack_result: bool) {
        let mut program = if opcode < 128 {
            vec![opcode as u8]
        } else {
            vec![0x80 | (opcode >> 8) as u8, opcode as u8]
        };
        let mut modes = vec![3; arguments.len()];
        if stack_result {
            modes.push(8);
        }
        for pair in modes.chunks(2) {
            program.push(pair[0] | (pair.get(1).copied().unwrap_or(0) << 4));
        }
        for value in arguments {
            program.extend_from_slice(&value.to_be_bytes());
        }
        bytes(vm, 0x180, &program);
        vm.pc = 0x180;
        vm.step().unwrap();
    }

    #[test]
    fn registration_replacement_cancellation_and_unknown_parameters() {
        let mut vm = vm(7);
        for index in 1..=13 {
            assert_eq!(vm.gestalt(10, index), 1);
        }
        assert_eq!(vm.gestalt(10, 0), 0);
        assert_eq!(vm.gestalt(10, 14), 0);
        instruction(&mut vm, 0x180, &[1, FUNC], false);
        instruction(&mut vm, 0x181, &[1, 300], false);
        assert_eq!(vm.acceleration.parameters[1], 300);
        instruction(&mut vm, 0x181, &[u32::MAX, 7], false);
        assert_eq!(vm.acceleration.parameters[1], 300);
        assert_eq!(vm.accelerate(1, &[]).unwrap(), 0);
        instruction(&mut vm, 0x180, &[42, FUNC], false);
        assert!(!vm.acceleration.functions.contains_key(&FUNC));
        vm.call(FUNC, &[OBJ], Destination::Stack).unwrap();
        vm.step().unwrap();
        assert_eq!(vm.stack.pop_u32().unwrap(), 99);
        vm.set_acceleration(1, FUNC).unwrap();
        vm.set_acceleration(0, FUNC).unwrap();
        assert!(!vm.acceleration.functions.contains_key(&FUNC));
        assert!(vm.set_acceleration(1, OBJ).is_err());
        // Unknown requests consume stack operands, even at unusable addresses.
        vm.stack.push_u32(u32::MAX).unwrap();
        vm.stack.push_u32(42).unwrap();
        bytes(&mut vm, 0x180, &[0x81, 0x80, 0x88]);
        vm.pc = 0x180;
        vm.step().unwrap();
        assert_eq!(vm.stack.len(), vm.stack.frame_end().unwrap());
    }

    #[test]
    fn region_recognizes_unsigned_bounds_and_object_ram_requirement() {
        let mut vm = vm(7);
        vm.memory.write8(0xa80, 0xff).unwrap();
        vm.memory.write8(0xa81, 0xdf).unwrap();
        vm.memory.write8(0xa82, 0x7f).unwrap();
        for (address, expected) in [
            (0, 0),
            (35, 0),
            (u32::MAX, 0),
            (vm.memory.len(), 0),
            (0x40, 2),
            (OBJ, 1),
            (0xa80, 3),
            (0xa81, 2),
            (0xa82, 1),
        ] {
            assert_eq!(vm.accelerate(1, &[address]).unwrap(), expected);
        }
    }

    #[test]
    fn property_access_handles_privacy_qualification_length_and_defaults() {
        for (attributes, table, address, length, value, provides) in [
            (7, 2, 3, 4, 6, 7),
            (7, 8, 9, 10, 12, 13),
            (11, 8, 9, 10, 12, 13),
        ] {
            let mut vm = vm(attributes);
            assert_eq!(vm.accelerate(table, &[OBJ, 10]).unwrap(), TABLE + 14);
            assert_eq!(vm.accelerate(address, &[OBJ, 10]).unwrap(), 0xa10);
            assert_eq!(vm.accelerate(length, &[OBJ, 10]).unwrap(), 8);
            assert_eq!(vm.accelerate(value, &[OBJ, 10]).unwrap(), 123);
            assert_eq!(vm.accelerate(provides, &[OBJ, 10]).unwrap(), 1);
            assert_eq!(vm.accelerate(address, &[OBJ, 20]).unwrap(), 0);
            assert_eq!(vm.accelerate(length, &[OBJ, 20]).unwrap(), 0);
            assert_eq!(vm.accelerate(provides, &[OBJ, 20]).unwrap(), 0);
            vm.memory.write32(0x940, OBJ).unwrap();
            assert_eq!(vm.accelerate(value, &[OBJ, 20]).unwrap(), 456);
            assert_eq!(vm.accelerate(value, &[OBJ, 30]).unwrap(), 999);
            assert_eq!(vm.accelerate(address, &[CLASS, 10]).unwrap(), 0);
            assert_eq!(vm.accelerate(value, &[OBJ, 10 << 16]).unwrap(), 789);
            assert_eq!(vm.accelerate(length, &[OBJ, 10 << 16]).unwrap(), 4);
            assert_eq!(vm.accelerate(address, &[OBJECT, 10 << 16]).unwrap(), 0);
            assert_eq!(vm.accelerate(provides, &[CLASS, 256]).unwrap(), 1);
        }
    }

    #[test]
    fn legacy_property_offset_stays_fixed_when_attributes_change() {
        let mut vm = vm(11);
        assert_eq!(vm.accelerate(2, &[OBJ, 10]).unwrap(), 0);
        assert_eq!(vm.accelerate(8, &[OBJ, 10]).unwrap(), TABLE + 14);
    }

    #[test]
    fn metaclasses_and_builtin_properties_match_inform_semantics() {
        let mut vm = vm(7);
        vm.memory.write8(0xa80, 0xe0).unwrap();
        for ofclass in [5, 11] {
            for (obj, class, expected) in [
                (OBJ, CLASS, 1),
                (OBJ, OBJECT, 1),
                (OBJ, METACLASS, 0),
                (CLASS, METACLASS, 1),
                (CLASS, OBJECT, 0),
                (OBJECT, METACLASS, 1),
                (OBJECT, OBJECT, 0),
                (0xa80, STRING, 1),
                (0xa80, OBJECT, 0),
                (FUNC, ROUTINE, 1),
                (FUNC, CLASS, 0),
                (0, CLASS, 0),
            ] {
                assert_eq!(vm.accelerate(ofclass, &[obj, class]).unwrap(), expected);
            }
        }
        for provides in [7, 13] {
            assert_eq!(vm.accelerate(provides, &[0xa80, 262]).unwrap(), 1);
            assert_eq!(vm.accelerate(provides, &[0xa80, 263]).unwrap(), 1);
            assert_eq!(vm.accelerate(provides, &[FUNC, 261]).unwrap(), 1);
            assert_eq!(vm.accelerate(provides, &[FUNC, 262]).unwrap(), 0);
        }
    }

    #[test]
    fn call_callf_and_tailcall_use_acceleration_and_restore_the_caller() {
        let mut vm = vm(7);
        vm.set_acceleration(1, FUNC).unwrap();
        let initial_stack = vm.stack.len();
        vm.stack.push_u32(OBJ).unwrap();
        instruction(&mut vm, 0x30, &[FUNC, 1], true);
        vm.step().unwrap();
        assert_eq!(vm.stack.pop_u32().unwrap(), 1);
        assert_eq!(vm.stack.len(), initial_stack);
        for argument_count in 0..=3 {
            let mut arguments = vec![FUNC];
            arguments.extend(std::iter::repeat_n(OBJ, argument_count));
            instruction(&mut vm, 0x160 + argument_count as u32, &arguments, true);
            vm.step().unwrap();
            assert_eq!(vm.stack.pop_u32().unwrap(), u32::from(argument_count > 0));
            assert_eq!(vm.stack.len(), initial_stack);
        }
        // An ordinary nested function tailcalls the accelerated function.
        bytes(&mut vm, 0x880, &[0xc0, 0, 0, 0x00]);
        vm.pc = 0x43;
        vm.call(0x880, &[], Destination::Memory(0x120)).unwrap();
        vm.stack.push_u32(OBJ).unwrap();
        instruction(&mut vm, 0x34, &[FUNC, 1], false);
        vm.step().unwrap();
        assert_eq!(vm.memory.read32(0x120).unwrap(), 1);
        assert_eq!(vm.pc, 0x43);
        assert_eq!(vm.stack.len(), initial_stack);
        // A top-level accelerated tailcall returns by halting.
        vm.stack.push_u32(OBJ).unwrap();
        instruction(&mut vm, 0x34, &[FUNC, 1], false);
        vm.step().unwrap();
        assert_eq!(vm.state, RunState::Halted);
        assert_eq!(vm.stack.len(), 0);
    }

    #[test]
    fn accelerated_filter_callbacks_do_not_recurse_on_the_host_stack() {
        let mut vm = vm(7);
        vm.memory.resize(0x10000).unwrap();
        vm.set_acceleration(1, FUNC).unwrap();
        vm.io_system = 1;
        vm.io_rock = FUNC;
        vm.memory.write8(0x2000, 0xe0).unwrap();
        bytes(&mut vm, 0x2001, &vec![b'x'; 20_000]);
        vm.pc = 0x43;
        vm.stream_string(0x2000).unwrap();
        assert_eq!(vm.run_steps(20_002).unwrap(), RunState::Halted);
        assert_eq!(vm.take_output(), "");
    }

    #[test]
    fn compressed_string_function_nodes_use_acceleration_and_resume_decoding() {
        for kind in 8..=11 {
            let mut vm = vm(7);
            vm.io_system = 2;
            // The ordinary function prints a marker which acceleration must bypass.
            bytes(&mut vm, FUNC, &[0xc1, 0, 0, 0x70, 0x01, b'!', 0x31, 0]);
            vm.set_acceleration(1, FUNC).unwrap();
            vm.story.header.decoding_table = 0xb00;
            vm.memory.write32(0xb08, 0xb20).unwrap();
            bytes(&mut vm, 0xb20, &[0]);
            vm.memory.write32(0xb21, 0xb40).unwrap();
            vm.memory.write32(0xb25, 0xb60).unwrap();
            vm.memory.write8(0xb40, kind).unwrap();
            vm.memory
                .write32(0xb41, if kind % 2 == 0 { FUNC } else { 0xb80 })
                .unwrap();
            vm.memory.write32(0xb45, 1).unwrap();
            vm.memory.write32(0xb49, OBJ).unwrap();
            vm.memory.write8(0xb60, 1).unwrap();
            vm.memory.write32(0xb80, FUNC).unwrap();
            bytes(&mut vm, 0xba0, &[0xe1, 0b10]);
            vm.stream_string(0xba0).unwrap();
            assert_eq!(vm.accelerated_return, Some(u32::from(kind >= 10)));
            assert_eq!(vm.run_steps(3).unwrap(), RunState::Halted);
            assert_eq!(vm.take_output(), "");
        }
    }

    #[test]
    fn portable_restores_and_restart_preserve_current_acceleration_settings() {
        let mut vm = vm(7);
        vm.set_acceleration(1, FUNC).unwrap();
        let saved = vm.encode_save(&Destination::Memory(0x120)).unwrap();
        instruction(&mut vm, 0x125, &[], true);
        vm.stack.pop_u32().unwrap();
        vm.set_acceleration(8, FUNC).unwrap();
        vm.acceleration.set_parameter(1, 300);
        instruction(&mut vm, 0x126, &[], true);
        vm.stack.pop_u32().unwrap();
        assert_eq!(vm.acceleration.functions[&FUNC], 8);
        assert_eq!(vm.acceleration.parameters[1], 300);
        vm.decode_save(&saved).unwrap();
        assert_eq!(vm.acceleration.functions[&FUNC], 8);
        assert_eq!(vm.acceleration.parameters[1], 300);
        vm.restart().unwrap();
        assert_eq!(vm.acceleration.functions[&FUNC], 8);
        assert_eq!(vm.acceleration.parameters[1], 300);
    }

    #[test]
    fn desktop_session_resumes_pending_accelerated_return() {
        let mut vm = vm(7);
        vm.set_acceleration(1, FUNC).unwrap();
        vm.call(FUNC, &[OBJ], Destination::Memory(0x120)).unwrap();
        let encoded = serde_json::to_string(&vm).unwrap();
        let decoded: Vm = serde_json::from_str(&encoded).unwrap();
        let mut restored = decoded.validate_session().unwrap();
        restored.step().unwrap();
        assert_eq!(restored.memory.read32(0x120).unwrap(), 1);
        assert_eq!(restored.acceleration.functions[&FUNC], 1);
        assert_eq!(restored.pc, 0x43);
    }
}
