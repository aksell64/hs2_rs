use crate::types::clock::Clock;

impl Clock for u64 {
    type Duration = u64;

    fn add(self, d: u64) -> Self {
        self + d
    }

    fn scale(d: u64, times: u32) -> u64 {
        d * times as u64
    }
}
