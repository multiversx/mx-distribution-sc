elrond_wasm::imports!();
elrond_wasm::derive_imports!();

const MINT_TOKENS_GAS_LIMIT: u64 = 5000000;

#[elrond_wasm_derive::module(AssetModuleImpl)]
pub trait AssetModule {
    fn mint_and_send(&self, address: &Address, amount: &BigUint) {
        if amount > &0 {
            let token_id = self.token_id().get();
            self.send().esdt_local_mint(
                MINT_TOKENS_GAS_LIMIT,
                &token_id.as_esdt_identifier(),
                amount,
            );
            self.send().transfer_tokens(&token_id, 0, amount, address);
        }
    }

    #[view(getDistributedTokenId)]
    #[storage_mapper("distributed_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;
}
