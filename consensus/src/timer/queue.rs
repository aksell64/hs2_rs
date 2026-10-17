use crate::timer::service::TimerEvent;
use crate::types::clock::Clock;
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::collections::BTreeMap;

pub struct TimerQueue<CL: Clock> {
    view_timers: BTreeMap<View, CL>,
    lock_timer: Option<ViewTimeout<CL>>,
}

impl<CL: Clock> Default for TimerQueue<CL> {
    fn default() -> Self {
        Self {
            view_timers: BTreeMap::new(),
            lock_timer: None,
        }
    }
}

impl<CL: Clock> TimerQueue<CL> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_epoch_timers(&mut self, epoch: EpochTimeouts<CL>) {
        let timers = epoch.timeouts.into_iter().map(|t| (t.view, t.deadline));
        self.view_timers.extend(timers);
    }

    pub fn set_lock_timer(&mut self, timeout: ViewTimeout<CL>) {
        self.lock_timer = Some(timeout);
    }

    pub fn stop_lock_timer(&mut self, view: View) {
        if self.lock_timer.as_ref().is_some_and(|t| t.view == view) {
            self.lock_timer = None;
        }
    }

    pub fn stop_view_timers_below(&mut self, view: View) {
        self.view_timers = self.view_timers.split_off(&view);
    }

    pub fn next_deadline(&self) -> Option<CL> {
        let view = self.view_timers.first_key_value().map(|(_, deadline)| *deadline);
        let lock = self.lock_timer.as_ref().map(|t| t.deadline);
        view.into_iter().chain(lock).min()
    }

    pub fn pop_due(&mut self, now: CL) -> Option<TimerEvent> {
        let lock = self.lock_timer.as_ref().map(|t| t.deadline).filter(|d| *d <= now);
        let view = self.view_timers.first_key_value().map(|(_, d)| *d).filter(|d| *d <= now);
        match (lock, view) {
            (Some(lock), Some(view)) if lock <= view => self.take_lock(),
            (Some(_), None) => self.take_lock(),
            (_, Some(_)) => self.take_view(),
            (None, None) => None,
        }
    }

    fn take_lock(&mut self) -> Option<TimerEvent> {
        self.lock_timer.take().map(|t| TimerEvent::LockTimeout(t.view))
    }

    fn take_view(&mut self) -> Option<TimerEvent> {
        self.view_timers.pop_first().map(|(view, _)| TimerEvent::ViewTimeout(view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch(timers: &[(View, u64)]) -> EpochTimeouts<u64> {
        let timeouts = timers
            .iter()
            .map(|&(view, deadline)| ViewTimeout { view, deadline })
            .collect();
        EpochTimeouts { timeouts }
    }

    #[test]
    fn view_timers_fire_in_view_order_when_due() {
        let mut queue = TimerQueue::new();
        queue.set_epoch_timers(epoch(&[(3, 10), (4, 20), (5, 30)]));
        assert_eq!(queue.next_deadline(), Some(10));
        assert_eq!(queue.pop_due(5), None);
        assert_eq!(queue.pop_due(25), Some(TimerEvent::ViewTimeout(3)));
        assert_eq!(queue.pop_due(25), Some(TimerEvent::ViewTimeout(4)));
        assert_eq!(queue.pop_due(25), None);
        assert_eq!(queue.next_deadline(), Some(30));
    }

    #[test]
    fn lock_timer_fires_first_on_equal_deadlines() {
        let mut queue = TimerQueue::new();
        queue.set_epoch_timers(epoch(&[(3, 10)]));
        queue.set_lock_timer(ViewTimeout { view: 3, deadline: 10 });
        assert_eq!(queue.pop_due(10), Some(TimerEvent::LockTimeout(3)));
        assert_eq!(queue.pop_due(10), Some(TimerEvent::ViewTimeout(3)));
        assert_eq!(queue.next_deadline(), None);
    }

    #[test]
    fn earlier_view_timer_fires_before_lock_timer() {
        let mut queue = TimerQueue::new();
        queue.set_epoch_timers(epoch(&[(3, 10)]));
        queue.set_lock_timer(ViewTimeout { view: 4, deadline: 15 });
        assert_eq!(queue.next_deadline(), Some(10));
        assert_eq!(queue.pop_due(20), Some(TimerEvent::ViewTimeout(3)));
        assert_eq!(queue.pop_due(20), Some(TimerEvent::LockTimeout(4)));
    }

    #[test]
    fn stop_view_timers_below_keeps_given_view_and_later() {
        let mut queue = TimerQueue::new();
        queue.set_epoch_timers(epoch(&[(3, 10), (4, 20), (5, 30)]));
        queue.stop_view_timers_below(4);
        assert_eq!(queue.next_deadline(), Some(20));
        assert_eq!(queue.pop_due(100), Some(TimerEvent::ViewTimeout(4)));
        assert_eq!(queue.pop_due(100), Some(TimerEvent::ViewTimeout(5)));
        assert_eq!(queue.pop_due(100), None);
    }

    #[test]
    fn stop_lock_timer_ignores_other_view() {
        let mut queue = TimerQueue::<u64>::new();
        queue.set_lock_timer(ViewTimeout { view: 3, deadline: 10 });
        queue.stop_lock_timer(2);
        assert_eq!(queue.next_deadline(), Some(10));
        queue.stop_lock_timer(3);
        assert_eq!(queue.next_deadline(), None);
    }
}
