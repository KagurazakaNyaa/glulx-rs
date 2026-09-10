//! Background media preparation that does not require an egui or Glk owner.

use std::{sync::mpsc, thread};

use image::RgbaImage;

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
