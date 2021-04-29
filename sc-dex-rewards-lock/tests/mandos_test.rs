extern crate sc_dex_rewards_lock;
use sc_dex_rewards_lock::*;
use elrond_wasm::*;
use elrond_wasm_debug::*;

fn _contract_map() -> ContractMap<TxContext> {
	let mut contract_map = ContractMap::new();
	contract_map.register_contract(
		"file:../output/dex-rewards-lock.wasm",
		Box::new(|context| Box::new(DexRewardsLockImpl::new(context))),
	);
	contract_map
}
