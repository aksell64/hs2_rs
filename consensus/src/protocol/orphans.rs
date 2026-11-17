use crate::protocol::driver::DriverInput;
use crate::types::block::Command;
use hotstuff_crypto::sign::Scheme;
use std::collections::HashMap;

pub struct Orphans<S: Scheme, C: Command> {
    missing: HashMap<S::Digest, Vec<DriverInput<S, C>>>,
}

impl<S: Scheme, C: Command> Orphans<S, C> {
    pub fn new() -> Self {
        Self {
            missing: HashMap::new(),
        }
    }

    pub fn buffer_input(&mut self, missing: S::Digest, input: DriverInput<S, C>) {
        self.missing.entry(missing).or_default().push(input);
    }

    pub fn take_inputs(&mut self, digest: &S::Digest) -> Vec<DriverInput<S, C>> {
        self.missing.remove(digest).unwrap_or_default()
    }
}
