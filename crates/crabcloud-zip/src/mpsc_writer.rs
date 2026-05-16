//! `AsyncWrite` adapter that forwards every write to an
//! `mpsc::Sender<Result<Bytes, io::Error>>`. Lets `stream_folder` push
//! bytes through a tokio channel that an axum `Body::from_stream` reads
//! from.

use bytes::Bytes;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

type PermitResult = Result<mpsc::OwnedPermit<Result<Bytes, io::Error>>, mpsc::error::SendError<()>>;
type PermitFuture = Pin<Box<dyn Future<Output = PermitResult> + Send>>;

pub struct MpscBytesWriter {
    tx: mpsc::Sender<Result<Bytes, io::Error>>,
    in_flight: Option<PermitFuture>,
}

impl MpscBytesWriter {
    pub fn new(tx: mpsc::Sender<Result<Bytes, io::Error>>) -> Self {
        Self {
            tx,
            in_flight: None,
        }
    }
}

impl AsyncWrite for MpscBytesWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let len = buf.len();
        // Lazily kick off a reservation if none is in flight.
        if self.in_flight.is_none() {
            let tx = self.tx.clone();
            self.in_flight = Some(Box::pin(tx.reserve_owned()));
        }
        let fut = self.in_flight.as_mut().expect("just set");
        match fut.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => {
                self.in_flight = None;
                let bytes = Bytes::copy_from_slice(buf);
                permit.send(Ok(bytes));
                Poll::Ready(Ok(len))
            }
            Poll::Ready(Err(_)) => {
                self.in_flight = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "receiver dropped",
                )))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn writes_forwarded_in_order() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut writer = MpscBytesWriter::new(tx);
        writer.write_all(b"hello ").await.unwrap();
        writer.write_all(b"world").await.unwrap();
        drop(writer);
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        assert_eq!(combined, b"hello world");
    }

    #[tokio::test]
    async fn receiver_drop_yields_broken_pipe() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, io::Error>>(1);
        drop(rx);
        let mut writer = MpscBytesWriter::new(tx);
        let err = writer.write_all(b"data").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }
}
