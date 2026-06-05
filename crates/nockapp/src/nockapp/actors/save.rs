use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::fs;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;
use tracing::{error, trace};

use crate::kernel::checkpoint::JammedCheckpoint;
use crate::nockapp::NockAppError;

// Save actor messages
pub(crate) enum SaveMessage {
    SaveCheckpoint(JammedCheckpoint),
    // SaveCheckpoint(JammedCheckpoint, Option<tokio::sync::oneshot::Sender<NockAppResult>>),
    // SaveCheckpointWithResponse(JammedCheckpoint, tokio::sync::oneshot::Sender<NockAppResult>),
    // IntervalSaveCheckpoint(JammedCheckpoint),
    Shutdown,
}

// enum ShutdownMessage {
//     Shutdown,
//     ShutdownNow,
// }

pub(crate) struct SaveActor {
    receiver: mpsc::Receiver<SaveMessage>,
    jam_paths: (PathBuf, PathBuf),
    save_interval: Duration,
    last_save: std::time::Instant,
    buffer_toggle: Arc<AtomicBool>,
    event_sender: tokio::sync::watch::Sender<u64>,
}

impl SaveActor {
    pub(crate) fn new(
        receiver: mpsc::Receiver<SaveMessage>,
        jam_paths: (PathBuf, PathBuf),
        save_interval: Duration,
        buffer_toggle: Arc<AtomicBool>,
        event_sender: tokio::sync::watch::Sender<u64>,
    ) -> Self {
        Self {
            receiver,
            jam_paths,
            save_interval,
            last_save: std::time::Instant::now(),
            buffer_toggle,
            event_sender,
        }
    }

    pub(crate) async fn run(mut self) {
        while let Some(msg) = self.receiver.recv().await {
            match msg {
                SaveMessage::SaveCheckpoint(checkpoint) => {
                    if let Err(e) = self.handle_save(checkpoint).await {
                        error!("Save error: {:?}", e);
                    }
                }
                SaveMessage::Shutdown => break,
            }
        }
    }

    async fn handle_save(&mut self, checkpoint: JammedCheckpoint) -> Result<(), NockAppError> {
        // Enforce minimum interval between saves
        let elapsed = self.last_save.elapsed();
        if elapsed < self.save_interval {
            tokio::time::sleep(self.save_interval - elapsed).await;
        }

        let bytes = checkpoint.encode()?;
        let path = if self.buffer_toggle.load(Ordering::SeqCst) {
            &self.jam_paths.1
        } else {
            &self.jam_paths.0
        };

        let mut file = fs::File::create(path)
            .await
            .map_err(NockAppError::SaveError)?;

        file.write_all(&bytes)
            .await
            .map_err(NockAppError::SaveError)?;

        file.sync_all().await.map_err(NockAppError::SaveError)?;

        trace!(
            "Write to {:?} successful, ker_hash: {}, event: {}",
            path.display(),
            checkpoint.ker_hash,
            checkpoint.event_num
        );

        // Flip toggle after successful write
        self.buffer_toggle
            .store(!self.buffer_toggle.load(Ordering::SeqCst), Ordering::SeqCst);
        self.event_sender.send(checkpoint.event_num)?;
        self.last_save = std::time::Instant::now();

        Ok(())
    }
}
