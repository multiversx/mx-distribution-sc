#![no_std]
#![allow(non_snake_case)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

use distrib_common::*;
use modules::*;

mod locked_asset;

#[elrond_wasm_derive::contract]
pub trait LockedAssetFactory: asset::AssetModule + locked_asset::LockedAssetModule {
    #[init]
    fn init(
        &self,
        asset_token_id: TokenIdentifier,
        #[var_args] default_unlock_period: VarArgs<UnlockMilestone>,
    ) -> SCResult<()> {
        require!(!default_unlock_period.is_empty(), "Empty param");
        self.asset_token_id().set(&asset_token_id);
        self.validate_unlock_milestones(&default_unlock_period)?;
        self.default_unlock_period().set(&default_unlock_period.0);
        Ok(())
    }

    #[endpoint]
    fn whitelist(&self, address: Address) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        self.whitelisted_contracts().insert(address);
        Ok(())
    }

    #[endpoint(removeWhitelist)]
    fn remove_whitelist(&self, address: Address) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        self.whitelisted_contracts().remove(&address);
        Ok(())
    }

    #[endpoint]
    fn createAndForward(&self, amount: Self::BigUint, address: Address) -> SCResult<()> {
        let caller = self.blockchain().get_caller();
        require!(
            self.whitelisted_contracts().contains(&caller),
            "Permission denied"
        );
        require!(!self.locked_asset_token_id().is_empty(), "No SFT issued");
        require!(amount > 0, "Zero input amount");
        self.create_and_send_locked_assets(
            &amount,
            &self.create_default_unlock_milestones(),
            &address,
        );
        Ok(())
    }

    #[endpoint]
    fn createAndForwardCustomSchedule(
        &self,
        amount: Self::BigUint,
        address: Address,
        #[var_args] schedule: VarArgs<UnlockMilestone>,
    ) -> SCResult<()> {
        let caller = self.blockchain().get_caller();
        require!(
            self.whitelisted_contracts().contains(&caller),
            "Permission denied"
        );
        require!(!self.locked_asset_token_id().is_empty(), "No SFT issued");
        require!(amount > 0, "Zero input amount");
        require!(!schedule.is_empty(), "Empty param");
        self.validate_unlock_milestones(&schedule)?;
        self.create_and_send_locked_assets(&amount, &schedule.0, &address);
        Ok(())
    }

    #[payable("EGLD")]
    #[endpoint(issueNft)]
    fn issue_nft(
        &self,
        token_display_name: BoxedBytes,
        token_ticker: BoxedBytes,
        #[payment] issue_cost: Self::BigUint,
    ) -> SCResult<AsyncCall<Self::SendApi>> {
        only_owner!(self, "Permission denied");
        require!(
            self.locked_asset_token_id().is_empty(),
            "NFT already issued"
        );

        Ok(ESDTSystemSmartContractProxy::new_proxy_obj(self.send())
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
            .with_callback(self.callbacks().issue_nft_callback()))
    }

    #[callback]
    fn issue_nft_callback(&self, #[call_result] result: AsyncCallResult<TokenIdentifier>) {
        match result {
            AsyncCallResult::Ok(token_id) => {
                self.locked_asset_token_id().set(&token_id);
            }
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
    ) -> SCResult<AsyncCall<Self::SendApi>> {
        only_owner!(self, "Permission denied");
        require!(token == self.locked_asset_token_id().get(), "Bad token id");
        require!(!roles.is_empty(), "Empty roles");
        Ok(ESDTSystemSmartContractProxy::new_proxy_obj(self.send())
            .set_special_roles(&address, token.as_esdt_identifier(), &roles.as_slice())
            .async_call())
    }

    fn create_default_unlock_milestones(&self) -> Vec<UnlockMilestone> {
        let current_epoch = self.blockchain().get_block_epoch();
        self.default_unlock_period()
            .get()
            .iter()
            .map(|x| UnlockMilestone {
                unlock_epoch: x.unlock_epoch + current_epoch,
                unlock_percent: x.unlock_percent,
            })
            .collect()
    }

    #[storage_mapper("whitelist")]
    fn whitelisted_contracts(&self) -> SetMapper<Self::Storage, Address>;

    #[storage_mapper("default_unlock_period")]
    fn default_unlock_period(&self) -> SingleValueMapper<Self::Storage, Vec<UnlockMilestone>>;
}
