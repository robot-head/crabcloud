//! Placeholder — replaced in Task A5.

use bytes::Bytes;
use std::io;
use tokio::sync::mpsc;

pub struct MpscBytesWriter {
    _tx: mpsc::Sender<Result<Bytes, io::Error>>,
}

impl MpscBytesWriter {
    pub fn new(tx: mpsc::Sender<Result<Bytes, io::Error>>) -> Self {
        Self { _tx: tx }
    }
}
