#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

pub mod proxy_common;
pub mod proxy_farm;
pub mod proxy_pair;

pub use crate::proxy_common::*;
pub use crate::proxy_farm::*;
pub use crate::proxy_pair::*;

#[derive(TopEncode, TopDecode, TypeAbi)]
pub enum IssueRequestType {
    ProxyFarm,
    ProxyPair,
}

#[elrond_wasm_derive::contract(ProxyDex)]
pub trait ProxyDexImpl {
    #[module(ProxyPairModule)]
    fn proxy_pair(&self) -> ProxyPairModule<T, BigInt, BigUint>;

    #[module(ProxyFarmModule)]
    fn proxy_farm(&self) -> ProxyFarmModule<T, BigInt, BigUint>;

    #[module(ProxyCommonModule)]
    fn common(&self) -> ProxyCommonModule<T, BigInt, BigUint>;

    #[init]
    fn init(
        &self,
        asset_token_id: TokenIdentifier,
        proxy_pair_params: ProxyPairParams,
        proxy_farm_params: ProxyFarmParams,
    ) {
        self.common().asset_token_id().set(&asset_token_id);
        self.proxy_pair().init(proxy_pair_params);
        self.proxy_farm().init(proxy_farm_params);
    }

    #[payable("EGLD")]
    #[endpoint(issueSftProxyPair)]
    fn issue_sft_proxy_pair(
        &self,
        token_display_name: BoxedBytes,
        token_ticker: BoxedBytes,
        #[payment] issue_cost: BigUint,
    ) -> SCResult<AsyncCall<BigUint>> {
        only_owner!(self, "Permission denied");
        require!(
            self.proxy_pair().token_id().is_empty(),
            "SFT already issued"
        );
        self.issue_nft(
            token_display_name,
            token_ticker,
            issue_cost,
            IssueRequestType::ProxyPair,
        )
    }

    #[payable("EGLD")]
    #[endpoint(issueSftProxyFarm)]
    fn issue_sft_proxy_farm(
        &self,
        token_display_name: BoxedBytes,
        token_ticker: BoxedBytes,
        #[payment] issue_cost: BigUint,
    ) -> SCResult<AsyncCall<BigUint>> {
        only_owner!(self, "Permission denied");
        require!(
            self.proxy_farm().token_id().is_empty(),
            "SFT already issued"
        );
        self.issue_nft(
            token_display_name,
            token_ticker,
            issue_cost,
            IssueRequestType::ProxyFarm,
        )
    }

    fn issue_nft(
        &self,
        token_display_name: BoxedBytes,
        token_ticker: BoxedBytes,
        issue_cost: BigUint,
        request_type: IssueRequestType,
    ) -> SCResult<AsyncCall<BigUint>> {
        Ok(ESDTSystemSmartContractProxy::new()
            .issue_semi_fungible(
                issue_cost,
                &token_display_name,
                &token_ticker,
                SemiFungibleTokenProperties {
                    can_add_special_roles: true,
                    can_change_owner: false,
                    can_freeze: false,
                    can_pause: false,
                    can_upgrade: true,
                    can_wipe: false,
                },
            )
            .async_call()
            .with_callback(self.callbacks().issue_nft_callback(request_type)))
    }

    #[callback]
    fn issue_nft_callback(
        &self,
        request_type: IssueRequestType,
        #[call_result] result: AsyncCallResult<TokenIdentifier>,
    ) {
        match result {
            AsyncCallResult::Ok(token_id) => match request_type {
                IssueRequestType::ProxyPair => {
                    self.proxy_pair().token_id().set(&token_id);
                }
                IssueRequestType::ProxyFarm => {
                    self.proxy_farm().token_id().set(&token_id);
                }
            },
            AsyncCallResult::Err(_) => {
                // return payment to initial caller, which can only be the owner
                let (payment, token_id) = self.call_value().payment_token_pair();
                self.send().direct(
                    &self.blockchain().get_owner_address(),
                    &token_id,
                    &payment,
                    &[],
                );
            }
        };
    }

    #[endpoint(setLocalRoles)]
    fn set_local_roles(
        &self,
        token: TokenIdentifier,
        address: Address,
        #[var_args] roles: VarArgs<EsdtLocalRole>,
    ) -> SCResult<AsyncCall<BigUint>> {
        only_owner!(self, "Permission denied");
        require!(!roles.is_empty(), "Empty roles");
        Ok(ESDTSystemSmartContractProxy::new()
            .set_special_roles(&address, token.as_esdt_identifier(), &roles.as_slice())
            .async_call())
    }
}
