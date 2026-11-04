use crate::mocks::scheme::TestCommand;
use crate::types::mempool::Mempool;
use std::collections::VecDeque;

pub struct TestMempool {
    next: u64,
    pending: VecDeque<TestCommand>,
}

impl TestMempool {
    pub fn new(first: u64) -> Self {
        Self {
            next: first,
            pending: VecDeque::new(),
        }
    }
}

impl Mempool<TestCommand> for TestMempool {
    fn add_command(&mut self, cmd: TestCommand) {
        self.pending.push_back(cmd);
    }

    fn next_command(&mut self) -> TestCommand {
        self.pending.pop_front().unwrap_or_else(|| {
            let cmd = TestCommand(self.next);
            self.next += 1;
            cmd
        })
    }

    fn on_commands_commit<'a>(&mut self, _: impl IntoIterator<Item = &'a TestCommand>)
    where
        TestCommand: 'a,
    {
    }
}
