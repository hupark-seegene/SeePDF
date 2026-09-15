//! Render pipeline: geometry, the one render configuration, the encode pool and the
//! encoded-PNG LRU (`ARCHITECTURE.md` §3).

pub mod cache;
pub mod encode;
pub mod geometry;
pub mod tiles;

pub use cache::{EncodedImage, Night, RenderKind, TileCache, TileKey};
pub use encode::{EncodePool, RawImage};
pub use tiles::{render, RenderRequest};
