#!/bin/bash

set -e

function wait_for_tx() {
  until (secretcli q tx "$1"); do
      sleep 5
  done
}

export wasm_path=build

export revision="11"
export deployer_name=test
export viewing_key="123"
export gov_addr="secret12q2c5s5we5zn9pq43l0rlsygtql6646my0sqfm"
export token_code_hash="c7fe67b243dfedc625a28ada303434d6f5a46a3086e7d2b5063a814e9f9a379d"
export master_addr="secret13hqxweum28nj0c53nnvrpd23ygguhteqggf852"
export master_code_hash="c8555c2de49967ca484ba21cf563c2b27227a39ad6f32ff3de9758f20159d2d2"
export inc_token="secret1gh6f0gxn20ckjxhwgkq3xeve3aq4l53wkyfyen"
export inc_token_hash="ea3df9d5e17246e4ef2f2e8071c91299852a07a84c4eb85007476338b7547ce8"
#export staking_addr="secret1uk6cmegnfvc7q0wa20ex7fpx76hgsy9guma4t9"
#export staking_hash='"c2c1aabec6308b1639af67e075175f7ac4080c7dddd9ba52ea26851f5b85b2c7"'


echo "Storing Staking Contract"
resp=$(secretcli tx compute store "${wasm_path}/lp_staking.wasm" --from "$deployer_name" --gas 3000000 -b block -y)
echo $resp
staking_code_id=$(echo $resp | jq -r '.logs[0].events[0].attributes[] | select(.key == "code_id") | .value')
staking_hash=$(secretcli q compute list-code | jq '.[] | select(.id == '"$staking_code_id"') | .data_hash')
echo "Stored lp staking: '$staking_code_id', '$staking_hash'"

echo "Storing Bulk Distributor"
resp=$(secretcli tx compute store "${wasm_path}/bulk_distributor.wasm" --from "$deployer_name" --gas 3000000 -b block -y)
echo $resp
bulk_code_id=$(echo $resp | jq -r '.logs[0].events[0].attributes[] | select(.key == "code_id") | .value')
bulk_hash=$(secretcli q compute list-code | jq '.[] | select(.id == '"$bulk_code_id"') | .data_hash')
echo "Stored bulk distributor: '$bulk_code_id', '$bulk_hash'"

echo "Deploying Staking Contract.."
export TX_HASH=$(
  secretcli tx compute instantiate $staking_code_id '{"reward_token":{"address":"'"$gov_addr"'", "contract_hash":"'"$token_code_hash"'"},"inc_token":{"address":"'"$inc_token"'", "contract_hash":"'"$inc_token_hash"'"},"reward_sources":[{"address":"'"$master_addr"'", "contract_hash":"'"$master_code_hash"'"}],"viewing_key":"'"$viewing_key"'","token_info":{"name":"bulk-rewards","symbol":"BULKRWRDS"},"prng_seed":"YWE="}' --from $deployer_name --gas 1500000 --label bulk-stake-$revision -b block -y |
  jq -r .txhash
)
wait_for_tx "$TX_HASH" "Waiting for tx to finish on-chain..."
secretcli q compute tx $TX_HASH
staking_addr=$(secretcli query compute list-contract-by-code $staking_code_id | jq -r '.[-1].address')

echo "Setting SEFI Staking weight.."
export TX_HASH=$(
  secretcli tx compute execute "$master_addr" '{"set_weights":{"weights":[{"address":"'"$staking_addr"'","hash":'"$staking_hash"',"weight":500}]}}' --from $deployer_name --gas 1500000 -b block -y |
  jq -r .txhash
)
wait_for_tx "$TX_HASH" "Waiting for tx to finish on-chain..."
secretcli q compute tx $TX_HASH

echo "Deploying Bulk Distributor.."
export TX_HASH=$(
secretcli tx compute instantiate $bulk_code_id '{"reward_token":{"address":"'"$gov_addr"'","contract_hash":"'"$token_code_hash"'"},"spy_to_reward":{"address":"'"$staking_addr"'","contract_hash":'"$staking_hash"'}}' --from $deployer_name --gas 1500000 --label bulk-distr-$revision -b block -y |
  jq -r .txhash
)
wait_for_tx "$TX_HASH" "Waiting for tx to finish on-chain..."
secretcli q compute tx $TX_HASH
bulk_addr=$(secretcli query compute list-contract-by-code $bulk_code_id | jq -r '.[-1].address')
echo "Bulk Distributor address: '$bulk_addr'"

echo "Set bulk distributor as a reward source.."
export TX_HASH=$(
  secretcli tx compute execute $staking_addr '{"add_reward_sources":{"contracts":[{"address":"'"$bulk_addr"'","contract_hash":'"$bulk_hash"'}]}}' --from $deployer_name --gas 1500000 -b block -y |
  jq -r .txhash
)
wait_for_tx "$TX_HASH" "Waiting for tx to finish on-chain..."
secretcli q compute tx $TX_HASH

echo "Addresses:"
echo "Staking contract: ""$staking_addr"
echo "Bulk distributor: ""$bulk_addr"
