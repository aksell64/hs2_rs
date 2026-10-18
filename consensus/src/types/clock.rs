pub trait Clock: Copy + Ord + Send + Sync + 'static {
    type Duration: Copy + Send + Sync;
    fn add(self, d: Self::Duration) -> Self;
    fn scale(d: Self::Duration, times: u32) -> Self::Duration;
}

pub trait Source: Send + 'static {
    type Clock: Clock;
    fn now(&self) -> Self::Clock;
}
