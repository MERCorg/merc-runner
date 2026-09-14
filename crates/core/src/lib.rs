#![doc = include_str!("../README.md")]

pub mod error;
pub mod resolve;
pub mod spec;

pub use error::JobSpecError;
pub use resolve::ResolvedPlan;
pub use resolve::ResolvedStep;
pub use resolve::resolve;
pub use spec::JobSpec;
pub use spec::Tool;
