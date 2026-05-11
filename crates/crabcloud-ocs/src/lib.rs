//! OCS envelope and capabilities aggregator.
//!
//! See `docs/superpowers/specs/2026-05-10-platform-core-design.md` §9.3.

mod capabilities;
mod core_caps;
mod envelope;
mod format;
mod status;

pub use capabilities::{
    aggregate, CapabilitiesPayload, CapabilityContext, CapabilityError, CapabilityProvider,
};
pub use core_caps::CoreCapabilities;
pub use envelope::{render, OcsResponse};
pub use format::{negotiate, Format};
pub use status::{OcsStatus, OcsVersion};
