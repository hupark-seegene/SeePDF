//! Text layer, per-page text cache and search (`ARCHITECTURE.md` §5).

pub mod layer;
pub mod search;
pub mod serialize;

pub use layer::{TextCache, TextLayer};
