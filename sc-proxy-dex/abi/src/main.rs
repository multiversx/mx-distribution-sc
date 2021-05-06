use sc_proxy_dex::*;
use elrond_wasm_debug::*;

fn main() {
	let contract = ProxyDex::new(TxContext::dummy());
	print!("{}", abi_json::contract_abi(&contract));
}