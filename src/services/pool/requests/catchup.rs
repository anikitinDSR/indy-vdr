use futures::stream::StreamExt;

use crate::utils::error::prelude::*;
use crate::utils::merkletree::MerkleTree;

use super::check_cons_proofs;
use super::networker::{Networker, RequestEvent, RequestTimeout, TimingResult};
use super::types::{CatchupReq, Message};

#[derive(Debug)]
pub enum CatchupRequestResult {
    Synced(
        Vec<Vec<u8>>, // new transactions
        Option<TimingResult>,
    ),
    Timeout(),
}

pub async fn perform_catchup_request<T: Networker>(
    merkle: MerkleTree,
    target_mt_root: Vec<u8>,
    target_mt_size: usize,
    networker: &T,
) -> LedgerResult<CatchupRequestResult> {
    trace!("fetch status");
    let message = build_catchup_req(&merkle, target_mt_size)?;
    let mut req = networker.create_request(&message).await?;
    let mut handler = CatchupSingleHandler::new(merkle, target_mt_root, target_mt_size);
    req.send_to_any(RequestTimeout::Ack)?;
    loop {
        match req.next().await {
            Some(RequestEvent::Received(_node_alias, message)) => {
                match message {
                    Message::CatchupRep(cr) => {
                        match handler.process_catchup_reply(cr.load_txns()?, cr.consProof.clone()) {
                            Ok(txns) => {
                                return Ok(CatchupRequestResult::Synced(txns, req.get_timing()))
                            }
                            Err(_) => {
                                req.send_to_any(RequestTimeout::Ack)?;
                            }
                        }
                    }
                    _ => {
                        // FIXME - add req.unexpected(message) to raise an appropriate exception
                        return Err(err_msg(
                            LedgerErrorKind::InvalidState,
                            "Unexpected response",
                        ));
                    }
                }
            }
            Some(RequestEvent::Timeout(_node_alias)) => {
                req.send_to_any(RequestTimeout::Ack)?;
            }
            None => {
                return Err(err_msg(
                    LedgerErrorKind::InvalidState,
                    "Request ended prematurely",
                ))
            }
        }
    }
}

#[derive(Debug)]
struct CatchupSingleHandler {
    merkle_tree: MerkleTree,
    target_mt_root: Vec<u8>,
    target_mt_size: usize,
}

impl CatchupSingleHandler {
    fn new(merkle_tree: MerkleTree, target_mt_root: Vec<u8>, target_mt_size: usize) -> Self {
        Self {
            merkle_tree,
            target_mt_root,
            target_mt_size,
        }
    }

    fn process_catchup_reply(
        &mut self,
        txns: Vec<Vec<u8>>,
        cons_proof: Vec<String>,
    ) -> LedgerResult<Vec<Vec<u8>>> {
        let mut merkle = self.merkle_tree.clone();
        for txn in &txns {
            merkle.append(txn.clone())?;
        }
        check_cons_proofs(
            &merkle,
            &cons_proof,
            &self.target_mt_root,
            self.target_mt_size,
        )?;
        Ok(txns)
    }
}

fn build_catchup_req(merkle: &MerkleTree, target_mt_size: usize) -> LedgerResult<Message> {
    if merkle.count() >= target_mt_size {
        return Err(err_msg(
            LedgerErrorKind::InvalidState,
            "No transactions to catch up",
        ));
    }
    let seq_no_start = merkle.count() + 1;
    let seq_no_end = target_mt_size;

    let cr = CatchupReq {
        ledgerId: 0,
        seqNoStart: seq_no_start,
        seqNoEnd: seq_no_end,
        catchupTill: target_mt_size,
    };
    Ok(Message::CatchupReq(cr))
}
