// Copyright (C) 2022 ComposableFi.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg_attr(not(feature = "std"), no_std)]
#![allow(clippy::all)]

//! Common utilities for light clients.

extern crate alloc;
extern crate core;

use alloc::{string::ToString, vec::Vec};
use anyhow::anyhow;
use codec::Compact;
use core::{
	fmt,
	fmt::{Debug, Display, Formatter},
	str::FromStr,
	time::Duration,
};
use ibc::{
	core::{
		ics03_connection::connection::ConnectionEnd,
		ics23_commitment::commitment::{CommitmentPrefix, CommitmentProofBytes, CommitmentRoot},
		ics24_host::Path,
		ics26_routing::context::ReaderContext,
	},
	Height,
};

use serde::{Deserialize, Serialize};
use sp_core::H256;
use sp_storage::ChildInfo;
use sp_trie::{verify_trie_proof, LayoutV0};

pub mod state_machine;

/// Host functions that allow the light client perform cryptographic operations in native.
pub trait HostFunctions: Clone + Send + Sync + Eq + Debug + Default {
	/// Blake2-256 hashing implementation
	type BlakeTwo256: hash_db::Hasher<Out = H256> + Debug + 'static;
}

/// Proof for main and child trie
#[derive(codec::Encode, codec::Decode, Serialize, Deserialize)]
pub struct IbcProof {
	child_trie_proof: Vec<Vec<u8>>,
	child_trie_root_proof: Vec<Vec<u8>>,
}

/// Membership proof verification via child trie host function
pub fn verify_membership<H, P>(
	prefix: &CommitmentPrefix,
	proof: &CommitmentProofBytes,
	root: &CommitmentRoot,
	path: P,
	value: Vec<u8>,
) -> Result<(), anyhow::Error>
where
	P: Into<Path>,
	H: hash_db::Hasher<Out = H256> + Debug + 'static,
{
	if root.as_bytes().len() != 32 {
		return Err(anyhow!("invalid commitment root length: {}", root.as_bytes().len()));
	}
	let path: Path = path.into();
	let path = path.to_string();
	let mut key = prefix.as_bytes().to_vec();
	key.extend(path.as_bytes());
	let ibc_proof: IbcProof = codec::Decode::decode(&mut &*proof.as_bytes())
		.map_err(|err| anyhow!("Failed to decode proof nodes for path: {path}: {err:#?}"))?;
	let root = H256::from_slice(root.as_bytes());
	let child_info = ChildInfo::new_default(prefix.as_bytes());
	let binding = child_info.prefixed_storage_key();
	let trie_key = binding.as_slice();
	// todo: get child root
	let child_root_proof = &ibc_proof.child_trie_root_proof;
	let child_proof = &ibc_proof.child_trie_proof;

	let child_trie_root = match verify_trie_proof::<LayoutV0<H>, _, _, _>(
		&root.into(),
		child_root_proof,
		&[(trie_key, Some(&value))],
	) {
		Ok(()) => Ok(()),
		Err(sp_trie::VerifyError::RootMismatch(child_trie_root)) => Err(Ok(child_trie_root)),
		Err(err) => Err(Err(err)),
	};

	let child_trie_root = match child_trie_root {
		Ok(()) => return Err(anyhow!("Unexpected child trie root")),
		Err(Ok(child_trie_root)) => child_trie_root,
		Err(Err(_err)) => return Err(anyhow!("Child root extraction failed")),
	};

	match verify_trie_proof::<LayoutV0<H>, _, _, _>(
		&child_trie_root.into(),
		child_proof,
		&[(key, Some(&value))],
	) {
		Ok(_) => Ok(()),
		Err(_e) => Err(anyhow!("verification failed")),
	}
}

/// Non-membership proof verification via child trie host function
pub fn verify_non_membership<H, P>(
	prefix: &CommitmentPrefix,
	proof: &CommitmentProofBytes,
	root: &CommitmentRoot,
	path: P,
) -> Result<(), anyhow::Error>
where
	P: Into<Path>,
	H: hash_db::Hasher<Out = H256> + Debug + 'static,
{
	if root.as_bytes().len() != 32 {
		return Err(anyhow!("invalid commitment root length: {}", root.as_bytes().len()));
	}
	let path: Path = path.into();
	let path = path.to_string();
	let mut key = prefix.as_bytes().to_vec();
	key.extend(path.as_bytes());
	let ibc_proof: IbcProof = codec::Decode::decode(&mut &*proof.as_bytes())
		.map_err(|err| anyhow!("Failed to decode proof nodes for path: {path}: {err:#?}"))?;
	let root = H256::from_slice(root.as_bytes());
	let child_info = ChildInfo::new_default(prefix.as_bytes());
	let binding = child_info.prefixed_storage_key();
	let trie_key = binding.as_slice();
	// todo: get child root
	let child_root_proof = &ibc_proof.child_trie_root_proof;
	let child_proof = &ibc_proof.child_trie_proof;

	let verify_child_trie_root_result = verify_trie_proof::<LayoutV0<H>, _, _, _>(
		&root.into(),
		child_root_proof,
		&[(trie_key, None::<Vec<u8>>)],
	);
	let verify_child_trie_result = verify_trie_proof::<LayoutV0<H>, _, _, _>(
		&root.into(),
		child_proof,
		&[(key, None::<Vec<u8>>)],
	);

	match (verify_child_trie_root_result, verify_child_trie_result) {
		(Ok(_), Ok(_)) => Ok(()),
		(Err(_e), _) => Err(anyhow!("child trie root verification failed")),
		(_, Err(_e)) => Err(anyhow!("child trie verification failed")),
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RelayChain {
	Polkadot = 0,
	Kusama = 1,
	Rococo = 2,
}

impl Default for RelayChain {
	fn default() -> Self {
		RelayChain::Rococo
	}
}

impl Display for RelayChain {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.as_str())
	}
}

// Unbonding period for relay chains in days
const POLKADOT_UNBONDING_PERIOD: u64 = 28;
const KUSAMA_UNBONDING_PERIOD: u64 = 7;
// number of seconds in a day
const DAY: u64 = 24 * 60 * 60;

impl RelayChain {
	/// Yields the Order as a string
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::Polkadot => "Polkadot",
			Self::Kusama => "Kusama",
			Self::Rococo => "Rococo",
		}
	}

	// Parses the Order out from a i32.
	pub fn from_i32(nr: i32) -> Result<Self, anyhow::Error> {
		match nr {
			0 => Ok(Self::Polkadot),
			1 => Ok(Self::Kusama),
			2 => Ok(Self::Rococo),
			id => Err(anyhow!("Unknown relay chain {id}")),
		}
	}

	pub fn unbonding_period(&self) -> Duration {
		match self {
			Self::Polkadot => Duration::from_secs(POLKADOT_UNBONDING_PERIOD * DAY),
			Self::Kusama | Self::Rococo => Duration::from_secs(KUSAMA_UNBONDING_PERIOD * DAY),
		}
	}

	pub fn trusting_period(&self) -> Duration {
		let unbonding_period = self.unbonding_period();
		// Trusting period is 1/3 of unbonding period
		unbonding_period.checked_div(3).unwrap()
	}
}

impl FromStr for RelayChain {
	type Err = anyhow::Error;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().trim_start_matches("order_") {
			"polkadot" => Ok(Self::Polkadot),
			"kusama" => Ok(Self::Kusama),
			"rococo" => Ok(Self::Rococo),
			_ => Err(anyhow!("Unknown relay chain {s}")),
		}
	}
}

/// Attempt to extract the timestamp extrinsic from the parachain header
pub fn decode_timestamp_extrinsic(ext: &Vec<u8>) -> Result<u64, anyhow::Error> {
	// Timestamp extrinsic should be the first inherent and hence the first extrinsic
	// https://github.com/paritytech/substrate/blob/d602397a0bbb24b5d627795b797259a44a5e29e9/primitives/trie/src/lib.rs#L99-L101
	// Decoding from the [2..] because the timestamp inherently has two extra bytes before the call
	// that represents the call length and the extrinsic version.
	let (_, _, timestamp): (u8, u8, Compact<u64>) = codec::Decode::decode(&mut &ext[2..])
		.map_err(|err| anyhow!("Failed to decode extrinsic: {err}"))?;
	Ok(timestamp.into())
}

/// This will verify that the connection delay has elapsed for a given [`ibc::Height`]
pub fn verify_delay_passed<H, C>(
	ctx: &C,
	height: Height,
	connection_end: &ConnectionEnd,
) -> Result<(), anyhow::Error>
where
	H: Clone,
	C: ReaderContext,
{
	let current_time = ctx.host_timestamp();
	let current_height = ctx.host_height();

	let client_id = connection_end.client_id();
	let processed_time = ctx.client_update_time(client_id, height).map_err(anyhow::Error::msg)?;
	let processed_height =
		ctx.client_update_height(client_id, height).map_err(anyhow::Error::msg)?;

	let delay_period_time = connection_end.delay_period();
	let delay_period_blocks = ctx.block_delay(delay_period_time);

	let earliest_time =
		(processed_time + delay_period_time).map_err(|_| anyhow!("Timestamp overflowed!"))?;
	if !(current_time == earliest_time || current_time.after(&earliest_time)) {
		return Err(anyhow!(
			"Not enough time elapsed current time: {current_time}, earliest time: {earliest_time}"
		));
	}

	let earliest_height = processed_height.add(delay_period_blocks);
	if current_height < earliest_height {
		return Err(anyhow!("Not enough blocks elapsed, current height: {current_height}, earliest height: {earliest_height}"));
	}

	Ok(())
}
