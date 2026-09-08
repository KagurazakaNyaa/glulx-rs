use super::*;

fn word(bytes: &[u8], offset: usize) -> Result<u32, VmError> {
    bytes
        .get(offset..offset + 4)
        .map(|v| u32::from_be_bytes(v.try_into().unwrap()))
        .ok_or(VmError::InvalidSave)
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        out.push(0);
    }
}

impl Vm {
    pub(super) fn encode_save(&self, destination: &Destination) -> Result<Vec<u8>, VmError> {
        let mut stack = self.stack.clone();
        let (kind, address) = destination_parts(destination);
        for value in [kind, address, self.pc, stack.frame_ptr] {
            stack.push_u32(value)?;
        }
        let mut compressed = self.memory.len().to_be_bytes().to_vec();
        let mut zeros = 0u16;
        for address in self.memory.ram_start()..self.memory.len() {
            let byte = self.memory.read8(address)?
                ^ self.story.image.get(address as usize).copied().unwrap_or(0);
            if byte == 0 {
                zeros += 1;
                if zeros == 256 {
                    compressed.extend_from_slice(&[0, 255]);
                    zeros = 0;
                }
            } else {
                if zeros != 0 {
                    compressed.extend_from_slice(&[0, (zeros - 1) as u8]);
                    zeros = 0;
                }
                compressed.push(byte);
            }
        }
        let mut out = b"FORM\0\0\0\0IFZS".to_vec();
        chunk(&mut out, b"IFhd", &self.story.image[..128]);
        chunk(&mut out, b"CMem", &compressed);
        chunk(&mut out, b"Stks", &stack.bytes);
        if !self.heap_blocks.is_empty() {
            let mut heap = self.heap_next.to_be_bytes().to_vec();
            heap.extend_from_slice(&(self.heap_blocks.len() as u32).to_be_bytes());
            for (&address, &length) in &self.heap_blocks {
                heap.extend_from_slice(&address.to_be_bytes());
                heap.extend_from_slice(&length.to_be_bytes());
            }
            chunk(&mut out, b"MAll", &heap);
        }
        let length = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&length.to_be_bytes());
        Ok(out)
    }

    pub(super) fn decode_save(&mut self, data: &[u8]) -> Result<(), VmError> {
        if data.get(..4) != Some(b"FORM")
            || data.get(8..12) != Some(b"IFZS")
            || word(data, 4)? as usize != data.len().saturating_sub(8)
        {
            return Err(VmError::InvalidSave);
        }
        let mut chunks = BTreeMap::new();
        let mut cursor = 12usize;
        while cursor < data.len() {
            let length = word(data, cursor + 4)? as usize;
            let end = cursor
                .checked_add(8)
                .and_then(|v| v.checked_add(length))
                .ok_or(VmError::InvalidSave)?;
            let payload = data.get(cursor + 8..end).ok_or(VmError::InvalidSave)?;
            let tag = data.get(cursor..cursor + 4).ok_or(VmError::InvalidSave)?;
            // Quetzal 1.4 sections 8.8–8.9 permit repeated annotations and
            // unknown chunks; later copies of singleton chunks are ignored.
            chunks.entry(tag).or_insert(payload);
            cursor = end.checked_add(length % 2).ok_or(VmError::InvalidSave)?;
        }
        if cursor != data.len()
            || chunks.get(b"IFhd".as_slice()).copied() != Some(&self.story.image[..128])
        {
            return Err(VmError::InvalidSave);
        }
        let compressed = chunks.get(b"CMem".as_slice());
        if compressed.is_some() && chunks.contains_key(b"UMem".as_slice()) {
            return Err(VmError::InvalidSave);
        }
        let bytes = compressed
            .or_else(|| chunks.get(b"UMem".as_slice()))
            .ok_or(VmError::InvalidSave)?;
        let size = word(bytes, 0)?;
        let mut memory = Memory::new(&self.story);
        if !memory.resize(size)? {
            return Err(VmError::InvalidSave);
        }
        let mut address = memory.ram_start();
        if compressed.is_some() {
            let mut index = 4;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if byte == 0 {
                    let run = *bytes.get(index).ok_or(VmError::InvalidSave)? as u32 + 1;
                    index += 1;
                    address = address
                        .checked_add(run)
                        .filter(|v| *v <= size)
                        .ok_or(VmError::InvalidSave)?;
                } else {
                    memory.write8(address, memory.read8(address)? ^ byte)?;
                    address = address.checked_add(1).ok_or(VmError::InvalidSave)?;
                }
            }
        } else {
            if bytes.len() != 4 + (size - address) as usize {
                return Err(VmError::InvalidSave);
            }
            for &byte in &bytes[4..] {
                memory.write8(address, byte)?;
                address += 1;
            }
        }
        let mut heap_next = size;
        let mut heap_blocks = BTreeMap::new();
        if let Some(heap) = chunks
            .get(b"MAll".as_slice())
            .filter(|heap| !heap.is_empty())
        {
            // Glulxe emits an empty MAll chunk when no heap is active.
            heap_next = word(heap, 0)?;
            let count = word(heap, 4)? as usize;
            if count.checked_mul(8).and_then(|v| v.checked_add(8)) != Some(heap.len()) {
                return Err(VmError::InvalidSave);
            }
            if count == 0 {
                if heap_next != 0 {
                    return Err(VmError::InvalidSave);
                }
                heap_next = size;
            } else {
                if heap_next < self.story.header.end_mem
                    || heap_next >= size
                    || !heap_next.is_multiple_of(256)
                {
                    return Err(VmError::InvalidSave);
                }
                for i in 0..count {
                    let start = word(heap, 8 + 8 * i)?;
                    let length = word(heap, 12 + 8 * i)?;
                    if length == 0
                        || start < heap_next
                        || start.checked_add(length).is_none_or(|v| v > size)
                        || heap_blocks.insert(start, length).is_some()
                    {
                        return Err(VmError::InvalidSave);
                    }
                }
                let mut end = heap_next;
                for (&start, &length) in &heap_blocks {
                    if start < end {
                        return Err(VmError::InvalidSave);
                    }
                    end = start + length;
                }
            }
        }
        let bytes = chunks.get(b"Stks".as_slice()).ok_or(VmError::InvalidSave)?;
        if bytes.len() < 28 || bytes.len() > self.stack.maximum as usize || bytes.len() % 4 != 0 {
            return Err(VmError::InvalidSave);
        }
        let mut stack = Stack {
            bytes: bytes.to_vec(),
            frame_ptr: 0,
            maximum: self.stack.maximum,
        };
        stack.frame_ptr = stack.pop_raw_u32()?;
        let pc = stack.pop_raw_u32()?;
        let address = stack.pop_raw_u32()?;
        let kind = stack.pop_raw_u32()?;
        if pc >= size {
            return Err(VmError::InvalidSave);
        }
        validate_stack(&stack, size)?;
        let destination = match kind {
            0 => Destination::Discard,
            1 => Destination::Memory(address),
            2 => Destination::Local(address),
            3 => Destination::Stack,
            _ => return Err(VmError::InvalidSave),
        };
        // Validate the saved result store before committing any VM state.
        match destination {
            Destination::Memory(address) => {
                memory.write32(address, memory.read32(address)?)?;
            }
            Destination::Local(offset) => {
                stack.read_local(offset, Width::Word)?;
            }
            Destination::Stack if stack.len().checked_add(4).is_none_or(|v| v > stack.maximum) => {
                return Err(VmError::InvalidSave);
            }
            _ => {}
        }
        self.memory.restore(&memory, self.protection);
        self.stack = stack;
        self.pc = pc;
        self.heap_next = heap_next;
        self.heap_blocks = heap_blocks;
        self.state = RunState::Running;
        self.store_destination(&destination, u32::MAX, Width::Word)
    }
}

pub(super) fn validate_stack(stack: &Stack, memory_size: u32) -> Result<(), VmError> {
    let mut frame = stack.frame_ptr;
    let mut ceiling = stack.len();
    loop {
        if !frame.is_multiple_of(4) {
            return Err(VmError::InvalidSave);
        }
        let length = stack.read_raw_u32(frame)?;
        let locals = stack.read_raw_u32(frame.checked_add(4).ok_or(VmError::InvalidSave)?)?;
        if locals < 12
            || !locals.is_multiple_of(4)
            || length < locals
            || !length.is_multiple_of(4)
            || frame.checked_add(length).is_none_or(|v| v > ceiling)
        {
            return Err(VmError::InvalidSave);
        }
        let mut offset = frame + 8;
        let mut local_end = locals;
        loop {
            if offset + 2 > frame + locals {
                return Err(VmError::InvalidSave);
            }
            let format = stack.read_raw(offset, 2)?;
            offset += 2;
            if format == [0, 0] {
                break;
            }
            let width = format[0] as u32;
            if !matches!(width, 1 | 2 | 4) {
                return Err(VmError::InvalidSave);
            }
            local_end = align(local_end, width)
                .checked_add(width * format[1] as u32)
                .ok_or(VmError::InvalidSave)?;
            if local_end > length {
                return Err(VmError::InvalidSave);
            }
        }
        if align(local_end, 4) != length {
            return Err(VmError::InvalidSave);
        }
        if frame == 0 {
            break;
        }
        if frame < 16 {
            return Err(VmError::InvalidSave);
        }
        let old = stack.read_raw_u32(frame - 4)?;
        if old >= frame - 16
            || (stack.read_raw_u32(frame - 16)? != 12
                && stack.read_raw_u32(frame - 8)? >= memory_size)
        {
            return Err(VmError::InvalidSave);
        }
        ceiling = frame - 16;
        frame = old;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_saves_with_repeated_annotations_and_extension_chunks() {
        let mut vm = Vm::new(
            Story::from_bytes(
                &super::super::tests::image_with_program(&[0x81, 0x20]),
                None,
            )
            .unwrap(),
        )
        .unwrap();
        vm.memory.write32(0x100, 42).unwrap();
        let mut data = vm.encode_save(&Destination::Stack).unwrap();
        for tag in [b"ANNO", b"IntD", b"Xtra"] {
            chunk(&mut data, tag, b"first");
            chunk(&mut data, tag, b"second");
        }
        // Later singleton chunks must not replace the original valid data.
        for tag in [b"IFhd", b"CMem", b"Stks"] {
            chunk(&mut data, tag, b"ignored");
        }
        let length = (data.len() - 8) as u32;
        data[4..8].copy_from_slice(&length.to_be_bytes());
        vm.memory.write32(0x100, 99).unwrap();
        vm.decode_save(&data).unwrap();
        assert_eq!(vm.memory.read32(0x100).unwrap(), 42);
        assert_eq!(vm.stack.pop_u32().unwrap(), u32::MAX);
    }
}
