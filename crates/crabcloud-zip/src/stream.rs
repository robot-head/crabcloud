//! Streaming zip writer: walks the plan and writes each entry into the
//! supplied `AsyncWrite` sink. Uses sync `std::io::Write` underneath since
//! `zip = "5"` only ships a sync writer; bridges via
//! `tokio_util::io::SyncIoBridge`.

use crate::compression::compression_for_mime;
use crate::error::{WalkError, ZipError};
use crate::types::{PlanKind, PlannedEntry, ZipCaps, ZipPlan, ZipSummary};
use crate::walk::walk_for_caps;
use chrono::{DateTime, Datelike, Timelike, Utc};
use crabcloud_fs::path::UserPath;
use crabcloud_fs::View;
use tokio::io::{AsyncReadExt, AsyncWrite};
use tokio_util::io::SyncIoBridge;
use zip::write::{ExtendedFileOptions, FileOptions, ZipWriter};
use zip::CompressionMethod;

/// Walk `root`, enforce `caps`, and stream a zip archive into `sink`.
///
/// On `WalkError::TooLarge` the caller hasn't written any bytes yet, so
/// the HTTP handler can return 413 with a JSON body. On success, returns
/// the entry count and total uncompressed byte count.
pub async fn stream_folder<W>(
    view: &View,
    root: &UserPath,
    caps: ZipCaps,
    sink: W,
) -> Result<ZipSummary, ZipError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    // 1. Pre-flight walk. Errors return early before any byte hits `sink`.
    let plan = walk_for_caps(view, root, &caps).await?;

    // 2. Read each file body up-front so the spawned-blocking task can
    //    hand it to the sync `ZipWriter`. Total memory is bounded by
    //    `caps.max_bytes` (pre-flight already enforced).
    write_zip(view, plan, sink).await
}

async fn write_zip<W>(view: &View, plan: ZipPlan, sink: W) -> Result<ZipSummary, ZipError>
where
    W: AsyncWrite + Send + Unpin + 'static,
{
    let mut bodies: Vec<(PlannedEntry, Vec<u8>)> = Vec::with_capacity(plan.entries.len());
    for entry in plan.entries.into_iter() {
        if entry.kind == PlanKind::File {
            let user_path = user_path_from_zip_entry(&entry);
            let mut reader = view.read(&user_path).await.map_err(WalkError::View)?;
            let mut buf = Vec::with_capacity(entry.size as usize);
            reader.read_to_end(&mut buf).await?;
            bodies.push((entry, buf));
        } else {
            bodies.push((entry, Vec::new()));
        }
    }

    let summary = tokio::task::spawn_blocking(move || -> Result<ZipSummary, ZipError> {
        let bridge = SyncIoBridge::new(sink);
        // `SyncIoBridge<AsyncWrite>` doesn't `Seek`, so use the streaming
        // constructor — `ZipWriter::new_stream` wraps the writer in a
        // `StreamWriter` that errors on actual back-seek attempts but
        // satisfies the `Seek` bound the rest of the API needs.
        let mut zw = ZipWriter::new_stream(bridge);
        let mut bytes_total: u64 = 0;
        let mut count: u64 = 0;
        for (entry, body) in bodies {
            let method = match entry.kind {
                PlanKind::Dir => CompressionMethod::Stored,
                PlanKind::File => compression_for_mime(&entry.mime),
            };
            // `FileOptions::<ExtendedFileOptions>::default()` in zip 5 sets
            // bit 11 (UTF-8) on the general-purpose flags. `large_file(false)`
            // skips Zip64 (caps keep us under 4 GiB).
            let mut options: FileOptions<'_, ExtendedFileOptions> = FileOptions::default()
                .compression_method(method)
                .large_file(false)
                .unix_permissions(0o644);
            if let Some(dt) = zip_datetime_from_system_time(entry.mtime) {
                options = options.last_modified_time(dt);
            }
            match entry.kind {
                PlanKind::Dir => {
                    zw.add_directory(entry.zip_name, options)?;
                }
                PlanKind::File => {
                    zw.start_file(entry.zip_name, options)?;
                    use std::io::Write as _;
                    zw.write_all(&body)?;
                    bytes_total = bytes_total.saturating_add(body.len() as u64);
                }
            }
            count += 1;
        }
        zw.finish()?;
        Ok(ZipSummary {
            entries: count,
            bytes: bytes_total,
        })
    })
    .await
    .map_err(|e| {
        ZipError::Io(std::io::Error::other(format!(
            "zip writer task panicked: {e}"
        )))
    })??;
    Ok(summary)
}

/// `zip_name` is rooted at the walk root's basename (e.g. `Photos/...`).
/// The handler always sends a request whose user-facing path is
/// `/<basename>`, so re-prepending `/` reconstructs a valid `UserPath`.
fn user_path_from_zip_entry(entry: &PlannedEntry) -> UserPath {
    let candidate = format!("/{}", entry.zip_name);
    UserPath::new(candidate).expect("zip_name was derived from valid path segments")
}

/// Best-effort conversion of `SystemTime` to a `zip::DateTime`. Returns
/// `None` for times that pre-date 1980 (the MS-DOS epoch) or post-date
/// 2107 — `zip::DateTime::from_date_and_time` rejects those. Callers fall
/// back to the `FileOptions` default (current time) in that case.
fn zip_datetime_from_system_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    let dur = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    let secs = i64::try_from(dur.as_secs()).ok()?;
    let dt: DateTime<Utc> = DateTime::<Utc>::from_timestamp(secs, 0)?;
    let year = u16::try_from(dt.year()).ok()?;
    let month = u8::try_from(dt.month()).ok()?;
    let day = u8::try_from(dt.day()).ok()?;
    let hour = u8::try_from(dt.hour()).ok()?;
    let minute = u8::try_from(dt.minute()).ok()?;
    let second = u8::try_from(dt.second()).ok()?;
    zip::DateTime::from_date_and_time(year, month, day, hour, minute, second).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ZipCaps;
    use std::io::Cursor;

    #[tokio::test]
    async fn stream_produces_valid_zip() {
        let (view, _dir) = crate::walk::tests::seed_view().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        let handle = tokio::spawn(async move {
            stream_folder(
                &view,
                &UserPath::new("/Photos").unwrap(),
                ZipCaps {
                    max_entries: 100,
                    max_bytes: 1024 * 1024,
                },
                writer,
            )
            .await
        });
        let mut combined: Vec<u8> = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        let summary = handle.await.unwrap().unwrap();
        assert!(summary.entries >= 3);

        let mut archive = zip::ZipArchive::new(Cursor::new(combined)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "Photos/cat.jpg"));
        assert!(names.iter().any(|n| n == "Photos/dog.jpg"));
        assert!(names.iter().any(|n| n == "Photos/vacation/beach.jpg"));
    }

    #[tokio::test]
    async fn stream_returns_too_large_without_writing() {
        let (view, _dir) = crate::walk::tests::seed_view().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        let r = stream_folder(
            &view,
            &UserPath::new("/Photos").unwrap(),
            ZipCaps {
                max_entries: 1,
                max_bytes: 1024,
            },
            writer,
        )
        .await;
        assert!(matches!(r, Err(ZipError::Walk(WalkError::TooLarge { .. }))));
        // No bytes should have been emitted — channel closes on drop with
        // nothing in flight.
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        assert!(
            combined.is_empty(),
            "expected zero bytes, got {} bytes",
            combined.len()
        );
    }

    #[tokio::test]
    async fn stream_preserves_unicode_names() {
        use crabcloud_storage::{NoopEventSink, StoragePath};
        let (view, _dir) = crate::walk::tests::seed_view().await;
        // Seed a non-ASCII file into the same view's underlying storage.
        let storage = view.mounts()[0].storage.clone();
        storage
            .put_file(
                &StoragePath::new("Photos/Vacaci\u{00f3}nes.txt").unwrap(),
                Box::pin(Cursor::new(b"ole".to_vec())),
                &NoopEventSink,
            )
            .await
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(64);
        let writer = crate::mpsc_writer::MpscBytesWriter::new(tx);
        tokio::spawn(async move {
            stream_folder(
                &view,
                &UserPath::new("/Photos").unwrap(),
                ZipCaps {
                    max_entries: 100,
                    max_bytes: 1024 * 1024,
                },
                writer,
            )
            .await
        });
        let mut combined = Vec::new();
        while let Some(item) = rx.recv().await {
            combined.extend_from_slice(&item.unwrap());
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(combined)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(
            names.iter().any(|n| n == "Photos/Vacaci\u{00f3}nes.txt"),
            "non-ASCII name lost; got {names:?}",
        );
    }
}
