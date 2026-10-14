use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::CertifiedBlock;
use codec::{Decode, Encode};
use hotstuff_crypto::sign::{
    AggregateSignature, AssemblyError, CertificateCollector, Digest, PartialSignature, Scheme,
    Signature,
};
use hotstuff_types::node::NodeID;
use hotstuff_types::threshold::{Quorum, Threshold};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Encode, Decode)]
pub struct TestDigest(pub [u8; 4]);

impl AsRef<[u8]> for TestDigest {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Digest for TestDigest {}

#[derive(Clone, Debug, Encode, Decode)]
pub struct TestSignature;

impl Signature for TestSignature {}

#[derive(Clone, Debug, Encode, Decode)]
pub struct TestAggregate {
    subject: TestDigest,
    signers: BTreeSet<NodeID>,
}

impl AggregateSignature for TestAggregate {}

pub struct TestScheme {
    pub n: u32,
}

impl Scheme for TestScheme {
    type Signature = TestSignature;
    type Digest = TestDigest;
    type Aggregate = TestAggregate;

    fn committee_size(&self) -> u32 {
        self.n
    }

    fn sign(&self, _: TestDigest) -> Option<PartialSignature<TestSignature>> {
        None
    }

    fn verify_partial(&self, _: TestDigest, _: &PartialSignature<TestSignature>) -> bool {
        true
    }

    fn aggregate<I>(&self, data_hash: TestDigest, partials: I) -> Result<TestAggregate, AssemblyError>
    where
        I: Iterator<Item = PartialSignature<TestSignature>>,
    {
        Ok(TestAggregate {
            subject: data_hash,
            signers: partials.map(|partial| partial.node_id()).collect(),
        })
    }

    fn verify_aggregate<T: Threshold>(&self, data_hash: TestDigest, aggregate: &TestAggregate) -> bool {
        aggregate.subject == data_hash && aggregate.signers.len() as u32 >= T::required(self.n)
    }
}

#[derive(Clone, PartialEq, Debug, Encode, Decode)]
pub struct TestCommand(pub u64);

impl Command for TestCommand {}

pub fn digest(n: u32) -> TestDigest {
    TestDigest(n.to_be_bytes())
}

pub fn qc(scheme: &TestScheme, view: View, block: TestDigest) -> CertifiedBlock<TestScheme> {
    let mut collector = CertificateCollector::<Quorum, _>::new(scheme, block);
    let certificate = (0..scheme.n)
        .find_map(|i| {
            collector
                .add_partial(PartialSignature::new(NodeID::from(i), TestSignature))
                .unwrap()
        })
        .unwrap();
    CertifiedBlock {
        certificate,
        block,
        view,
    }
}

pub fn block(
    hash: TestDigest,
    parent: TestDigest,
    justify: CertifiedBlock<TestScheme>,
    height: u64,
) -> Block<TestScheme, TestCommand> {
    Block {
        parent,
        cmd: TestCommand(height),
        justify,
        height,
        hash,
    }
}
