use codec::{Decode, Encode};
use finality_grandpa::voter_set::VoterSet;
use sp_blockchain::Error;
use sp_finality_grandpa::{
    AuthorityId, AuthoritySignature, AuthorityWeight, ConsensusLog, ScheduledChange,
    GRANDPA_ENGINE_ID,
};
use sp_runtime::generic::OpaqueDigestItemId;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, NumberFor};
use std::collections::{HashMap, HashSet};

/// A list of Grandpa authorities with associated weights.
pub type AuthorityList = Vec<(AuthorityId, AuthorityWeight)>; // ed25519;

/// A commit message for this chain's block type.
pub type Commit<Block> = finality_grandpa::Commit<
    <Block as BlockT>::Hash,
    NumberFor<Block>,
    AuthoritySignature,
    AuthorityId,
>;

/// Finality for block B is proved by providing:
/// 1) the justification for the descendant block F;
/// 2) headers sub-chain (B; F] if B != F;
#[derive(Debug, PartialEq, Encode, Decode, Clone)]
pub struct FinalityProof<Header: HeaderT> {
    /// The hash of block F for which justification is provided.
    pub block: Header::Hash,
    /// Justification of the block F.
    pub justification: Vec<u8>,
    /// The set of headers in the range (B; F] that we believe are unknown to the caller. Ordered.
    pub unknown_headers: Vec<Header>,
}

/// A GRANDPA justification for block finality, it includes a commit message and
/// an ancestry proof including all headers routing all precommit target blocks
/// to the commit target block. Due to the current voting strategy the precommit
/// targets should be the same as the commit target, since honest voters don't
/// vote past authority set change blocks.
///
/// This is meant to be stored in the db and passed around the network to other
/// nodes, and are used by syncing nodes to prove authority set handoffs.
#[derive(Clone, Encode, Decode, PartialEq, Eq, Debug)]
pub struct GrandpaJustification<Block: BlockT> {
    pub(crate) round: u64,
    pub(crate) commit: Commit<Block>,
    pub(crate) votes_ancestries: Vec<Block::Header>,
}

impl<Block: BlockT> GrandpaJustification<Block> {
    /// Decode a GRANDPA justification and validate the commit and the votes'
    /// ancestry proofs finalize the given block.
    pub fn decode_and_verify_finalizes(
        encoded: &[u8],
        finalized_target: (Block::Hash, NumberFor<Block>),
        set_id: u64,
        voters: &VoterSet<AuthorityId>,
    ) -> Result<GrandpaJustification<Block>, Error>
    where
        NumberFor<Block>: finality_grandpa::BlockNumberOps,
    {
        let justification = GrandpaJustification::<Block>::decode(&mut &*encoded)
            .map_err(|_| Error::JustificationDecode)?;

        if (
            justification.commit.target_hash,
            justification.commit.target_number,
        ) != finalized_target
        {
            let msg = "invalid commit target in grandpa justification".to_string();
            Err(Error::BadJustification(msg))
        } else {
            justification
                .verify_with_voter_set(set_id, voters)
                .map(|_| justification)
        }
    }

    /// Validate the commit and the votes' ancestry proofs.
    pub fn verify(&self, set_id: u64, authorities: &AuthorityList) -> Result<(), Error>
    where
        NumberFor<Block>: finality_grandpa::BlockNumberOps,
    {
        let voters = VoterSet::new(authorities.iter().cloned())
            .ok_or(Error::Consensus(sp_consensus::Error::InvalidAuthoritiesSet))?;

        self.verify_with_voter_set(set_id, &voters)
    }

    /// Validate the commit and the votes' ancestry proofs.
    pub(crate) fn verify_with_voter_set(
        &self,
        set_id: u64,
        voters: &VoterSet<AuthorityId>,
    ) -> Result<(), Error>
    where
        NumberFor<Block>: finality_grandpa::BlockNumberOps,
    {
        use finality_grandpa::Chain;

        let ancestry_chain = AncestryChain::<Block>::new(&self.votes_ancestries);

        match finality_grandpa::validate_commit(&self.commit, voters, &ancestry_chain) {
            Ok(ref result) if result.is_valid() => {}
            _ => {
                let msg = "invalid commit in grandpa justification".to_string();
                return Err(Error::BadJustification(msg));
            }
        }

        // we pick the precommit for the lowest block as the base that
        // should serve as the root block for populating ancestry (i.e.
        // collect all headers from all precommit blocks to the base)
        let base_hash = self
            .commit
            .precommits
            .iter()
            .map(|signed| &signed.precommit)
            .min_by_key(|precommit| precommit.target_number)
            .map(|precommit| precommit.target_hash.clone())
            .expect(
                "can only fail if precommits is empty; \
				 commit has been validated above; \
				 valid commits must include precommits; \
				 qed.",
            );

        let mut buf = Vec::new();
        let mut visited_hashes = HashSet::new();
        for signed in self.commit.precommits.iter() {
            if !sp_finality_grandpa::check_message_signature_with_buffer(
                &finality_grandpa::Message::Precommit(signed.precommit.clone()),
                &signed.id,
                &signed.signature,
                self.round,
                set_id,
                &mut buf,
            ) {
                return Err(Error::BadJustification(
                    "invalid signature for precommit in grandpa justification".to_string(),
                ));
            }

            if base_hash == signed.precommit.target_hash {
                continue;
            }

            match ancestry_chain.ancestry(base_hash, signed.precommit.target_hash) {
                Ok(route) => {
                    // ancestry starts from parent hash but the precommit target hash has been
                    // visited
                    visited_hashes.insert(signed.precommit.target_hash);
                    for hash in route {
                        visited_hashes.insert(hash);
                    }
                }
                _ => {
                    return Err(Error::BadJustification(
                        "invalid precommit ancestry proof in grandpa justification".to_string(),
                    ))
                }
            }
        }

        let ancestry_hashes: HashSet<_> = self
            .votes_ancestries
            .iter()
            .map(|h: &Block::Header| h.hash())
            .collect();

        if visited_hashes != ancestry_hashes {
            return Err(Error::BadJustification(
                "invalid precommit ancestries in grandpa justification with unused headers"
                    .to_string(),
            ));
        }

        Ok(())
    }

    /// The target block number and hash that this justifications proves finality for.
    pub fn target(&self) -> (NumberFor<Block>, Block::Hash) {
        (self.commit.target_number, self.commit.target_hash)
    }
}

/// A utility trait implementing `finality_grandpa::Chain` using a given set of headers.
/// This is useful when validating commits, using the given set of headers to
/// verify a valid ancestry route to the target commit block.
struct AncestryChain<Block: BlockT> {
    ancestry: HashMap<Block::Hash, Block::Header>,
}

impl<Block: BlockT> AncestryChain<Block> {
    fn new(ancestry: &[Block::Header]) -> AncestryChain<Block> {
        let ancestry: HashMap<_, _> = ancestry
            .iter()
            .cloned()
            .map(|h: Block::Header| (h.hash(), h))
            .collect();

        AncestryChain { ancestry }
    }
}

impl<Block: BlockT> finality_grandpa::Chain<Block::Hash, NumberFor<Block>> for AncestryChain<Block>
where
    NumberFor<Block>: finality_grandpa::BlockNumberOps,
{
    fn ancestry(
        &self,
        base: Block::Hash,
        block: Block::Hash,
    ) -> Result<Vec<Block::Hash>, finality_grandpa::Error> {
        let mut route = Vec::new();
        let mut current_hash = block;
        loop {
            if current_hash == base {
                break;
            }
            match self.ancestry.get(&current_hash) {
                Some(current_header) => {
                    current_hash = *current_header.parent_hash();
                    route.push(current_hash);
                }
                _ => return Err(finality_grandpa::Error::NotDescendent),
            }
        }
        route.pop(); // remove the base

        Ok(route)
    }
}

/// Checks the given header for a consensus digest signalling a **standard** scheduled change and
/// extracts it.
pub fn find_scheduled_change<B: BlockT>(
    header: &B::Header,
) -> Option<ScheduledChange<NumberFor<B>>> {
    let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

    let filter_log = |log: ConsensusLog<NumberFor<B>>| match log {
        ConsensusLog::ScheduledChange(change) => Some(change),
        _ => None,
    };

    // find the first consensus digest with the right ID which converts to
    // the right kind of consensus log.
    header
        .digest()
        .convert_first(|l| l.try_to(id).and_then(filter_log))
}

/// Checks the given header for a consensus digest signalling a **forced** scheduled change and
/// extracts it.
pub fn find_forced_change<B: BlockT>(
    header: &B::Header,
) -> Option<(NumberFor<B>, ScheduledChange<NumberFor<B>>)> {
    let id = OpaqueDigestItemId::Consensus(&GRANDPA_ENGINE_ID);

    let filter_log = |log: ConsensusLog<NumberFor<B>>| match log {
        ConsensusLog::ForcedChange(delay, change) => Some((delay, change)),
        _ => None,
    };

    // find the first consensus digest with the right ID which converts to
    // the right kind of consensus log.
    header
        .digest()
        .convert_first(|l| l.try_to(id).and_then(filter_log))
}

#[cfg(test)]
mod tests {
    use crate::justification::{find_forced_change, find_scheduled_change, FinalityProof, GrandpaJustification, AuthorityList};
    use codec::{Decode, Encode};
    use finality_grandpa_rpc::GrandpaApiClient;
    use polkadot_core_primitives::{Block, Header};
    use serde::{Deserialize, Serialize};
    use futures::StreamExt;
    use sp_core::H256;
    // use sp_runtime::traits::Header as _;
    use std::mem::size_of_val;
    use subxt::DefaultConfig;
    use subxt::rpc::{ClientT, rpc_params};
    use crate::kusama;

    type Justification = GrandpaJustification<Block>;

    /// An encoded justification proving that the given header has been finalized
    #[derive(Clone, Serialize, Deserialize)]
    pub struct JustificationNotification(sp_core::Bytes);

    #[tokio::test]
    async fn follow_grandpa_justifications() {
        let url = std::env::var("NODE_ENDPOINT")
            .unwrap_or("wss://kusama-rpc.polkadot.io:443".to_string());
        let client = subxt::ClientBuilder::new()
            .set_url(url)
            .build::<subxt::DefaultConfig>()
            .await
            .unwrap();

        let api = client
            .clone()
            .to_runtime_api::<kusama::api::RuntimeApi<DefaultConfig, subxt::PolkadotExtrinsicParams<_>>>();

        let mut subscription = client.rpc().subscribe_finalized_blocks().await.unwrap().chunks(3);

        while let Some(headers) = subscription.next().await {
            let headers = headers.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
            let header = headers.last().unwrap();
            let current_set_id = api.storage().grandpa().current_set_id(Some(header.hash())).await.unwrap();

            let header = Header::decode(&mut &header.encode()[..]).unwrap();
            println!("========= New Header =========");
            println!(
                "Got header: Hash({}), Number({})",
                header.hash(),
                header.number
            );

            let scheduled_authority_set = find_scheduled_change::<Block>(&header);
            println!("scheduled_authority_set: {scheduled_authority_set:#?}");
            let forced_authority_set = find_forced_change::<Block>(&header);
            println!("forced_authority_set: {forced_authority_set:#?}");

            let encoded = GrandpaApiClient::<JustificationNotification, H256, u32>::prove_finality(
                &*client.rpc().client,
                header.number,
            )
            .await
            .unwrap()
            .unwrap()
            .0
            .0;

            println!("justification size: {}kb", size_of_val(&*encoded) / 1000);
            let finality_proof = FinalityProof::<Header>::decode(&mut &encoded[..]).unwrap();
            let unknown_headers = finality_proof
                .unknown_headers
                .iter()
                .map(|h| h.number)
                .collect::<Vec<_>>();
            println!("unknown_headers: {unknown_headers:#?}",);

            let mut justification =
                Justification::decode(&mut &finality_proof.justification[..]).unwrap();

            let authorities = client
                .rpc()
                .client
                .request::<String>("state_call", rpc_params!("GrandpaApi_grandpa_authorities", "0x"))
                .await
                .unwrap();

            let authorities = hex::decode(&authorities[2..]).unwrap();
            let authorities = AuthorityList::decode(&mut &authorities[..]).unwrap();
            justification.verify(current_set_id, &authorities).expect("Failed to verify proof");

            let pre_commits = justification
                .commit
                .precommits
                .drain(..)
                .collect::<Vec<_>>();
            println!("{justification:#?}");
            println!("Signatures: {:#?}", pre_commits.len());

            println!("========= Successfully verified grandpa justification =========");
        }
    }
}
