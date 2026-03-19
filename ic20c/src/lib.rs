//! IC20 compiler library.
//!
//! Provides the full compilation pipeline from IC20 source text to IC10 assembly:
//! lexing, parsing, name resolution, CFG construction, SSA conversion,
//! optimization, register allocation, and code generation.

pub mod bind;
pub mod cfg;
pub mod codegen;
pub mod crc32;
pub mod diagnostic;
pub mod ir;
pub mod lexer;
pub mod opt;
pub mod parser;
pub mod regalloc;
pub mod ssa;
