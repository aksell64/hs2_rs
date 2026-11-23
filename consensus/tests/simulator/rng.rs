use std::cell::RefCell;
use std::rc::Rc;

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

#[derive(Clone)]
pub struct Randomness {
    seed: u64,
    inner: Rc<RefCell<SplitMix64>>,
}

impl Randomness {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            inner: Rc::new(RefCell::new(SplitMix64::new(seed))),
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn range(&self, lo: u64, hi: u64) -> u64 {
        self.inner.borrow_mut().range(lo, hi)
    }

    pub fn chance(&self, percent: u64) -> bool {
        self.inner.borrow_mut().chance(percent)
    }

    pub fn rand(&self) -> u64 {
        self.inner.borrow_mut().next_u64()
    }

    /// Selects a random number of distinct values from `0..n`, at most `f`.
    pub fn sample_range(&self, n: u32, f: u32) -> Vec<u32> {
        let count = self.range(0, u64::from(f.min(n))) as u32;
        self.sample_exact(n, count)
    }

    /// Selects exactly `f` distinct values from `0..n`. Panics if `f > n`.
    pub fn sample_exact(&self, n: u32, f: u32) -> Vec<u32> {
        assert!(f <= n, "cannot sample {f} distinct values from 0..{n}");
        let count = f as usize;
        let mut nodes: Vec<_> = (0..n).collect();
        for i in 0..count {
            let j = self.range(i as u64, u64::from(n) - 1) as usize;
            nodes.swap(i, j);
        }
        nodes.truncate(count);
        nodes
    }
}
