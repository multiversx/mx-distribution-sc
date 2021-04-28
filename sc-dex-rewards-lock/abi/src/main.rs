use sc_dex_rewards_lock::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = DexRewardsLockImpl::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}
