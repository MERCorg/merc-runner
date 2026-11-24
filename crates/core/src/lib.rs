#![doc = include_str!("../README.md")]

pub mod composition;
pub mod error;
pub mod resolve;
pub mod schedule;
pub mod spec;

pub use composition::Composition;
pub use composition::CompositionError;
pub use error::JobSpecError;
pub use resolve::ResolvedPlan;
pub use resolve::ResolvedStep;
pub use resolve::resolve;
pub use schedule::Launch;
pub use schedule::Scheduler;
pub use schedule::ThreadDemand;
pub use spec::JobSpec;
pub use spec::Tool;
pub use spec::Toolset;
