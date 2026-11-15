use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::ops::RangeInclusive;

pub trait TimeoutPolicy<CL: Clock>: Send + 'static {
    /// Schedules `views` relative to the first entry into the epoch.
    /// Slot numbers are counted from `epoch_start`, including skipped views.
    fn epoch_deadlines(
        &self,
        epoch_start: View,
        views: RangeInclusive<View>,
        entered_at: CL,
    ) -> EpochTimeouts<CL>;
    fn lock_deadline(&self, view: View, start: CL) -> ViewTimeout<CL>;
}
