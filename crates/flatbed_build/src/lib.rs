//! Build-time code generation for FlatBuffers with plain Rust struct API
//!
//! This crate provides build-time generation of:
//! - FlatBuffer Rust code via flatc (hidden internally)
//! - Plain Rust structs as the primary API with serde support
//! - Automatic serialization/deserialization to FlatBuffer binary format
//!
//! # Example
//!
//! ```rust,ignore
//! // In build.rs — multiple schemas can be chained
//! fn main() {
//!     flatbed_build::Config::new()
//!         .schema("schemas/ping.fbs")
//!         .schema("schemas/user.fbs")
//!         .compile()
//!         .expect("schema compilation failed");
//! }
//! ```
//!
//! With versioned schemas using includes:
//! ```fbs
//! // schemas/agent.fbs
//! include "v1/agent.fbs";
//! include "v2/agent.fbs";
//! ```
//!
//! Access types as plain structs:
//! - `generated::v_1::PingRequest` (plain struct - primary API)
//! - `generated::v_1::PingResponse` (plain struct - primary API)
//!
//! # Supported field types
//!
//! Each FlatBuffer table field maps to a Rust field on the generated struct.
//! All checks below are **build-time codegen** errors — runtime decode failures
//! continue to be detected by the `flatbuffers` crate's verifier.
//!
//! | FlatBuffer | Rust |
//! |---|---|
//! | `bool`, `byte`/`int8`, `ubyte`/`uint8`, `short`/`int16`, `ushort`/`uint16`, `int`/`int32`, `uint`/`uint32`, `long`/`int64`, `ulong`/`uint64`, `float`/`float32`, `double`/`float64` | the matching Rust scalar (`bool`, `i8`, `u8`, …) |
//! | `string` | `Option<String>` |
//! | `[T]` where `T` is a scalar | `Option<Vec<T>>` |
//! | `[string]` | `Option<Vec<String>>` |
//! | `Table` (single nested table) | `Option<Table>` |
//! | `[Table]` (vector of tables) | `Option<Vec<Table>>` |
//! | `Enum` (named enum) | bare `Enum` (no `Option`) |
//! | `[Enum]` (vector of enum values) | `Option<Vec<Enum>>` |
//!
//! ```fbs
//! enum Severity : byte { Info, Warning, Error }
//!
//! table Address {
//!   street: string;
//!   city: string;
//! }
//!
//! table AddressBook {
//!   owner: string;            // Option<String>
//!   primary: Address;         // Option<Address>
//!   contacts: [Address];      // Option<Vec<Address>>
//!   tags: [string];           // Option<Vec<String>>
//!   ports: [uint16];          // Option<Vec<u16>>
//!   default_severity: Severity; // bare Severity (defaults to first variant)
//!   history: [Severity];      // Option<Vec<Severity>>
//! }
//! ```
//!
//! Reference fields (strings, vectors, tables) are always `Option<…>` because
//! FlatBuffers treats them as optional at the wire level. Scalars (including
//! enums, which are wire-level fixed-width integers) are non-optional and
//! round-trip through their default value when absent.
//!
//! # How the codegen sees the schema
//!
//! The high-level codegen is fed by flatc's canonical reflection output. For
//! every input `.fbs`, flatbed_build runs flatc twice: once with `--rust` to
//! emit `<stem>_generated.rs` (the FlatBuffer wire format), and once with
//! `-b --schema` to emit `<stem>.bfbs` (a binary FlatBuffer conforming to
//! `reflection.fbs`). The `.bfbs` is then decoded with the
//! `flatbuffers-reflection` crate and walked to drive `_flatbed.rs` emission.
//! Anything flatc accepts, the codegen accepts; anything flatc rejects
//! never reaches this crate.
//!
//! # Module layout
//!
//! - `compile` — build orchestration (`Config`, the two flatc invocations,
//!   per-include Rust concatenation). Public surface.
//! - `reflection` — bfbs reader + `Table`/`Field`/`Enum` shadow types
//!   produced from the reflection graph.
//! - `source_order` — per-file `table`/`enum` declaration-position helper
//!   used to keep generated output in source order.
//! - `fbs_types` — adapter from the small `fbs_type` string language to
//!   Rust + OpenAPI shapes.
//! - `codegen` — emit `<stem>_flatbed.rs` from the bucketed shadow types.
//!
//! Not yet supported:
//! - FlatBuffer unions and `struct` (fixed-layout) types — use a `table`
//!   with the relevant fields instead.
//! - Cross-namespace field references in the high-level codegen. flatc
//!   parses `field: other_ns.SomeType` cleanly and the reflection-driven
//!   adapter buckets the referenced type under its own namespace, but the
//!   codegen's per-namespace table/enum lookup resolves field types
//!   against the *referring* namespace only. A cross-namespace field
//!   surfaces as the bare suffix and only compiles when a same-named
//!   type also exists locally; full `super::other_ns::Type` resolution
//!   is not yet implemented.

mod codegen;
mod compile;
mod fbs_types;
mod reflection;
mod source_order;

pub use compile::Config;
