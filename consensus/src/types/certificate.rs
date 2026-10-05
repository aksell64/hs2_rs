use hotstuff_crypto::sign::Certificate;
use hotstuff_types::threshold::{OneHonest, Quorum};

pub type QuorumCertificate<D, A> = Certificate<D, Quorum, A>;

pub type TimeoutCertificate<D, A> = Certificate<D, OneHonest, A>;
