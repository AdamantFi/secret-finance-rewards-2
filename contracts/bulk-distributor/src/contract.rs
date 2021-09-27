use cosmwasm_std::{
    from_binary, log, to_binary, Api, Binary, Env, Extern, HandleResponse, HumanAddr, InitResponse,
    Querier, StdError, StdResult, Storage, Uint128, WasmMsg,
};

use crate::msg::{
    HandleAnswer, HandleMsg, InitMsg, QueryAnswer, QueryMsg, ReceiveMsg, ResponseStatus,
};
use crate::state::{
    config, config_read, updated_reward_bulks, RewardBulk, State, REWARD_BULKS_KEY,
};
use scrt_finance::lp_staking_msg::LPStakingHandleMsg;
use scrt_finance::types::SecretContract;
use secret_toolkit::snip20;
use secret_toolkit::storage::{TypedStore, TypedStoreMut};

pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    config(&mut deps.storage).save(&State {
        admin: env.message.sender,
        reward_token: msg.reward_token.clone(),
        spy_to_reward: msg.spy_to_reward,
        last_awarded_block: 0,
    })?;

    TypedStoreMut::<Vec<RewardBulk>, S>::attach(&mut deps.storage)
        .store(REWARD_BULKS_KEY, &vec![])?;

    Ok(InitResponse {
        messages: vec![snip20::register_receive_msg(
            env.contract_code_hash,
            None,
            1, // This is public data, no need to pad
            msg.reward_token.contract_hash,
            msg.reward_token.address,
        )?],
        log: vec![],
    })
}

pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse> {
    match msg {
        HandleMsg::UpdateAllocation { .. } => update_allocation(deps, env),
        HandleMsg::Receive {
            from, amount, msg, ..
        } => receive(deps, env, from, amount.u128(), msg),
        HandleMsg::ChangeAdmin { address } => change_admin(deps, env, address),
    }
}

fn update_allocation<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
) -> StdResult<HandleResponse> {
    let mut state = config_read(&deps.storage).load()?;

    let mut rewards = 0;
    let mut messages = vec![];

    if state.last_awarded_block < env.block.height {
        let reward_bulks = updated_reward_bulks(&mut deps.storage, &state)?;
        rewards = get_spy_rewards(reward_bulks, state.last_awarded_block, env.block.height);
        if rewards > 0 {
            messages.push(snip20::transfer_msg(
                state.spy_to_reward.address.clone(),
                Uint128(rewards),
                None,
                1,
                state.reward_token.contract_hash.clone(),
                state.reward_token.address.clone(),
            )?);
        }

        state.last_awarded_block = env.block.height;
        config(&mut deps.storage).save(&state)?;
    }

    messages.push(
        WasmMsg::Execute {
            contract_addr: state.spy_to_reward.address.clone(),
            callback_code_hash: state.spy_to_reward.contract_hash,
            msg: to_binary(&LPStakingHandleMsg::NotifyAllocation {
                amount: Uint128(rewards),
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

fn receive<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    from: HumanAddr,
    amount: u128,
    msg: Binary,
) -> StdResult<HandleResponse> {
    let msg: ReceiveMsg = from_binary(&msg)?;

    match msg {
        ReceiveMsg::NewBulkReward { distribute_over } => {
            new_bulk_reward(deps, env, from, distribute_over, amount)
        }
    }
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
        data: Some(to_binary(&HandleAnswer::ChangeAdmin {
            status: ResponseStatus::Success,
        })?),
    })
}

fn new_bulk_reward<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    from: HumanAddr,
    distribute_over: u64,
    amount: u128,
) -> StdResult<HandleResponse> {
    // Only admin is allowed to create reward bulks, to protect from DOS attacks
    let state = config_read(&deps.storage).load()?;
    if from != state.admin {
        return Err(StdError::unauthorized());
    }

    // Updates and notifies allocation to the reward contract. This should happen before the new
    // bulk is added to the bulk list, to not mess up the rewards calculation
    let update_allocation_resp = update_allocation(deps, env.clone());

    let new_bulk = RewardBulk {
        end_block: env.block.height + distribute_over,
        amount_per_block: amount / distribute_over as u128,
    };

    let mut bulks: Vec<RewardBulk> = TypedStore::attach(&deps.storage).load(REWARD_BULKS_KEY)?;
    bulks.push(new_bulk);
    TypedStoreMut::attach(&mut deps.storage).store(REWARD_BULKS_KEY, &bulks)?;

    update_allocation_resp
}

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: QueryMsg,
) -> StdResult<Binary> {
    match msg {
        QueryMsg::Admin {} => to_binary(&query_admin(deps)?),
        QueryMsg::RewardToken {} => to_binary(&query_reward_token(deps)?),
        QueryMsg::Spy {} => to_binary(&query_spy(deps)?),
        QueryMsg::Pending { block } => to_binary(&query_pending_rewards(deps, block)?),
    }
}

fn query_admin<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<QueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(QueryAnswer::Admin {
        address: state.admin,
    })
}

fn query_reward_token<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<QueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(QueryAnswer::RewardToken {
        contract: SecretContract {
            address: state.reward_token.address,
            contract_hash: state.reward_token.contract_hash,
        },
    })
}

fn query_spy<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<QueryAnswer> {
    let state = config_read(&deps.storage).load()?;

    Ok(QueryAnswer::Spy {
        contract: SecretContract {
            address: state.spy_to_reward.address,
            contract_hash: state.spy_to_reward.contract_hash,
        },
    })
}

fn query_pending_rewards<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    block: u64,
) -> StdResult<QueryAnswer> {
    let state = config_read(&deps.storage).load()?;
    let bulks: Vec<RewardBulk> = TypedStore::attach(&deps.storage).load(REWARD_BULKS_KEY)?;

    let amount = get_spy_rewards(bulks, state.last_awarded_block, block);

    Ok(QueryAnswer::Pending {
        amount: Uint128(amount),
    })
}

fn get_spy_rewards(bulks: Vec<RewardBulk>, last_awarded_block: u64, current_block: u64) -> u128 {
    let mut amount = 0;
    for bulk in bulks {
        if current_block < bulk.end_block {
            amount += (current_block - last_awarded_block) as u128 * bulk.amount_per_block;
        } else if last_awarded_block < bulk.end_block {
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
