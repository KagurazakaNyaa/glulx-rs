//! Background media preparation that does not require an egui or Glk owner.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use image::RgbaImage;

use crate::{ResourceSelection, Vm, memory::ResourceLimits};

pub(super) struct ImageDecodeWorker {
    sender: Option<mpsc::Sender<ImageDecodeTask>>,
    results: mpsc::Receiver<ImageDecodeResult>,
    next_id: u64,
}

struct ImageDecodeTask {
    id: u64,
    data: Vec<u8>,
    maximum: u64,
}

pub(super) struct ImageDecodeResult {
    pub id: u64,
    pub image: Result<std::sync::Arc<RgbaImage>, String>,
}

impl Default for ImageDecodeWorker {
    fn default() -> Self {
        let (task_sender, task_receiver) = mpsc::channel::<ImageDecodeTask>();
        let (result_sender, result_receiver) = mpsc::channel::<ImageDecodeResult>();
        let sender = thread::Builder::new()
            .name("glulx-image-decode".to_owned())
            .spawn(move || {
                while let Ok(task) = task_receiver.recv() {
                    let image = crate::picture::decode_with_limit(&task.data, task.maximum)
                        .map(std::sync::Arc::new)
                        .map_err(|error| error.to_string());
                    if result_sender
                        .send(ImageDecodeResult { id: task.id, image })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .ok()
            .map(|_| task_sender);
        Self {
            sender,
            results: result_receiver,
            next_id: 0,
        }
    }
}

impl ImageDecodeWorker {
    pub(super) fn submit(&mut self, data: Vec<u8>, maximum: u64) -> Option<u64> {
        let sender = self.sender.as_ref()?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        sender
            .send(ImageDecodeTask { id, data, maximum })
            .ok()
            .map(|_| id)
    }

    pub(super) fn poll(&self) -> impl Iterator<Item = ImageDecodeResult> + '_ {
        self.results.try_iter()
    }
}

pub(super) struct StoryLoadWorker {
    sender: Option<mpsc::Sender<StoryLoadTask>>,
    results: mpsc::Receiver<StoryLoadResult>,
    next_id: u64,
    latest: Arc<AtomicU64>,
}

struct StoryLoadTask {
    id: u64,
    path: PathBuf,
    selection: ResourceSelection,
    maximum: u32,
    resources: ResourceLimits,
}

pub(super) struct StoryLoadResult {
    pub id: u64,
    pub path: PathBuf,
    pub vm: Result<Vm, String>,
}

impl Default for StoryLoadWorker {
    fn default() -> Self {
        let (task_sender, task_receiver) = mpsc::channel::<StoryLoadTask>();
        let (result_sender, result_receiver) = mpsc::channel::<StoryLoadResult>();
        let latest = Arc::new(AtomicU64::new(u64::MAX));
        let worker_latest = latest.clone();
        let sender = thread::Builder::new()
            .name("glulx-story-load".to_owned())
            .spawn(move || {
                while let Ok(task) = task_receiver.recv() {
                    if worker_latest.load(Ordering::Acquire) != task.id {
                        continue;
                    }
                    let vm = (|| {
                        let story = crate::Story::open_with_resources(&task.path, task.selection)
                            .map_err(|error| error.to_string())?;
                        let mut vm = Vm::new_with_memory_limit(story, task.maximum)
                            .map_err(|error| error.to_string())?;
                        vm.set_resource_limits(task.resources);
                        vm.enable_audio();
                        Ok(vm)
                    })();
                    if worker_latest.load(Ordering::Acquire) != task.id {
                        continue;
                    }
                    if result_sender
                        .send(StoryLoadResult {
                            id: task.id,
                            path: task.path,
                            vm,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .ok()
            .map(|_| task_sender);
        Self {
            sender,
            results: result_receiver,
            next_id: 0,
            latest,
        }
    }
}

impl StoryLoadWorker {
    pub(super) fn submit(
        &mut self,
        path: PathBuf,
        selection: ResourceSelection,
        maximum: u32,
        resources: ResourceLimits,
    ) -> Option<u64> {
        let sender = self.sender.as_ref()?;
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.latest.store(id, Ordering::Release);
        sender
            .send(StoryLoadTask {
                id,
                path,
                selection,
                maximum,
                resources,
            })
            .ok()
            .map(|_| id)
    }

    pub(super) fn poll(&self) -> impl Iterator<Item = StoryLoadResult> + '_ {
        self.results.try_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_load_worker_returns_a_ready_vm() {
        let path = std::env::temp_dir().join(format!(
            "glulx-rs-load-{}-{}.ulx",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let image = crate::vm::tests::image_with_program(&[0x81, 0x20]);
        std::fs::write(&path, image).unwrap();
        let mut worker = StoryLoadWorker::default();
        let id = worker
            .submit(
                path.clone(),
                ResourceSelection::None,
                1024 * 1024,
                ResourceLimits::default(),
            )
            .unwrap();
        let result = (0..1000).find_map(|_| {
            worker.poll().find(|result| result.id == id).or_else(|| {
                std::thread::sleep(std::time::Duration::from_millis(1));
                None
            })
        });
        let result = result.unwrap();
        assert!(result.vm.is_ok());
        assert_eq!(result.path, path);
        let _ = std::fs::remove_file(path);
    }
}
