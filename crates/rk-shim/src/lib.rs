//! The tracing compiler shim of the rucc-kernel plan, section 13.4.
//!
//! kbuild calls `$(CC)` tens of thousands of times in an `allmodconfig` build: once per unit, once
//! per `.S` file, and several hundred times more while Kconfig and the Makefiles ask the compiler
//! what it can do. `rk build` points all of those at `rk-cc`, which runs the real compiler with the
//! arguments unchanged and appends one line of JSON to `compile.jsonl` for every call.
//!
//! The shim adds nothing that changes code. The one argument it may add is `-frucc-trace=<file>`,
//! and only when the real compiler is rucc and says it understands the option. The one thing it
//! may change is which compiler runs, and only under `RK_BRINGUP`, which marks the call as
//! delegated so that the run can never be graded.
//!
//! This library holds the parts a test can reach: the record, the reading of a command line, the
//! configuration, and hashing. The binary in `main.rs` is the glue that runs the compiler.

pub mod args;
pub mod bringup;
pub mod config;
pub mod digest;
pub mod record;
pub mod usage;
