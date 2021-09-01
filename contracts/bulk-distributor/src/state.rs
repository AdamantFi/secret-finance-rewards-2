use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use cosmwasm_std::{HumanAddr, StdResult, Storage};
use cosmwasm_storage::{singleton, singleton_read, ReadonlySingleton, Singleton};
use scrt_finance::types::SecretContract;
use secret_toolkit::storage::{TypedStore, TypedStoreMut};

pub static CONFIG_KEY: &[u8] = b"config";
pub static REWARD_BULKS_KEY: &[u8] = b"rewardbulks";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct State {
    pub admin: HumanAddr,
    pub reward_token: SecretContract,
    pub spy_to_reward: SecretContract,
    pub last_awarded_block: u64,
}

pub fn config<S: Storage>(storage: &mut S) -> Singleton<S, State> {
    singleton(storage, CONFIG_KEY)
}

pub fn config_read<S: Storage>(storage: &S) -> ReadonlySingleton<S, State> {
    singleton_read(storage, CONFIG_KEY)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct RewardBulk {
    pub end_block: u64,
    pub amount_per_block: u128,
}

// This function returns an updated list of reward bulks. If the list is changed, we save the new list to state
pub fn updated_reward_bulks<S: Storage>(
    storage: &mut S,
    state: &State,
) -> StdResult<Vec<RewardBulk>> {
    let bulks: Vec<RewardBulk> = TypedStore::attach(storage).load(REWARD_BULKS_KEY)?;
    let bulks_len = bulks.len();
    let updated_bulks: Vec<RewardBulk> = bulks
        .into_iter()
        .filter(|b| b.end_block > state.last_awarded_block)
        .collect();

    if updated_bulks.len() != bulks_len {
        TypedStoreMut::attach(storage).store(REWARD_BULKS_KEY, &updated_bulks)?;
    }

    Ok(updated_bulks)
}
