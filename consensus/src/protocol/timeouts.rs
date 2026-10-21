use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::ops::RangeInclusive;

pub trait TimeoutPolicy<CL: Clock>: Send + 'static {
    fn epoch_deadlines(&self, views: RangeInclusive<View>, start: CL) -> EpochTimeouts<CL>;
    fn lock_deadline(&self, view: View, start: CL) -> ViewTimeout<CL>;
}
