use cosmwasm_std::{
    log, to_binary, Api, Binary, Env, Extern, HandleResponse, HumanAddr, InitResponse, Querier,
    StdError, StdResult, Storage, Uint128, WasmMsg,
};

use crate::msg::{HandleAnswer, HandleMsg, InitMsg, ResponseStatus};
use crate::state::{
    config, config_read, reward_bulks, reward_bulks_read, updated_reward_bulks, RewardBulk, State,
};
use scrt_finance::lp_staking_msg::LPStakingHandleMsg;
use scrt_finance::types::{sort_schedule, Schedule, SpySettings, WeightInfo};
use secret_toolkit::snip20;
use secret_toolkit::storage::{TypedStore, TypedStoreMut};

pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    config(&mut deps.storage).save(&State {
        admin: env.message.sender,
        reward_token: msg.reward_token,
        spy_to_reward: msg.spy_to_reward,
        last_awarded_block: 0,
    })?;

    Ok(InitResponse::default())
}

pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse> {
    match msg {
        HandleMsg::UpdateAllocation { .. } => {}
        HandleMsg::Receive { .. } => {}
    }

    unimplemented!()
}

fn update_allocation<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    hook: Option<Binary>,
) -> StdResult<HandleResponse> {
    let mut state = config_read(&deps.storage).load()?;

    let mut rewards = 0;
    let mut messages = vec![];

    if state.last_awarded_block < env.block.height {
        let mut reward_bulks = updated_reward_bulks(&mut deps.storage, &state)?;
        rewards = get_spy_rewards(reward_bulks, state.last_awarded_block, env.block.height);
        messages.push(snip20::send_msg(
            state.spy_to_reward.address.clone(),
            Uint128(rewards),
            None,
            None,
            1,
            state.reward_token.contract_hash,
            state.reward_token.address,
        )?);

        state.last_awarded_block = env.block.height;
        config(&mut deps.storage).save(&state)?;
    }

    messages.push(
        WasmMsg::Execute {
            contract_addr: state.spy_to_reward.address.clone(),
            callback_code_hash: state.spy_to_reward.contract_hash,
            msg: to_binary(&LPStakingHandleMsg::NotifyAllocation {
                amount: Uint128(rewards),
                hook,
            })?,
            send: vec![],
        }
        .into(),
    );

    Ok(HandleResponse {
        messages,
        log: vec![log("update_allocation", state.spy_to_reward.address)],
        data: Some(to_binary(&HandleAnswer::UpdateAllocation {
            status: ResponseStatus::Success,
        })?),
    })
}

fn set_gov_token<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    gov_addr: HumanAddr,
    gov_hash: String,
) -> StdResult<HandleResponse> {
    let mut state = config_read(&deps.storage).load()?;

    enforce_admin(state.clone(), env)?;

    state.gov_token_addr = gov_addr.clone();
    state.gov_token_hash = gov_hash;

    config(&mut deps.storage).save(&state)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![log("set_gov_token", gov_addr.0)],
        data: Some(to_binary(&MasterHandleAnswer::Success)?),
    })
}

fn change_admin<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    admin_addr: HumanAddr,
) -> StdResult<HandleResponse> {
    let mut state = config_read(&deps.storage).load()?;

    enforce_admin(state.clone(), env)?;

    state.admin = admin_addr;

    config(&mut deps.storage).save(&state)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&MasterHandleAnswer::Success)?),
    })
}

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: MasterQueryMsg,
) -> StdResult<Binary> {
    match msg {
        MasterQueryMsg::Admin {} => to_binary(&query_admin(deps)?),
        MasterQueryMsg::GovToken {} => to_binary(&query_gov_token(deps)?),
        MasterQueryMsg::Schedule {} => to_binary(&query_schedule(deps)?),
        MasterQueryMsg::SpyWeight { addr } => to_binary(&query_spy_weight(deps, addr)?),
        MasterQueryMsg::Pending { spy_addr, block } => {
            to_binary(&query_pending_rewards(deps, spy_addr, block)?)
        }
    }
}

fn query_admin<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<MasterQueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(MasterQueryAnswer::Admin {
        address: state.admin,
    })
}

fn query_gov_token<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<MasterQueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(MasterQueryAnswer::GovToken {
        token_addr: state.gov_token_addr,
        token_hash: state.gov_token_hash,
    })
}

fn query_schedule<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<MasterQueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(MasterQueryAnswer::Schedule {
        schedule: state.minting_schedule,
    })
}

fn query_spy_weight<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    spy_address: HumanAddr,
) -> StdResult<MasterQueryAnswer> {
    let spy = TypedStore::attach(&deps.storage)
        .load(spy_address.0.as_bytes())
        .unwrap_or(SpySettings {
            weight: 0,
            last_update_block: 0,
        });

    Ok(MasterQueryAnswer::SpyWeight { weight: spy.weight })
}

fn query_pending_rewards<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    spy_addr: HumanAddr,
    block: u64,
) -> StdResult<MasterQueryAnswer> {
    let state = config_read(&deps.storage).load()?;
    let spy = TypedStore::attach(&deps.storage)
        .load(spy_addr.0.as_bytes())
        .unwrap_or(SpySettings {
            weight: 0,
            last_update_block: block,
        });

    let amount = get_spy_rewards(block, state.total_weight, &state.minting_schedule, spy);

    Ok(MasterQueryAnswer::Pending {
        amount: Uint128(amount),
    })
}

fn get_spy_rewards<S: Storage, A: Api, Q: Querier>(
    bulks: Vec<RewardBulk>,
    last_awarded_block: u64,
    current_block: u64,
) -> u128 {
    let mut amount = 0;
    for bulk in bulks {
        if current_block < bulk.end_block {
            amount += (current_block - last_awarded_block) as u128 * bulk.amount_per_block;
        } else {
            amount += (bulk.end_block - last_awarded_block) as u128 * bulk.amount_per_block;
        }
    }

    amount
}

fn enforce_admin(config: State, env: Env) -> StdResult<()> {
    if config.admin != env.message.sender {
        return Err(StdError::generic_err(format!(
            "not an admin: {}",
            env.message.sender
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env};
    use cosmwasm_std::{coins, from_binary, StdError};
}
