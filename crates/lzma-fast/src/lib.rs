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
//! # Example
//!
//! ```no_run
//! use std::fs::File;
//! use std::io::Read;
//! use lzma_fast::LzmaReader;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut out = Vec::new();
//! LzmaReader::new(File::open("archive.lzma")?)?.read_to_end(&mut out)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Provenance
//!
//! Derived from `C/LzmaDec.c` and `C/Lzma2Dec.c` of the LZMA SDK, which are in
//! the public domain. Each ported function names its C counterpart in a
//! comment.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs, rust_2018_idioms)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

mod error;
mod lzma;
mod lzma2;
mod lzma_alone;

#[cfg(feature = "std")]
mod reader;

pub use error::{Error, FinishMode, Progress, Status};
pub use lzma::consts::{LZMA_PROPS_SIZE, LZMA_REQUIRED_INPUT_MAX};
pub use lzma::{LzmaDecoder, LzmaProps};
pub use lzma_alone::{LZMA_ALONE_HEADER_SIZE, LzmaAloneHeader};
pub use lzma2::Lzma2Decoder;

#[cfg(feature = "std")]
pub use reader::{Lzma2Reader, LzmaReader};

#[cfg(feature = "crc")]
pub mod crc;
#[cfg(any(feature = "crypto", feature = "aws-lc"))]
pub mod crypto;

/// Whether this build decodes with the assembly loop ported from the LZMA
/// SDK's `Asm/` tree rather than with the portable Rust port of the C loop.
/// False without the `asm` feature and on every target that has no such loop.
pub const ASM_LOOP: bool = lzma::decode_opt::ENABLED;

/// Crate version, for consumers that record which decoder produced an output.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
