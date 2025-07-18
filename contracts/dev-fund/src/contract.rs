use cosmwasm_std::{
    to_binary, Api, Binary, Env, Extern, HandleResponse, HumanAddr, InitResponse, Querier,
    StdError, StdResult, Storage, Uint128, WasmMsg,
};
use secret_toolkit::snip20;
use secret_toolkit::storage::{TypedStore, TypedStoreMut};
use secret_toolkit::utils::{pad_handle_result, pad_query_result};

use crate::constants::*;
use crate::msg::ResponseStatus::Success;
use crate::msg::{HandleAnswer, HandleMsg, HookMsg, InitMsg, QueryAnswer, QueryMsg};
use crate::querier::query_pending;
use crate::state::Config;
use scrt_finance::master_msg::MasterHandleMsg;
use scrt_finance::types::SecretContract;

pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    // Initialize state
    let mut config_store = TypedStoreMut::attach(&mut deps.storage);
    config_store.store(
        CONFIG_KEY,
        &Config {
            admin: env.message.sender.clone(),
            beneficiary: msg.beneficiary.unwrap_or(env.message.sender),
            sefi: msg.sefi.clone(),
            master: msg.master,
            viewing_key: msg.viewing_key.clone(),
            own_addr: env.contract.address,
        },
    )?;

    let messages = vec![snip20::set_viewing_key_msg(
        msg.viewing_key,
        None,
        RESPONSE_BLOCK_SIZE, // This is private data, need to pad
        msg.sefi.contract_hash,
        msg.sefi.address,
    )?];

    Ok(InitResponse {
        messages,
        log: vec![],
    })
}

pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse> {
    let response = match msg {
        HandleMsg::Redeem { amount, to } => redeem(deps, env, amount, to),
        HandleMsg::ChangeAdmin { address } => change_admin(deps, env, address),
        HandleMsg::ChangeBeneficiary { address } => change_beneficiary(deps, env, address),
        HandleMsg::NotifyAllocation { amount } => notify_allocation(deps, env, amount.u128()),
        HandleMsg::RefreshBalance {} => refresh_balance(deps, env),
        HandleMsg::SelfCallback { message } => self_callback(deps, env, message),
        HandleMsg::SetMaster { contract } => set_master(deps, env, contract),
    };

    pad_handle_result(response, RESPONSE_BLOCK_SIZE)
}

pub fn query<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: QueryMsg,
) -> StdResult<Binary> {
    let response = match msg {
        QueryMsg::Sefi {} => query_sefi(deps),
        QueryMsg::Balance { block } => query_balance(deps, block),
        QueryMsg::Admin {} => query_admin(deps),
        QueryMsg::Beneficiary {} => query_beneficiary(deps),
    };

    pad_query_result(response, RESPONSE_BLOCK_SIZE)
}

// Handle functions

fn notify_allocation<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    amount: u128,
) -> StdResult<HandleResponse> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;
    if env.message.sender != config.master.address && env.message.sender != config.admin {
        return Err(StdError::unauthorized());
    }

    let mut balance_store = TypedStoreMut::attach(&mut deps.storage);
    let mut balance: u128 = balance_store.load(ACCUMULATED_REWARDS_KEY).unwrap_or(0); // If this is called for the first time, use 0
    balance += amount;
    balance_store.store(ACCUMULATED_REWARDS_KEY, &balance)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::NotifyAllocation {
            status: Success,
        })?),
    })
}

fn self_callback<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    message: HookMsg,
) -> StdResult<HandleResponse> {
    if env.message.sender != env.contract.address {
        return Err(StdError::unauthorized());
    }

    let mut messages = vec![];
    match message {
        HookMsg::Redeem { to, amount } => {
            let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;
            let mut balance_store = TypedStoreMut::attach(&mut deps.storage);
            let mut balance: u128 = balance_store.load(ACCUMULATED_REWARDS_KEY).unwrap_or(0); // If this is called for the first time, use 0

            let amount = amount.unwrap_or(Uint128(balance)).u128();

            if amount > balance {
                return Err(StdError::generic_err(format!(
                    "insufficient funds to redeem: balance={}, required={}",
                    balance, amount,
                )));
            }

            // NOTE: If no amount was specified, we redeem everything because `amount == balance`
            balance -= amount;
            balance_store.store(ACCUMULATED_REWARDS_KEY, &balance)?;

            messages.push(secret_toolkit::snip20::transfer_msg(
                to,
                Uint128(amount),
                None,
                RESPONSE_BLOCK_SIZE,
                config.sefi.contract_hash,
                config.sefi.address,
            )?);
        }
    }

    Ok(HandleResponse {
        messages,
        log: vec![],
        data: Some(to_binary(&HandleAnswer::Redeem { status: Success })?),
    })
}

fn redeem<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    amount: Option<Uint128>,
    to: Option<HumanAddr>,
) -> StdResult<HandleResponse> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;

    if env.message.sender != config.beneficiary {
        return Err(StdError::unauthorized());
    }

    update_allocation(
        env.clone(),
        config,
        Some(HookMsg::Redeem {
            to: to.unwrap_or(env.message.sender),
            amount,
        }),
    )
}

fn change_admin<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
) -> StdResult<HandleResponse> {
    let mut config_store = TypedStoreMut::attach(&mut deps.storage);
    let mut config: Config = config_store.load(CONFIG_KEY)?;

    enforce_admin(&config, &env)?;

    config.admin = address;
    config_store.store(CONFIG_KEY, &config)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::ChangeAdmin { status: Success })?),
    })
}

fn change_beneficiary<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
) -> StdResult<HandleResponse> {
    let mut config_store = TypedStoreMut::attach(&mut deps.storage);
    let mut config: Config = config_store.load(CONFIG_KEY)?;

    enforce_admin(&config, &env)?;

    config.beneficiary = address;
    config_store.store(CONFIG_KEY, &config)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::ChangeBeneficiary {
            status: Success,
        })?),
    })
}

fn set_master<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    new_master: SecretContract,
) -> StdResult<HandleResponse> {
    let mut config_store = TypedStoreMut::attach(&mut deps.storage);
    let mut config: Config = config_store.load(CONFIG_KEY)?;

    enforce_admin(&config, &env)?;

    config.master = new_master;
    config_store.store(CONFIG_KEY, &config)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetMaster { status: Success })?),
    })
}

// This exists for an unlikely weird case where the stored balance is not correct
fn refresh_balance<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
) -> StdResult<HandleResponse> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;
    enforce_admin(&config, &env)?;

    let balance = snip20::balance_query(
        &deps.querier,
        env.contract.address,
        config.viewing_key,
        1,
        config.sefi.contract_hash,
        config.sefi.address,
    )?;
    TypedStoreMut::attach(&mut deps.storage)
        .store(ACCUMULATED_REWARDS_KEY, &balance.amount.u128())?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::RefreshBalance {
            status: Success,
        })?),
    })
}

// Query functions

fn query_balance<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    block: u64,
) -> StdResult<Binary> {
    let new_rewards = query_pending(deps, block)?;
    let balance: u128 = TypedStore::attach(&deps.storage)
        .load(ACCUMULATED_REWARDS_KEY)
        .unwrap_or(0);

    to_binary(&QueryAnswer::Balance {
        amount: Uint128(new_rewards + balance),
    })
}

fn query_sefi<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<Binary> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;

    to_binary(&QueryAnswer::Sefi { sefi: config.sefi })
}

fn query_admin<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<Binary> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;

    to_binary(&QueryAnswer::Admin {
        address: config.admin,
    })
}

fn query_beneficiary<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<Binary> {
    let config: Config = TypedStore::attach(&deps.storage).load(CONFIG_KEY)?;

    to_binary(&QueryAnswer::Beneficiary {
        address: config.beneficiary,
    })
}

// Helper functions

fn enforce_admin(config: &Config, env: &Env) -> StdResult<()> {
    if config.admin != env.message.sender {
        return Err(StdError::generic_err(format!(
            "not an admin: {}",
            env.message.sender
        )));
    }

    Ok(())
}

fn update_allocation(
    env: Env,
    config: Config,
    self_callback: Option<HookMsg>,
) -> StdResult<HandleResponse> {
    let mut messages = vec![WasmMsg::Execute {
        contract_addr: config.master.address,
        callback_code_hash: config.master.contract_hash,
        msg: to_binary(&MasterHandleMsg::UpdateAllocation {
            spy_addr: env.contract.address.clone(),
            spy_hash: env.contract_code_hash.clone(),
        })?,
        send: vec![],
    }
    .into()];

    if let Some(cb) = self_callback {
        messages.push(
            WasmMsg::Execute {
                contract_addr: env.contract.address,
                callback_code_hash: env.contract_code_hash,
                msg: to_binary(&HandleMsg::SelfCallback { message: cb })?,
                send: vec![],
            }
            .into(),
        );
    }

    Ok(HandleResponse {
        messages,
        log: vec![],
        data: None,
    })
}

#[cfg(test)]
mod tests {}
