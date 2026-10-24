use crate::simulator::rng::SplitMix64;
use crate::simulator::simulator::Event;
use hotstuff_consensus::types::clock::Clock;
use std::collections::{BTreeMap, VecDeque};

pub enum TieBreak {
    Fifo,
    Shuffle(SplitMix64),
}

pub struct Timeline<CL: Clock> {
    queue: BTreeMap<CL, VecDeque<Event>>,
    current_time: CL,
    tie_break: TieBreak,
}

impl<CL: Clock> Timeline<CL> {
    pub fn new(current_time: CL, tie_break: TieBreak) -> Self {
        Self {
            queue: BTreeMap::new(),
            current_time,
            tie_break,
        }
    }

    pub fn now(&self) -> CL {
        self.current_time
    }

    pub fn add_event(&mut self, event: Event, fired_at: CL) {
        self.queue.entry(fired_at).or_default().push_back(event);
    }

    pub fn next_time(&self) -> Option<CL> {
        self.queue.first_key_value().map(|(at, _)| *at)
    }

    pub fn pending(&self, at: CL) -> usize {
        self.queue.get(&at).map_or(0, VecDeque::len)
    }

    pub fn advance_to(&mut self, time: CL) {
        assert!(time >= self.current_time, "time moved backwards");
        self.current_time = time;
    }

    pub fn choose(&mut self, candidates: usize) -> usize {
        match &mut self.tie_break {
            TieBreak::Shuffle(rng) if candidates > 1 => {
                rng.range(0, candidates as u64 - 1) as usize
            }
            _ => 0,
        }
    }

    pub fn take(&mut self, at: CL, index: usize) -> Option<Event> {
        let bucket = self.queue.get_mut(&at)?;
        let event = bucket.remove(index);
        if bucket.is_empty() {
            self.queue.remove(&at);
        }
        event
    }
}
