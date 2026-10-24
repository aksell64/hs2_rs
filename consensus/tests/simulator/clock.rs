use hotstuff_consensus::types::clock::Clock;
use hotstuff_types::node::NodeID;
use std::collections::{BTreeMap};
use crate::simulator::simulator::Event;

pub struct Timeline<CL: Clock> {
    queue: BTreeMap<(CL, u64), Event>,
    current_time: CL,
    seq: u64,
}

impl<CL: Clock> Timeline<CL> {
    pub fn new(current_time: CL) -> Self {
        Self {
            queue: BTreeMap::new(),
            current_time,
            seq: 0,
        }
    }

    pub fn now(&self) -> CL {
        self.current_time
    }

    pub fn add_event(&mut self, event: Event, fired_at: CL) {
        let new_seq = self.seq + 1;
        self.queue.insert((fired_at, new_seq), event);
        self.seq = new_seq;
    }

    pub fn next_time(&self) -> Option<CL> {
        self.queue.first_key_value().map(|((at, _), _)| *at)
    }

    pub fn advance_to(&mut self, time: CL) {
        assert!(time >= self.current_time, "time moved backwards");
        self.current_time = time;
    }

    pub fn fire_next_event(&mut self) -> Option<Event> {
        let (key, event) = self.queue.pop_first()?;
        self.advance_to(key.0);
        Some(event)
    }
}
