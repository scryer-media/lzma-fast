//! The variable-length integers xz stores sizes with.
//!
//! Spec: `xz-file-format.txt` §1.2. Seven bits per byte, little end first,
//! high bit continues, at most nine bytes, at most 63 bits. A multibyte
//! encoding whose last byte is zero is not the shortest encoding of its value
//! and is rejected, exactly as the spec's reference `decode()` does.
//!
//! C: `Xz.h`'s `Xz_ReadVarInt` / the `READ_VARINT` loops in `XzIn.c` and
//! `XzDec.c`.

use super::error::XzErrorKind;

/// The most bytes a VLI may occupy.
pub const VLI_MAX_BYTES: usize = 9;

/// The largest value a VLI may hold: 63 bits.
pub const VLI_MAX: u64 = u64::MAX / 2;

/// Decodes a VLI from the front of `buf`.
///
/// Returns the value and how many bytes it used.
///
/// # Errors
///
/// [`XzErrorKind::TruncatedVli`] if `buf` ends inside the integer, and
/// [`XzErrorKind::BadVli`] if it is longer than nine bytes, wider than 63
/// bits, or not the shortest encoding of its value.
pub fn decode(buf: &[u8]) -> Result<(u64, usize), XzErrorKind> {
    let Some(&first) = buf.first() else {
        return Err(XzErrorKind::TruncatedVli);
    };
    let mut value = u64::from(first & 0x7F);
    if first & 0x80 == 0 {
        return Ok((value, 1));
    }
    let mut i = 1usize;
    loop {
        if i >= VLI_MAX_BYTES {
            return Err(XzErrorKind::BadVli);
        }
        let Some(&b) = buf.get(i) else {
            return Err(XzErrorKind::TruncatedVli);
        };
        // A zero continuation byte would encode the same value in fewer
        // bytes, so the encoding is not canonical. Spec §1.2's `decode()`
        // rejects it; so does this.
        if b == 0 {
            return Err(XzErrorKind::BadVli);
        }
        value |= u64::from(b & 0x7F) << (i * 7);
        i += 1;
        if b & 0x80 == 0 {
            break;
        }
    }
    if value > VLI_MAX {
        return Err(XzErrorKind::BadVli);
    }
    Ok((value, i))
}

/// Decodes a VLI at `*pos` in `buf` and advances `*pos` past it.
///
/// # Errors
///
/// As [`decode`].
pub fn decode_at(buf: &[u8], pos: &mut usize) -> Result<u64, XzErrorKind> {
    let (v, n) = decode(buf.get(*pos..).unwrap_or(&[]))?;
    *pos += n;
    Ok(v)
}

/// How many bytes `value` occupies when encoded.
#[must_use]
pub fn encoded_len(value: u64) -> usize {
    let mut n = 1usize;
    let mut v = value >> 7;
    while v != 0 {
        n += 1;
        v >>= 7;
    }
    n
}

/// Encodes `value` as a VLI into the front of `buf`.
///
/// Returns how many bytes it used. The encoding is the shortest one, which
/// is the only one [`decode`] accepts.
///
/// # Panics
///
/// If `value` is above [`VLI_MAX`], or `buf` is shorter than
/// [`encoded_len`] of it.
pub fn encode(value: u64, buf: &mut [u8]) -> usize {
    assert!(value <= VLI_MAX, "VLI out of range");
    let mut v = value;
    let mut i = 0usize;
    while v >= 0x80 {
        buf[i] = (v as u8) | 0x80;
        v >>= 7;
        i += 1;
    }
    buf[i] = v as u8;
    i + 1
}

/// Appends `value` to `out` as a VLI.
///
/// # Panics
///
/// If `value` is above [`VLI_MAX`].
pub fn push(value: u64, out: &mut alloc::vec::Vec<u8>) {
    let mut buf = [0u8; VLI_MAX_BYTES];
    let n = encode(value, &mut buf);
    out.extend_from_slice(&buf[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_spec_examples() {
        assert_eq!(decode(&[0x00]), Ok((0, 1)));
        assert_eq!(decode(&[0x7F]), Ok((127, 1)));
        assert_eq!(decode(&[0x80, 0x01]), Ok((128, 2)));
        assert_eq!(decode(&[0xFF, 0x7F]), Ok((16383, 2)));
        // 63 bits, all set: nine bytes, the last without a continuation bit.
        let mut max = [0xFFu8; 9];
        max[8] = 0x7F;
        assert_eq!(decode(&max), Ok((VLI_MAX, 9)));
        // The same nine bytes with the last continuation bit still set would
        // be a tenth byte, which the format does not have.
        assert_eq!(decode(&[0xFF; 9]), Err(XzErrorKind::BadVli));
    }

    #[test]
    fn rejects_non_canonical_and_oversized() {
        // 0x80 0x00 encodes 0 in two bytes.
        assert_eq!(decode(&[0x80, 0x00]), Err(XzErrorKind::BadVli));
        // Ten bytes.
        let mut ten = [0xFFu8; 10];
        ten[9] = 0x01;
        assert_eq!(decode(&ten), Err(XzErrorKind::BadVli));
        // Truncated.
        assert_eq!(decode(&[0x80]), Err(XzErrorKind::TruncatedVli));
        assert_eq!(decode(&[]), Err(XzErrorKind::TruncatedVli));
    }

    #[test]
    fn encoded_len_agrees_with_decode() {
        for v in [0u64, 1, 127, 128, 16383, 16384, 1 << 40, VLI_MAX] {
            let mut buf = [0u8; 9];
            let mut x = v;
            let mut i = 0;
            while x >= 0x80 {
                buf[i] = (x as u8) | 0x80;
                x >>= 7;
                i += 1;
            }
            buf[i] = x as u8;
            i += 1;
            assert_eq!(encoded_len(v), i, "len of {v}");
            assert_eq!(decode(&buf[..i]), Ok((v, i)));
            let mut enc = [0u8; 9];
            assert_eq!(encode(v, &mut enc), i, "encode of {v}");
            assert_eq!(&enc[..i], &buf[..i], "encoding of {v}");
        }
    }
}
