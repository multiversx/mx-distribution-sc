use sc_distribution_rs::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = EsdtDistribution::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}