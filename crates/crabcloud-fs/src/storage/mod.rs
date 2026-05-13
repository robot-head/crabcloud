//! `crabcloud-fs::storage` — `Storage`-trait adapters that compose with the
//! backend storages from `crabcloud-storage`. SP7 introduces `share_subroot`
//! which subroots an owner's home storage to a recipient-facing view + filters
//! mutating ops through a `SharePermissions` mask.

pub mod share_subroot;

pub use share_subroot::SharedSubrootStorage;
