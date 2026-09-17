//! A small seeded generator, so generated corpora are the same bytes on every
//! machine. SplitMix64: not for anything but test data.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `low..=high`.
    pub fn range(&mut self, low: usize, high: usize) -> usize {
        low + (self.next() % (high - low + 1) as u64) as usize
    }

    /// True with probability `percent` in a hundred.
    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn fill(&mut self, out: &mut Vec<u8>, len: usize) {
        let end = out.len() + len;
        while out.len() < end {
            let bytes = self.next().to_le_bytes();
            let take = bytes.len().min(end - out.len());
            out.extend_from_slice(&bytes[..take]);
        }
    }
}
