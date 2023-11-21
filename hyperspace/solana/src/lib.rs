#![feature(more_qualified_paths)]
extern crate alloc;

use alloc::rc::Rc;
use core::{pin::Pin, str::FromStr, time::Duration};
use ibc_storage::{PrivateStorage, SequenceTripleIdx};
use ids::{ClientIdx, ConnectionIdx};
use prost::Message;
use trie_key::{SequencePath, TrieKey};

use anchor_client::{
	solana_client::{
		nonblocking::rpc_client::RpcClient as AsyncRpcClient, rpc_config::RpcSendTransactionConfig,
	},
	solana_sdk::{
		commitment_config::{CommitmentConfig, CommitmentLevel},
		signature::{Keypair, Signature},
		signer::Signer as AnchorSigner,
	},
	Client as AnchorClient, Cluster, Program,
};
use anchor_lang::{prelude::*, system_program};
use error::Error;
use ibc::{
	core::{
		ics02_client::{client_state::ClientType, events::UpdateClient},
		ics04_channel::packet::Sequence,
		ics23_commitment::commitment::{CommitmentPath, CommitmentPrefix},
		ics24_host::{
			identifier::{ChannelId, ClientId, ConnectionId, PortId},
			path::{
				ChannelEndsPath, ClientConsensusStatePath, ClientStatePath, CommitmentsPath,
				ConnectionsPath, ReceiptsPath, AcksPath,
			},
		},
	},
	events::IbcEvent,
	Height,
};
use ibc_proto::{
	google::protobuf::Any,
	ibc::core::{
		channel::v1::{
			Channel, QueryChannelResponse, QueryNextSequenceReceiveResponse,
			QueryPacketCommitmentResponse, QueryPacketReceiptResponse, QueryPacketAcknowledgementResponse,
		},
		client::v1::{QueryClientStateResponse, QueryConsensusStateResponse},
		connection::v1::{ConnectionEnd, QueryConnectionResponse},
	},
};
use instructions::AnyCheck;
use pallet_ibc::light_clients::AnyClientMessage;
use primitives::{
	Chain, CommonClientConfig, CommonClientState, IbcProvider, KeyProvider, LightClientSync,
	MisbehaviourHandler, UndeliveredType,
};
use std::{
	collections::{BTreeMap, HashSet},
	result::Result,
	sync::{Arc, Mutex},
};
use tendermint_rpc::Url;
use tokio_stream::Stream;

mod accounts;
mod error;
mod ibc_storage;
mod ids;
mod instructions;
mod trie;
mod trie_key;

const SOLANA_IBC_STORAGE_SEED: &[u8] = b"solana_ibc_storage";
const TRIE_SEED: &[u8] = b"trie";

// Random key added to implement `#[account]` macro for the storage
declare_id!("EnfDJsAK7BGgetnmKzBx86CsgC5kfSPcsktFCQ4YLC81");

pub struct InnerAny {
	pub type_url: String,
	pub value: Vec<u8>,
}

/// Implements the [`crate::Chain`] trait for solana
#[derive(Clone)]
pub struct Client {
	/// Chain name
	pub name: String,
	/// rpc url for solana
	pub rpc_url: String,
	/// Solana chain Id
	pub chain_id: String,
	/// Light client id on counterparty chain
	pub client_id: Option<ClientId>,
	/// Connection Id
	pub connection_id: Option<ConnectionId>,
	/// Account prefix
	pub account_prefix: String,
	pub fee_denom: String,
	/// The key that signs transactions
	pub keybase: KeyEntry,
	/// Maximun transaction size
	pub max_tx_size: usize,
	pub commitment_level: CommitmentLevel,
	pub program_id: Pubkey,
	pub common_state: CommonClientState,
	pub client_type: ClientType,
	/// Reference to commitment
	pub commitment_prefix: CommitmentPrefix,
	/// Channels cleared for packet relay
	pub channel_whitelist: Arc<Mutex<HashSet<(ChannelId, PortId)>>>,
}

pub struct ClientConfig {
	/// Chain name
	pub name: String,
	/// rpc url for cosmos
	pub rpc_url: Url,
	/// Solana chain Id
	pub chain_id: String,
	/// Light client id on counterparty chain
	pub client_id: Option<ClientId>,
	/// Connection Id
	pub connection_id: Option<ConnectionId>,
	/// Account prefix
	pub account_prefix: String,
	/// Fee denom
	pub fee_denom: String,
	/// Fee amount
	pub fee_amount: String,
	/// Fee amount
	pub gas_limit: u64,
	/// Store prefix
	pub store_prefix: String,
	/// Maximun transaction size
	pub max_tx_size: usize,
	/// All the client states and headers will be wrapped in WASM ones using the WASM code ID.
	pub wasm_code_id: Option<String>,
	pub common_state_config: CommonClientConfig,
	/// Reference to commitment
	pub commitment_prefix: CommitmentPrefix,
}

#[derive(Clone)]
pub struct KeyEntry {
	pub public_key: Pubkey,
	pub private_key: Vec<u8>,
}

impl KeyEntry {
	fn keypair(&self) -> Keypair {
		Keypair::from_bytes(&self.private_key).unwrap()
	}
}

impl Client {
	pub fn get_trie_key(&self) -> Pubkey {
		let trie_seeds = &[TRIE_SEED];
		let trie = Pubkey::find_program_address(trie_seeds, &self.program_id).0;
		trie
	}

	pub fn get_ibc_storage_key(&self) -> Pubkey {
		let storage_seeds = &[SOLANA_IBC_STORAGE_SEED];
		let ibc_storage = Pubkey::find_program_address(storage_seeds, &self.program_id).0;
		ibc_storage
	}

	pub async fn get_trie(&self) -> trie::AccountTrie<Vec<u8>> {
		let trie_key = self.get_trie_key();
		let rpc_client = self.rpc_client();
		let trie_account = rpc_client
			.get_account_with_commitment(&trie_key, CommitmentConfig::processed())
			.await
			.unwrap()
			.value
			.unwrap();
		let trie = trie::AccountTrie::new(trie_account.data).unwrap();
		trie
	}

	pub fn get_ibc_storage(&self) -> PrivateStorage {
		let program = self.program();
		let ibc_storage_key = self.get_ibc_storage_key();
		let storage = program.account(ibc_storage_key).unwrap();
		storage
	}

	pub fn rpc_client(&self) -> AsyncRpcClient {
		let program = self.program();
		program.async_rpc()
	}

	pub fn client(&self) -> AnchorClient<Rc<Keypair>> {
		let cluster = Cluster::from_str(&self.rpc_url).unwrap();
		let signer = self.keybase.keypair();
		let authority = Rc::new(signer);
		let client =
			AnchorClient::new_with_options(cluster, authority, CommitmentConfig::processed());
		client
	}

	pub fn program(&self) -> Program<Rc<Keypair>> {
		let anchor_client = self.client();
		anchor_client.program(self.program_id).unwrap()
	}
}

#[async_trait::async_trait]
impl IbcProvider for Client {
	type FinalityEvent = Vec<u8>;

	type TransactionId = String;

	type AssetId = String;

	type Error = Error;

	async fn query_latest_ibc_events<T>(
		&mut self,
		finality_event: Self::FinalityEvent,
		counterparty: &T,
	) -> Result<Vec<(Any, Height, Vec<IbcEvent>, primitives::UpdateType)>, anyhow::Error>
	where
		T: Chain,
	{
		todo!()
	}

	async fn ibc_events(&self) -> Pin<Box<dyn Stream<Item = IbcEvent> + Send + 'static>> {
		todo!()
	}

	async fn query_client_consensus(
		&self,
		at: Height,
		client_id: ClientId,
		consensus_height: Height,
	) -> Result<QueryConsensusStateResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let Height { revision_height, revision_number } = consensus_height;
		let consensus_state_path = ClientConsensusStatePath {
			height: revision_height,
			epoch: revision_number,
			client_id: client_id.clone(),
		};
		let consensus_state_trie_key = TrieKey::for_consensus_state(
			ClientIdx::from_str(client_id.as_str()).unwrap(),
			consensus_height,
		);
		let (_, consensus_state_proof) = trie
			.prove(&consensus_state_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let serialized_consensus_state = storage
			.consensus_states
			.get(&(client_id.to_string(), (revision_height, revision_number)))
			.ok_or(Error::Custom("No value at given key".to_owned()))?;
		let consensus_state = Any::decode(&*borsh::to_vec(serialized_consensus_state).unwrap())?;
		Ok(QueryConsensusStateResponse {
			consensus_state: Some(consensus_state),
			proof: borsh::to_vec(&consensus_state_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_client_state(
		&self,
		at: Height,
		client_id: ClientId,
	) -> Result<QueryClientStateResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let client_state_path = ClientStatePath(client_id.clone());
		let client_state_trie_key =
			TrieKey::for_client_state(ClientIdx::from_str(client_id.as_str()).unwrap());
		let (_, client_state_proof) = trie
			.prove(&client_state_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let serialized_client_state = storage
			.clients
			.get(&(client_id.to_string()))
			.ok_or(Error::Custom("No value at given key".to_owned()))?;
		let client_state = Any::decode(&*borsh::to_vec(serialized_client_state).unwrap())?;
		Ok(QueryClientStateResponse {
			client_state: Some(client_state),
			proof: borsh::to_vec(&client_state_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_connection_end(
		&self,
		at: Height,
		connection_id: ConnectionId,
	) -> Result<QueryConnectionResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let connection_idx = ConnectionIdx::try_from(connection_id.clone()).unwrap();
		let connection_end_trie_key = TrieKey::for_connection(connection_idx);
		let (_, connection_end_proof) = trie
			.prove(&connection_end_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let serialized_connection_end = storage
			.clients
			.get(&(connection_id.to_string()))
			.ok_or(Error::Custom("No value at given key".to_owned()))?;
		let connection_end: ConnectionEnd = serde_json::from_str(&serialized_connection_end)
			.map_err(|_| Error::Custom("Could not deserialize connection end".to_owned()))?;
		Ok(QueryConnectionResponse {
			connection: Some(connection_end),
			proof: borsh::to_vec(&connection_end_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_channel_end(
		&self,
		at: Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> Result<QueryChannelResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let channel_end_path = ChannelEndsPath(port_id.clone(), channel_id.clone());
		let channel_end_trie_key = TrieKey::from(&channel_end_path);
		let (_, channel_end_proof) = trie
			.prove(&channel_end_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let serialized_channel_end = storage
			.clients
			.get(&(channel_id.to_string()))
			.ok_or(Error::Custom("No value at given key".to_owned()))?;
		let channel_end: Channel = serde_json::from_str(&serialized_channel_end)
			.map_err(|_| Error::Custom("Could not deserialize connection end".to_owned()))?;
		Ok(QueryChannelResponse {
			channel: Some(channel_end),
			proof: borsh::to_vec(&channel_end_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_proof(&self, _at: Height, keys: Vec<Vec<u8>>) -> Result<Vec<u8>, Self::Error> {
		let trie = self.get_trie().await;
		let (_, proof) = trie
			.prove(&keys[0])
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		Ok(borsh::to_vec(&proof).unwrap())
	}

	async fn query_packet_commitment(
		&self,
		at: Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		seq: u64,
	) -> Result<QueryPacketCommitmentResponse, Self::Error> {
		let trie = self.get_trie().await;
		let packet_commitment_path = CommitmentsPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: ibc::core::ics04_channel::packet::Sequence(seq),
		};
		let packet_commitment_trie_key = TrieKey::from(&packet_commitment_path);
		let (packet_commitment, packet_commitment_proof) = trie
			.prove(&packet_commitment_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let commitment = packet_commitment.ok_or(Error::Custom("No value at given key".to_owned()))?;
		Ok(QueryPacketCommitmentResponse {
			commitment: commitment.0.to_vec(),
			proof: borsh::to_vec(&packet_commitment_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_packet_acknowledgement(
		&self,
		at: Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		seq: u64,
	) -> Result<QueryPacketAcknowledgementResponse, Self::Error>
	{
		let trie = self.get_trie().await;
		let packet_ack_path = AcksPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: ibc::core::ics04_channel::packet::Sequence(seq),
		};
		let packet_ack_trie_key = TrieKey::from(&packet_ack_path);
		let (packet_ack, packet_ack_proof) = trie
			.prove(&packet_ack_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let ack = packet_ack.ok_or(Error::Custom("No value at given key".to_owned()))?;
		Ok(QueryPacketAcknowledgementResponse {
			acknowledgement: ack.0.to_vec(),
			proof: borsh::to_vec(&packet_ack_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_next_sequence_recv(
		&self,
		at: Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
	) -> Result<QueryNextSequenceReceiveResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let next_sequence_recv_path = SequencePath { port_id, channel_id };
		let next_sequence_recv_trie_key = TrieKey::from(next_sequence_recv_path);
		let (_, next_sequence_recv_proof) = trie
			.prove(&next_sequence_recv_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let next_seq = storage
			.next_sequence
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or(Error::Custom("No value at given key".to_owned()))?;
		let next_seq_recv = next_seq
			.get(SequenceTripleIdx::Recv)
			.ok_or(Error::Custom("No value set for the next sequence receive".to_owned()))?;
		Ok(QueryNextSequenceReceiveResponse {
			next_sequence_receive: next_seq_recv.into(),
			proof: borsh::to_vec(&next_sequence_recv_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn query_packet_receipt(
		&self,
		at: Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		seq: u64,
	) -> Result<QueryPacketReceiptResponse, Self::Error> {
		let trie = self.get_trie().await;
		let storage = self.get_ibc_storage();
		let packet_receipt_path = ReceiptsPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: Sequence(seq),
		};
		let packet_receipt_trie_key = TrieKey::from(&packet_receipt_path);
		let (_, packet_receipt_proof) = trie
			.prove(&packet_receipt_trie_key)
			.map_err(|_| Error::Custom("value is sealed and cannot be fetched".to_owned()))?;
		let packet_receipt_sequence = storage
			.packet_receipt_sequence_sets
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or("No value found at given key".to_owned())?;
		let packet_received = match packet_receipt_sequence.binary_search(&seq) {
			Ok(_) => true,
			Err(_) => false,
		};
		Ok(QueryPacketReceiptResponse {
			received: packet_received,
			proof: borsh::to_vec(&packet_receipt_proof).unwrap(),
			proof_height: increment_proof_height(Some(at.into())),
		})
	}

	async fn latest_height_and_timestamp(
		&self,
	) -> Result<(Height, ibc::timestamp::Timestamp), Self::Error> {
		todo!();
	}

	async fn query_packet_commitments(
		&self,
		at: Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> Result<Vec<u64>, Self::Error> {
		let storage = self.get_ibc_storage();
		let packet_commitment_sequence = storage
			.packet_commitment_sequence_sets
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or("No value found at given key".to_owned())?;
		Ok(packet_commitment_sequence.clone())
	}

	async fn query_packet_acknowledgements(
		&self,
		at: Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> Result<Vec<u64>, Self::Error> {
		let storage = self.get_ibc_storage();
		let packet_acknowledgement_sequence = storage
			.packet_acknowledgement_sequence_sets
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or("No value found at given key".to_owned())?;
		Ok(packet_acknowledgement_sequence.clone())
	}

	async fn query_unreceived_packets(
		&self,
		at: Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> Result<Vec<u64>, Self::Error> {
		let storage = self.get_ibc_storage();
		let packet_receipt_sequences = storage
			.packet_receipt_sequence_sets
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or("No value found at given key".to_owned())?;
		Ok(seqs
			.iter()
			.flat_map(|&seq| {
				match packet_receipt_sequences.iter().find(|&&receipt_seq| receipt_seq == seq) {
					Some(_) => None,
					None => Some(seq),
				}
			})
			.collect())
	}

	async fn query_unreceived_acknowledgements(
		&self,
		at: Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> Result<Vec<u64>, Self::Error> {
		let storage = self.get_ibc_storage();
		let packet_ack_sequences = storage
			.packet_acknowledgement_sequence_sets
			.get(&(port_id.to_string(), channel_id.to_string()))
			.ok_or("No value found at given key".to_owned())?;
		Ok(seqs
			.iter()
			.flat_map(|&seq| match packet_ack_sequences.iter().find(|&&ack_seq| ack_seq == seq) {
				Some(_) => None,
				None => Some(seq),
			})
			.collect())
	}

	fn channel_whitelist(
		&self,
	) -> std::collections::HashSet<(
		ibc::core::ics24_host::identifier::ChannelId,
		ibc::core::ics24_host::identifier::PortId,
	)> {
		self.channel_whitelist.lock().unwrap().clone()
	}

	async fn query_connection_channels(
		&self,
		at: Height,
		connection_id: &ConnectionId,
	) -> Result<ibc_proto::ibc::core::channel::v1::QueryChannelsResponse, Self::Error> {
		todo!()
	}

	async fn query_send_packets(
		&self,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> Result<Vec<ibc_rpc::PacketInfo>, Self::Error> {
		todo!()
	}

	async fn query_received_packets(
		&self,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> Result<Vec<ibc_rpc::PacketInfo>, Self::Error> {
		todo!()
	}

	fn expected_block_time(&self) -> Duration {
		// solana block time is roughly 400 milliseconds
		Duration::from_millis(400)
	}

	async fn query_client_update_time_and_height(
		&self,
		client_id: ClientId,
		client_height: Height,
	) -> Result<(Height, ibc::timestamp::Timestamp), Self::Error> {
		todo!()
	}

	async fn query_host_consensus_state_proof(
		&self,
		client_state: &pallet_ibc::light_clients::AnyClientState,
	) -> Result<Option<Vec<u8>>, Self::Error> {
		todo!()
	}

	async fn query_ibc_balance(
		&self,
		asset_id: Self::AssetId,
	) -> Result<Vec<ibc::applications::transfer::PrefixedCoin>, Self::Error> {
		todo!()
	}

	fn connection_prefix(&self) -> ibc::core::ics23_commitment::commitment::CommitmentPrefix {
		self.commitment_prefix.clone()
	}

	fn client_id(&self) -> ClientId {
		self.client_id.clone().expect("No client ID found")
	}

	fn set_client_id(&mut self, client_id: ClientId) {
		self.client_id = Some(client_id);
	}

	fn connection_id(&self) -> Option<ConnectionId> {
		self.connection_id.clone()
	}

	fn set_channel_whitelist(
		&mut self,
		channel_whitelist: std::collections::HashSet<(
			ibc::core::ics24_host::identifier::ChannelId,
			ibc::core::ics24_host::identifier::PortId,
		)>,
	) {
		*self.channel_whitelist.lock().unwrap() = channel_whitelist;
	}

	fn add_channel_to_whitelist(
		&mut self,
		channel: (
			ibc::core::ics24_host::identifier::ChannelId,
			ibc::core::ics24_host::identifier::PortId,
		),
	) {
		self.channel_whitelist.lock().unwrap().insert(channel);
	}

	fn set_connection_id(&mut self, connection_id: ConnectionId) {
		self.connection_id = Some(connection_id)
	}

	fn client_type(&self) -> ibc::core::ics02_client::client_state::ClientType {
		self.client_type.clone()
	}

	async fn query_timestamp_at(&self, block_number: u64) -> Result<u64, Self::Error> {
		todo!()
	}

	async fn query_clients(&self) -> Result<Vec<ClientId>, Self::Error> {
		let storage = self.get_ibc_storage();
		let client_ids: Vec<ClientId> = BTreeMap::keys(&storage.clients)
			.map(|client_id| ClientId::from_str(client_id).unwrap())
			.collect();
		Ok(client_ids)
	}

	async fn query_channels(
		&self,
	) -> Result<
		Vec<(
			ibc::core::ics24_host::identifier::ChannelId,
			ibc::core::ics24_host::identifier::PortId,
		)>,
		Self::Error,
	> {
		let storage = self.get_ibc_storage();
		let channels: Vec<(ChannelId, PortId)> = BTreeMap::keys(&storage.channel_ends)
			.map(|channel_end| {
				(
					ChannelId::from_str(&channel_end.1).unwrap(),
					PortId::from_str(&channel_end.0).unwrap(),
				)
			})
			.collect();
		Ok(channels)
	}

	async fn query_connection_using_client(
		&self,
		height: u32,
		client_id: String,
	) -> Result<Vec<ibc_proto::ibc::core::connection::v1::IdentifiedConnection>, Self::Error> {
		todo!()
	}

	async fn is_update_required(
		&self,
		latest_height: u64,
		latest_client_height_on_counterparty: u64,
	) -> Result<bool, Self::Error> {
		// we never need to use LightClientSync trait in this case, because
		// all the events will be eventually submitted via `finality_notifications`
		Ok(false)
	}

	async fn initialize_client_state(
		&self,
	) -> Result<
		(pallet_ibc::light_clients::AnyClientState, pallet_ibc::light_clients::AnyConsensusState),
		Self::Error,
	> {
		todo!()
	}

	async fn query_client_id_from_tx_hash(
		&self,
		tx_id: Self::TransactionId,
	) -> Result<ClientId, Self::Error> {
		todo!()
	}

	async fn query_connection_id_from_tx_hash(
		&self,
		tx_id: Self::TransactionId,
	) -> Result<ConnectionId, Self::Error> {
		todo!()
	}

	async fn query_channel_id_from_tx_hash(
		&self,
		tx_id: Self::TransactionId,
	) -> Result<
		(ibc::core::ics24_host::identifier::ChannelId, ibc::core::ics24_host::identifier::PortId),
		Self::Error,
	> {
		todo!()
	}

	async fn upload_wasm(&self, wasm: Vec<u8>) -> Result<Vec<u8>, Self::Error> {
		todo!()
	}
}

impl KeyProvider for Client {
	fn account_id(&self) -> ibc::signer::Signer {
		let key_entry = &self.keybase;
		let public_key = key_entry.public_key;
		ibc::signer::Signer::from_str(&public_key.to_string()).unwrap()
	}
}

#[async_trait::async_trait]
impl MisbehaviourHandler for Client {
	async fn check_for_misbehaviour<C: Chain>(
		&self,
		_counterparty: &C,
		_client_message: AnyClientMessage,
	) -> Result<(), anyhow::Error> {
		Ok(())
	}
}

#[async_trait::async_trait]
impl LightClientSync for Client {
	async fn is_synced<C: Chain>(&self, _counterparty: &C) -> Result<bool, anyhow::Error> {
		Ok(true)
	}

	async fn fetch_mandatory_updates<C: Chain>(
		&self,
		_counterparty: &C,
	) -> Result<(Vec<Any>, Vec<IbcEvent>), anyhow::Error> {
		Ok((vec![], vec![]))
	}
}

#[async_trait::async_trait]
impl Chain for Client {
	fn name(&self) -> &str {
		&self.name
	}

	fn block_max_weight(&self) -> u64 {
		self.max_tx_size as u64
	}

	async fn estimate_weight(&self, msg: Vec<Any>) -> Result<u64, Self::Error> {
		todo!()
	}

	async fn finality_notifications(
		&self,
	) -> Result<
		Pin<Box<dyn Stream<Item = <Self as IbcProvider>::FinalityEvent> + Send + Sync>>,
		Error,
	> {
		todo!()
	}

	async fn submit(&self, messages: Vec<Any>) -> Result<Self::TransactionId, Error> {
		let keypair = self.keybase.keypair();
		let authority = Rc::new(keypair);
		let program = self.program();

		// Build, sign, and send program instruction
		let solana_ibc_storage_key = self.get_ibc_storage_key();
		let trie_key = self.get_trie_key();

		let all_messages = messages
			.into_iter()
			.map(|message| AnyCheck { type_url: message.type_url, value: message.value })
			.collect();

		let sig: Signature = program
			.request()
			.accounts(accounts::LocalDeliver::new(
				authority.pubkey(),
				solana_ibc_storage_key,
				trie_key,
				system_program::ID,
			))
			.args(instructions::Deliver { messages: all_messages })
			.payer(authority.clone())
			.signer(&*authority)
			.send_with_spinner_and_config(RpcSendTransactionConfig {
				skip_preflight: true,
				..RpcSendTransactionConfig::default()
			})
			.unwrap();
		Ok(sig.to_string())
	}

	async fn query_client_message(
		&self,
		update: UpdateClient,
	) -> Result<AnyClientMessage, Self::Error> {
		todo!()
	}

	async fn get_proof_height(&self, block_height: Height) -> Height {
		block_height.increment()
	}

	async fn handle_error(&mut self, error: &anyhow::Error) -> Result<(), anyhow::Error> {
		todo!()
	}

	fn common_state(&self) -> &CommonClientState {
		&self.common_state
	}

	fn common_state_mut(&mut self) -> &mut CommonClientState {
		&mut self.common_state
	}

	async fn reconnect(&mut self) -> anyhow::Result<()> {
		todo!()
	}

	async fn on_undelivered_sequences(&self, has: bool, kind: UndeliveredType) {
		let _ = Box::pin(async move {
			let __self = self;
			let has = has;
			let kind = kind;
			let () = { __self.common_state().on_undelivered_sequences(has, kind).await };
		});
	}

	fn has_undelivered_sequences(&self, kind: UndeliveredType) -> bool {
		self.common_state().has_undelivered_sequences(kind)
	}

	fn rpc_call_delay(&self) -> Duration {
		self.common_state().rpc_call_delay()
	}

	fn initial_rpc_call_delay(&self) -> Duration {
		self.common_state().initial_rpc_call_delay
	}

	fn set_rpc_call_delay(&mut self, delay: Duration) {
		self.common_state_mut().set_rpc_call_delay(delay)
	}
}

fn increment_proof_height(
	height: Option<ibc_proto::ibc::core::client::v1::Height>,
) -> Option<ibc_proto::ibc::core::client::v1::Height> {
	height.map(|height| ibc_proto::ibc::core::client::v1::Height {
		revision_height: height.revision_height + 1,
		..height
	})
}
