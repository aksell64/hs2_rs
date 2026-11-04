use crate::timer::queue::TimerQueue;
use crate::types::clock::{Clock, ClockSource};
use crate::types::{EpochTimeouts, View, ViewTimeout};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub enum TimerEvent {
    ViewTimeout(View),
    LockTimeout(View),
}

pub enum TimerCommand<CL: Clock> {
    SetEpochTimers(EpochTimeouts<CL>),
    SetLockTimer(ViewTimeout<CL>),
    ClearLockTimer(View),
    ClearViewTimersBelow(View),
}

pub trait TimerService<CL: Clock>: Send + 'static {
    fn set_epoch_timers(&mut self, timeouts: EpochTimeouts<CL>);
    fn set_lock_timer(&mut self, timeout: ViewTimeout<CL>);
    fn stop_lock_timer(&mut self, view: View);
    fn stop_view_timers_below(&mut self, view: View);

    fn next_event(&mut self) -> impl Future<Output = TimerEvent> + Send;
}

impl Clock for Instant {
    type Duration = Duration;

    fn add(self, d: Duration) -> Self {
        self + d
    }

    fn scale(d: Duration, times: u32) -> Duration {
        d * times
    }
}

pub struct TokioSource;

impl ClockSource for TokioSource {
    type Clock = Instant;

    fn now(&self) -> Instant {
        Instant::now()
    }
}

#[derive(Default)]
pub struct TokioTimerService {
    queue: TimerQueue<Instant>,
}

impl TokioTimerService {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TimerService<Instant> for TokioTimerService {
    fn set_epoch_timers(&mut self, timeouts: EpochTimeouts<Instant>) {
        self.queue.set_epoch_timers(timeouts);
    }

    fn set_lock_timer(&mut self, timeout: ViewTimeout<Instant>) {
        self.queue.set_lock_timer(timeout);
    }

    fn stop_lock_timer(&mut self, view: View) {
        self.queue.stop_lock_timer(view);
    }

    fn stop_view_timers_below(&mut self, view: View) {
        self.queue.stop_view_timers_below(view);
    }

    async fn next_event(&mut self) -> TimerEvent {
        loop {
            match self.queue.next_deadline() {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
            if let Some(event) = self.queue.pop_due(Instant::now()) {
                return event;
            }
        }
    }
}
