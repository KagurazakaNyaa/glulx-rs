use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{Story, VmError, story::StoryImage};

pub(crate) type MemoryPage = Arc<Vec<u8>>;
const PAGE_SIZE: usize = 256;
static ZERO_PAGE: [u8; PAGE_SIZE] = [0; PAGE_SIZE];

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

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryLayout {
    LegacyAbsolute,
    RamRelative,
}

fn default_memory_layout() -> MemoryLayout {
    MemoryLayout::LegacyAbsolute
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Memory {
    #[serde(skip, default = "default_memory_limit")]
    maximum: u32,
    // Kept only to read desktop sessions written before the paged layout.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bytes: Vec<u8>,
    #[serde(default)]
    pages: Vec<Option<MemoryPage>>,
    initial: Arc<StoryImage>,
    #[serde(default = "default_memory_layout")]
    layout: MemoryLayout,
    ram_start: u32,
    ext_start: u32,
    original_end: u32,
    #[serde(skip)]
    dirty_pages: BTreeSet<u32>,
}

impl Memory {
    pub(crate) fn snapshot_byte_len(&self) -> usize {
        let pages = self
            .pages
            .iter()
            .fold(self.pages.len().saturating_mul(8), |size, page| {
                size.saturating_add(page.as_ref().map_or(0, |page| page.len()))
            });
        self.bytes
            .len()
            .saturating_add(pages)
            .saturating_add(self.initial.len())
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
            let baseline = self.baseline_page(address);
            let current = self.current_page((address - self.ram_start) as usize / PAGE_SIZE);
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
                let index = (address - self.ram_start) as usize / PAGE_SIZE;
                let page = self.pages[index]
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(current.to_vec()));
                pages.insert(address, page);
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

    pub(crate) fn validate_session(&mut self, story: &Story) -> Result<(), VmError> {
        // Older desktop sessions stored Memory.bytes from address zero. Convert
        // that representation before validating the current RAM-relative one.
        if !self.bytes.is_empty() {
            let mut bytes = std::mem::take(&mut self.bytes);
            if self.layout == MemoryLayout::LegacyAbsolute {
                if bytes.len() < self.ram_start as usize {
                    return Err(VmError::InvalidSave);
                }
                bytes = bytes.split_off(self.ram_start as usize);
            }
            if !bytes.len().is_multiple_of(PAGE_SIZE) {
                return Err(VmError::InvalidSave);
            }
            self.pages.clear();
            self.pages
                .try_reserve_exact(bytes.len() / PAGE_SIZE)
                .map_err(|_| VmError::InvalidSave)?;
            for (index, page) in bytes.as_chunks::<PAGE_SIZE>().0.iter().enumerate() {
                let address = self.ram_start + (index * PAGE_SIZE) as u32;
                let baseline = if address < self.ext_start {
                    &self.initial[address as usize..address as usize + PAGE_SIZE]
                } else {
                    &ZERO_PAGE[..]
                };
                self.pages
                    .push((page != baseline).then(|| Arc::new(page.to_vec())));
            }
            self.layout = MemoryLayout::RamRelative;
        }
        if self.len() > self.maximum
            || self.ram_start != story.header.ram_start
            || self.ext_start != story.header.ext_start
            || self.original_end != story.header.end_mem
            || self.initial != story.image
            || self.len() < self.original_end
            || !self.len().is_multiple_of(PAGE_SIZE as u32)
            || self
                .pages
                .iter()
                .any(|page| page.as_ref().is_some_and(|page| page.len() != PAGE_SIZE))
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
        let writable_len = story.header.end_mem - story.header.ram_start;
        let page_count = writable_len as usize / PAGE_SIZE;
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(page_count)
            .map_err(|_| VmError::MemoryAllocation(story.header.end_mem))?;
        pages.resize(page_count, None);
        Ok(Self {
            maximum,
            bytes: Vec::new(),
            pages,
            initial: Arc::clone(&story.image),
            layout: MemoryLayout::RamRelative,
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
        self.ram_start
            .saturating_add((self.pages.len() * PAGE_SIZE) as u32)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn ram_start(&self) -> u32 {
        self.ram_start
    }

    pub fn read8(&self, address: u32) -> Result<u8, VmError> {
        if address < self.ram_start {
            self.initial
                .get(address as usize)
                .copied()
                .ok_or(VmError::MemoryRead(address))
        } else if address < self.len() {
            let page = (address - self.ram_start) as usize / PAGE_SIZE;
            let offset = (address as usize) % PAGE_SIZE;
            Ok(self.current_page(page)[offset])
        } else {
            Err(VmError::MemoryRead(address))
        }
    }

    pub fn read16(&self, address: u32) -> Result<u16, VmError> {
        if let Ok(raw) = self.slice(address, 2) {
            return Ok(u16::from_be_bytes(raw.try_into().expect("length checked")));
        }
        let next = address.checked_add(1).ok_or(VmError::MemoryRead(address))?;
        Ok(u16::from_be_bytes([
            self.read8(address)?,
            self.read8(next)?,
        ]))
    }

    pub fn read32(&self, address: u32) -> Result<u32, VmError> {
        if let Ok(raw) = self.slice(address, 4) {
            return Ok(u32::from_be_bytes(raw.try_into().expect("length checked")));
        }
        let end = address.checked_add(3).ok_or(VmError::MemoryRead(address))?;
        Ok(u32::from_be_bytes([
            self.read8(address)?,
            self.read8(address + 1)?,
            self.read8(address + 2)?,
            self.read8(end)?,
        ]))
    }

    pub(crate) fn key(&self, address: u32, length: u32) -> Result<[u8; 4], VmError> {
        let length = usize::try_from(length).map_err(|_| VmError::MemoryRead(address))?;
        let end = address
            .checked_add(length as u32)
            .ok_or(VmError::MemoryRead(address))?;
        let mut result = [0; 4];
        if length > result.len() || end > self.len() {
            return Err(VmError::MemoryRead(address));
        }
        if let Ok(raw) = self.slice(address, length as u32) {
            result[..length].copy_from_slice(raw);
        } else {
            for (offset, byte) in result.iter_mut().take(length).enumerate() {
                *byte = self.read8(address + offset as u32)?;
            }
        }
        Ok(result)
    }

    pub(crate) fn key_equals(
        &self,
        address: u32,
        length: u32,
        key: &[u8; 4],
    ) -> Result<bool, VmError> {
        let length = usize::try_from(length).map_err(|_| VmError::MemoryRead(address))?;
        if length > key.len() {
            return Err(VmError::MemoryRead(address));
        }
        if let Ok(raw) = self.slice(address, length as u32) {
            return Ok(raw == &key[..length]);
        }
        for (offset, expected) in key.iter().take(length).enumerate() {
            if self.read8(address + offset as u32)? != *expected {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn key_is_zero(&self, address: u32, length: u32) -> Result<bool, VmError> {
        let length = usize::try_from(length).map_err(|_| VmError::MemoryRead(address))?;
        if length > 4 {
            return Err(VmError::MemoryRead(address));
        }
        if let Ok(raw) = self.slice(address, length as u32) {
            return Ok(raw.iter().all(|byte| *byte == 0));
        }
        for offset in 0..length {
            if self.read8(address + offset as u32)? != 0 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn key_cmp(
        &self,
        address: u32,
        length: u32,
        key: &[u8; 4],
    ) -> Result<std::cmp::Ordering, VmError> {
        let length = usize::try_from(length).map_err(|_| VmError::MemoryRead(address))?;
        if length > key.len() {
            return Err(VmError::MemoryRead(address));
        }
        if let Ok(raw) = self.slice(address, length as u32) {
            return Ok(raw.cmp(&key[..length]));
        }
        let candidate = self.key(address, length as u32)?;
        Ok(candidate[..length].cmp(&key[..length]))
    }

    pub fn write8(&mut self, address: u32, value: u8) -> Result<(), VmError> {
        self.write_bytes(address, &[value])
    }

    pub fn write16(&mut self, address: u32, value: u16) -> Result<(), VmError> {
        self.write_bytes(address, &value.to_be_bytes())
    }

    pub fn write32(&mut self, address: u32, value: u32) -> Result<(), VmError> {
        self.write_bytes(address, &value.to_be_bytes())
    }

    pub fn zero(&mut self, address: u32, length: u32) -> Result<(), VmError> {
        if length == 0 {
            return Ok(());
        }
        self.check_write(address, length)?;
        let mut offset = 0usize;
        while offset < length as usize {
            let current = address as usize - self.ram_start as usize + offset;
            let page = current / PAGE_SIZE;
            let within = current % PAGE_SIZE;
            let count = (length as usize - offset).min(PAGE_SIZE - within);
            let page_address = self.ram_start + (page * PAGE_SIZE) as u32;
            let baseline_is_zero = self
                .baseline_page(page_address)
                .iter()
                .all(|byte| *byte == 0);
            if within == 0 && count == PAGE_SIZE && baseline_is_zero {
                self.pages[page] = None;
            } else {
                self.ensure_page(page)[within..within + count].fill(0);
            }
            offset += count;
        }
        self.mark_dirty_range(address, length);
        Ok(())
    }

    pub fn copy(&mut self, source: u32, destination: u32, length: u32) -> Result<(), VmError> {
        if length == 0 {
            return Ok(());
        }
        let source_end = source
            .checked_add(length)
            .filter(|end| *end <= self.len())
            .ok_or(VmError::MemoryRead(source))?;
        let copied = (source..source_end)
            .map(|address| self.read8(address))
            .collect::<Result<Vec<_>, _>>()?;
        self.write_bytes(destination, &copied)
    }

    pub fn resize(&mut self, new_size: u32) -> Result<bool, VmError> {
        if new_size < self.original_end
            || new_size > self.maximum
            || !new_size.is_multiple_of(0x100)
        {
            return Ok(false);
        }
        let old_size = self.len();
        let page_count = (new_size - self.ram_start) as usize / PAGE_SIZE;
        if page_count > self.pages.len()
            && self
                .pages
                .try_reserve_exact(page_count - self.pages.len())
                .is_err()
        {
            return Ok(false);
        }
        self.pages.resize(page_count, None);
        if old_size != new_size {
            self.mark_dirty_range(old_size.min(new_size), old_size.abs_diff(new_size));
        }
        Ok(true)
    }

    pub fn restart(&mut self, protected: Option<(u32, u32)>) {
        let protected_bytes = self.protected_bytes(protected, self.original_end);
        self.pages.resize(
            (self.original_end - self.ram_start) as usize / PAGE_SIZE,
            None,
        );
        self.pages.fill(None);
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
        self.pages.fill(None);
        for (&address, page) in pages {
            if address < self.ram_start
                || address
                    .checked_add(PAGE_SIZE as u32)
                    .is_none_or(|end| end > target_len)
                || !address.is_multiple_of(PAGE_SIZE as u32)
                || page.len() != PAGE_SIZE
            {
                return Err(VmError::InvalidSave);
            }
            let index = (address - self.ram_start) as usize / PAGE_SIZE;
            self.pages[index] = Some(page.clone());
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

    fn baseline_page(&self, address: u32) -> &[u8] {
        if address < self.ext_start {
            &self.initial[address as usize..address as usize + PAGE_SIZE]
        } else {
            &ZERO_PAGE
        }
    }

    fn current_page(&self, index: usize) -> &[u8] {
        let address = self.ram_start + (index * PAGE_SIZE) as u32;
        self.pages[index]
            .as_ref()
            .map_or_else(|| self.baseline_page(address), |page| page.as_slice())
    }

    fn ensure_page(&mut self, index: usize) -> &mut [u8] {
        if self.pages[index].is_none() {
            let address = self.ram_start + (index * PAGE_SIZE) as u32;
            self.pages[index] = Some(Arc::new(self.baseline_page(address).to_vec()));
        }
        Arc::make_mut(self.pages[index].as_mut().unwrap()).as_mut_slice()
    }

    fn write_bytes(&mut self, address: u32, bytes: &[u8]) -> Result<(), VmError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let length = u32::try_from(bytes.len()).map_err(|_| VmError::MemoryWrite(address))?;
        self.check_write(address, length)?;
        let mut offset = 0usize;
        while offset < bytes.len() {
            let current = address as usize - self.ram_start as usize + offset;
            let page = current / PAGE_SIZE;
            let within = current % PAGE_SIZE;
            let count = (bytes.len() - offset).min(PAGE_SIZE - within);
            self.ensure_page(page)[within..within + count]
                .copy_from_slice(&bytes[offset..offset + count]);
            offset += count;
        }
        self.mark_dirty_range(address, length);
        Ok(())
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

    fn slice(&self, address: u32, length: u32) -> Result<&[u8], VmError> {
        if length == 0 {
            return Ok(&[]);
        }
        let end = address
            .checked_add(length)
            .filter(|end| *end <= self.len())
            .ok_or(VmError::MemoryRead(address))?;
        if end <= self.ram_start {
            Ok(&self.initial[address as usize..end as usize])
        } else if address >= self.ram_start {
            let offset = (address - self.ram_start) as usize;
            let within = offset % PAGE_SIZE;
            if within + length as usize > PAGE_SIZE {
                return Err(VmError::MemoryRead(address));
            }
            let page = offset / PAGE_SIZE;
            Ok(&self.current_page(page)[within..within + length as usize])
        } else {
            Err(VmError::MemoryRead(address))
        }
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
            for (offset, byte) in bytes
                .iter_mut()
                .take((source_end - start) as usize)
                .enumerate()
            {
                *byte = self
                    .read8(start + offset as u32)
                    .expect("protected range checked");
            }
        }
        Some((start, bytes))
    }

    fn restore_protected(&mut self, protected: Option<(u32, Vec<u8>)>) {
        let Some((start, bytes)) = protected else {
            return;
        };
        let length = bytes.len().min(self.len().saturating_sub(start) as usize);
        if length != 0 {
            self.write_bytes(start, &bytes[..length])
                .expect("protected range checked");
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
    fn memory_stores_only_the_writable_range() {
        let story = story();
        let memory = Memory::new_with_limit(&story, 0x400).unwrap();

        assert_eq!(memory.len(), 0x200);
        assert_eq!(memory.pages.len(), 1);
        assert!(memory.pages.iter().all(Option::is_none));
        assert_eq!(memory.read8(0x20).unwrap(), story.image[0x20]);
        assert_eq!(memory.read8(0x100).unwrap(), 0);
    }

    #[test]
    fn extended_zero_memory_stays_lazy_until_written() {
        let story = story();
        let mut memory = Memory::new_with_limit(&story, 16 * 1024 * 1024).unwrap();
        assert!(memory.resize(16 * 1024 * 1024).unwrap());
        let before = memory.snapshot_byte_len();

        assert_eq!(memory.read8(0x00f0_0000).unwrap(), 0);
        assert!(before < 1024 * 1024);

        memory.write8(0x00f0_0000, 7).unwrap();
        assert_eq!(memory.read8(0x00f0_0000).unwrap(), 7);
        assert!(memory.snapshot_byte_len() >= before + 256);
        assert!(memory.snapshot_byte_len() < 1024 * 1024);
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
        let before = memory.clone();
        for address in [0, 0x20, 0x100, memory.len(), u32::MAX] {
            memory.zero(address, 0).unwrap();
            memory.copy(address, u32::MAX, 0).unwrap();
            memory.copy(u32::MAX, address, 0).unwrap();
        }
        assert_eq!(memory.pages, before.pages);
    }
}
