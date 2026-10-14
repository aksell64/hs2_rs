use crate::types::View;

pub enum TimerEvent {
    ViewTimeout(View),
    LockTimeout(View),
}

pub enum TimerCommand {
    StartView(View),
    StartLock(View),
    StopLock(View),
}

pub trait TimerService {
    fn start_view_timer(&mut self, view: View);
    fn start_lock_timer(&mut self, view: View);
    fn stop_lock_timer(&mut self, view: View);

    fn next_event(&mut self) -> impl Future<Output = TimerEvent> + Send;
}


