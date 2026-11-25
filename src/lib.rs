pub mod paths;
pub mod redact;
pub mod adapters;
pub mod collector;
pub mod model;
pub mod query;
pub mod spool;
pub mod storage;
#[cfg(feature = "desktop")]
pub mod viewer;
#[cfg(windows)]
pub mod service;

pub const FILE_LIMIT: usize = 50_000_000;
pub const FRAME_LIMIT: usize = 256 * 1024;
