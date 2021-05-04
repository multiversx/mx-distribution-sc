#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

use distrib_common::*;
use modules::*;

const PERCENTAGE_TOTAL: u32 = 100;

#[elrond_wasm_derive::contract(DexRewardsLockImpl)]
pub trait DexRewardsLock {
    #[module(AssetModuleImpl)]
    fn asset(&self) -> AssetModuleImpl<T, BigInt, BigUint>;

    #[module(LockedAssetModuleImpl)]
    fn locked_asset(&self) -> LockedAssetModuleImpl<T, BigInt, BigUint>;

    /// Epoch refers to duration in epochs, not a specific deadline
    #[init]
    fn init(
        &self,
        mex_token_id: TokenIdentifier,
        #[var_args] epoch_reward_percentage_pairs: VarArgs<MultiArg2<u64, BigUint>>,
    ) -> SCResult<()> {
        require!(
            mex_token_id.is_valid_esdt_identifier(),
            "Invalid token provided"
        );
        require!(
            !epoch_reward_percentage_pairs.is_empty(),
            "Must provide at least one epoch-reward pair"
        );

        self.asset().token_id().set(&mex_token_id);

        for pair in epoch_reward_percentage_pairs.into_vec() {
            let (epoch, percentage) = pair.into_tuple();

            require!(epoch > 0, "Can't have lock time for 0 epochs");
            require!(percentage > 0, "Percentage must be higher than 0");

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
        #[payment] issue_cost: BigUint,
    ) -> SCResult<AsyncCall<BigUint>> {
        sc_try!(self.require_caller_owner());
        require!(
            self.locked_asset().token_id().is_empty(),
            "NFT already issued"
        );

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
            .with_callback(self.callbacks().issue_nft_callback()))
    }

    #[endpoint(addEpochReward)]
    fn add_epoch_reward(&self, epoch: u64, percentage: BigUint) -> SCResult<()> {
        sc_try!(self.require_caller_owner());
        require!(
            !self.epoch_rewards_map().contains_key(&epoch),
            "There is already a reward set for that epoch"
        );

        require!(epoch > 0, "Can't have lock time for 0 epochs");
        require!(percentage > 0, "Percentage must be higher than 0");

        self.epoch_rewards_map().insert(epoch, percentage);

        Ok(())
    }

    #[endpoint(removeEpochReward)]
    fn remove_epoch_reward(&self, epoch: u64) -> SCResult<()> {
        sc_try!(self.require_caller_owner());
        require!(
            self.epoch_rewards_map().contains_key(&epoch),
            "There is no reward set for that epoch"
        );

        self.epoch_rewards_map().remove(&epoch);

        require!(!self.epoch_rewards_map().is_empty(), "Can't remove all epoch rewards");

        Ok(())
    }

    // endpoints

    #[payable("*")]
    #[endpoint(lockMexTokens)]
    fn lock_mex_tokens(
        &self,
        epochs_lock_time: u64,
        #[payment_token] token_id: TokenIdentifier,
        #[payment] amount: BigUint,
    ) -> SCResult<u64> {
        sc_try!(self.require_nft_issued());
        require!(
            token_id == self.asset().token_id().get(),
            "Wrong token sent as payment"
        );
        require!(amount > 0, "Must lock more than 0 tokens");

        let caller = self.blockchain().get_caller();
        let latest_reward_epoch = self.find_latest_reward_epoch(epochs_lock_time);

        require!(
            latest_reward_epoch > 0,
            "Could not lock, epoch lock time too low"
        );

        let interest = self.calculate_interest(&amount, latest_reward_epoch);
        let nft_amount = &amount + &interest;
        let unlock_epoch = self.blockchain().get_block_epoch() + latest_reward_epoch;

        // send locked tokens as NFTs to caller
        self.locked_asset().create_and_send(
            &caller,
            &self.locked_asset().token_id().get(),
            &nft_amount,
            &[UnlockMilestone {
                unlock_epoch,
                unlock_precent: PERCENTAGE_TOTAL as u8,
            }],
        );

        // burn received MEX tokens
        self.asset().burn(&self.asset().token_id().get(), &amount);

        Ok(latest_reward_epoch)
    }

    // views

    #[view(getRewardPercentageForEpochsWaited)]
    fn get_reward_percentage_for_epochs_waited(&self, epochs_waited: u64) -> BigUint {
        match self.epoch_rewards_map().get(&epochs_waited) {
            Some(percentage) => percentage,
            None => BigUint::zero(),
        }
    }

    #[view(calculateInterest)]
    fn calculate_interest_endpoint(
        &self,
        deposit_amount: &BigUint,
        epoch_lock_time: u64,
    ) -> BigUint {
        let latest_reward_epoch = self.find_latest_reward_epoch(epoch_lock_time);

        self.calculate_interest(&deposit_amount, latest_reward_epoch)
    }

    // private

    fn require_caller_owner(&self) -> SCResult<()> {
        only_owner!(self, "Only owner may call this function");
        Ok(())
    }

    fn require_nft_issued(&self) -> SCResult<()> {
        require!(
            !self.locked_asset().token_id().is_empty(),
            "Nft not issued yet"
        );
        Ok(())
    }

    fn find_latest_reward_epoch(&self, epochs_waited: u64) -> u64 {
        let mut latest_valid_epoch = 0;
        for reward_epoch in self.epoch_rewards_map().keys() {
            if epochs_waited == reward_epoch {
                latest_valid_epoch = reward_epoch;
                break;
            }

            if epochs_waited > reward_epoch && latest_valid_epoch < reward_epoch {
                latest_valid_epoch = reward_epoch;
            }
        }

        latest_valid_epoch
    }

    fn calculate_interest(&self, deposit_amount: &BigUint, epochs_lock_time: u64) -> BigUint {
        let reward_percentage = self.get_reward_percentage_for_epochs_waited(epochs_lock_time);

        deposit_amount * &reward_percentage / BigUint::from(PERCENTAGE_TOTAL)
    }

    // callbacks

    #[callback]
    fn issue_nft_callback(
        &self,
        #[call_result] result: AsyncCallResult<TokenIdentifier>,
    ) -> OptionalResult<AsyncCall<BigUint>> {
        match result {
            AsyncCallResult::Ok(token_id) => {
                self.locked_asset().token_id().set(&token_id);

                OptionalResult::Some(
                    ESDTSystemSmartContractProxy::new()
                        .set_special_roles(
                            &self.blockchain().get_sc_address(),
                            token_id.as_esdt_identifier(),
                            &[EsdtLocalRole::NftCreate, EsdtLocalRole::NftBurn],
                        )
                        .async_call(),
                )
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

                OptionalResult::None
            }
        }
    }

    // Storage

    #[storage_mapper("epochRewardsMap")]
    fn epoch_rewards_map(&self) -> MapMapper<Self::Storage, u64, BigUint>;
}
