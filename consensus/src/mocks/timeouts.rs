use crate::protocol::timeouts::TimeoutPolicy;
use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::ops::RangeInclusive;

pub struct FixedPolicy<CL: Clock> {
    pub view_timeout: CL::Duration,
    pub lock_timeout: CL::Duration,
}

impl<CL: Clock> TimeoutPolicy for FixedPolicy<CL> {
    type Clock = CL;

    fn epoch_deadlines(&self, views: RangeInclusive<View>, start: CL) -> EpochTimeouts<CL> {
        let first = *views.start();
        let timeouts = views
            .map(|view| {
                let slots = (view - first + 1) as u32;
                let deadline = start.add(CL::scale(self.view_timeout, slots));
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
