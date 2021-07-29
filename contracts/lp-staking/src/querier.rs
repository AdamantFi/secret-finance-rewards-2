use crate::constants::CONFIG_KEY;
use crate::state::Config;
use cosmwasm_std::{
    to_binary, Api, Extern, Querier, QueryRequest, StdError, StdResult, Storage, WasmQuery,
};
use scrt_finance::master_msg::{MasterQueryAnswer, MasterQueryMsg};
use secret_toolkit::storage::TypedStore;

pub fn query_pending<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    block: u64,
) -> StdResult<u128> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;

    let mut total_amount = 0;
    for rs in config.reward_sources {
        let response = deps.querier.query(&QueryRequest::Wasm(WasmQuery::Smart {
            callback_code_hash: rs.contract_hash,
            contract_addr: rs.address.clone(),
            msg: to_binary(&MasterQueryMsg::Pending {
                spy_addr: config.own_addr.clone(),
                block,
            })?,
        }))?;

        total_amount += match response {
            MasterQueryAnswer::Pending { amount } => amount.u128(),
            _ => {
                return Err(StdError::generic_err(format!(
                    "something is wrong with the reward source: {}",
                    rs.address
                )));
            }
        }
    }

    Ok(total_amount)
}
