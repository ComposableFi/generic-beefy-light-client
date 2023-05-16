use std::sync::Arc;

use ethers::{abi::Abi, middleware::contract::Contract, providers::Middleware, types::H256};
use ibc::core::{
	ics04_channel::packet::Sequence,
	ics24_host::{
		path::{AcksPath, CommitmentsPath, ReceiptsPath, SeqRecvsPath},
		Path,
	},
};
use primitives::IbcProvider;

use futures::Stream;
use thiserror::Error;

use crate::client::{Client, ClientError};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Height(pub(crate) ethers::types::BlockNumber);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FinalityEvent {
	Ethereum { hash: H256 },
}

#[async_trait::async_trait]
impl IbcProvider for Client {
	type FinalityEvent = FinalityEvent;

	type TransactionId = ();

	type AssetId = ();

	type Error = ClientError;

	async fn query_latest_ibc_events<T>(
		&mut self,
		finality_event: Self::FinalityEvent,
		counterparty: &T,
	) -> Result<
		Vec<(ibc_proto::google::protobuf::Any, Vec<ibc::events::IbcEvent>, primitives::UpdateType)>,
		anyhow::Error,
	>
	where
		T: primitives::Chain,
	{
		tracing::debug!(?finality_event, "querying latest ibc events");
		tracing::warn!("TODO: implement query_latest_ibc_events");
		Ok(vec![])
	}

	fn ibc_events<'life0, 'async_trait>(
		&'life0 self,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = std::pin::Pin<
						Box<dyn Stream<Item = ibc::events::IbcEvent> + Send + 'static>,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_client_consensus<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		client_id: ibc::core::ics24_host::identifier::ClientId,
		consensus_height: ibc::Height,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::client::v1::QueryConsensusStateResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_client_state<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		client_id: ibc::core::ics24_host::identifier::ClientId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::client::v1::QueryClientStateResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_connection_end<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		connection_id: ibc::core::ics24_host::identifier::ConnectionId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::connection::v1::QueryConnectionResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_channel_end<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::channel::v1::QueryChannelResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	async fn query_proof(
		&self,
		at: ibc::Height,
		keys: Vec<Vec<u8>>,
	) -> Result<Vec<u8>, Self::Error> {
		use ibc::core::ics23_commitment::{error::Error, merkle::MerkleProof};
		use ibc_proto::ibc::core::commitment::v1::MerkleProof as RawMerkleProof;

		let rpc = self.http_rpc.clone();

		let key = String::from_utf8(keys[0].clone()).unwrap();

		let proof_result = self.eth_query_proof(&key, Some(at.revision_height)).await?;

		let bytes = proof_result
			.storage_proof
			.first()
			.map(|p| p.proof.first())
			.flatten()
			.map(|b| b.to_vec())
			.unwrap_or_default();

		Ok(bytes)
	}

	async fn query_packet_commitment(
		&self,
		at: ibc::Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		seq: u64,
	) -> Result<ibc_proto::ibc::core::channel::v1::QueryPacketCommitmentResponse, Self::Error> {
		let path = Path::Commitments(CommitmentsPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: Sequence::from(seq),
		})
		.to_string();

		let proof = self.eth_query_proof(&path, Some(at.revision_height)).await?;
		let storage = proof.storage_proof.first().unwrap();

		Ok(ibc_proto::ibc::core::channel::v1::QueryPacketCommitmentResponse {
			commitment: storage.value.as_u128().to_be_bytes().to_vec(),
			proof: storage.proof.last().map(|p| p.to_vec()).unwrap_or_default(),
			proof_height: Some(at.into()),
		})
	}

	async fn query_packet_acknowledgement(
		&self,
		at: ibc::Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		seq: u64,
	) -> Result<ibc_proto::ibc::core::channel::v1::QueryPacketAcknowledgementResponse, Self::Error>
	{
		let path = Path::Acks(AcksPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: Sequence::from(seq),
		})
		.to_string();

		let proof = self.eth_query_proof(&path, Some(at.revision_height)).await?;
		let storage = proof.storage_proof.first().unwrap();

		Ok(ibc_proto::ibc::core::channel::v1::QueryPacketAcknowledgementResponse {
			acknowledgement: storage.value.as_u128().to_be_bytes().to_vec(),
			proof: storage.proof.last().map(|p| p.to_vec()).unwrap_or_default(),
			proof_height: Some(at.into()),
		})
	}

	fn query_next_sequence_recv<'life0, 'life1, 'life2, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		port_id: &'life1 ibc::core::ics24_host::identifier::PortId,
		channel_id: &'life2 ibc::core::ics24_host::identifier::ChannelId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::channel::v1::QueryNextSequenceReceiveResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		'life1: 'async_trait,
		'life2: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	async fn query_packet_receipt(
		&self,
		at: ibc::Height,
		port_id: &ibc::core::ics24_host::identifier::PortId,
		channel_id: &ibc::core::ics24_host::identifier::ChannelId,
		sequence: u64,
	) -> Result<ibc_proto::ibc::core::channel::v1::QueryPacketReceiptResponse, Self::Error> {
		let path = Path::Receipts(ReceiptsPath {
			port_id: port_id.clone(),
			channel_id: channel_id.clone(),
			sequence: Sequence::from(sequence),
		})
		.to_string();

		let proof = self.eth_query_proof(&path, Some(at.revision_height)).await?;
		let storage = proof.storage_proof.first().unwrap();

		let received = self
			.has_packet_receipt(port_id.as_str().to_owned(), format!("{channel_id}"), sequence)
			.await?;

		Ok(ibc_proto::ibc::core::channel::v1::QueryPacketReceiptResponse {
			received,
			proof: storage.proof.last().map(|p| p.to_vec()).unwrap_or_default(),
			proof_height: Some(at.into()),
		})
	}

	fn latest_height_and_timestamp<'life0, 'async_trait>(
		&'life0 self,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<(ibc::Height, ibc::timestamp::Timestamp), Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_packet_commitments<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<u64>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_packet_acknowledgements<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<u64>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_unreceived_packets<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<u64>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_unreceived_acknowledgements<'life0, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<u64>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn channel_whitelist(
		&self,
	) -> Vec<(
		ibc::core::ics24_host::identifier::ChannelId,
		ibc::core::ics24_host::identifier::PortId,
	)> {
		self.config.channel_whitelist.clone()
	}

	fn query_connection_channels<'life0, 'life1, 'async_trait>(
		&'life0 self,
		at: ibc::Height,
		connection_id: &'life1 ibc::core::ics24_host::identifier::ConnectionId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						ibc_proto::ibc::core::channel::v1::QueryChannelsResponse,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		'life1: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_send_packets<'life0, 'async_trait>(
		&'life0 self,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<ibc_rpc::PacketInfo>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_recv_packets<'life0, 'async_trait>(
		&'life0 self,
		channel_id: ibc::core::ics24_host::identifier::ChannelId,
		port_id: ibc::core::ics24_host::identifier::PortId,
		seqs: Vec<u64>,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Vec<ibc_rpc::PacketInfo>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn expected_block_time(&self) -> std::time::Duration {
		todo!()
	}

	fn query_client_update_time_and_height<'life0, 'async_trait>(
		&'life0 self,
		client_id: ibc::core::ics24_host::identifier::ClientId,
		client_height: ibc::Height,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<(ibc::Height, ibc::timestamp::Timestamp), Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_host_consensus_state_proof<'life0, 'async_trait>(
		&'life0 self,
		height: ibc::Height,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<Option<Vec<u8>>, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_ibc_balance<'life0, 'async_trait>(
		&'life0 self,
		asset_id: Self::AssetId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<Vec<ibc::applications::transfer::PrefixedCoin>, Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn connection_prefix(&self) -> ibc::core::ics23_commitment::commitment::CommitmentPrefix {
		todo!()
	}

	fn client_id(&self) -> ibc::core::ics24_host::identifier::ClientId {
		todo!()
	}

	fn set_client_id(&mut self, client_id: ibc::core::ics24_host::identifier::ClientId) {
		todo!()
	}

	fn connection_id(&self) -> Option<ibc::core::ics24_host::identifier::ConnectionId> {
		todo!()
	}

	fn set_channel_whitelist(
		&mut self,
		channel_whitelist: Vec<(
			ibc::core::ics24_host::identifier::ChannelId,
			ibc::core::ics24_host::identifier::PortId,
		)>,
	) {
		self.config.channel_whitelist = channel_whitelist;
	}

	fn add_channel_to_whitelist(
		&mut self,
		channel: (
			ibc::core::ics24_host::identifier::ChannelId,
			ibc::core::ics24_host::identifier::PortId,
		),
	) {
		self.config.channel_whitelist.push(channel)
	}

	fn set_connection_id(
		&mut self,
		connection_id: ibc::core::ics24_host::identifier::ConnectionId,
	) {
		todo!()
	}

	fn client_type(&self) -> ibc::core::ics02_client::client_state::ClientType {
		todo!()
	}

	fn query_timestamp_at<'life0, 'async_trait>(
		&'life0 self,
		block_number: u64,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<u64, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_clients<'life0, 'async_trait>(
		&'life0 self,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<Vec<ibc::core::ics24_host::identifier::ClientId>, Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_channels<'life0, 'async_trait>(
		&'life0 self,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						Vec<(
							ibc::core::ics24_host::identifier::ChannelId,
							ibc::core::ics24_host::identifier::PortId,
						)>,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_connection_using_client<'life0, 'async_trait>(
		&'life0 self,
		height: u32,
		client_id: String,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						Vec<ibc_proto::ibc::core::connection::v1::IdentifiedConnection>,
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn is_update_required<'life0, 'async_trait>(
		&'life0 self,
		latest_height: u64,
		latest_client_height_on_counterparty: u64,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<Output = Result<bool, Self::Error>>
				+ core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn initialize_client_state<'life0, 'async_trait>(
		&'life0 self,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						(
							pallet_ibc::light_clients::AnyClientState,
							pallet_ibc::light_clients::AnyConsensusState,
						),
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_client_id_from_tx_hash<'life0, 'async_trait>(
		&'life0 self,
		tx_id: Self::TransactionId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<ibc::core::ics24_host::identifier::ClientId, Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_connection_id_from_tx_hash<'life0, 'async_trait>(
		&'life0 self,
		tx_id: Self::TransactionId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<ibc::core::ics24_host::identifier::ConnectionId, Self::Error>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}

	fn query_channel_id_from_tx_hash<'life0, 'async_trait>(
		&'life0 self,
		tx_id: Self::TransactionId,
	) -> core::pin::Pin<
		Box<
			dyn core::future::Future<
					Output = Result<
						(
							ibc::core::ics24_host::identifier::ChannelId,
							ibc::core::ics24_host::identifier::PortId,
						),
						Self::Error,
					>,
				> + core::marker::Send
				+ 'async_trait,
		>,
	>
	where
		'life0: 'async_trait,
		Self: 'async_trait,
	{
		todo!()
	}
}
