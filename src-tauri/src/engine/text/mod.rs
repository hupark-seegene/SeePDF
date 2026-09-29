//! Text layer, per-page text cache and search (`ARCHITECTURE.md` §5).

pub mod layer;
pub mod search;
pub mod serialize;
// v0.3 pkg6: reading order from the structure tree (V6) and plain-text web links (V5)
pub mod structtree;
pub mod weblinks;

pub use layer::{TextCache, TextLayer};
