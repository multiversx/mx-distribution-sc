#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

use core::iter::FromIterator;
use distrib_common::*;
use modules::*;

const PERCENTAGE_TOTAL: u32 = 100;
const BURN_TOKENS_GAS_LIMIT: u64 = 5000000;
#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct LockRewardOption<BigUint: BigUintApi> {
    num_epochs_to_lock: u64,
    interest: BigUint,
}

#[elrond_wasm_derive::contract]
pub trait LockedRewardsImpl: asset::AssetModuleImpl + locked_asset::LockedAssetModule {
    /// Epoch refers to duration in epochs, not a specific deadline
    #[init]
    fn init(
        &self,
        mex_token_id: TokenIdentifier,
        #[var_args] epoch_reward_percentage_pairs: VarArgs<MultiArg2<u64, Self::BigUint>>,
    ) -> SCResult<()> {
        require!(
            mex_token_id.is_valid_esdt_identifier(),
            "Invalid token provided"
        );
        require!(
            !epoch_reward_percentage_pairs.is_empty(),
            "Must provide at least one epoch-reward pair"
        );

        self.asset_token_id().set(&mex_token_id);

        for pair in epoch_reward_percentage_pairs.into_vec() {
            let (epoch, percentage) = pair.into_tuple();
            self.epoch_rewards_map().insert(epoch, percentage);
        }

        Ok(())
    }

    // endpoints - owner-only

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

    #[endpoint(addEpochReward)]
    fn add_epoch_reward(&self, epoch: u64, percentage: Self::BigUint) -> SCResult<()> {
        self.require_caller_owner()?;
        require!(
            !self.epoch_rewards_map().contains_key(&epoch),
            "There is already a reward set for that epoch"
        );

        self.epoch_rewards_map().insert(epoch, percentage);

        Ok(())
    }

    #[endpoint(removeEpochReward)]
    fn remove_epoch_reward(&self, epoch: u64) -> SCResult<()> {
        self.require_caller_owner()?;
        require!(
            self.epoch_rewards_map().contains_key(&epoch),
            "There is no reward set for that epoch"
        );

        self.epoch_rewards_map().remove(&epoch);

        Ok(())
    }

    // endpoints

    #[payable("*")]
    #[endpoint(lockMexTokens)]
    fn lock_mex_tokens(
        &self,
        epochs_lock_time: u64,
        #[payment_token] token_id: TokenIdentifier,
        #[payment] amount: Self::BigUint,
    ) -> SCResult<()> {
        self.require_nft_issued()?;
        require!(
            token_id == self.asset_token_id().get(),
            "Wrong token sent as payment"
        );
        require!(amount > 0, "Must lock more than 0 tokens");

        let caller = self.blockchain().get_caller();
        let latest_reward_epoch = self.find_latest_reward_epoch(epochs_lock_time);
        let percentage_reward = match self.epoch_rewards_map().get(&latest_reward_epoch) {
            Some(percentage) => percentage,
            None => return sc_error!("Couldn't find percentage reward"),
        };

        let bonus_amount = &amount * &percentage_reward / Self::BigUint::from(PERCENTAGE_TOTAL);
        let nft_amount = &amount + &bonus_amount;
        let unlock_epoch = self.blockchain().get_block_epoch() + epochs_lock_time;

        // send locked tokens as NFTs to caller
        self.create_and_send_locked_assets(
            &caller,
            &self.locked_asset_token_id().get(),
            &nft_amount,
            &[UnlockMilestone {
                unlock_epoch,
                unlock_percent: PERCENTAGE_TOTAL as u8,
            }],
        );

        // burn received MEX tokens
        self.send().burn_tokens(
            &self.asset_token_id().get(),
            0,
            &amount,
            BURN_TOKENS_GAS_LIMIT,
        );

        Ok(())
    }

    #[payable("*")]
    #[endpoint(unlockAssets)]
    fn unlock_assets_endpoint(&self) -> SCResult<()> {
        self.unlock_assets()
    }

    // views

    #[view(getInterestForEpochsWaited)]
    fn get_interest_for_epochs_waited(&self, epochs_waited: u64) -> Self::BigUint {
        match self.epoch_rewards_map().get(&epochs_waited) {
            Some(percentage) => percentage,
            None => Self::BigUint::zero(),
        }
    }

    #[view(computeEarnings)]
    fn calculate_earnings(
        &self,
        deposit_amount: Self::BigUint,
        epochs_waited: u64,
    ) -> Self::BigUint {
        let latest_reward_epoch = self.find_latest_reward_epoch(epochs_waited);
        let interest = self.get_interest_for_epochs_waited(latest_reward_epoch);

        deposit_amount * interest / Self::BigUint::from(PERCENTAGE_TOTAL)
    }

    #[view(getAllLockRewardOptions)]
    fn get_all_lock_reward_options(&self) -> MultiResultVec<LockRewardOption<Self::BigUint>> {
        let map: Vec<LockRewardOption<Self::BigUint>> = self
            .epoch_rewards_map()
            .iter()
            .map(|x| LockRewardOption {
                num_epochs_to_lock: x.0,
                interest: x.1,
            })
            .collect();
        MultiResultVec::from_iter(map)
    }

    // private

    fn require_caller_owner(&self) -> SCResult<()> {
        only_owner!(self, "Only owner may call this function");
        Ok(())
    }

    fn require_nft_issued(&self) -> SCResult<()> {
        require!(
            !self.locked_asset_token_id().is_empty(),
            "Nft not issued yet"
        );
        Ok(())
    }

    fn find_latest_reward_epoch(&self, epochs_waited: u64) -> u64 {
        let mut latest_valid_epoch = 0;
        for reward_epoch in self.epoch_rewards_map().keys() {
            if epochs_waited > reward_epoch && latest_valid_epoch < reward_epoch {
                latest_valid_epoch = reward_epoch;
            }
        }

        latest_valid_epoch
    }

    // callbacks

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

    // Storage

    #[storage_mapper("epochRewardsMap")]
    fn epoch_rewards_map(&self) -> MapMapper<Self::Storage, u64, Self::BigUint>;
}
