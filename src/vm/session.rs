use super::*;

impl Vm {
    /// Raw story and memory bytes that the text desktop snapshot would expand
    /// into integer arrays. Check before serializing, including undo memories.
    pub(crate) fn snapshot_byte_len(&self) -> usize {
        self.undo.iter().fold(
            self.story
                .snapshot_byte_len()
                .saturating_add(self.memory.snapshot_byte_len()),
            |bytes, undo| bytes.saturating_add(undo.byte_len()),
        )
    }

    /// Validate a desktop session before resuming it. This is a versioned player
    /// snapshot, separate from portable IFZS saves (which exclude Glk state).
    pub fn validate_session(mut self) -> Result<Self, VmError> {
        let original = self
            .story
            .validated_session_story()
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
        for undo in &mut self.undo {
            if let Some(mut memory) = undo.memory.take() {
                memory.validate_session(&original)?;
                memory.mark_all_pages_dirty();
                undo.memory_len = memory.len();
                undo.memory_pages = memory.snapshot_pages(None);
            }
            if undo.memory_len < original.header.end_mem
                || undo.memory_len > self.memory.maximum()
                || !undo.memory_len.is_multiple_of(256)
                || undo.memory_pages.values().any(|page| page.len() != 256)
                || undo.memory_pages.keys().any(|address| {
                    *address < original.header.ram_start
                        || address
                            .checked_add(256)
                            .is_none_or(|end| end > undo.memory_len)
                        || !address.is_multiple_of(256)
                })
            {
                return Err(VmError::InvalidSave);
            }
        }
        save::validate_stack(&self.stack, self.memory.len())?;
        for window in self
            .glk_windows
            .values_mut()
            .filter(|window| window.kind == 3)
        {
            window.recount_text_chars();
        }
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
    pub fn resource_path(&self) -> Option<&std::path::Path> {
        self.story.resource_path()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ResourceSelection,
        story::tests::{ResourceDirectory, resource_blorb},
    };

    #[derive(Default)]
    struct SessionStorage(BTreeMap<String, String>);

    impl eframe::Storage for SessionStorage {
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

    #[test]
    fn sessions_retain_external_archives_and_directories_after_files_are_removed() {
        for (bundled_story, loose) in [(false, false), (false, true), (true, false)] {
            let directory = ResourceDirectory::new();
            let image = super::super::tests::image_with_program(&[0x81, 0x20]);
            let story_path = directory.0.join(if bundled_story {
                "game.gblorb"
            } else {
                "game.ulx"
            });
            let story_bytes = if bundled_story {
                resource_blorb(&[
                    ((*b"GLUL", Some((*b"Exec", 0))), &image),
                    ((*b"TEXT", Some((*b"Data", 99))), b"old bundled resource"),
                ])
            } else {
                image.clone()
            };
            std::fs::write(&story_path, &story_bytes).unwrap();
            let metadata = b"<ifindex><title>External title</title></ifindex>";
            let resource_path = directory
                .0
                .join(if loose { "assets" } else { "assets.blorb" });
            if loose {
                std::fs::create_dir(&resource_path).unwrap();
                for (name, data) in [
                    ("DATA1.txt", "é\n".as_bytes()),
                    ("PIC2.png", &b"picture"[..]),
                    ("SND3.aiff", &b"FORM\0\0\0\x04AIFF"[..]),
                    ("METADATA.xml", &metadata[..]),
                    ("IDENT", &image[..128]),
                ] {
                    std::fs::write(resource_path.join(name), data).unwrap();
                }
            } else {
                std::fs::write(
                    &resource_path,
                    resource_blorb(&[
                        ((*b"TEXT", Some((*b"Data", 1))), "é\n".as_bytes()),
                        ((*b"PNG ", Some((*b"Pict", 2))), b"picture"),
                        ((*b"FORM", Some((*b"Snd ", 3))), b"AIFF"),
                        ((*b"IFmd", None), metadata),
                        ((*b"IFhd", None), &image[..128]),
                    ]),
                )
                .unwrap();
            }
            let story = Story::open_with_resources(
                &story_path,
                ResourceSelection::Path(resource_path.clone()),
            )
            .unwrap();
            let mut vm = Vm::new(story).unwrap();
            let stream = vm.open_resource_stream(&[1, 17], true);
            assert_ne!(stream, 0);
            assert_eq!(vm.read_stream_value(stream).unwrap(), Some('é' as u32));
            vm.memory.write32(0x100, 0x12345678).unwrap();
            let mut snapshot = SessionStorage::default();
            eframe::set_value(&mut snapshot, "vm", &vm);
            std::fs::remove_dir_all(&directory.0).unwrap();
            let restored: Vm = eframe::get_value(&snapshot, "vm").unwrap();
            let mut restored = restored.validate_session().unwrap();
            assert_eq!(restored.story_path(), Some(story_path.as_path()));
            assert_eq!(restored.resource_path(), Some(resource_path.as_path()));
            assert_eq!(restored.story_title(), "External title");
            assert_eq!(restored.story.image.as_slice(), image.as_slice());
            assert_eq!(restored.story.container.is_some(), bundled_story);
            assert_eq!(restored.memory.read32(0x100).unwrap(), 0x12345678);
            assert_eq!(restored.image_resource(2), Some(&b"picture"[..]));
            assert_eq!(
                restored.story.sound_resource(3),
                Some(&b"FORM\0\0\0\x04AIFF"[..])
            );
            assert_eq!(restored.read_stream_value(stream).unwrap(), Some(10));
            assert_eq!(restored.read_stream_value(stream).unwrap(), None);
            let new_stream = restored.open_resource_stream(&[1, 0], true);
            assert_eq!(
                restored.read_stream_value(new_stream).unwrap(),
                Some('é' as u32)
            );
            assert_eq!(restored.open_resource_stream(&[99, 0], false), 0);
        }
    }

    #[test]
    fn legacy_sessions_without_external_fields_restore_original_resource_maps() {
        let image = super::super::tests::image_with_program(&[0x81, 0x20]);
        for bundled in [false, true] {
            let bytes = if bundled {
                resource_blorb(&[
                    ((*b"GLUL", Some((*b"Exec", 0))), &image),
                    ((*b"TEXT", Some((*b"Data", 1))), b"legacy"),
                ])
            } else {
                image.clone()
            };
            let vm = Vm::new(Story::from_bytes(&bytes, Some("Legacy")).unwrap()).unwrap();
            let mut serialized = serde_json::to_value(&vm).unwrap();
            serialized["story"]
                .as_object_mut()
                .unwrap()
                .remove("external_resources");
            let restored: Vm = serde_json::from_value(serialized).unwrap();
            let restored = restored.validate_session().unwrap();
            assert_eq!(restored.story.image.as_slice(), image.as_slice());
            assert_eq!(restored.story_title(), "Legacy");
            assert_eq!(
                restored.story.resource(*b"Data", 1),
                bundled.then_some(&b"legacy"[..])
            );
        }
    }

    #[test]
    fn sessions_validate_saved_external_blorb_identity_and_index() {
        let image = super::super::tests::image_with_program(&[0x81, 0x20]);
        let mut story = Story::from_bytes(&image, None).unwrap();
        story
            .attach_blorb(&resource_blorb(&[
                ((*b"TEXT", Some((*b"Data", 1))), b"data"),
                ((*b"IFhd", None), &image[..128]),
            ]))
            .unwrap();
        let vm = Vm::new(story).unwrap();
        let original = serde_json::to_value(&vm).unwrap();
        for byte in [
            35,
            original["story"]["external_resources"]["bytes"]
                .as_array()
                .unwrap()
                .len()
                - 1,
        ] {
            let mut corrupted = original.clone();
            let value = &mut corrupted["story"]["external_resources"]["bytes"][byte];
            *value = serde_json::Value::from(value.as_u64().unwrap() ^ 1);
            let restored: Vm = serde_json::from_value(corrupted).unwrap();
            assert!(matches!(
                restored.validate_session(),
                Err(VmError::InvalidSave)
            ));
        }
    }

    #[test]
    fn sessions_migrate_legacy_full_undo_memory_to_pages() {
        let program = [0x81, 0x25, 0x0d, 0x20, 0x81, 0x20];
        let story =
            Story::from_bytes(&super::super::tests::image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.run_steps(1).unwrap();
        vm.memory.write8(0x120, 9).unwrap();

        let mut serialized = serde_json::to_value(&vm).unwrap();
        let legacy_memory = serde_json::to_value(&vm.memory).unwrap();
        let undo = serialized["undo"]
            .as_array_mut()
            .unwrap()
            .first_mut()
            .unwrap();
        undo["memory"] = legacy_memory;
        undo.as_object_mut().unwrap().remove("memory_len");
        undo.as_object_mut().unwrap().remove("memory_pages");

        let restored: Vm = serde_json::from_value(serialized).unwrap();
        let restored = restored.validate_session().unwrap();
        let undo = restored.undo.front().unwrap();
        assert!(undo.memory.is_none());
        assert_eq!(undo.memory_len, restored.memory.len());
        assert_eq!(undo.memory_pages[&0x100].as_slice()[0x20], 9);
    }

    #[test]
    fn sessions_reject_undo_pages_with_invalid_lengths() {
        let program = [0x81, 0x25, 0x0d, 0x20, 0x81, 0x20];
        let story =
            Story::from_bytes(&super::super::tests::image_with_program(&program), None).unwrap();
        let mut vm = Vm::new(story).unwrap();
        vm.run_steps(1).unwrap();
        let mut serialized = serde_json::to_value(&vm).unwrap();
        serialized["undo"][0]["memory_pages"]["256"] = serde_json::json!([0]);
        let restored: Vm = serde_json::from_value(serialized).unwrap();
        assert!(matches!(
            restored.validate_session(),
            Err(VmError::InvalidSave)
        ));
    }
}
