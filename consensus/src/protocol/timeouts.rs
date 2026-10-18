use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::ops::RangeInclusive;

pub trait TimeoutPolicy: Send + 'static {
    type Clock: Clock;

    fn epoch_deadlines(&self, views: RangeInclusive<View>, start: Self::Clock) -> EpochTimeouts<Self::Clock>;
    fn lock_deadline(&self, view: View, start: Self::Clock) -> ViewTimeout<Self::Clock>;
}
