use crate::mocks::scheme::{TestCommand, TestScheme};
use crate::types::block::Block;
use crate::types::output::Output;
use std::sync::{Arc, Mutex};

pub type CommitLog = Arc<Mutex<Vec<TestCommand>>>;

pub struct TestOutput {
    committed: CommitLog,
}

impl TestOutput {
    pub fn new(committed: CommitLog) -> Self {
        Self { committed }
    }
}

impl Output<TestScheme, TestCommand> for TestOutput {
    fn on_committed(&mut self, block: Block<TestScheme, TestCommand>) {
        self.committed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(block.cmd);
    }
}
