/// This contract implements SNIP-20 standard:
/// https://github.com/SecretFoundation/SNIPs/blob/master/SNIP-20.md
use cosmwasm_std::{
    log, to_binary, Api, Binary, CanonicalAddr, CosmosMsg, Env, Extern, HandleResponse, HumanAddr,
    InitResponse, Querier, QueryResult, ReadonlyStorage, StdError, StdResult, Storage, Uint128,
    WasmMsg,
};

use crate::msg::{
    space_pad, ContractStatusLevel, HandleAnswer, HandleMsg, HookMsg, InitMsg, QueryAnswer,
    QueryMsg, ResponseStatus::Success,
};
use crate::rand::sha_256;
use crate::receiver::Snip20ReceiveMsg;
use crate::state::{
    get_receiver_hash, read_allowance, read_viewing_key, set_receiver_hash, write_allowance,
    write_viewing_key, Balances, Config, Constants, ReadonlyBalances, ReadonlyConfig,
    SecretContract, KEY_MASTER_CONTRACT, REWARD_BALANCE_KEY, SEFI_KEY,
};
use crate::transaction_history::{get_transfers, get_txs, store_burn, store_mint, store_transfer};
use crate::viewing_key::{ViewingKey, VIEWING_KEY_SIZE};
use scrt_finance::master_msg::MasterHandleMsg;
use secret_toolkit::storage::{TypedStore, TypedStoreMut};

/// We make sure that responses from `handle` are padded to a multiple of this size.
pub const RESPONSE_BLOCK_SIZE: usize = 256;

pub fn init<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: InitMsg,
) -> StdResult<InitResponse> {
    let admin = env.message.sender;
    let canon_admin = deps.api.canonical_address(&admin)?;

    let mut total_supply: u128 = 0;
    {
        let initial_balances = msg.initial_balances.unwrap_or_default();
        for balance in initial_balances {
            let balance_address = deps.api.canonical_address(&balance.address)?;
            let amount = balance.amount.u128();
            let mut balances = Balances::from_storage(&mut deps.storage);
            balances.set_account_balance(&balance_address, amount);
            if let Some(new_total_supply) = total_supply.checked_add(amount) {
                total_supply = new_total_supply;
            } else {
                return Err(StdError::generic_err(
                    "The sum of all initial balances exceeds the maximum possible total supply",
                ));
            }
            store_mint(
                &mut deps.storage,
                &canon_admin,
                &balance_address,
                balance.amount,
                "CSHBK".to_string(),
                Some("Initial Balance".to_string()),
                &env.block,
            )?;
        }
    }

    let prng_seed_hashed = sha_256(&msg.prng_seed.0);

    TypedStoreMut::<SecretContract, S>::attach(&mut deps.storage)
        .store(KEY_MASTER_CONTRACT, &msg.master)?;
    TypedStoreMut::<SecretContract, S>::attach(&mut deps.storage).store(SEFI_KEY, &msg.sefi)?;

    let mut config = Config::from_storage(&mut deps.storage);
    config.set_constants(&Constants {
        name: "Cashback Token".to_string(),
        symbol: "CSHBK".to_string(),
        decimals: 6,
        admin: admin.clone(),
        prng_seed: prng_seed_hashed.to_vec(),
        total_supply_is_public: true,
    })?;
    config.set_total_supply(total_supply);
    config.set_contract_status(ContractStatusLevel::NormalRun);
    config.set_minters(Vec::from([admin]))?;

    Ok(InitResponse::default())
}

fn pad_response(response: StdResult<HandleResponse>) -> StdResult<HandleResponse> {
    response.map(|mut response| {
        response.data = response.data.map(|mut data| {
            space_pad(RESPONSE_BLOCK_SIZE, &mut data.0);
            data
        });
        response
    })
}

pub fn handle<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    msg: HandleMsg,
) -> StdResult<HandleResponse> {
    let contract_status = ReadonlyConfig::from_storage(&deps.storage).contract_status();

    match contract_status {
        ContractStatusLevel::StopAll | ContractStatusLevel::StopAllButRedeems => {
            let response = match msg {
                HandleMsg::SetContractStatus { level, .. } => set_contract_status(deps, env, level),
                _ => Err(StdError::generic_err(
                    "This contract is stopped and this action is not allowed",
                )),
            };
            return pad_response(response);
        }
        ContractStatusLevel::NormalRun => {} // If it's a normal run just continue
    }

    let response = match msg {
        // SNIP-20
        HandleMsg::Transfer {
            recipient,
            amount,
            memo,
            ..
        } => try_transfer(deps, env, &recipient, amount, memo),
        HandleMsg::Send {
            recipient,
            amount,
            msg,
            memo,
            ..
        } => try_send(deps, env, &recipient, amount, memo, msg),
        HandleMsg::Burn { amount, memo, .. } => try_burn(deps, env, amount, memo),
        HandleMsg::RegisterReceive { code_hash, .. } => try_register_receive(deps, env, code_hash),
        HandleMsg::CreateViewingKey { entropy, .. } => try_create_key(deps, env, entropy),
        HandleMsg::SetViewingKey { key, .. } => try_set_key(deps, env, key),
        HandleMsg::IncreaseAllowance {
            spender,
            amount,
            expiration,
            ..
        } => try_increase_allowance(deps, env, spender, amount, expiration),
        HandleMsg::DecreaseAllowance {
            spender,
            amount,
            expiration,
            ..
        } => try_decrease_allowance(deps, env, spender, amount, expiration),
        HandleMsg::TransferFrom {
            owner,
            recipient,
            amount,
            memo,
            ..
        } => try_transfer_from(deps, env, &owner, &recipient, amount, memo),
        HandleMsg::SendFrom {
            owner,
            recipient,
            amount,
            msg,
            memo,
            ..
        } => try_send_from(deps, env, &owner, &recipient, amount, memo, msg),
        HandleMsg::BurnFrom {
            owner,
            amount,
            memo,
            ..
        } => try_burn_from(deps, env, &owner, amount, memo),
        HandleMsg::Mint {
            recipient,
            amount,
            memo,
            ..
        } => try_mint(deps, env, recipient, amount, memo),
        HandleMsg::AddMinters { minters, .. } => add_minters(deps, env, minters),
        HandleMsg::RemoveMinters { minters, .. } => remove_minters(deps, env, minters),
        HandleMsg::SetMinters { minters, .. } => set_minters(deps, env, minters),

        // SPY
        HandleMsg::NotifyAllocation { amount } => notify_allocation(deps, env, amount.u128()),

        // Other
        HandleMsg::ChangeAdmin { address, .. } => change_admin(deps, env, address),
        HandleMsg::SetContractStatus { level, .. } => set_contract_status(deps, env, level),
        HandleMsg::SetSefi { address, hash } => set_sefi(deps, env, address, hash),
        HandleMsg::SetMaster { address, hash } => set_master(deps, env, address, hash),
        HandleMsg::SelfCallback { message } => self_callback(deps, env, message),
    };

    pad_response(response)
}

pub fn query<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>, msg: QueryMsg) -> QueryResult {
    match msg {
        QueryMsg::TokenInfo {} => query_token_info(&deps.storage),
        QueryMsg::Minters {} => query_minters(deps),
        QueryMsg::RewardBalance {} => query_reward_balance(deps),
        _ => authenticated_queries(deps, msg),
    }
}

pub fn authenticated_queries<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    msg: QueryMsg,
) -> QueryResult {
    let (addresses, key) = msg.get_validation_params();

    for address in addresses {
        let canonical_addr = deps.api.canonical_address(address)?;

        let expected_key = read_viewing_key(&deps.storage, &canonical_addr);

        if expected_key.is_none() {
            // Checking the key will take significant time. We don't want to exit immediately if it isn't set
            // in a way which will allow to time the command and determine if a viewing key doesn't exist
            key.check_viewing_key(&[0u8; VIEWING_KEY_SIZE]);
        } else if key.check_viewing_key(expected_key.unwrap().as_slice()) {
            return match msg {
                // Base
                QueryMsg::Balance { address, .. } => query_balance(&deps, &address),
                QueryMsg::TransferHistory {
                    address,
                    page,
                    page_size,
                    ..
                } => query_transfers(&deps, &address, page.unwrap_or(0), page_size),
                QueryMsg::TransactionHistory {
                    address,
                    page,
                    page_size,
                    ..
                } => query_transactions(&deps, &address, page.unwrap_or(0), page_size),
                QueryMsg::Allowance { owner, spender, .. } => {
                    try_check_allowance(deps, owner, spender)
                }
                _ => panic!("This query type does not require authentication"),
            };
        }
    }

    Ok(to_binary(&QueryAnswer::ViewingKeyError {
        msg: "Wrong viewing key for this address or viewing key not set".to_string(),
    })?)
}

fn query_token_info<S: ReadonlyStorage>(storage: &S) -> QueryResult {
    let config = ReadonlyConfig::from_storage(storage);
    let constants = config.constants()?;

    let total_supply = if constants.total_supply_is_public {
        Some(Uint128(config.total_supply()))
    } else {
        None
    };

    to_binary(&QueryAnswer::TokenInfo {
        name: constants.name,
        symbol: constants.symbol,
        decimals: constants.decimals,
        total_supply,
    })
}

pub fn query_transfers<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    account: &HumanAddr,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    let address = deps.api.canonical_address(account)?;
    let (txs, total) = get_transfers(&deps.api, &deps.storage, &address, page, page_size)?;

    let result = QueryAnswer::TransferHistory {
        txs,
        total: Some(total),
    };
    to_binary(&result)
}

pub fn query_transactions<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    account: &HumanAddr,
    page: u32,
    page_size: u32,
) -> StdResult<Binary> {
    let address = deps.api.canonical_address(account)?;
    let (txs, total) = get_txs(&deps.api, &deps.storage, &address, page, page_size)?;

    let result = QueryAnswer::TransactionHistory {
        txs,
        total: Some(total),
    };
    to_binary(&result)
}

pub fn query_balance<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    account: &HumanAddr,
) -> StdResult<Binary> {
    let address = deps.api.canonical_address(account)?;

    let amount = Uint128(ReadonlyBalances::from_storage(&deps.storage).account_amount(&address));
    let response = QueryAnswer::Balance { amount };
    to_binary(&response)
}

fn query_minters<S: Storage, A: Api, Q: Querier>(deps: &Extern<S, A, Q>) -> StdResult<Binary> {
    let minters = ReadonlyConfig::from_storage(&deps.storage).minters();

    let response = QueryAnswer::Minters { minters };
    to_binary(&response)
}

fn query_reward_balance<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
) -> StdResult<Binary> {
    let reward_balance = TypedStore::<u128, S>::attach(&deps.storage)
        .load(REWARD_BALANCE_KEY)
        .unwrap_or(0);

    to_binary(&QueryAnswer::RewardBalance {
        balance: Uint128(reward_balance),
    })
}

fn change_admin<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);

    check_if_admin(&config, &env.message.sender)?;

    let mut consts = config.constants()?;
    consts.admin = address;
    config.set_constants(&consts)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::ChangeAdmin { status: Success })?),
    })
}

fn try_mint<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);
    let constants = config.constants()?;
    let minters = config.minters();
    if !minters.contains(&env.message.sender) {
        return Err(StdError::generic_err(
            "Minting is allowed to minter accounts only",
        ));
    }

    let raw_amount = amount.u128();

    let mut total_supply = config.total_supply();
    if let Some(new_total_supply) = total_supply.checked_add(raw_amount) {
        total_supply = new_total_supply;
    } else {
        return Err(StdError::generic_err(
            "This mint attempt would increase the total supply above the supported maximum",
        ));
    }
    config.set_total_supply(total_supply);

    let recipient_account = &deps.api.canonical_address(&address)?;

    let mut balances = Balances::from_storage(&mut deps.storage);

    let mut account_balance = balances.balance(recipient_account);

    if let Some(new_balance) = account_balance.checked_add(raw_amount) {
        account_balance = new_balance;
    } else {
        // This error literally can not happen, since the account's funds are a subset
        // of the total supply, both are stored as u128, and we check for overflow of
        // the total supply just a couple lines before.
        // Still, writing this to cover all overflows.
        return Err(StdError::generic_err(
            "This mint attempt would increase the account's balance above the supported maximum",
        ));
    }

    balances.set_account_balance(recipient_account, account_balance);

    let minter = &deps.api.canonical_address(&env.message.sender)?;

    store_mint(
        &mut deps.storage,
        minter,
        recipient_account,
        amount,
        constants.symbol,
        memo,
        &env.block,
    )?;

    let res = HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::Mint { status: Success })?),
    };
    Ok(res)
}

pub fn try_set_key<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    key: String,
) -> StdResult<HandleResponse> {
    let vk = ViewingKey(key);

    let message_sender = deps.api.canonical_address(&env.message.sender)?;
    write_viewing_key(&mut deps.storage, &message_sender, &vk);

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetViewingKey { status: Success })?),
    })
}

pub fn try_create_key<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    entropy: String,
) -> StdResult<HandleResponse> {
    let constants = ReadonlyConfig::from_storage(&deps.storage).constants()?;
    let prng_seed = constants.prng_seed;

    let key = ViewingKey::new(&env, &prng_seed, (&entropy).as_ref());

    let message_sender = deps.api.canonical_address(&env.message.sender)?;
    write_viewing_key(&mut deps.storage, &message_sender, &key);

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::CreateViewingKey { key })?),
    })
}

fn set_contract_status<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    status_level: ContractStatusLevel,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);

    check_if_admin(&config, &env.message.sender)?;

    config.set_contract_status(status_level);

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetContractStatus {
            status: Success,
        })?),
    })
}

pub fn try_check_allowance<S: Storage, A: Api, Q: Querier>(
    deps: &Extern<S, A, Q>,
    owner: HumanAddr,
    spender: HumanAddr,
) -> StdResult<Binary> {
    let owner_address = deps.api.canonical_address(&owner)?;
    let spender_address = deps.api.canonical_address(&spender)?;

    let allowance = read_allowance(&deps.storage, &owner_address, &spender_address)?;

    let response = QueryAnswer::Allowance {
        owner,
        spender,
        allowance: Uint128(allowance.amount),
        expiration: allowance.expiration,
    };
    to_binary(&response)
}

fn try_transfer_impl<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<()> {
    let sender_address = deps.api.canonical_address(&env.message.sender)?;
    let recipient_address = deps.api.canonical_address(recipient)?;

    perform_transfer(
        &mut deps.storage,
        &sender_address,
        &recipient_address,
        amount.u128(),
    )?;

    let symbol = Config::from_storage(&mut deps.storage).constants()?.symbol;

    store_transfer(
        &mut deps.storage,
        &sender_address,
        &sender_address,
        &recipient_address,
        amount,
        symbol,
        memo,
        &env.block,
    )?;

    Ok(())
}

fn try_transfer<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    try_transfer_impl(deps, env, recipient, amount, memo)?;

    let res = HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::Transfer { status: Success })?),
    };
    Ok(res)
}

fn try_add_receiver_api_callback<S: ReadonlyStorage>(
    messages: &mut Vec<CosmosMsg>,
    storage: &S,
    recipient: &HumanAddr,
    msg: Option<Binary>,
    sender: HumanAddr,
    from: HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<()> {
    let receiver_hash = get_receiver_hash(storage, recipient);
    if let Some(receiver_hash) = receiver_hash {
        let receiver_hash = receiver_hash?;
        let receiver_msg = Snip20ReceiveMsg::new(sender, from, amount, memo, msg);
        let callback_msg = receiver_msg.into_cosmos_msg(receiver_hash, recipient.clone())?;

        messages.push(callback_msg);
    }
    Ok(())
}

fn try_send<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
    msg: Option<Binary>,
) -> StdResult<HandleResponse> {
    let sender = env.message.sender.clone();
    try_transfer_impl(deps, env, recipient, amount, memo.clone())?;

    let mut messages = vec![];

    try_add_receiver_api_callback(
        &mut messages,
        &deps.storage,
        recipient,
        msg,
        sender.clone(),
        sender,
        amount,
        memo,
    )?;

    let res = HandleResponse {
        messages,
        log: vec![],
        data: Some(to_binary(&HandleAnswer::Send { status: Success })?),
    };
    Ok(res)
}

fn try_register_receive<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    code_hash: String,
) -> StdResult<HandleResponse> {
    set_receiver_hash(&mut deps.storage, &env.message.sender, code_hash);
    let res = HandleResponse {
        messages: vec![],
        log: vec![log("register_status", "success")],
        data: Some(to_binary(&HandleAnswer::RegisterReceive {
            status: Success,
        })?),
    };
    Ok(res)
}

fn insufficient_allowance(allowance: u128, required: u128) -> StdError {
    StdError::generic_err(format!(
        "insufficient allowance: allowance={}, required={}",
        allowance, required
    ))
}

fn try_transfer_from_impl<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<()> {
    let spender_address = deps.api.canonical_address(&env.message.sender)?;
    let owner_address = deps.api.canonical_address(owner)?;
    let recipient_address = deps.api.canonical_address(recipient)?;
    let amount_raw = amount.u128();

    let mut allowance = read_allowance(&deps.storage, &owner_address, &spender_address)?;

    if allowance.expiration.map(|ex| ex < env.block.time) == Some(true) {
        allowance.amount = 0;
        write_allowance(
            &mut deps.storage,
            &owner_address,
            &spender_address,
            allowance,
        )?;
        return Err(insufficient_allowance(0, amount_raw));
    }

    if let Some(new_allowance) = allowance.amount.checked_sub(amount_raw) {
        allowance.amount = new_allowance;
    } else {
        return Err(insufficient_allowance(allowance.amount, amount_raw));
    }

    write_allowance(
        &mut deps.storage,
        &owner_address,
        &spender_address,
        allowance,
    )?;
    perform_transfer(
        &mut deps.storage,
        &owner_address,
        &recipient_address,
        amount_raw,
    )?;

    let symbol = Config::from_storage(&mut deps.storage).constants()?.symbol;

    store_transfer(
        &mut deps.storage,
        &owner_address,
        &spender_address,
        &recipient_address,
        amount,
        symbol,
        memo,
        &env.block,
    )?;

    Ok(())
}

fn try_transfer_from<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    try_transfer_from_impl(deps, env, owner, recipient, amount, memo)?;

    let res = HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::TransferFrom { status: Success })?),
    };
    Ok(res)
}

fn try_send_from<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    recipient: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
    msg: Option<Binary>,
) -> StdResult<HandleResponse> {
    let sender = env.message.sender.clone();
    try_transfer_from_impl(deps, env, owner, recipient, amount, memo.clone())?;

    let mut messages = vec![];

    try_add_receiver_api_callback(
        &mut messages,
        &deps.storage,
        recipient,
        msg,
        sender,
        owner.clone(),
        amount,
        memo,
    )?;

    let res = HandleResponse {
        messages,
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SendFrom { status: Success })?),
    };
    Ok(res)
}

fn try_burn_from<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    owner: &HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    let spender_address = deps.api.canonical_address(&env.message.sender)?;
    let owner_address = deps.api.canonical_address(owner)?;
    let raw_amount = amount.u128();

    let mut allowance = read_allowance(&deps.storage, &owner_address, &spender_address)?;

    if allowance.expiration.map(|ex| ex < env.block.time) == Some(true) {
        allowance.amount = 0;
        write_allowance(
            &mut deps.storage,
            &owner_address,
            &spender_address,
            allowance,
        )?;
        return Err(insufficient_allowance(0, raw_amount));
    }

    if let Some(new_allowance) = allowance.amount.checked_sub(raw_amount) {
        allowance.amount = new_allowance;
    } else {
        return Err(insufficient_allowance(allowance.amount, raw_amount));
    }

    write_allowance(
        &mut deps.storage,
        &owner_address,
        &spender_address,
        allowance,
    )?;

    let master =
        TypedStore::<SecretContract, S>::attach(&deps.storage).load(KEY_MASTER_CONTRACT)?;
    update_allocation(
        env,
        master,
        Some(HookMsg::Burn {
            owner: owner.clone(),
            amount,
            memo,
        }),
    )
}

fn try_increase_allowance<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    spender: HumanAddr,
    amount: Uint128,
    expiration: Option<u64>,
) -> StdResult<HandleResponse> {
    let owner_address = deps.api.canonical_address(&env.message.sender)?;
    let spender_address = deps.api.canonical_address(&spender)?;

    let mut allowance = read_allowance(&deps.storage, &owner_address, &spender_address)?;
    allowance.amount = allowance.amount.saturating_add(amount.u128());
    if expiration.is_some() {
        allowance.expiration = expiration;
    }
    let new_amount = allowance.amount;
    write_allowance(
        &mut deps.storage,
        &owner_address,
        &spender_address,
        allowance,
    )?;

    let res = HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::IncreaseAllowance {
            owner: env.message.sender,
            spender,
            allowance: Uint128(new_amount),
        })?),
    };
    Ok(res)
}

fn try_decrease_allowance<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    spender: HumanAddr,
    amount: Uint128,
    expiration: Option<u64>,
) -> StdResult<HandleResponse> {
    let owner_address = deps.api.canonical_address(&env.message.sender)?;
    let spender_address = deps.api.canonical_address(&spender)?;

    let mut allowance = read_allowance(&deps.storage, &owner_address, &spender_address)?;
    allowance.amount = allowance.amount.saturating_sub(amount.u128());
    if expiration.is_some() {
        allowance.expiration = expiration;
    }
    let new_amount = allowance.amount;
    write_allowance(
        &mut deps.storage,
        &owner_address,
        &spender_address,
        allowance,
    )?;

    let res = HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::DecreaseAllowance {
            owner: env.message.sender,
            spender,
            allowance: Uint128(new_amount),
        })?),
    };
    Ok(res)
}

fn add_minters<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    minters_to_add: Vec<HumanAddr>,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);

    check_if_admin(&config, &env.message.sender)?;

    config.add_minters(minters_to_add)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::AddMinters { status: Success })?),
    })
}

fn remove_minters<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    minters_to_remove: Vec<HumanAddr>,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);

    check_if_admin(&config, &env.message.sender)?;

    config.remove_minters(minters_to_remove)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::RemoveMinters { status: Success })?),
    })
}

fn set_minters<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    minters_to_set: Vec<HumanAddr>,
) -> StdResult<HandleResponse> {
    let mut config = Config::from_storage(&mut deps.storage);

    check_if_admin(&config, &env.message.sender)?;

    config.set_minters(minters_to_set)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetMinters { status: Success })?),
    })
}

fn set_sefi<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
    hash: String,
) -> StdResult<HandleResponse> {
    let config = Config::from_storage(&mut deps.storage);
    check_if_admin(&config, &env.message.sender)?;

    TypedStoreMut::attach(&mut deps.storage).store(SEFI_KEY, &SecretContract { address, hash })?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetSefi { status: Success })?),
    })
}

fn set_master<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    address: HumanAddr,
    hash: String,
) -> StdResult<HandleResponse> {
    let config = Config::from_storage(&mut deps.storage);
    check_if_admin(&config, &env.message.sender)?;

    TypedStoreMut::attach(&mut deps.storage)
        .store(KEY_MASTER_CONTRACT, &SecretContract { address, hash })?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: Some(to_binary(&HandleAnswer::SetMaster { status: Success })?),
    })
}

/// Burn tokens
///
/// Remove `amount` tokens from the system irreversibly, from signer account
///
/// @param amount the amount of money to burn
fn try_burn<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    let master =
        TypedStore::<SecretContract, S>::attach(&deps.storage).load(KEY_MASTER_CONTRACT)?;
    let sender = env.message.sender.clone();
    update_allocation(
        env,
        master,
        Some(HookMsg::Burn {
            owner: sender,
            amount,
            memo,
        }),
    )
}

fn burn_hook<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    burner: HumanAddr,
    amount: Uint128,
    memo: Option<String>,
) -> StdResult<HandleResponse> {
    let sender_address = deps.api.canonical_address(&burner)?;
    let raw_amount = amount.u128();

    let config = Config::from_storage(&mut deps.storage);
    let constants = config.constants()?;

    let mut balances = Balances::from_storage(&mut deps.storage);
    let mut account_balance = balances.balance(&sender_address);

    if let Some(new_account_balance) = account_balance.checked_sub(raw_amount) {
        account_balance = new_account_balance;
    } else {
        return Err(StdError::generic_err(format!(
            "insufficient funds to burn: balance={}, required={}",
            account_balance, raw_amount
        )));
    }

    balances.set_account_balance(&sender_address, account_balance);

    let mut total_supply = Config::from_storage(&mut deps.storage).total_supply();

    // Create a message to transfer eligible SEFI
    let messages = transfer_reward(deps, burner, raw_amount, total_supply)?;

    if let Some(new_total_supply) = total_supply.checked_sub(raw_amount) {
        total_supply = new_total_supply;
    } else {
        return Err(StdError::generic_err(
            "You're trying to burn more than is available in the total supply",
        ));
    }
    Config::from_storage(&mut deps.storage).set_total_supply(total_supply);

    store_burn(
        &mut deps.storage,
        &sender_address,
        &sender_address,
        amount,
        constants.symbol,
        memo,
        &env.block,
    )?;

    let res = HandleResponse {
        messages,
        log: vec![],
        data: Some(to_binary(&HandleAnswer::Burn { status: Success })?),
    };

    Ok(res)
}

fn notify_allocation<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    env: Env,
    amount: u128,
) -> StdResult<HandleResponse> {
    let master: SecretContract = TypedStore::attach(&deps.storage).load(KEY_MASTER_CONTRACT)?;
    let constants = ReadonlyConfig::from_storage(&deps.storage).constants()?;
    if env.message.sender != master.address && env.message.sender != constants.admin {
        return Err(StdError::unauthorized());
    }

    let mut reward_balance = TypedStoreMut::<u128, S>::attach(&mut deps.storage)
        .load(REWARD_BALANCE_KEY)
        .unwrap_or(0);

    reward_balance += amount;

    TypedStoreMut::<u128, S>::attach(&mut deps.storage)
        .store(REWARD_BALANCE_KEY, &reward_balance)?;

    Ok(HandleResponse {
        messages: vec![],
        log: vec![],
        data: None,
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

    match message {
        HookMsg::Burn {
            owner,
            amount,
            memo,
        } => burn_hook(deps, env, owner, amount, memo),
    }
}

fn perform_transfer<T: Storage>(
    store: &mut T,
    from: &CanonicalAddr,
    to: &CanonicalAddr,
    amount: u128,
) -> StdResult<()> {
    let mut balances = Balances::from_storage(store);

    let mut from_balance = balances.balance(from);
    if let Some(new_from_balance) = from_balance.checked_sub(amount) {
        from_balance = new_from_balance;
    } else {
        return Err(StdError::generic_err(format!(
            "insufficient funds: balance={}, required={}",
            from_balance, amount
        )));
    }
    balances.set_account_balance(from, from_balance);

    let mut to_balance = balances.balance(to);
    to_balance = to_balance.checked_add(amount).ok_or_else(|| {
        StdError::generic_err("This tx will literally make them too rich. Try transferring less")
    })?;
    balances.set_account_balance(to, to_balance);

    Ok(())
}

fn is_admin<S: Storage>(config: &Config<S>, account: &HumanAddr) -> StdResult<bool> {
    let consts = config.constants()?;
    if &consts.admin != account {
        return Ok(false);
    }

    Ok(true)
}

fn check_if_admin<S: Storage>(config: &Config<S>, account: &HumanAddr) -> StdResult<()> {
    if !is_admin(config, account)? {
        return Err(StdError::generic_err(
            "This is an admin command. Admin commands can only be run from admin address",
        ));
    }

    Ok(())
}

fn update_allocation(
    env: Env,
    master: SecretContract,
    self_callback: Option<HookMsg>,
) -> StdResult<HandleResponse> {
    let mut messages = vec![WasmMsg::Execute {
        contract_addr: master.address,
        callback_code_hash: master.hash,
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

fn transfer_reward<S: Storage, A: Api, Q: Querier>(
    deps: &mut Extern<S, A, Q>,
    burner: HumanAddr,
    burn_amount: u128,
    total_supply: u128,
) -> StdResult<Vec<CosmosMsg>> {
    let reward_balance = TypedStore::<u128, S>::attach(&deps.storage)
        .load(REWARD_BALANCE_KEY)
        .unwrap_or(0);
    let sefi = TypedStore::<SecretContract, S>::attach(&deps.storage)
        .load(SEFI_KEY)
        .unwrap();
    let reward = burn_amount * reward_balance / total_supply;

    if reward > 0 {
        let new_balance = reward_balance - reward;
        TypedStoreMut::<u128, S>::attach(&mut deps.storage)
            .store(REWARD_BALANCE_KEY, &new_balance)?;

        Ok(vec![secret_toolkit::snip20::transfer_msg(
            burner,
            Uint128(reward),
            None,
            1,
            sefi.hash,
            sefi.address,
        )?])
    } else {
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::ResponseStatus;
    use crate::msg::{InitConfig, InitialBalance};
    use cosmwasm_std::testing::*;
    use cosmwasm_std::{from_binary, BlockInfo, ContractInfo, MessageInfo, QueryResponse, WasmMsg};
    use std::any::Any;
}
