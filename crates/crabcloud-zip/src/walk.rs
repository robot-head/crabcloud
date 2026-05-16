//! Placeholder — replaced in Task A3.

use crate::error::WalkError;
use crate::types::{ZipCaps, ZipPlan};
use crabcloud_fs::path::UserPath;
use crabcloud_fs::View;

pub async fn walk_for_caps(
    _view: &View,
    _root: &UserPath,
    _caps: &ZipCaps,
) -> Result<ZipPlan, WalkError> {
    Ok(ZipPlan {
        entries: Vec::new(),
        total_bytes: 0,
    })
}
