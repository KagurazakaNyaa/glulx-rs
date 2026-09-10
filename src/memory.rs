use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{Story, VmError};

pub(crate) type MemoryPage = Arc<Vec<u8>>;

/// Per-VM allocation ceiling; failed growth is reported through setmemsize/malloc.
pub const MAX_MEMORY_SIZE: u32 = 1024 * 1024 * 1024;

/// Independent payload budgets. Allocator and third-party decoder overhead is
/// not included; these are not a process RSS limit. Zero disables retention.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ResourceLimits {
    pub undo_mib: u32,
    pub graphics_cache_mib: u32,
    pub text_image_cache_mib: u32,
    pub decoded_image_mib: u32,
    pub audio_resource_mib: u32,
    pub song_pcm_mib: u32,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            undo_mib: 256,
            graphics_cache_mib: 512,
            text_image_cache_mib: 256,
            decoded_image_mib: 256,
            audio_resource_mib: 256,
            song_pcm_mib: 128,
        }
    }
}
impl ResourceLimits {
    pub fn normalized(mut self) -> Self {
        for value in [
            &mut self.undo_mib,
            &mut self.graphics_cache_mib,
            &mut self.text_image_cache_mib,
            &mut self.decoded_image_mib,
            &mut self.audio_resource_mib,
            &mut self.song_pcm_mib,
        ] {
            *value = (*value).min((usize::MAX / (1024 * 1024)).min(u32::MAX as usize) as u32);
        }
        self
    }
    pub fn bytes(mib: u32) -> usize {
        (u64::from(mib) * 1024 * 1024).min(usize::MAX as u64) as usize
    }
}

fn default_memory_limit() -> u32 {
    MAX_MEMORY_SIZE
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Memory {
    #[serde(skip, default = "default_memory_limit")]
    maximum: u32,
    bytes: Vec<u8>,
    initial: Arc<Vec<u8>>,
    ram_start: u32,
    ext_start: u32,
    original_end: u32,
    #[serde(skip)]
    dirty_pages: BTreeSet<u32>,
}

impl Memory {
    pub(crate) fn snapshot_byte_len(&self) -> usize {
        self.bytes.len().saturating_add(self.initial.len())
    }

    pub(crate) fn snapshot_pages(
        &self,
        previous: Option<&BTreeMap<u32, MemoryPage>>,
    ) -> BTreeMap<u32, MemoryPage> {
        self.snapshot_pages_with_limit(previous, usize::MAX)
            .expect("unlimited page snapshot must not hit its page limit")
    }

    pub(crate) fn snapshot_pages_with_limit(
        &self,
        previous: Option<&BTreeMap<u32, MemoryPage>>,
        maximum_pages: usize,
    ) -> Option<BTreeMap<u32, MemoryPage>> {
        if previous.is_some_and(|pages| {
            pages
                .keys()
                .filter(|address| address.saturating_add(256) <= self.len())
                .count()
                > maximum_pages
        }) {
            return None;
        }
        let mut pages = previous.cloned().unwrap_or_default();
        pages.retain(|address, _| address.saturating_add(256) <= self.len());
        for &address in &self.dirty_pages {
            if address < self.ram_start || address.saturating_add(256) > self.len() {
                continue;
            }
            let mut baseline = [0; 256];
            let page_start = address as usize;
            let page_end = page_start + 256;
            if address < self.ext_start {
                baseline.copy_from_slice(&self.initial[page_start..page_end]);
            }
            let current = &self.bytes[page_start..page_end];
            if current == baseline {
                pages.remove(&address);
            } else if previous
                .and_then(|pages| pages.get(&address))
                .is_some_and(|page| page.as_slice() == current)
            {
                // Keep the shared page from the previous snapshot.
            } else {
                if !pages.contains_key(&address) && pages.len() >= maximum_pages {
                    return None;
                }
                pages.insert(address, Arc::new(current.to_vec()));
            }
        }
        Some(pages)
    }

    pub(crate) fn clear_dirty_pages(&mut self) {
        self.dirty_pages.clear();
    }

    pub(crate) fn mark_all_pages_dirty(&mut self) {
        self.dirty_pages
            .extend((self.ram_start..self.len()).step_by(256));
    }

    pub(crate) fn validate_session(&self, story: &Story) -> Result<(), VmError> {
        if self.bytes.len() > self.maximum as usize
            || self.ram_start != story.header.ram_start
            || self.ext_start != story.header.ext_start
            || self.original_end != story.header.end_mem
            || self.initial != story.image
            || self.bytes.len() < self.original_end as usize
            || !self.bytes.len().is_multiple_of(256)
            || self.bytes.get(..self.ram_start as usize)
                != story.image.get(..self.ram_start as usize)
        {
            return Err(VmError::InvalidSave);
        }
        Ok(())
    }

    pub fn new(story: &Story) -> Self {
        Self::new_with_limit(story, MAX_MEMORY_SIZE).expect("story exceeds default memory limit")
    }

    pub fn new_with_limit(story: &Story, maximum: u32) -> Result<Self, VmError> {
        if maximum == 0 || !maximum.is_multiple_of(256) {
            return Err(VmError::InvalidMemoryLimit(maximum));
        }
        if story.header.end_mem > maximum {
            return Err(VmError::MemoryLimit {
                required: story.header.end_mem,
                maximum,
            });
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(story.header.end_mem as usize)
            .map_err(|_| VmError::MemoryAllocation(story.header.end_mem))?;
        bytes.extend_from_slice(&story.image);
        bytes.resize(story.header.end_mem as usize, 0);
        Ok(Self {
            maximum,
            bytes,
            initial: Arc::clone(&story.image),
            ram_start: story.header.ram_start,
            ext_start: story.header.ext_start,
            original_end: story.header.end_mem,
            dirty_pages: BTreeSet::new(),
        })
    }

    pub fn maximum(&self) -> u32 {
        self.maximum
    }

    pub(crate) fn set_maximum(&mut self, maximum: u32) -> Result<(), VmError> {
        if maximum == 0 || !maximum.is_multiple_of(256) {
            return Err(VmError::InvalidMemoryLimit(maximum));
        }
        if self.len() > maximum {
            return Err(VmError::MemoryLimit {
                required: self.len(),
                maximum,
            });
        }
        self.maximum = maximum;
        Ok(())
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
        self.mark_dirty_range(address, 1);
        Ok(())
    }

    pub fn write16(&mut self, address: u32, value: u16) -> Result<(), VmError> {
        self.check_write(address, 2)?;
        self.bytes[address as usize..address as usize + 2].copy_from_slice(&value.to_be_bytes());
        self.mark_dirty_range(address, 2);
        Ok(())
    }

    pub fn write32(&mut self, address: u32, value: u32) -> Result<(), VmError> {
        self.check_write(address, 4)?;
        self.bytes[address as usize..address as usize + 4].copy_from_slice(&value.to_be_bytes());
        self.mark_dirty_range(address, 4);
        Ok(())
    }

    pub fn zero(&mut self, address: u32, length: u32) -> Result<(), VmError> {
        if length == 0 {
            return Ok(());
        }
        self.check_write(address, length)?;
        self.bytes[address as usize..(address + length) as usize].fill(0);
        self.mark_dirty_range(address, length);
        Ok(())
    }

    pub fn copy(&mut self, source: u32, destination: u32, length: u32) -> Result<(), VmError> {
        if length == 0 {
            return Ok(());
        }
        self.slice(source, length)?;
        self.check_write(destination, length)?;
        self.bytes.copy_within(
            source as usize..(source + length) as usize,
            destination as usize,
        );
        self.mark_dirty_range(destination, length);
        Ok(())
    }

    pub fn resize(&mut self, new_size: u32) -> Result<bool, VmError> {
        if new_size < self.original_end
            || new_size > self.maximum
            || !new_size.is_multiple_of(0x100)
        {
            return Ok(false);
        }
        if new_size > self.len()
            && self
                .bytes
                .try_reserve_exact((new_size - self.len()) as usize)
                .is_err()
        {
            return Ok(false);
        }
        let old_size = self.len();
        self.bytes.resize(new_size as usize, 0);
        if old_size != new_size {
            self.mark_dirty_range(old_size.min(new_size), old_size.abs_diff(new_size));
        }
        Ok(true)
    }

    pub fn restart(&mut self, protected: Option<(u32, u32)>) {
        let protected_bytes = self.protected_bytes(protected, self.original_end);
        self.bytes.resize(self.original_end as usize, 0);
        self.bytes[self.ram_start as usize..self.ext_start as usize]
            .copy_from_slice(&self.initial[self.ram_start as usize..self.ext_start as usize]);
        self.bytes[self.ext_start as usize..].fill(0);
        self.restore_protected(protected_bytes);
        self.mark_all_pages_dirty();
    }

    pub fn restore(
        &mut self,
        snapshot: &Self,
        protected: Option<(u32, u32)>,
    ) -> Result<(), VmError> {
        if snapshot.len() > self.maximum {
            return Err(VmError::MemoryLimit {
                required: snapshot.len(),
                maximum: self.maximum,
            });
        }
        let protected_bytes = self.protected_bytes(protected, snapshot.len());
        let maximum = self.maximum;
        *self = snapshot.clone();
        self.maximum = maximum;
        self.restore_protected(protected_bytes);
        self.mark_all_pages_dirty();
        Ok(())
    }

    pub(crate) fn restore_pages(
        &mut self,
        target_len: u32,
        pages: &BTreeMap<u32, MemoryPage>,
        protected: Option<(u32, u32)>,
    ) -> Result<(), VmError> {
        if target_len < self.original_end
            || target_len > self.maximum
            || !target_len.is_multiple_of(256)
        {
            return Err(VmError::InvalidSave);
        }
        let protected_bytes = self.protected_bytes(protected, target_len);
        if !self.resize(target_len)? {
            return Err(VmError::MemoryAllocation(target_len));
        }
        self.bytes[self.ram_start as usize..self.ext_start as usize]
            .copy_from_slice(&self.initial[self.ram_start as usize..self.ext_start as usize]);
        self.bytes[self.ext_start as usize..].fill(0);
        for (&address, page) in pages {
            if address < self.ram_start
                || address.checked_add(256).is_none_or(|end| end > target_len)
                || !address.is_multiple_of(256)
                || page.len() != 256
            {
                return Err(VmError::InvalidSave);
            }
            self.bytes[address as usize..address as usize + 256].copy_from_slice(page.as_slice());
        }
        self.restore_protected(protected_bytes);
        self.mark_all_pages_dirty();
        Ok(())
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

    pub(crate) fn slice(&self, address: u32, length: u32) -> Result<&[u8], VmError> {
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

    fn mark_dirty_range(&mut self, address: u32, length: u32) {
        if length == 0 {
            return;
        }
        let start = address.max(self.ram_start) & !255;
        let end = address
            .saturating_add(length.saturating_sub(1))
            .min(self.len().saturating_sub(1))
            & !255;
        if start > end {
            return;
        }
        self.dirty_pages.extend((start..=end).step_by(256));
    }

    fn protected_bytes(
        &self,
        protected: Option<(u32, u32)>,
        target_end: u32,
    ) -> Option<(u32, Vec<u8>)> {
        let (start, length) = protected?;
        let end = start.saturating_add(length).min(target_end);
        let start = start.max(self.ram_start).min(end);
        let mut bytes = vec![0; (end - start) as usize];
        let source_end = end.min(self.len());
        if start < source_end {
            bytes[..(source_end - start) as usize]
                .copy_from_slice(&self.bytes[start as usize..source_end as usize]);
        }
        Some((start, bytes))
    }

    fn restore_protected(&mut self, protected: Option<(u32, Vec<u8>)>) {
        let Some((start, bytes)) = protected else {
            return;
        };
        let length = bytes.len().min(self.len().saturating_sub(start) as usize);
        if length != 0 {
            self.bytes[start as usize..start as usize + length].copy_from_slice(&bytes[..length]);
        }
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
    fn configured_limit_controls_initial_allocation_growth_and_restore() {
        let story = story();
        assert!(matches!(
            Memory::new_with_limit(&story, 0x100),
            Err(VmError::MemoryLimit { .. })
        ));
        for limit in [0, 511] {
            assert!(matches!(
                Memory::new_with_limit(&story, limit),
                Err(VmError::InvalidMemoryLimit(_))
            ));
        }
        let mut memory = Memory::new_with_limit(&story, 0x300).unwrap();
        assert!(memory.resize(0x300).unwrap());
        assert!(!memory.resize(0x400).unwrap());
        assert_eq!(memory.len(), 0x300);
        assert!(!memory.resize(0x301).unwrap());
        let snapshot = memory.clone();
        let mut smaller = Memory::new_with_limit(&story, 0x200).unwrap();
        assert!(smaller.restore(&snapshot, None).is_err());
        assert_eq!(smaller.len(), 0x200);
        memory.set_maximum(0x400).unwrap();
        memory.restore(&snapshot, None).unwrap();
        assert_eq!(memory.maximum(), 0x400);
        memory.restart(None);
        assert_eq!(memory.maximum(), 0x400);
        assert!(memory.resize(0x400).unwrap());
        let larger = Memory::new_with_limit(&story, MAX_MEMORY_SIZE * 2).unwrap();
        assert_eq!(larger.maximum(), MAX_MEMORY_SIZE * 2);
    }

    #[test]
    fn page_snapshots_share_unchanged_pages_and_restore_protected_bytes() {
        let story = story();
        let mut memory = Memory::new_with_limit(&story, 0x400).unwrap();
        assert!(memory.resize(0x400).unwrap());
        memory.write8(0x100, 1).unwrap();
        memory.write8(0x200, 2).unwrap();
        let first = memory.snapshot_pages(None);
        memory.write8(0x100, 3).unwrap();
        let second = memory.snapshot_pages(Some(&first));
        assert!(!std::sync::Arc::ptr_eq(
            first.get(&0x100).unwrap(),
            second.get(&0x100).unwrap()
        ));
        assert!(std::sync::Arc::ptr_eq(
            first.get(&0x200).unwrap(),
            second.get(&0x200).unwrap()
        ));

        memory.write8(0x100, 9).unwrap();
        memory.write8(0x101, 8).unwrap();
        memory.write8(0x200, 7).unwrap();
        memory
            .restore_pages(0x400, &second, Some((0x100, 1)))
            .unwrap();
        assert_eq!(memory.read8(0x100).unwrap(), 9);
        assert_eq!(memory.read8(0x101).unwrap(), 0);
        assert_eq!(memory.read8(0x200).unwrap(), 2);
    }

    #[test]
    fn limited_page_snapshots_reject_dense_dirty_memory() {
        let story = story();
        let mut memory = Memory::new_with_limit(&story, 0x4000).unwrap();
        assert!(memory.resize(0x4000).unwrap());
        for page in 0..8 {
            memory.write8(0x100 + page * 256, (page + 1) as u8).unwrap();
        }

        assert!(memory.snapshot_pages_with_limit(None, 1).is_none());
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_dirty_page_snapshots() {
        for (name, dirty_pages) in [("sparse", 1usize), ("dense", 4096)] {
            let mut memory = Memory::new_with_limit(&story(), 64 * 1024 * 1024).unwrap();
            assert!(memory.resize(64 * 1024 * 1024).unwrap());
            memory.clear_dirty_pages();
            for page in 0..dirty_pages {
                memory
                    .write8(0x100 + page as u32 * 256, (page % 255 + 1) as u8)
                    .unwrap();
            }
            let started = std::time::Instant::now();
            let pages = memory.snapshot_pages(None);
            let elapsed = started.elapsed();
            assert_eq!(pages.len(), dirty_pages);
            eprintln!(
                "BENCHMARK name=dirty_page_snapshot class={name} memory_bytes={} dirty_pages={dirty_pages} elapsed_ns={} ns_per_page={:.3}",
                memory.len(),
                elapsed.as_nanos(),
                elapsed.as_secs_f64() * 1_000_000_000.0 / dirty_pages as f64
            );
        }
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

    #[test]
    fn restart_preserves_the_requested_ram_range() {
        let mut memory = Memory::new(&story());
        memory.write32(0x100, 0x1234_5678).unwrap();
        memory.write32(0x104, 0x8765_4321).unwrap();

        memory.restart(Some((0x100, 4)));

        assert_eq!(memory.read32(0x100).unwrap(), 0x1234_5678);
        assert_eq!(memory.read32(0x104).unwrap(), 0);
    }

    #[test]
    fn zero_length_block_operations_do_not_access_memory() {
        let mut memory = Memory::new(&story());
        let before = memory.bytes.clone();
        for address in [0, 0x20, 0x100, memory.len(), u32::MAX] {
            memory.zero(address, 0).unwrap();
            memory.copy(address, u32::MAX, 0).unwrap();
            memory.copy(u32::MAX, address, 0).unwrap();
        }
        assert_eq!(memory.bytes, before);
    }
}
