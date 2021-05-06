use sc_proxy_pair::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = ProxyPair::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}