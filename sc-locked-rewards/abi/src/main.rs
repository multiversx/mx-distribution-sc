use sc_locked_rewards::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = LockedRewards::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}