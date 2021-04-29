#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

use distrib_common::*;

// Min precision of 100, i.e. no precision
const MIN_PRECISION: u32 = 100;

const NFT_AMOUNT: u32 = 1;

#[elrond_wasm_derive::contract(DexRewardsLockImpl)]
pub trait DexRewardsLock {
    /// Epoch refers to duration in epochs, not a specific deadline
    #[init]
    fn init(
        &self,
        mex_token_id: TokenIdentifier,
        percentage_precision: BigUint,
        #[var_args] epoch_reward_percentage_pairs: VarArgs<MultiArg2<u64, BigUint>>,
    ) -> SCResult<()> {
        require!(
            mex_token_id.is_valid_esdt_identifier(),
            "Invalid token provided"
        );
        require!(
            percentage_precision >= BigUint::from(MIN_PRECISION),
            "Precision too low"
        );
        require!(
            &percentage_precision / &BigUint::from(10u32) == 0,
            "Precision must be a multiple of 10"
        );
        require!(
            !epoch_reward_percentage_pairs.is_empty(),
            "Must provide at least one epoch-reward pair"
        );

        self.mex_token_id().set(&mex_token_id);
        self.precentage_precision().set(&percentage_precision);

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
        #[payment] issue_cost: BigUint,
    ) -> SCResult<AsyncCall<BigUint>> {
        sc_try!(self.require_caller_owner());

        Ok(ESDTSystemSmartContractProxy::new()
            .issue_non_fungible(
                issue_cost,
                &token_display_name,
                &token_ticker,
                NonFungibleTokenProperties {
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

        Ok(())
    }

    // endpoints

    #[payable("*")]
    #[endpoint(lockMexTokens)]
    fn lock_mex_tokens(
        &self,
        #[payment_token] token_id: TokenIdentifier,
        #[payment] amount: BigUint,
    ) -> SCResult<()> {
        sc_try!(self.require_nft_issued());
        require!(
            token_id == self.mex_token_id().get(),
            "Wrong token sent as payment"
        );
        require!(amount > 0, "Must locked more than 0 tokens");

        let caller = self.blockchain().get_caller();
        let current_epoch = self.blockchain().get_block_epoch();

        // create and send NFT to user, used to reclaim the deposit later
        self.create_nft(current_epoch);

        let nft_id = self.nft_id().get();
        let nft_nonce = self.blockchain().get_current_esdt_nft_nonce(
            &self.blockchain().get_sc_address(),
            nft_id.as_esdt_identifier(),
        );

        self.mex_deposit(nft_nonce).set(&amount);

        match self.send().direct_esdt_nft_via_transfer_exec(
            &caller,
            nft_id.as_esdt_identifier(),
            nft_nonce,
            &BigUint::from(NFT_AMOUNT),
            &[],
        ) {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed sending NFT to caller"),
        }
    }

    /// Paying back the NFT to retrieve the funds + the interest
    /// No need to check the amount, as that will always be 1 (since only 1 of each if created)
    #[payable("*")]
    #[endpoint]
    fn withdraw(&self, #[payment_token] nft_id: TokenIdentifier) -> SCResult<()> {
        sc_try!(self.require_nft_issued());
        require!(nft_id == self.nft_id().get(), "Wrong NFT sent as payment");

        let nft_nonce = self.call_value().esdt_token_nonce();
        let nft_attributes = self.blockchain().get_esdt_token_data(
            &self.blockchain().get_sc_address(),
            nft_id.as_esdt_identifier(),
            nft_nonce,
        );

        let deposit_epoch = match nft_attributes.decode_attributes::<LockedTokenAttributes>() {
            Result::Ok(attr) => {
                match attr.unlock_milestones.first() {
                    Some(unlock_mil) => unlock_mil.unlock_epoch,
                    None => return sc_error!("Empty attributes")
                }
            }
            Result::Err(_) => return sc_error!("Failed decoding attributes"),
        };
        let current_epoch = self.blockchain().get_block_epoch();
        let epochs_waited = current_epoch - deposit_epoch;

        let deposit_amount = self.mex_deposit(nft_nonce).get();
        let interest_amount = self.calculate_interest(&deposit_amount, epochs_waited);

        // mint required tokens and send mex tokens to the caller
        self.mint_mex_tokens(&interest_amount);
        self.send().direct(
            &self.blockchain().get_caller(),
            &self.mex_token_id().get(),
            &(deposit_amount + interest_amount),
            &[],
        );

        // burn the received nft and clear the storage
        self.burn_nft(nft_nonce);
        self.mex_deposit(nft_nonce).clear();

        Ok(())
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
    fn calculate_interest(&self, deposit_amount: &BigUint, epochs_waited: u64) -> BigUint {
        let latest_reward_epoch = self.find_latest_reward_epoch(epochs_waited);
        let reward_percentage = self.get_reward_percentage_for_epochs_waited(latest_reward_epoch);
        let precision = self.precentage_precision().get();

        deposit_amount * &reward_percentage / precision
    }

    // private

    fn mint_mex_tokens(&self, amount: &BigUint) {
        self.send().esdt_local_mint(
            self.blockchain().get_gas_left(),
            self.mex_token_id().get().as_esdt_identifier(),
            amount,
        );
    }

    fn create_nft(&self, deposit_epoch: u64) {
        self.send().esdt_nft_create::<LockedTokenAttributes>(
            self.blockchain().get_gas_left(),
            self.nft_id().get().as_esdt_identifier(),
            &BigUint::from(NFT_AMOUNT),
            &BoxedBytes::empty(),
            &BigUint::zero(),
            &H256::zero(),
            &LockedTokenAttributes {
                unlock_milestones: [UnlockMilestone {
                    unlock_epoch: deposit_epoch,
                    unlock_precent: 100
                }].to_vec()
            },
            &[BoxedBytes::empty()],
        );
    }

    fn burn_nft(&self, nft_nonce: u64) {
        self.send().esdt_nft_burn(
            self.blockchain().get_gas_left(),
            self.nft_id().get().as_esdt_identifier(),
            nft_nonce,
            &BigUint::from(NFT_AMOUNT),
        );
    }

    fn require_caller_owner(&self) -> SCResult<()> {
        only_owner!(self, "Only owner may call this function");
        Ok(())
    }

    fn require_nft_issued(&self) -> SCResult<()> {
        require!(!self.nft_id().is_empty(), "Nft not issued yet");
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
    fn issue_nft_callback(
        &self,
        #[call_result] result: AsyncCallResult<TokenIdentifier>,
    ) -> OptionalResult<AsyncCall<BigUint>> {
        match result {
            AsyncCallResult::Ok(token_id) => {
                self.nft_id().set(&token_id);

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

    #[view(getMexTokenId)]
    #[storage_mapper("mexTokenId")]
    fn mex_token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[view(getNftId)]
    #[storage_mapper("nftId")]
    fn nft_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[view(getPrecentagePrecision)]
    #[storage_mapper("percentagePrecision")]
    fn precentage_precision(&self) -> SingleValueMapper<Self::Storage, BigUint>;

    #[storage_mapper("epochRewardsMap")]
    fn epoch_rewards_map(&self) -> MapMapper<Self::Storage, u64, BigUint>;

    #[view(getMexDeposit)]
    #[storage_mapper("mexDeposit")]
    fn mex_deposit(&self, nft_nonce: u64) -> SingleValueMapper<Self::Storage, BigUint>;
}
