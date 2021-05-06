use sc_proxy_farm::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = ProxyFarm::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}