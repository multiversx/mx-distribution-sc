#![allow(non_snake_case)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
type Epoch = u64;
use super::asset;

use distrib_common::*;
use elrond_wasm::{require, sc_error};

const BURN_TOKENS_GAS_LIMIT: u64 = 5000000;

#[elrond_wasm_derive::module]
pub trait LockedAssetModule: asset::AssetModuleImpl {
    fn create_and_send_multiple_locked_assets(
        &self,
        caller: &Address,
        asset_amounts: &[Self::BigUint],
        unlock_milestones_vec: &[Vec<UnlockMilestone>],
    ) -> SCResult<()> {
        let locked_token_id = self.locked_asset_token_id().get();
        for (amount, unlock_milestones) in asset_amounts.iter().zip(unlock_milestones_vec.iter()) {
            self.create_and_send_locked_assets(
                caller,
                &locked_token_id,
                &amount,
                &unlock_milestones,
            );
        }
        Ok(())
    }

    fn create_and_send_locked_assets(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        amount: &Self::BigUint,
        unlock_milestones: &[UnlockMilestone],
    ) {
        if amount > &0 {
            self.create_tokens(&token_id, &amount, unlock_milestones);
            let last_created_nonce = self.locked_asset_token_nonce().get();
            self.send()
                .transfer_tokens(&token_id, last_created_nonce, &amount, &caller);
        }
    }

    fn create_tokens(
        &self,
        token: &TokenIdentifier,
        amount: &Self::BigUint,
        unlock_milestones: &[UnlockMilestone],
    ) {
        let attributes = LockedTokenAttributes {
            unlock_milestones: unlock_milestones.to_vec(),
        };
        self.send().esdt_nft_create::<LockedTokenAttributes>(
            self.blockchain().get_gas_left(),
            token.as_esdt_identifier(),
            amount,
            &BoxedBytes::empty(),
            &Self::BigUint::zero(),
            &H256::zero(),
            &attributes,
            &[BoxedBytes::empty()],
        );
        self.increase_nonce();
    }

    fn get_attributes(
        &self,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
    ) -> SCResult<LockedTokenAttributes> {
        let token_info = self.blockchain().get_esdt_token_data(
            &self.blockchain().get_sc_address(),
            token_id.as_esdt_identifier(),
            token_nonce,
        );

        let attributes = token_info.decode_attributes::<LockedTokenAttributes>();
        match attributes {
            Result::Ok(decoded_obj) => Ok(decoded_obj),
            Result::Err(_) => {
                return sc_error!("Decoding error");
            }
        }
    }

    fn unlock_assets(&self) -> SCResult<()> {
        let (amount, token_id) = self.call_value().payment_token_pair();
        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_id == self.locked_asset_token_id().get(), "Bad payment token");

        let attributes = self.get_attributes(&token_id, token_nonce)?;
        let current_block_epoch = self.blockchain().get_block_epoch();
        let unlock_amount =
            self.get_unlock_amount(&amount, current_block_epoch, &attributes.unlock_milestones);
        require!(amount >= unlock_amount, "Cannot unlock more than locked");
        require!(unlock_amount > 0, "Method called too soon");

        let caller = self.blockchain().get_caller();
        self.mint_and_send_assets(&caller, &unlock_amount);

        let new_unlock_milestones =
            self.create_new_unlock_milestones(current_block_epoch, &attributes.unlock_milestones);
        let locked_remaining = amount.clone() - unlock_amount;
        self.create_and_send_locked_assets(
            &caller,
            &token_id,
            &locked_remaining,
            &new_unlock_milestones,
        );

        self.send()
            .burn_tokens(&token_id, token_nonce, &amount, BURN_TOKENS_GAS_LIMIT);
        Ok(())
    }

    fn get_unlock_amount(
        &self,
        amount: &Self::BigUint,
        current_epoch: Epoch,
        unlock_milestones: &[UnlockMilestone],
    ) -> Self::BigUint {
        amount
            * &Self::BigUint::from(self.get_unlock_percent(current_epoch, unlock_milestones) as u64)
            / Self::BigUint::from(100u64)
    }

    fn get_unlock_percent(
        &self,
        current_epoch: Epoch,
        unlock_milestones: &[UnlockMilestone],
    ) -> u8 {
        let mut unlock_percent = 0u8;
        for milestone in unlock_milestones {
            if milestone.unlock_epoch < current_epoch {
                unlock_percent += milestone.unlock_percent;
            }
        }
        unlock_percent
    }

    fn create_new_unlock_milestones(
        &self,
        current_epoch: Epoch,
        old_unlock_milestones: &[UnlockMilestone],
    ) -> Vec<UnlockMilestone> {
        let mut new_unlock_milestones = Vec::<UnlockMilestone>::new();
        let unlock_percent = self.get_unlock_percent(current_epoch, old_unlock_milestones);
        let unlock_percent_remaining = 100u64 - (unlock_percent as u64);
        if unlock_percent_remaining == 0 {
            return new_unlock_milestones;
        }
        for old_milestone in old_unlock_milestones.iter() {
            if old_milestone.unlock_epoch >= current_epoch {
                let new_unlock_percent: u64 =
                    (old_milestone.unlock_percent as u64) * 100u64 / unlock_percent_remaining;
                new_unlock_milestones.push(UnlockMilestone {
                    unlock_epoch: old_milestone.unlock_epoch,
                    unlock_percent: new_unlock_percent as u8,
                });
            }
        }
        let mut sum_of_new_percents = 0u8;
        for new_milestone in new_unlock_milestones.iter() {
            sum_of_new_percents += new_milestone.unlock_percent;
        }
        new_unlock_milestones[0].unlock_percent += 100 - sum_of_new_percents;
        new_unlock_milestones
    }

    fn increase_nonce(&self) -> Nonce {
        let new_nonce = self.locked_asset_token_nonce().get() + 1;
        self.locked_asset_token_nonce().set(&new_nonce);
        new_nonce
    }

    #[storage_mapper("locked_token_id")]
    fn locked_asset_token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("locked_token_nonce")]
    fn locked_asset_token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;
}
