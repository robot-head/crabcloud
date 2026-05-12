//! Local filesystem backend. Implementation lands in Batches D + E.
//!
//! The const-fn placeholders below keep `unused_crate_dependencies` quiet
//! for deps that this batch declares but doesn't yet exercise. They'll be
//! replaced by real call sites in batches D + E.

#[allow(dead_code)]
#[doc(hidden)]
pub(crate) mod _deps_anchor {
    // Force the compiler to "see" each declared workspace dep at least once
    // so `-D warnings`/`unused_crate_dependencies` stays quiet until the
    // real LocalStorage impl lands in batches D + E.
    pub(crate) const _BYTES_LEN: usize = std::mem::size_of::<bytes::Bytes>();
    pub(crate) const _INFER_LEN: usize = std::mem::size_of::<infer::Infer>();
    pub(crate) const _PHF_LEN: usize = std::mem::size_of::<phf::Map<&'static str, &'static str>>();

    pub(crate) fn _trace_anchor() {
        tracing::trace!("local backend stub loaded");
    }
}
