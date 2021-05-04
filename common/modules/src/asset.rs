elrond_wasm::imports!();
elrond_wasm::derive_imports!();

#[elrond_wasm_derive::module(AssetModuleImpl)]
pub trait AssetModule {
    fn mint_and_send(&self, address: &Address, amount: &BigUint) {
        let token_id = self.token_id().get();
        self.mint_tokens(&token_id, amount);
        self.send_tokens(&token_id, amount, address);
    }

    fn mint_tokens(&self, token_id: &TokenIdentifier, amount: &BigUint) {
        if amount > &0 {
            self.send().esdt_local_mint(
                self.blockchain().get_gas_left(),
                token_id.as_esdt_identifier(),
                amount,
            );
        }
    }

    fn send_tokens(&self, token_id: &TokenIdentifier, amount: &BigUint, address: &Address) {
        if amount > &0 {
            let _ = self.send().direct_esdt_via_transf_exec(
                address,
                token_id.as_esdt_identifier(),
                &amount,
                &[],
            );
        }
    }

    fn burn_balance(&self) {
        let token_id = self.token_id().get();
        let balance = self.blockchain().get_esdt_balance(
            &self.blockchain().get_sc_address(),
            token_id.as_esdt_identifier(),
            0,
        );
        self.burn(&token_id, &balance);
    }

    fn burn(&self, token_id: &TokenIdentifier, amount: &BigUint) {
        if amount > &0 {
            self.send().esdt_local_burn(
                self.blockchain().get_gas_left(),
                token_id.as_esdt_identifier(),
                amount,
            );
        }
    }

    #[view(getDistributedTokenId)]
    #[storage_mapper("distributed_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;
}
