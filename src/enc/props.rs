//! Encoder properties.
//!
//! C: `CLzmaEncProps`, `LzmaEncProps_Init`, `LzmaEncProps_Normalize` and
//! `LzmaEnc_WriteProperties` in `C/LzmaEnc.c` / `C/LzmaEnc.h`.
//!
//! The C's `-1` "not set" sentinels are kept, because `LzmaEncProps_Normalize`
//! is written in terms of them and the defaults it derives are part of what a
//! parity test compares. Callers never write `-1`: [`LzmaEncProps::new`] sets
//! every field to it and the setters take real values.
//!
//! One deliberate pin. `LzmaEncProps_Normalize`'s default dictionary size and
//! `kNumLogBits` are written in terms of `sizeof(size_t)`, so the C encodes a
//! file differently on a 32-bit host. This port always uses the 64-bit
//! numbers, so that one input and one setting give one output everywhere.

use crate::enc::consts::*;
use crate::enc::lz_find::MatchFinderKind;
use crate::error::Error;

/// C: `CLzmaEncProps`. The multi-threading fields (`numThreads`, `affinity*`)
/// are not carried: this encoder is single-threaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LzmaEncProps {
    pub(crate) level: i32,
    pub(crate) dict_size: u32,
    pub(crate) lc: i32,
    pub(crate) lp: i32,
    pub(crate) pb: i32,
    pub(crate) algo: i32,
    pub(crate) fb: i32,
    pub(crate) bt_mode: i32,
    pub(crate) num_hash_bytes: i32,
    pub(crate) num_hash_out_bits: u32,
    pub(crate) mc: u32,
    pub(crate) write_end_mark: bool,
    pub(crate) reduce_size: u64,
}

impl Default for LzmaEncProps {
    fn default() -> Self {
        Self::new()
    }
}

impl LzmaEncProps {
    /// C: `LzmaEncProps_Init`.
    #[must_use]
    pub const fn new() -> Self {
        LzmaEncProps {
            level: 5,
            dict_size: 0,
            lc: -1,
            lp: -1,
            pb: -1,
            algo: -1,
            fb: -1,
            bt_mode: -1,
            num_hash_bytes: -1,
            num_hash_out_bits: 0,
            mc: 0,
            write_end_mark: false,
            reduce_size: u64::MAX,
        }
    }

    /// Compression level, 0 to 9. C: `props.level`.
    #[must_use]
    pub const fn with_level(mut self, level: u32) -> Self {
        self.level = level as i32;
        self
    }

    /// Dictionary size in bytes. C: `props.dictSize`.
    #[must_use]
    pub const fn with_dict_size(mut self, dict_size: u32) -> Self {
        self.dict_size = dict_size;
        self
    }

    /// Literal context, literal position and position bits.
    /// C: `props.lc`, `props.lp`, `props.pb`.
    #[must_use]
    pub const fn with_lclppb(mut self, lc: u8, lp: u8, pb: u8) -> Self {
        self.lc = lc as i32;
        self.lp = lp as i32;
        self.pb = pb as i32;
        self
    }

    /// Number of fast bytes, 5 to 273. C: `props.fb`.
    #[must_use]
    pub const fn with_fast_bytes(mut self, fb: u32) -> Self {
        self.fb = fb as i32;
        self
    }

    /// Match finder cycles. C: `props.mc`, the match finder's `cutValue`.
    #[must_use]
    pub const fn with_match_cycles(mut self, mc: u32) -> Self {
        self.mc = mc;
        self
    }

    /// Which match finder to use. C: `props.btMode` and `props.numHashBytes`.
    #[must_use]
    pub const fn with_match_finder(mut self, kind: MatchFinderKind) -> Self {
        self.bt_mode = kind.bt_mode() as i32;
        self.num_hash_bytes = kind.num_hash_bytes() as i32;
        self
    }

    /// The optimal parser (`false`) or the fast one (`true`).
    /// C: `props.algo == 0`.
    #[must_use]
    pub const fn with_fast_mode(mut self, fast: bool) -> Self {
        self.algo = !fast as i32;
        self
    }

    /// Whether to write the end-of-payload marker.
    /// C: `props.writeEndMark`.
    #[must_use]
    pub const fn with_end_mark(mut self, write_end_mark: bool) -> Self {
        self.write_end_mark = write_end_mark;
        self
    }

    /// The size of the data about to be encoded, when it is known.
    ///
    /// C: `props.reduceSize`. It shrinks the dictionary to fit and, through
    /// `LzmaEnc_SetDataSize`, sizes the match finder's hash table — so it
    /// changes the bytes the encoder produces, not only what it allocates.
    #[must_use]
    pub const fn with_reduce_size(mut self, reduce_size: u64) -> Self {
        self.reduce_size = reduce_size;
        self
    }

    /// C: `LzmaEncProps_Normalize`.
    pub(crate) fn normalize(&mut self) {
        let mut level = self.level;
        if level < 0 {
            level = 5;
        }
        self.level = level;

        if self.dict_size == 0 {
            // C: the `sizeof(size_t)` terms, pinned to the 64-bit host.
            self.dict_size = if level as u32 <= 4 {
                1u32 << (level * 2 + 16)
            } else if level as u32 <= 8 {
                1u32 << (level + 20)
            } else {
                1u32 << 28
            };
        }

        if u64::from(self.dict_size) > self.reduce_size {
            let mut v = self.reduce_size as u32;
            let k_reduce_min = 1u32 << 12;
            if v < k_reduce_min {
                v = k_reduce_min;
            }
            if self.dict_size > v {
                self.dict_size = v;
            }
        }

        if self.lc < 0 {
            self.lc = 3;
        }
        if self.lp < 0 {
            self.lp = 0;
        }
        if self.pb < 0 {
            self.pb = 2;
        }
        if self.algo < 0 {
            self.algo = i32::from(level >= 5);
        }
        if self.fb < 0 {
            self.fb = if level < 7 { 32 } else { 64 };
        }
        if self.bt_mode < 0 {
            self.bt_mode = i32::from(self.algo != 0);
        }
        if self.num_hash_bytes < 0 {
            self.num_hash_bytes = if self.bt_mode != 0 { 4 } else { 5 };
        }
        if self.mc == 0 {
            self.mc = (16 + (self.fb as u32 >> 1)) >> u32::from(self.bt_mode == 0);
        }
    }

    /// The dictionary size this setting ends up with.
    ///
    /// C: `LzmaEncProps_GetDictSize`.
    #[must_use]
    pub fn dict_size(&self) -> u32 {
        let mut props = *self;
        props.normalize();
        props.dict_size
    }
}

/// C: `LzmaEnc_WriteProperties`.
pub(crate) fn write_properties(lc: u32, lp: u32, pb: u32, dict_size: u32) -> [u8; LZMA_PROPS_SIZE] {
    let mut props = [0u8; LZMA_PROPS_SIZE];
    props[0] = ((pb * 5 + lp) * 9 + lc) as u8;

    // C: "we write aligned dictionary value to properties for lzma decoder".
    let v = if dict_size >= (1 << 21) {
        let k_dict_mask = (1u32 << 20) - 1;
        let v = dict_size.wrapping_add(k_dict_mask) & !k_dict_mask;
        if v < dict_size { dict_size } else { v }
    } else {
        let mut i = 11 * 2u32;
        let mut v;
        loop {
            v = (2 + (i & 1)) << (i >> 1);
            i += 1;
            if v >= dict_size {
                break;
            }
        }
        v
    };
    props[1..5].copy_from_slice(&v.to_le_bytes());
    props
}

/// C: the parameter checks at the head of `LzmaEnc_SetProps`.
pub(crate) fn check(props: &LzmaEncProps) -> Result<(), Error> {
    if props.lc as u32 > LZMA_LC_MAX
        || props.lp as u32 > LZMA_LP_MAX
        || props.pb as u32 > LZMA_PB_MAX
    {
        return Err(Error::Param);
    }
    if u64::from(props.dict_size) > (1u64 << K_DIC_LOG_SIZE_MAX_COMPRESS) {
        return Err(Error::Param);
    }
    Ok(())
}
