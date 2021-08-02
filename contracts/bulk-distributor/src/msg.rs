use cosmwasm_std::{Binary, HumanAddr, Uint128};
use scrt_finance::types::SecretContract;

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
}

pub enum HandleAnswer {
    UpdateAllocation { status: ResponseStatus },
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
