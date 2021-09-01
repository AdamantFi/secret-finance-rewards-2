use cosmwasm_std::{Binary, HumanAddr, Uint128};
use schemars::JsonSchema;
use scrt_finance::types::SecretContract;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InitMsg {
    pub reward_token: SecretContract,
    pub spy_to_reward: SecretContract,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandleMsg {
    UpdateAllocation {
        spy_addr: HumanAddr,
        spy_hash: String,
        hook: Option<Binary>,
    },

    // Registered commands
    Receive {
        sender: HumanAddr,
        from: HumanAddr,
        amount: Uint128,
        msg: Binary,
    },

    // Admin commands
    ChangeAdmin {
        address: HumanAddr,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HandleAnswer {
    UpdateAllocation { status: ResponseStatus },
    ChangeAdmin { status: ResponseStatus },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReceiveMsg {
    NewBulkReward { distribute_over: u64 },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, JsonSchema, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Success,
    Failure,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    Admin {},
    RewardToken {},
    Spy {},
    Pending { block: u64 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryAnswer {
    Admin { address: HumanAddr },
    RewardToken { contract: SecretContract },
    Spy { contract: SecretContract },
    Pending { amount: Uint128 },
}
