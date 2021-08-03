use cosmwasm_std::{
    log, to_binary, Api, Binary, Env, Extern, HandleResponse, HumanAddr, InitResponse, Querier,
    StdError, StdResult, Storage, Uint128, WasmMsg,
};

use crate::msg::{HandleAnswer, HandleMsg, InitMsg, QueryAnswer, QueryMsg, ResponseStatus};
use crate::state::{
    config, config_read, reward_bulks_read, updated_reward_bulks, RewardBulk, State,
};
use scrt_finance::lp_staking_msg::LPStakingHandleMsg;
use scrt_finance::types::SecretContract;
use secret_toolkit::snip20;

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
        HandleMsg::UpdateAllocation { hook, .. } => update_allocation(deps, env, hook),
        HandleMsg::Receive {
            sender,
            from,
            amount,
            msg,
        } => unimplemented!(),
        HandleMsg::ChangeAdmin { address } => change_admin(deps, env, address),
    }
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
        let reward_bulks = updated_reward_bulks(&mut deps.storage, &state)?;
        rewards = get_spy_rewards(reward_bulks, state.last_awarded_block, env.block.height);
        messages.push(snip20::send_msg(
            state.spy_to_reward.address.clone(),
            Uint128(rewards),
            None,
            None,
            1,
            state.reward_token.contract_hash.clone(),
            state.reward_token.address.clone(),
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

fn receive<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
) -> StdResult<HandleResponse> {
    unimplemented!()
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

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: QueryMsg,
) -> StdResult<Binary> {
    match msg {
        QueryMsg::Admin {} => to_binary(&query_admin(deps)?),
        QueryMsg::RewardToken {} => to_binary(&query_reward_token(deps)?),
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

fn query_pending_rewards<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    block: u64,
) -> StdResult<QueryAnswer> {
    let state = config_read(&deps.storage).load()?;
    let bulks = reward_bulks_read(&deps.storage).load()?;

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
