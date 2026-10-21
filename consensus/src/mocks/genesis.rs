use crate::mocks::scheme::{TestCommand, TestScheme, qc};
use crate::types::crypto::Notary;
use crate::types::genesis::{GenesisConfig, genesis_hash};
use std::sync::Arc;

pub fn genesis_config(n: u32, tag: &str) -> GenesisConfig<TestScheme, TestCommand> {
    let notary = Notary::new(Arc::new(TestScheme { n, id: None }));
    let cmd = TestCommand(0);
    let hash = genesis_hash(&notary, &cmd, tag.as_bytes());
    GenesisConfig {
        qc: qc(&TestScheme { n, id: None }, 0, hash),
        cmd,
        tag: tag.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::genesis::genesis_block;

    #[test]
    fn the_test_genesis_is_valid() {
        let notary = Notary::new(Arc::new(TestScheme { n: 4, id: None }));
        assert!(genesis_block(&notary, genesis_config(4, "test")).is_some());
    }

    #[test]
    fn a_genesis_under_another_tag_is_rejected() {
        let notary = Notary::new(Arc::new(TestScheme { n: 4, id: None }));
        let mut config = genesis_config(4, "test");
        config.tag = "other".to_string();
        assert!(genesis_block(&notary, config).is_none());
    }
}
