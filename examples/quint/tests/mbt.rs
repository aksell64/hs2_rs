use quint_connect::*;
use serde::Deserialize;
use std::collections::BTreeMap;

use quint_counter::Counters;

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct SpecState {
    counters: BTreeMap<String, i64>,
}

impl State<Driver_> for SpecState {
    fn from_driver(driver: &Driver_) -> Result<Self> {
        Ok(SpecState {
            counters: driver.bank.snapshot(),
        })
    }
}

#[derive(Default)]
struct Driver_ {
    bank: Counters,
}

impl Driver for Driver_ {
    type State = SpecState;

    fn step(&mut self, step: &Step) -> Result {
        switch!(step {
            init => {
                self.bank = Counters::new(["p1", "p2", "p3"]);
            },
            increment(pid: String, delta: i64) => {
                self.bank.increment(&pid, delta)?;
            },
            decrement(pid: String, delta: i64) => {
                self.bank.decrement(&pid, delta)?;
            },
        })
    }
}

#[quint_run(spec = "spec/counter.qnt", max_samples = 20, max_steps = 10)]
fn random_traces() -> impl Driver {
    Driver_::default()
}
