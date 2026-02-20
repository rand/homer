//! Homer core library — pipeline, extractors, analyzers, renderers, and store.
//!
//! The main entry point is [`pipeline::HomerPipeline`], which runs the
//! Extract → Analyze → Render pipeline over a [`store::HomerStore`].

pub mod analyze;
pub mod config;
pub mod error;
pub mod extract;
pub mod llm;
pub mod pipeline;
pub mod progress;
pub mod query;
pub mod render;
pub mod store;
pub mod types;
