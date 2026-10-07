//! Library surface so examples and tests can reuse the relay's building blocks.
//! The `s380-relay` binary keeps its own module tree in `main.rs`; this exposes
//! the self-contained ISO-DEP layer for tools like `examples/terminal.rs`.

pub mod isodep;
