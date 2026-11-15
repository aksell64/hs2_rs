use crate::protocol::timeouts::TimeoutPolicy;
use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::ops::RangeInclusive;

pub struct FixedPolicy<CL: Clock> {
    pub view_timeout: CL::Duration,
    pub lock_timeout: CL::Duration,
}

impl<CL: Clock> TimeoutPolicy<CL> for FixedPolicy<CL> {
    fn epoch_deadlines(
        &self,
        epoch_start: View,
        views: RangeInclusive<View>,
        entered_at: CL,
    ) -> EpochTimeouts<CL> {
        let timeouts = views
            .map(|view| {
                let slots = (view - epoch_start + 1) as u32;
                let deadline = entered_at.add(CL::scale(self.view_timeout, slots));
                ViewTimeout { view, deadline }
            })
            .collect();
        EpochTimeouts { timeouts }
    }

    fn lock_deadline(&self, view: View, start: CL) -> ViewTimeout<CL> {
        ViewTimeout {
            view,
            deadline: start.add(self.lock_timeout),
        }
    }
}
