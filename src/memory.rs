use crate::{Story, VmError};

#[derive(Debug, Clone)]
pub struct Memory {
    bytes: Vec<u8>,
    initial: Vec<u8>,
    ram_start: u32,
    ext_start: u32,
    original_end: u32,
}

impl Memory {
    pub fn new(story: &Story) -> Self {
        let mut bytes = story.image.clone();
        bytes.resize(story.header.end_mem as usize, 0);
        Self {
            bytes,
            initial: story.image.clone(),
            ram_start: story.header.ram_start,
            ext_start: story.header.ext_start,
            original_end: story.header.end_mem,
        }
    }

    pub fn len(&self) -> u32 {
        self.bytes.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn ram_start(&self) -> u32 {
        self.ram_start
    }

    pub fn read8(&self, address: u32) -> Result<u8, VmError> {
        self.bytes
            .get(address as usize)
            .copied()
            .ok_or(VmError::MemoryRead(address))
    }

    pub fn read16(&self, address: u32) -> Result<u16, VmError> {
        let raw = self.slice(address, 2)?;
        Ok(u16::from_be_bytes(raw.try_into().expect("length checked")))
    }

    pub fn read32(&self, address: u32) -> Result<u32, VmError> {
        let raw = self.slice(address, 4)?;
        Ok(u32::from_be_bytes(raw.try_into().expect("length checked")))
    }

    pub fn write8(&mut self, address: u32, value: u8) -> Result<(), VmError> {
        self.check_write(address, 1)?;
        self.bytes[address as usize] = value;
        Ok(())
    }

    pub fn write16(&mut self, address: u32, value: u16) -> Result<(), VmError> {
        self.check_write(address, 2)?;
        self.bytes[address as usize..address as usize + 2].copy_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write32(&mut self, address: u32, value: u32) -> Result<(), VmError> {
        self.check_write(address, 4)?;
        self.bytes[address as usize..address as usize + 4].copy_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn zero(&mut self, address: u32, length: u32) -> Result<(), VmError> {
        self.check_write(address, length)?;
        self.bytes[address as usize..(address + length) as usize].fill(0);
        Ok(())
    }

    pub fn copy(&mut self, source: u32, destination: u32, length: u32) -> Result<(), VmError> {
        self.slice(source, length)?;
        self.check_write(destination, length)?;
        self.bytes.copy_within(
            source as usize..(source + length) as usize,
            destination as usize,
        );
        Ok(())
    }

    pub fn resize(&mut self, new_size: u32) -> Result<bool, VmError> {
        if new_size < self.original_end || !new_size.is_multiple_of(0x100) {
            return Ok(false);
        }
        self.bytes.resize(new_size as usize, 0);
        Ok(true)
    }

    pub fn restart(&mut self) {
        self.bytes.resize(self.original_end as usize, 0);
        self.bytes[self.ram_start as usize..self.ext_start as usize]
            .copy_from_slice(&self.initial[self.ram_start as usize..self.ext_start as usize]);
        self.bytes[self.ext_start as usize..].fill(0);
    }

    pub fn c_string(&self, address: u32) -> Result<String, VmError> {
        let mut out = String::new();
        let mut cursor = address;
        loop {
            let byte = self.read8(cursor)?;
            if byte == 0 {
                return Ok(out);
            }
            out.push(char::from(byte));
            cursor = cursor.wrapping_add(1);
        }
    }

    fn slice(&self, address: u32, length: u32) -> Result<&[u8], VmError> {
        let end = address
            .checked_add(length)
            .filter(|end| *end <= self.len())
            .ok_or(VmError::MemoryRead(address))?;
        Ok(&self.bytes[address as usize..end as usize])
    }

    fn check_write(&self, address: u32, length: u32) -> Result<(), VmError> {
        if address < self.ram_start {
            return Err(VmError::RomWrite(address));
        }
        address
            .checked_add(length)
            .filter(|end| *end <= self.len())
            .map(|_| ())
            .ok_or(VmError::MemoryWrite(address))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn story() -> Story {
        let mut bytes = vec![0; 0x100];
        bytes[0..4].copy_from_slice(b"Glul");
        for (offset, value) in [
            (4, 0x0003_0103u32),
            (8, 0x100),
            (12, 0x100),
            (16, 0x200),
            (20, 0x100),
            (24, 0x24),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        let checksum = (0..bytes.len())
            .step_by(4)
            .map(|o| u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()))
            .fold(0u32, u32::wrapping_add);
        bytes[32..36].copy_from_slice(&checksum.to_be_bytes());
        Story::from_bytes(&bytes, None).unwrap()
    }

    #[test]
    fn rom_is_read_only_and_ram_is_writable() {
        let mut memory = Memory::new(&story());
        assert!(matches!(
            memory.write8(0x20, 1),
            Err(VmError::RomWrite(0x20))
        ));
        memory.write32(0x100, 0x1234_5678).unwrap();
        assert_eq!(memory.read32(0x100).unwrap(), 0x1234_5678);
    }
}
