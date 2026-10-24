pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        assert!(lo <= hi, "empty range {lo}..={hi}");
        match (hi - lo).checked_add(1) {
            Some(width) => lo + self.next_u64() % width,
            None => self.next_u64(),
        }
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.range(0, 99) < percent
    }
}
