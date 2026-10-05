//! A bank of named counters -- the implementation under test.
//!
//! Nothing here knows about Quint. The specification lives in `spec/counter.qnt`
//! and the harness that connects the two lives in `tests/mbt.rs`, so this type
//! stays an ordinary piece of code that could just as well be hand-tested.

use std::collections::BTreeMap;

/// Counters keyed by process id, created up front and never added to or removed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counters {
    values: BTreeMap<String, i64>,
}

/// Raised when an operation names a process the bank was not created with.
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownProcess(pub String);

impl std::fmt::Display for UnknownProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown process: {}", self.0)
    }
}

impl std::error::Error for UnknownProcess {}

impl Counters {
    pub fn new<I, S>(processes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            values: processes.into_iter().map(|p| (p.into(), 0)).collect(),
        }
    }

    pub fn increment(&mut self, pid: &str, delta: i64) -> Result<(), UnknownProcess> {
        self.apply(pid, delta)
    }

    pub fn decrement(&mut self, pid: &str, delta: i64) -> Result<(), UnknownProcess> {
        self.apply(pid, -delta)
    }

    pub fn get(&self, pid: &str) -> Option<i64> {
        self.values.get(pid).copied()
    }

    pub fn snapshot(&self) -> BTreeMap<String, i64> {
        self.values.clone()
    }

    fn apply(&mut self, pid: &str, delta: i64) -> Result<(), UnknownProcess> {
        match self.values.get_mut(pid) {
            Some(value) => {
                *value += delta;
                Ok(())
            }
            None => Err(UnknownProcess(pid.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn increment_and_decrement_are_inverse() {
        let mut bank = Counters::new(["p1"]);
        bank.increment("p1", 3).unwrap();
        bank.decrement("p1", 3).unwrap();
        assert_eq!(bank.get("p1"), Some(0));
    }

    #[test]
    fn counters_are_independent() {
        let mut bank = Counters::new(["p1", "p2"]);
        bank.increment("p1", 2).unwrap();
        assert_eq!(bank.get("p2"), Some(0));
    }

    #[test]
    fn unknown_process_is_rejected() {
        let mut bank = Counters::new(["p1"]);
        assert_eq!(bank.increment("p9", 1), Err(UnknownProcess("p9".into())));
    }
}
