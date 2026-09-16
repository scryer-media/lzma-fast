//! LZMA and LZMA2 decoding, ported from the 7-Zip reference decoder.
//!
//! The decoder in this crate is a port of Igor Pavlov's `LzmaDec.c` and
//! `Lzma2Dec.c` from the LZMA SDK (public domain), kept structurally faithful
//! to the original so that its speed carries over: one large decode loop with
//! the range coder, probability base and dictionary pointers held in locals,
//! limits checked once per symbol under a caller-guaranteed margin, and a
//! separate careful path for the bytes near buffer edges.
//!
//! Decode only. There is no encoder here and none is planned.
//!
//! # Status
//!
//! Scaffold. The public API is not yet defined; see `docs/porting.md` in the
//! repository for the port plan and its acceptance gate.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs, rust_2018_idioms)]

/// Crate version, for consumers that record which decoder produced an output.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
