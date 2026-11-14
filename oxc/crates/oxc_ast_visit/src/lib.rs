#![forbid(unsafe_code)]
mod generated {
    pub mod visit;
    pub mod visit_mut;
}

pub use generated::{visit::*, visit_mut::*};
