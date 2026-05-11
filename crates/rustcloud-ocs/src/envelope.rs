// Implemented in Task 9.

use crate::format::Format;
use crate::status::OcsVersion;

#[derive(Debug)]
pub struct OcsResponse<T> {
    pub status: crate::status::OcsStatus,
    pub message: String,
    pub data: T,
    pub version: OcsVersion,
}

pub fn render<T>(_resp: &OcsResponse<T>, _format: Format) -> (String, &'static str) {
    todo!("implemented in Task 9")
}
