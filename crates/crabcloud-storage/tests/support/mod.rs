//! Test fixtures shared across `trait_suite.rs`, `local_specific.rs`,
//! and `memory_specific.rs`.

#![allow(dead_code)]

use crabcloud_storage::{EventSink, StorageEvent};
use std::sync::{Arc, Mutex};

/// EventSink that buffers every emission into a `Vec` for assertions.
#[derive(Clone, Default)]
pub struct RecordingSink {
    pub events: Arc<Mutex<Vec<StorageEvent>>>,
}

impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn drain(&self) -> Vec<StorageEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    pub fn snapshot(&self) -> Vec<StorageEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl EventSink for RecordingSink {
    async fn emit(&self, event: StorageEvent) {
        self.events.lock().unwrap().push(event);
    }
}
