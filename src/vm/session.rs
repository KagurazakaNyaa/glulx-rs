use super::*;

impl Vm {
    /// Validate a desktop session before resuming it. This is a versioned player
    /// snapshot, separate from portable IFZS saves (which exclude Glk state).
    pub fn validate_session(mut self) -> Result<Self, VmError> {
        let original = Story::from_bytes(
            self.story.container.as_deref().unwrap_or(&self.story.image),
            Some(&self.story.title),
        )
        .map_err(|_| VmError::InvalidSave)?;
        if original.image != self.story.image
            || original.header.ram_start != self.story.header.ram_start
            || original.header.ext_start != self.story.header.ext_start
            || original.header.end_mem != self.story.header.end_mem
            || original.header.stack_size != self.stack.maximum
            || self.pc >= self.memory.len()
        {
            return Err(VmError::InvalidSave);
        }
        self.memory.validate_session(&original)?;
        save::validate_stack(&self.stack, self.memory.len())?;
        for (&id, window) in &self.glk_windows {
            if window.kind == 4
                && window.grid.len() != window.width as usize * window.height as usize
            {
                return Err(VmError::InvalidSave);
            }
            if window.parent != 0 && !self.glk_windows.contains_key(&window.parent) {
                return Err(VmError::InvalidSave);
            }
            if window.stream != 0
                && !matches!(self.glk_streams.get(&window.stream).map(|s|&s.target),Some(GlkStreamTarget::Window(target)) if *target==id)
            {
                return Err(VmError::InvalidSave);
            }
            let mut seen = HashSet::new();
            let mut parent = id;
            while parent != 0 {
                if !seen.insert(parent) {
                    return Err(VmError::InvalidSave);
                }
                parent = self
                    .glk_windows
                    .get(&parent)
                    .ok_or(VmError::InvalidSave)?
                    .parent;
            }
        }
        if self.glk_root != 0 && !self.glk_windows.contains_key(&self.glk_root) {
            return Err(VmError::InvalidSave);
        }
        if matches!(
            self.state,
            RunState::WaitingForLine | RunState::WaitingForChar | RunState::WaitingForEvent
        ) && self.pending_select.is_none()
        {
            return Err(VmError::InvalidSave);
        }
        if self.state == RunState::WaitingForFile && self.file_request.is_none() {
            return Err(VmError::InvalidSave);
        }
        let table = self.story.header.decoding_table;
        let path = self.story.path.take();
        self.story = original;
        self.story.path = path;
        self.story.header.decoding_table = table;
        self.restore_file_streams()?;
        Ok(self)
    }
    pub fn metadata(&self) -> crate::story::Metadata {
        self.story.metadata()
    }
    pub fn cover(&self) -> Option<&[u8]> {
        self.story.cover()
    }
    pub fn story_title(&self) -> &str {
        &self.story.title
    }
    pub fn story_path(&self) -> Option<&std::path::Path> {
        self.story.path.as_deref()
    }
    pub fn timer_interval(&self) -> Option<u32> {
        self.timer
            .map(|(duration, _)| duration.as_millis().min(u32::MAX as u128) as u32)
    }
    pub fn resume_timer(&mut self, interval: Option<u32>) {
        if let Some(interval) = interval {
            self.request_timer(interval);
        }
    }
}
