//! A threshold-BLS adapter for [`hotstuff_crypto::sign::Scheme`], and one certificate
//! walked end to end: sign, collect, assemble, verify.
//!
//! Threshold BLS recovers a single group signature from `t` shares, and that signature
//! verifies against one group public key. Two consequences shape the adapter:
//!
//! * the recovered signature says nothing about *who* signed -- there is no bitmap, so
//!   the scheme cannot report a signer count and cannot attribute a fault;
//! * the threshold is baked into the key material at dealing time, so one dealing serves
//!   exactly one [`Threshold`] kind and must refuse the others.
//!
//! Dealing here is done by a trusted dealer, which is the shortcut a real deployment
//! replaces with a DKG.

use blsttc::{
    PublicKeySet, SIG_SIZE, SecretKeySet, SecretKeyShare, Signature as GroupSig, SignatureShare,
};
use codec::{Decode, Encode};
use hotstuff_crypto::sign::{
    AggregateSignature, AssemblyError, Certificate, Digest, PartialSignature, Scheme, Signature,
};
use hotstuff_types::node::NodeID;
use hotstuff_types::threshold::{OneHonest, Quorum, Threshold};
use std::collections::BTreeMap;
use tiny_keccak::{Hasher, Sha3};
use std::fmt::{self, Debug};

/// Hash of whatever is being certified -- a block, a view, a timeout.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Encode, Decode)]
pub struct Hash32([u8; 32]);

impl Hash32 {
    /// Stand-in for the protocol's real hasher: FNV-1a seeding an xorshift. Diffuses
    /// well enough to read in the output, and is not a cryptographic hash.
    fn of(bytes: &[u8]) -> Self {
        let mut state: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes {
            state ^= *byte as u64;
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut out = [0u8; 32];
        for chunk in out.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes());
        }
        Self(out)
    }
}

impl AsRef<[u8]> for Hash32 {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Debug for Hash32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0[..4] {
            write!(f, "{b:02x}")?;
        }
        f.write_str("..")
    }
}

impl Digest for Hash32 {}

/// One node's share of the group signature.
#[derive(Clone, Copy, Encode, Decode)]
pub struct Share([u8; SIG_SIZE]);

impl Debug for Share {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Share({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl Signature for Share {}

/// The recovered group signature. Carries no signer information -- that is the defining
/// property of threshold BLS, and the reason this scheme is not attributable.
#[derive(Clone, Copy, Encode, Decode)]
pub struct Recovered([u8; SIG_SIZE]);

impl Debug for Recovered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Recovered({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl AggregateSignature for Recovered {}

/// One dealing of key material: a committee of `n`, certifying at `threshold() + 1`
/// signatures.
#[derive(Clone)]
pub struct Tbls {
    /// Commitment to the sharing polynomial. Verifies both the shares and the recovered
    /// signature, and is all a verify-only node needs.
    keys: PublicKeySet,
    /// This node's secret share, absent on a node that only verifies.
    signer: Option<(NodeID, SecretKeyShare)>,
    /// Committee size this material was dealt for.
    n: u32,
}

impl Tbls {
    /// Deal key material for a committee of `n` that certifies at `required` signatures.
    ///
    /// Returns one scheme per committee member plus a verify-only scheme; the latter is
    /// what a replica checking someone else's certificate would hold.
    pub fn deal(n: u32, required: u32) -> (Vec<Tbls>, Tbls) {
        // blsttc counts in polynomial degree: a degree-`d` sharing needs `d + 1` shares.
        let degree = required.saturating_sub(1) as usize;
        let secrets = SecretKeySet::random(degree, &mut rand::thread_rng());
        let keys = secrets.public_keys();

        let signers = (0..n)
            .map(|i| Tbls {
                keys: keys.clone(),
                signer: Some((NodeID::from(i), secrets.secret_key_share(i as usize))),
                n,
            })
            .collect();

        let verifier = Tbls {
            keys,
            signer: None,
            n,
        };
        (signers, verifier)
    }

    /// Shares this dealing needs, which is the one threshold it can serve.
    pub fn required(&self) -> u32 {
        self.keys.threshold() as u32 + 1
    }
}

impl Scheme for Tbls {
    type Signature = Share;
    type Digest = Hash32;
    type Aggregate = Recovered;

    fn hash(&self, bytes: &[u8]) -> Hash32 {
        let mut out = [0u8; 32];
        let mut sha3 = Sha3::v256();
        sha3.update(bytes);
        sha3.finalize(&mut out);
        Hash32(out)
    }

    fn committee_size(&self) -> u32 {
        self.n
    }

    fn sign(&self, data_hash: Hash32) -> Option<PartialSignature<Share>> {
        let (id, secret) = self.signer.as_ref()?;
        Some(PartialSignature::new(
            *id,
            Share(secret.sign(data_hash).to_bytes()),
        ))
    }

    fn verify_partial(&self, data_hash: Hash32, partial: &PartialSignature<Share>) -> bool {
        let index: u32 = partial.node_id().into();
        if index >= self.n {
            return false;
        }
        let Ok(share) = SignatureShare::from_bytes(partial.signature().0) else {
            return false;
        };
        self.keys
            .public_key_share(index as usize)
            .verify(&share, data_hash)
    }

    fn aggregate<I>(&self, _data_hash: Hash32, partials: I) -> Result<Recovered, AssemblyError>
    where
        I: Iterator<Item = PartialSignature<Share>>,
    {
        // Lagrange interpolation needs each share's abscissa, and the node id is it.
        let mut samples: BTreeMap<usize, SignatureShare> = BTreeMap::new();
        for partial in partials {
            let id = partial.node_id();
            let index: u32 = id.into();
            if index >= self.n {
                return Err(AssemblyError::UnknownSigner(id));
            }
            let share = SignatureShare::from_bytes(partial.signature().0)
                .map_err(|_| AssemblyError::InvalidShare(id))?;
            if samples.insert(index as usize, share).is_some() {
                return Err(AssemblyError::Duplicate(id));
            }
        }

        let recovered = self
            .keys
            .combine_signatures(samples.iter().map(|(index, share)| (*index, share)))
            .map_err(|_| AssemblyError::Invalid)?;

        Ok(Recovered(recovered.to_bytes()))
    }

    fn verify_aggregate<T: Threshold>(&self, data_hash: Hash32, aggregate: &Recovered) -> bool {
        // The group public key already implies "at least `required` shares combined", so
        // there is nothing to count. What must be refused is a kind this dealing does not
        // serve: asked for a quorum, a sharing dealt at f + 1 would otherwise say yes.
        if T::required(self.n) != self.required() {
            return false;
        }
        let Ok(signature) = GroupSig::from_bytes(aggregate.0) else {
            return false;
        };
        self.keys.public_key().verify(&signature, data_hash)
    }
}

fn main() {
    // A committee of four tolerates one fault: a quorum is three.
    const N: u32 = 4;
    let quorum = Quorum::required(N);
    println!(
        "committee of {N}, f = 1, Quorum::required = {quorum}, OneHonest::required = {}\n",
        OneHonest::required(N)
    );

    let (signers, verifier) = Tbls::deal(N, quorum);
    let data_hash = Hash32::of(b"block #7");
    println!("subject: {data_hash:?}");

    // 1. Each replica signs with its own share.
    let partials: Vec<_> = signers
        .iter()
        .map(|node| node.sign(data_hash).expect("committee member"))
        .collect();
    for partial in &partials {
        println!(
            "  signed by {:?} -> {:?}",
            partial.node_id(),
            partial.signature()
        );
    }

    // 2. Verify each share, then assemble the first quorum that passes.
    let accepted: Vec<_> = partials
        .into_iter()
        .filter(|partial| verifier.verify_partial(data_hash, partial))
        .take(quorum as usize)
        .collect();
    println!("\nassembling from {} verified shares", accepted.len());
    let aggregate = verifier
        .aggregate(data_hash, accepted.into_iter())
        .expect("a quorum of honest shares certifies");
    let certificate = Certificate::<_, Quorum, _>::new(data_hash, aggregate);
    println!("  -> {certificate:?}");

    // 3. Verification: the cryptography and the threshold, against the verifier's own
    //    committee. The subject comes from the certificate; the committee size and the
    //    threshold kind do not.
    println!(
        "\nverify as Quorum:      {}",
        verifier.verify_certificate::<Quorum>(&certificate)
    );

    // Asking for the wrong kind of certificate does not even compile: the marker in
    // `Certificate<_, Quorum, _>` will not unify with `OneHonest`. Only bytes decoded
    // from the network can carry the wrong claim, and the adapter refuses those too --
    // this dealing serves one threshold and knows which.
    println!(
        "verify aggregate as OneHonest: {}",
        verifier.verify_aggregate::<OneHonest>(data_hash, certificate.aggregate())
    );

    // A certificate certifies its own subject and nothing else, so binding it to the
    // block in hand is a separate step the caller owns. `verify_certificate` takes no
    // hash to check against, precisely so that the two questions cannot be confused.
    let other = Hash32::of(b"block #8");
    println!(
        "certifies {other:?}:  {}",
        certificate.data_hash() == other && verifier.verify_certificate::<Quorum>(&certificate)
    );

    // 4. A share from outside the committee fails verification.
    let (outsiders, _) = Tbls::deal(N, quorum);
    let forged = outsiders[0].sign(data_hash).expect("committee member");
    println!(
        "\nshare from another dealing verifies: {}",
        verifier.verify_partial(data_hash, &forged)
    );
}
