//! How many signatures a certificate needs before it proves anything.
//!
//! There are exactly two kinds, and they differ in what they assert rather than in size:
//! [`Quorum`] buys intersection, [`OneHonest`] buys the presence of an honest signer.
//! Mixing them up costs safety, so the kind lives in the certificate's type rather than
//! in its data.

/// The number of signers a certificate of this kind requires.
///
/// An associated function rather than a method: the threshold is a property of the
/// certificate kind and of the committee, not of any value. All the state -- committee
/// size today, stake weights later -- lives in the committee and is passed in.
pub trait Threshold: Clone {
    /// Signatures required from a committee of `n` nodes.
    fn required(n: u32) -> u32;
}

/// `2f + 1`. Any two such sets intersect in at least `f + 1` nodes, hence in at least one
/// honest replica. That is where the uniqueness of a certified block at a height comes
/// from, and with it the safety of the two-chain commit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quorum;

impl Threshold for Quorum {
    fn required(n: u32) -> u32 {
        (n - faulty(n)).max(1)
    }
}

/// `f + 1`. **Not a quorum:** two such sets may not intersect at all -- at `n = 4` take
/// `{1,2}` and `{3,4}`. It proves exactly one thing: at least one signer is honest. That
/// is enough for timeout amplification and for "somebody has the block", and not enough
/// for anything resting on intersection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OneHonest;

impl Threshold for OneHonest {
    fn required(n: u32) -> u32 {
        faulty(n) + 1
    }
}

/// Faulty nodes a committee of `n` tolerates: `f = (n - 1) / 3`.
fn faulty(n: u32) -> u32 {
    n.saturating_sub(1) / 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classic_sizes() {
        for (n, f) in [(4, 1), (7, 2), (10, 3), (100, 33)] {
            assert_eq!(Quorum::required(n), 2 * f + 1, "n = {n}");
            assert_eq!(OneHonest::required(n), f + 1, "n = {n}");
        }
    }

    /// Two quorums intersect in an honest replica; two `f + 1` sets need not intersect.
    #[test]
    fn intersection() {
        for (n, f) in [(4, 1), (7, 2), (100, 33)] {
            let q = Quorum::required(n);
            assert!(
                2 * q - n > f,
                "two quorums must intersect in an honest node"
            );
            let h = OneHonest::required(n);
            assert!(2 * h <= n, "two f+1 sets can be disjoint");
        }
    }

    /// A degenerate committee must not yield a threshold of zero: an empty certificate is
    /// not a certificate.
    #[test]
    fn no_empty_threshold() {
        assert_eq!(Quorum::required(0), 1);
        assert_eq!(OneHonest::required(0), 1);
        assert_eq!(Quorum::required(1), 1);
    }
}
