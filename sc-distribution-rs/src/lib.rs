#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

mod rewards;
use rewards::*;

const GAS_CHECK_FREQUENCY: usize = 100;
const MAX_CLAIMABLE_DISTRIBUTION_ROUNDS: usize = 4;

#[elrond_wasm_derive::contract(EsdtDistributionImpl)]
pub trait EsdtDistribution {
    #[init]
    fn init(&self, distributed_token_id: TokenIdentifier) {
        self.distributed_token_id().set(&distributed_token_id);
    }

    #[endpoint(startGlobalOperation)]
    fn start_planning(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        self.global_operation_ongoing().set(&true);
        Ok(())
    }

    #[endpoint(endGlobalOperation)]
    fn end_planning(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        self.global_operation_ongoing().set(&false);
        Ok(())
    }

    #[endpoint(setCommunityReward)]
    fn set_community_reward(&self, total_amount: BigUint, unlock_epoch: u64) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        require!(
            unlock_epoch >= self.get_block_epoch(),
            "Unlock epoch in the past"
        );
        require!(
            self.community_reward_list()
                .front()
                .map(|community_reward| community_reward.unlock_epoch)
                .unwrap_or_default()
                < unlock_epoch,
            "Community reward distribution should be added in chronological order"
        );
        let reward = CommunityReward {
            total_amount: total_amount.clone(),
            unlock_epoch,
            after_planning_amount: total_amount,
        };
        self.community_reward_list().push_front(reward);
        Ok(())
    }

    #[endpoint(setPerUserRewards)]
    fn set_per_user_rewards(
        &self,
        unlock_epoch: u64,
        #[var_args] user_rewards: VarArgs<MultiArg2<Address, BigUint>>,
    ) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        require!(!user_rewards.is_empty(), "Empty rewards vec");
        require!(
            !self.community_reward_list().is_empty(),
            "Empty community rewards list"
        );
        self.add_all_user_rewards_to_map(unlock_epoch, user_rewards)
    }

    #[endpoint(claimRewards)]
    fn claim_rewards(&self) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        require!(
            !self.community_reward_list().is_empty(),
            "Empty community rewards"
        );
        let caller = self.get_caller();
        let cummulated_reward_amount = self.calculate_user_rewards(&caller, true);
        self.mint_and_send_rewards(&caller, &cummulated_reward_amount);
        Ok(cummulated_reward_amount)
    }

    #[endpoint(clearUnclaimableRewards)]
    fn clear_unclaimable_rewards(&self) -> SCResult<usize> {
        let biggest_unclaimable_reward_epoch = self.get_biggest_unclaimable_reward_epoch();
        self.undo_user_rewards_between_epochs(0, biggest_unclaimable_reward_epoch)
    }

    #[endpoint(undoLastCommunityReward)]
    fn undo_last_community_reward(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        require!(
            !self.community_reward_list().is_empty(),
            "Empty community rewards"
        );
        self.community_reward_list().pop_front();
        Ok(())
    }

    #[endpoint(undoUserRewardsBetweenEpochs)]
    fn undo_user_rewards_between_epochs(&self, lower: u64, higher: u64) -> SCResult<usize> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        require!(
            !self.community_reward_list().is_empty(),
            "Empty community rewards"
        );
        require!(lower <= higher, "Bad input values");
        Ok(self.remove_reward_entries_between_epochs(lower, higher))
    }

    #[view(calculateRewards)]
    fn calculate_rewards_view(&self, address: Address) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        require!(
            !self.community_reward_list().is_empty(),
            "Empty community rewards"
        );
        Ok(self.calculate_user_rewards(&address, false))
    }

    #[view(getLastCommunityRewardAmountAndEpoch)]
    fn get_last_community_reward_amount_and_epoch(&self) -> MultiResult2<BigUint, u64> {
        self.community_reward_list()
            .front()
            .map(|last_community_reward| {
                (
                    last_community_reward.total_amount,
                    last_community_reward.unlock_epoch,
                )
            })
            .unwrap_or((BigUint::zero(), 0u64))
            .into()
    }

    fn add_all_user_rewards_to_map(
        &self,
        unlock_epoch: u64,
        user_rewards: VarArgs<MultiArg2<Address, BigUint>>,
    ) -> SCResult<()> {
        let mut last_community_reward = self.community_reward_list().front().unwrap();
        require!(
            unlock_epoch == last_community_reward.unlock_epoch,
            "Bad unlock epoch"
        );
        for user_reward_multiarg in user_rewards.into_vec() {
            let (user_address, reward_amount) = user_reward_multiarg.into_tuple();
            require!(
                last_community_reward.after_planning_amount >= reward_amount,
                "User rewards sums above community total rewards"
            );
            last_community_reward.after_planning_amount -= reward_amount.clone();
            sc_try!(self.add_user_reward_entry(user_address, reward_amount, unlock_epoch));
        }
        self.community_reward_list().pop_front();
        self.community_reward_list()
            .push_front(last_community_reward);
        Ok(())
    }

    fn add_user_reward_entry(
        &self,
        user_address: Address,
        reward_amount: BigUint,
        unlock_epoch: u64,
    ) -> SCResult<()> {
        let user_reward_key = UserRewardKey {
            user_address,
            unlock_epoch,
        };
        require!(
            !self.user_reward_map().contains_key(&user_reward_key),
            "Vector has duplicates"
        );
        self.user_reward_map()
            .insert(user_reward_key, reward_amount);
        Ok(())
    }

    fn calculate_user_rewards(&self, address: &Address, delete_after_visit: bool) -> BigUint {
        let mut amount = BigUint::zero();
        let current_epoch = self.get_block_epoch();

        for community_reward in self
            .community_reward_list()
            .iter()
            .take(MAX_CLAIMABLE_DISTRIBUTION_ROUNDS)
            .filter(|x| x.unlock_epoch <= current_epoch)
        {
            let user_reward_key = UserRewardKey {
                user_address: address.clone(),
                unlock_epoch: community_reward.unlock_epoch,
            };

            if let Some(reward_amount) = self.user_reward_map().get(&user_reward_key) {
                amount += reward_amount;

                if delete_after_visit {
                    self.user_reward_map().remove(&user_reward_key);
                }
            }
        }
        amount
    }

    fn get_biggest_unclaimable_reward_epoch(&self) -> u64 {
        self.community_reward_list()
            .iter()
            .nth(MAX_CLAIMABLE_DISTRIBUTION_ROUNDS)
            .map(|community_reward| community_reward.unlock_epoch)
            .unwrap_or_default()
    }

    fn remove_reward_entries_between_epochs(&self, lower: u64, higher: u64) -> usize {
        if higher == 0 {
            return 0;
        }

        if higher < lower {
            return 0;
        }

        let mut to_remove_keys = Vec::new();
        let search_gas_limit = self.get_gas_left() / 2;
        for (user_reward_index, user_reward_key) in self.user_reward_map().keys().enumerate() {
            if (user_reward_index + 1) % GAS_CHECK_FREQUENCY == 0
                && self.get_gas_left() < search_gas_limit
            {
                break;
            }
            if lower <= user_reward_key.unlock_epoch && user_reward_key.unlock_epoch <= higher {
                to_remove_keys.push(user_reward_key);
            }
        }

        for key in to_remove_keys.iter() {
            self.user_reward_map().remove(&key);
        }
        to_remove_keys.len()
    }

    fn mint_and_send_rewards(&self, address: &Address, reward_amount: &BigUint) {
        if reward_amount > &0 {
            let reward_token_id = self.distributed_token_id().get();
            self.send().esdt_local_mint(
                self.get_gas_left(),
                reward_token_id.as_esdt_identifier(),
                &reward_amount,
            );
            self.send().direct_esdt_via_transf_exec(
                address,
                reward_token_id.as_esdt_identifier(),
                &reward_amount,
                &[],
            );
        }
    }

    fn require_global_operation_ongoing(&self) -> SCResult<()> {
        require!(
            self.global_operation_ongoing().get(),
            "Global Operation not ongoing"
        );
        Ok(())
    }

    fn require_global_operation_not_ongoing(&self) -> SCResult<()> {
        require!(
            !self.global_operation_ongoing().get(),
            "Global Operation ongoing"
        );
        Ok(())
    }

    #[storage_mapper("global_operation_ongoing")]
    fn global_operation_ongoing(&self) -> SingleValueMapper<Self::Storage, bool>;

    #[storage_mapper("community_reward_list")]
    fn community_reward_list(&self) -> LinkedListMapper<Self::Storage, CommunityReward<BigUint>>;

    #[storage_mapper("user_reward")]
    fn user_reward_map(&self) -> MapMapper<Self::Storage, UserRewardKey, BigUint>;

    #[view(getDistributedTokenId)]
    #[storage_mapper("distributed_token_id")]
    fn distributed_token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;
}
