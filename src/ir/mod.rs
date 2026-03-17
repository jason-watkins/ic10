//! Intermediate representations for each stage of the IC20 compilation pipeline.
//!
//! Each submodule defines a distinct IR type hierarchy consumed by one pipeline phase
//! and produced by the preceding one. Shared types (operators, intrinsics, device pins)
//! live in [`shared`] and are re-exported at this level.

/// Unresolved, untyped AST produced by the parser.
pub mod ast;
/// Name-resolved, type-checked IR produced by the binder.
pub mod bound;
/// Three-address CFG IR produced by the CFG builder, consumed by SSA and optimization passes.
pub mod cfg;
mod shared;

pub use shared::*;
