use sc_distribution_rs::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = EsdtDistributionImpl::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}
