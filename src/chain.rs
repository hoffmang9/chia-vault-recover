//! Blockchain backends: coinset (default) and optional full-node RPC.

use chia_protocol::{Bytes32, Coin, CoinSpend, SpendBundle};
use chia_puzzle_types::{EveProof, LineageProof, Proof};
use chia_sdk_coinset::{ChiaRpcClient, CoinRecord, CoinsetClient};
use chia_sdk_driver::{Layer, SingletonLayer, SpendContext};

use crate::error::{Error, Result};
use crate::network::{Backend, Network};

pub struct ChainClient {
    inner: CoinsetClient,
}

impl ChainClient {
    pub fn new(network: Network, backend: &Backend) -> Self {
        let url = match backend {
            Backend::Coinset => network.coinset_url().to_string(),
            Backend::FullNode { url } => url.trim_end_matches('/').to_string(),
        };
        Self {
            inner: CoinsetClient::new(url),
        }
    }

    pub async fn find_unspent_by_puzzle_hash(&self, puzzle_hash: Bytes32) -> Result<Option<Coin>> {
        let records = self
            .inner
            .get_coin_records_by_puzzle_hash(puzzle_hash, None, None, Some(false), None)
            .await
            .map_err(|e| Error::msg(format!("get_coin_records_by_puzzle_hash failed: {e}")))?;
        Ok(records
            .coin_records
            .unwrap_or_default()
            .into_iter()
            .find(|r| !r.spent)
            .map(|r| r.coin))
    }

    pub async fn get_coin_record(&self, coin_id: Bytes32) -> Result<Option<CoinRecord>> {
        let response = self
            .inner
            .get_coin_record_by_name(coin_id)
            .await
            .map_err(|e| Error::msg(format!("get_coin_record_by_name failed: {e}")))?;
        Ok(response.coin_record)
    }

    /// Build a lineage (or eve) proof for a singleton coin from its parent spend.
    pub async fn lineage_proof_for_coin(&self, coin: &Coin) -> Result<Proof> {
        let parent_record = self
            .get_coin_record(coin.parent_coin_info)
            .await?
            .ok_or_else(|| Error::msg("parent coin record not found"))?;

        let parent_spend = self
            .inner
            .get_puzzle_and_solution(coin.parent_coin_info, Some(parent_record.spent_block_index))
            .await
            .map_err(|e| Error::msg(format!("get_puzzle_and_solution failed: {e}")))?;

        let Some(coin_solution) = parent_spend.coin_solution else {
            return Ok(Proof::Eve(EveProof {
                parent_parent_coin_info: parent_record.coin.parent_coin_info,
                parent_amount: parent_record.coin.amount,
            }));
        };

        match singleton_inner_puzzle_hash(&coin_solution.puzzle_reveal) {
            Ok(inner_puzzle_hash) => Ok(Proof::Lineage(LineageProof {
                parent_parent_coin_info: parent_record.coin.parent_coin_info,
                parent_inner_puzzle_hash: inner_puzzle_hash,
                parent_amount: parent_record.coin.amount,
            })),
            Err(_) => Ok(Proof::Eve(EveProof {
                parent_parent_coin_info: parent_record.coin.parent_coin_info,
                parent_amount: parent_record.coin.amount,
            })),
        }
    }

    pub async fn push_tx(&self, spend_bundle: &SpendBundle) -> Result<String> {
        let response = self
            .inner
            .push_tx(spend_bundle.clone())
            .await
            .map_err(|e| Error::msg(format!("push_tx failed: {e}")))?;
        if let Some(status) = response.status
            && status != "SUCCESS"
        {
            return Err(Error::msg(format!(
                "push_tx status {status}: {:?}",
                response.error
            )));
        }
        let id = spend_bundle
            .coin_spends
            .first()
            .map(|cs: &CoinSpend| hex::encode(cs.coin.coin_id()))
            .unwrap_or_default();
        Ok(id)
    }
}

fn singleton_inner_puzzle_hash(puzzle: &chia_protocol::Program) -> Result<Bytes32> {
    use chia_sdk_driver::Puzzle;

    let mut ctx = SpendContext::new();
    let ptr = ctx
        .alloc(puzzle)
        .map_err(|e| Error::msg(format!("alloc parent puzzle: {e}")))?;
    let puzzle = Puzzle::parse(&ctx, ptr);
    let layer = SingletonLayer::<clvmr::NodePtr>::parse_puzzle(&ctx, puzzle)
        .map_err(|e| Error::msg(format!("parse singleton layer: {e}")))?
        .ok_or_else(|| Error::msg("parent puzzle is not a singleton"))?;
    Ok(ctx.tree_hash(layer.inner_puzzle).into())
}
