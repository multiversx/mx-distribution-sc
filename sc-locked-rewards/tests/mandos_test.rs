extern crate sc_locked_rewards;
use sc_locked_rewards::*;
use elrond_wasm::*;
use elrond_wasm_debug::*;

fn _contract_map() -> ContractMap<TxContext> {
	let mut contract_map = ContractMap::new();
	contract_map.register_contract(
		"file:../output/sc_locked_rewards.wasm",
		Box::new(|context| Box::new(lockedRewards::new(context))),
	);
	contract_map
}
