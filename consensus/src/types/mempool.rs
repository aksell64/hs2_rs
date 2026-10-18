use crate::types::block::Command;

pub trait Mempool<C: Command>: Send + 'static {
    fn next_command(&mut self) -> C;

    fn on_commands_commit<'a>(&mut self, commands: impl IntoIterator<Item = &'a C>)
    where
        C: 'a;
}
