elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
type Epoch = u64;
pub use crate::asset::*;
pub use crate::global_op::*;

use distrib_common::*;

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct LockedTokenAttributes {
    pub unlock_milestones: Vec<UnlockMilestone>,
}

#[elrond_wasm_derive::module(LockedAssetModuleImpl)]
pub trait LockedAssetModule {
    #[module(AssetModuleImpl)]
    fn asset(&self) -> AssetModuleImpl<T, BigInt, BigUint>;

    #[module(GlobalOperationModuleImpl)]
    fn global_operation(&self) -> GlobalOperationModuleImpl<T, BigInt, BigUint>;

    fn create_and_send_multiple(
        &self,
        caller: &Address,
        asset_amounts: &[BigUint],
        unlock_milestones_vec: &[Vec<UnlockMilestone>],
    ) -> SCResult<()> {
        let locked_token_id = self.token_id().get();
        for (amount, unlock_milestones) in asset_amounts.iter().zip(unlock_milestones_vec.iter()) {
            self.create_and_send(caller, &locked_token_id, &amount, &unlock_milestones);
        }
        Ok(())
    }

    fn create_and_send(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        amount: &BigUint,
        unlock_milestones: &[UnlockMilestone],
    ) {
        if amount > &0 {
            self.create_tokens(&token_id, &amount, unlock_milestones);
            let current_nonce = self.token_nonce().get();
            self.send_tokens(&token_id, current_nonce, &amount, &caller);
        }
    }

    fn create_tokens(
        &self,
        token: &TokenIdentifier,
        amount: &BigUint,
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
            &BigUint::zero(),
            &H256::zero(),
            &attributes,
            &[BoxedBytes::empty()],
        );

        self.increase_nonce();
    }

    fn burn_tokens(&self, token: &TokenIdentifier, nonce: Nonce, amount: &BigUint) {
        self.send().esdt_nft_burn(
            self.blockchain().get_gas_left(),
            token.as_esdt_identifier(),
            nonce,
            amount,
        );
    }

    fn send_tokens(
        &self,
        token_id: &TokenIdentifier,
        nonce: Nonce,
        amount: &BigUint,
        address: &Address,
    ) {
        let _ = self.send().direct_esdt_nft_via_transfer_exec(
            address,
            token_id.as_esdt_identifier(),
            nonce,
            &amount,
            &[],
        );
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

    #[payable("*")]
    #[endpoint(unlockAssets)]
    fn unlock_assets(&self) -> SCResult<()> {
        require!(
            !self.global_operation().is_ongoing().get(),
            "Global operation is ongoing"
        );
        let (amount, token_id) = self.call_value().payment_token_pair();
        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_id == self.token_id().get(), "Bad payment token");

        let attributes = sc_try!(self.get_attributes(&token_id, token_nonce));
        let current_block_epoch = self.blockchain().get_block_epoch();
        let unlock_amount =
            self.get_unlock_amount(&amount, current_block_epoch, &attributes.unlock_milestones);
        require!(amount >= unlock_amount, "Cannot unlock more than locked");
        require!(unlock_amount > 0, "Method called too soon");

        let caller = self.blockchain().get_caller();
        self.asset().mint_and_send(&caller, &unlock_amount);

        let new_unlock_milestones =
            self.create_new_unlock_milestones(current_block_epoch, &attributes.unlock_milestones);
        let locked_remaining = amount.clone() - unlock_amount;
        self.create_and_send(
            &caller,
            &token_id,
            &locked_remaining,
            &new_unlock_milestones,
        );

        self.burn_tokens(&token_id, token_nonce, &amount);
        Ok(())
    }

    fn get_unlock_amount(
        &self,
        amount: &BigUint,
        current_epoch: Epoch,
        unlock_milestones: &[UnlockMilestone],
    ) -> BigUint {
        amount * &BigUint::from(self.get_unlock_precent(current_epoch, unlock_milestones) as u64)
            / BigUint::from(100u64)
    }

    fn get_unlock_precent(
        &self,
        current_epoch: Epoch,
        unlock_milestones: &[UnlockMilestone],
    ) -> u8 {
        let mut unlock_precent = 0u8;
        for milestone in unlock_milestones {
            if milestone.unlock_epoch < current_epoch {
                unlock_precent += milestone.unlock_precent;
            }
        }
        unlock_precent
    }

    fn create_new_unlock_milestones(
        &self,
        current_epoch: Epoch,
        old_unlock_milestones: &[UnlockMilestone],
    ) -> Vec<UnlockMilestone> {
        let mut new_unlock_milestones = Vec::<UnlockMilestone>::new();
        let unlock_precent = self.get_unlock_precent(current_epoch, old_unlock_milestones);
        let unlock_precent_remaining = 100u64 - (unlock_precent as u64);
        if unlock_precent_remaining == 0 {
            return new_unlock_milestones;
        }
        for old_milestone in old_unlock_milestones.iter() {
            if old_milestone.unlock_epoch >= current_epoch {
                let new_unlock_precent: u64 =
                    (old_milestone.unlock_precent as u64) * 100u64 / unlock_precent_remaining;
                new_unlock_milestones.push(UnlockMilestone {
                    unlock_epoch: old_milestone.unlock_epoch,
                    unlock_precent: new_unlock_precent as u8,
                });
            }
        }
        let mut sum_of_new_precents = 0u8;
        for new_milestone in new_unlock_milestones.iter() {
            sum_of_new_precents += new_milestone.unlock_precent;
        }
        new_unlock_milestones[0].unlock_precent += 100 - sum_of_new_precents;
        new_unlock_milestones
    }

    fn increase_nonce(&self) -> Nonce {
        let new_nonce = self.token_nonce().get() + 1;
        self.token_nonce().set(&new_nonce);
        new_nonce
    }

    #[view(getLockedTokenId)]
    #[storage_mapper("locked_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("locked_token_nonce")]
    fn token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;
}
