//! The delta filter, decode side.
//!
//! C: `C/Delta.c`, `Delta_Decode`. Spec §5.3.3: the properties byte is the
//! distance minus one, so distances run from 1 to 256, and the filter adds
//! the byte `distance` back to each byte in turn.
//!
//! The C keeps its history in a `DELTA_STATE_SIZE` (256) byte buffer and
//! rotates it; this port keeps the same buffer and the same rotation, because
//! the alternative — a ring with a moving index — changes the inner loop.

use super::error::XzErrorKind;

/// C: `DELTA_STATE_SIZE`.
const DELTA_STATE_SIZE: usize = 256;

/// A delta decoder carrying the last `distance` bytes between calls.
#[derive(Debug, Clone, Copy)]
pub struct Delta {
    distance: usize,
    state: [u8; DELTA_STATE_SIZE],
}

impl Delta {
    /// A decoder for the one-byte filter property, which is the distance
    /// minus one.
    ///
    /// # Errors
    ///
    /// Never, for a one-byte property: every value 0-255 is a valid distance.
    /// The signature returns a result so that the caller's chain builder can
    /// treat every filter the same way.
    pub fn new(props: u8) -> Result<Self, XzErrorKind> {
        Ok(Delta {
            distance: usize::from(props) + 1,
            state: [0u8; DELTA_STATE_SIZE],
        })
    }

    /// The distance in bytes, 1 to 256.
    #[must_use]
    pub fn distance(&self) -> usize {
        self.distance
    }

    /// Decodes `data` in place. Every byte is consumed: the delta filter has
    /// no lookahead and no alignment, so it never leaves a tail.
    pub fn decode(&mut self, data: &mut [u8]) {
        // C: Delta_Decode(state, delta, data, size).
        let size = data.len();
        if size == 0 {
            return;
        }
        let delta = self.distance;

        if size <= delta {
            for (i, b) in data.iter_mut().enumerate() {
                *b = b.wrapping_add(self.state[i]);
            }
            // C: `for (; delta != i; state++, delta--) *state = state[i];`
            // — shift the unconsumed history down past what was just used.
            self.state.copy_within(size..delta, 0);
            let keep = delta - size;
            self.state[keep..delta].copy_from_slice(data);
        } else {
            // C: `for (i = 0; i < delta; i++) buf[i] = (Byte)(buf[i] + state[i]);`
            // - the index is the same one into two different buffers, so an
            // iterator over one of them does not express it.
            #[allow(clippy::needless_range_loop)]
            for i in 0..delta {
                data[i] = data[i].wrapping_add(self.state[i]);
            }
            for i in delta..size {
                data[i] = data[i].wrapping_add(data[i - delta]);
            }
            self.state[..delta].copy_from_slice(&data[size - delta..]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// The spec's own reference encoder (§5.3.3.1), so the test is against the
    /// document rather than against this file.
    fn spec_encode(distance: usize, data: &[u8]) -> Vec<u8> {
        let mut delta = [0u8; 256];
        let mut pos: u8 = 0;
        let mut out = Vec::with_capacity(data.len());
        for &byte in data {
            let tmp = delta[usize::from((distance as u8).wrapping_add(pos))];
            let tmp = byte.wrapping_sub(tmp);
            delta[usize::from(pos)] = byte;
            out.push(tmp);
            pos = pos.wrapping_sub(1);
        }
        out
    }

    #[test]
    fn decodes_what_the_spec_encoder_produced() {
        let data: Vec<u8> = (0u32..5000).map(|i| (i * 37 + 11) as u8).collect();
        for distance in [1usize, 2, 3, 4, 16, 255, 256] {
            let encoded = spec_encode(distance, &data);
            // In one call.
            let mut buf = encoded.clone();
            Delta::new((distance - 1) as u8)
                .expect("props")
                .decode(&mut buf);
            assert_eq!(buf, data, "distance {distance}");

            // And split every which way, which is what exercises the state.
            for chunk in [1usize, 5, 256, 257, 1000] {
                let mut d = Delta::new((distance - 1) as u8).expect("props");
                let mut out = Vec::new();
                for part in encoded.chunks(chunk) {
                    let mut piece = part.to_vec();
                    d.decode(&mut piece);
                    out.extend_from_slice(&piece);
                }
                assert_eq!(out, data, "distance {distance}, chunk {chunk}");
            }
        }
    }
}
