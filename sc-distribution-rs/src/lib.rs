#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

mod assets;
use assets::*;

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

    #[endpoint(setCommunityDistribution)]
    fn set_community_distrib(
        &self,
        total_amount: BigUint,
        spread_epoch: u64,
        #[var_args] unlock_milestones: VarArgs<UnlockMilestone>,
    ) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        require!(
            spread_epoch >= self.get_block_epoch(),
            "Spread epoch in the past"
        );
        require!(
            self.community_distribution_list()
                .front()
                .map(|community_distrib| community_distrib.spread_epoch)
                .unwrap_or_default()
                < spread_epoch,
            "Community distribution should be added in chronological order"
        );
        sc_try!(self.validate_unlock_milestones(&unlock_milestones));
        let distrib = CommunityDistribution {
            total_amount: total_amount.clone(),
            spread_epoch,
            after_planning_amount: total_amount,
            unlock_milestones: unlock_milestones.into_vec(),
        };
        self.community_distribution_list().push_front(distrib);
        Ok(())
    }

    #[endpoint(setPerUserDistributedAssets)]
    fn set_per_user_distributed_assets(
        &self,
        spread_epoch: u64,
        #[var_args] user_assets: VarArgs<MultiArg2<Address, BigUint>>,
    ) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        require!(!user_assets.is_empty(), "Empty assets vec");
        self.add_all_user_assets_to_map(spread_epoch, user_assets, false)
    }

    #[endpoint(setPerUserDistributedLockedAssets)]
    fn set_per_user_distributed_locked_assets(
        &self,
        spread_epoch: u64,
        #[var_args] user_assets: VarArgs<MultiArg2<Address, BigUint>>,
    ) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        require!(
            !self
                .community_distribution_list()
                .front()
                .unwrap()
                .unlock_milestones
                .is_empty(),
            "No unlock milestones set"
        );
        require!(!user_assets.is_empty(), "Empty assets vec");
        self.add_all_user_assets_to_map(spread_epoch, user_assets, true)
    }

    #[endpoint(claimAssets)]
    fn claim_assets(&self) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        let caller = self.get_caller();
        let (assets_amounts, _) = self.calculate_user_assets(&caller, false, true);
        let cummulated_amount = self.sum_of(&assets_amounts);
        self.mint_and_send_assets(&caller, &cummulated_amount);
        Ok(cummulated_amount)
    }

    #[endpoint(claimLockedAssets)]
    fn claim_locked_assets(&self) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        let caller = self.get_caller();
        let (assets_amounts, _unlock_milestones_vec) = self.calculate_user_assets(&caller, true, true);
        //self.mint_and_send_assets(&caller, &assets_amounts, &unlock_milestones_vec);
        let cummulated_amount = self.sum_of(&assets_amounts);
        Ok(cummulated_amount)
    }

    #[endpoint(clearUnclaimableAssets)]
    fn clear_unclaimable_assets(&self) -> SCResult<usize> {
        let biggest_unclaimable_asset_epoch = self.get_biggest_unclaimable_asset_epoch();
        self.undo_user_assets_between_epochs(0, biggest_unclaimable_asset_epoch)
    }

    #[endpoint(undoLastCommunityDistribution)]
    fn undo_last_community_distrib(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        self.community_distribution_list().pop_front();
        Ok(())
    }

    #[endpoint(undoUserDistributedAssetsBetweenEpochs)]
    fn undo_user_assets_between_epochs(&self, lower: u64, higher: u64) -> SCResult<usize> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_global_operation_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        require!(lower <= higher, "Bad input values");
        Ok(self.remove_asset_entries_between_epochs(lower, higher))
    }

    #[view(calculateAssets)]
    fn calculate_assets_view(&self, address: Address) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        let (assets_amounts, _) = self.calculate_user_assets(&address, false, false);
        let cummulated_amount = self.sum_of(&assets_amounts);
        Ok(cummulated_amount)
    }

    #[view(calculateLockedAssets)]
    fn calculate_locked_assets_view(&self, address: Address) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        let (assets_amounts, _) = self.calculate_user_assets(&address, true, false);
        let cummulated_amount = self.sum_of(&assets_amounts);
        Ok(cummulated_amount)
    }

    #[view(getLastCommunityDistributionAmountAndEpoch)]
    fn get_last_community_distrib_amount_and_epoch(&self) -> MultiResult2<BigUint, u64> {
        self.community_distribution_list()
            .front()
            .map(|last_community_distrib| {
                (
                    last_community_distrib.total_amount,
                    last_community_distrib.spread_epoch,
                )
            })
            .unwrap_or((BigUint::zero(), 0u64))
            .into()
    }

    #[view(getLastCommunityDistributionUnlockMilestones)]
    fn get_last_community_distrib_unlock_milestones(&self) -> MultiResultVec<UnlockMilestone> {
        self.community_distribution_list()
            .front()
            .map(|last_community_distrib| last_community_distrib.unlock_milestones)
            .unwrap_or_default()
            .into()
    }

    fn validate_unlock_milestones(
        &self,
        unlock_milestones: &VarArgs<UnlockMilestone>,
    ) -> SCResult<()> {
        let mut precents_sum: u64 = 0;
        let mut last_milestone_unlock_epoch: u64 = 0;
        for milestone in unlock_milestones.0.clone() {
            require!(
                milestone.unlock_epoch > last_milestone_unlock_epoch,
                "Unlock epochs not in order"
            );
            last_milestone_unlock_epoch = milestone.unlock_epoch;
            precents_sum += milestone.unlock_precent;
        }
        if !unlock_milestones.is_empty() {
            require!(precents_sum == 100, "Precents do not sum up to 100");
        }
        Ok(())
    }

    fn add_all_user_assets_to_map(
        &self,
        spread_epoch: u64,
        user_assets: VarArgs<MultiArg2<Address, BigUint>>,
        locked_assets: bool,
    ) -> SCResult<()> {
        let mut last_community_distrib = self.community_distribution_list().front().unwrap();
        require!(
            spread_epoch == last_community_distrib.spread_epoch,
            "Bad spread epoch"
        );
        for user_asset_multiarg in user_assets.into_vec() {
            let (user_address, asset_amount) = user_asset_multiarg.into_tuple();
            require!(
                last_community_distrib.after_planning_amount >= asset_amount,
                "User assets sums above community total assets"
            );
            last_community_distrib.after_planning_amount -= asset_amount.clone();
            sc_try!(self.add_user_asset_entry(
                user_address,
                asset_amount,
                spread_epoch,
                locked_assets
            ));
        }
        self.community_distribution_list().pop_front();
        self.community_distribution_list()
            .push_front(last_community_distrib);
        Ok(())
    }

    fn add_user_asset_entry(
        &self,
        user_address: Address,
        asset_amount: BigUint,
        spread_epoch: u64,
        locked_asset: bool,
    ) -> SCResult<()> {
        let user_asset_key = UserAssetKey {
            user_address,
            spread_epoch,
            locked_asset,
        };
        require!(
            !self.user_asset_map().contains_key(&user_asset_key),
            "Vector has duplicates"
        );
        self.user_asset_map().insert(user_asset_key, asset_amount);
        Ok(())
    }

    fn calculate_user_assets(
        &self,
        address: &Address,
        locked_asset: bool,
        delete_after_visit: bool
    ) -> (Vec<BigUint>, Vec<Vec<UnlockMilestone>>) {
        let current_epoch = self.get_block_epoch();
        let mut amounts = Vec::<BigUint>::new();
        let mut milestones = Vec::<Vec::<UnlockMilestone>>::new();

        for community_distrib in self
            .community_distribution_list()
            .iter()
            .take(MAX_CLAIMABLE_DISTRIBUTION_ROUNDS)
            .filter(|x| x.spread_epoch <= current_epoch)
        {
            let user_asset_key = UserAssetKey {
                user_address: address.clone(),
                spread_epoch: community_distrib.spread_epoch,
                locked_asset,
            };

            if let Some(asset_amount) = self.user_asset_map().get(&user_asset_key) {
                amounts.push(asset_amount);
                milestones.push(community_distrib.unlock_milestones);

                if delete_after_visit {
                    self.user_asset_map().remove(&user_asset_key);
                }
            }
        }
        (amounts, milestones)
    }

    fn get_biggest_unclaimable_asset_epoch(&self) -> u64 {
        self.community_distribution_list()
            .iter()
            .nth(MAX_CLAIMABLE_DISTRIBUTION_ROUNDS)
            .map(|community_distrib| community_distrib.spread_epoch)
            .unwrap_or_default()
    }

    fn remove_asset_entries_between_epochs(&self, lower: u64, higher: u64) -> usize {
        if higher == 0 {
            return 0;
        }

        if higher < lower {
            return 0;
        }

        let mut to_remove_keys = Vec::new();
        let search_gas_limit = self.get_gas_left() / 2;
        for (user_asset_index, user_asset_key) in self.user_asset_map().keys().enumerate() {
            if (user_asset_index + 1) % GAS_CHECK_FREQUENCY == 0
                && self.get_gas_left() < search_gas_limit
            {
                break;
            }
            if lower <= user_asset_key.spread_epoch && user_asset_key.spread_epoch <= higher {
                to_remove_keys.push(user_asset_key);
            }
        }

        for key in to_remove_keys.iter() {
            self.user_asset_map().remove(&key);
        }
        to_remove_keys.len()
    }

    fn mint_and_send_assets(&self, address: &Address, asset_amount: &BigUint) {
        if asset_amount > &0 {
            let asset_token_id = self.distributed_token_id().get();
            self.send().esdt_local_mint(
                self.get_gas_left(),
                asset_token_id.as_esdt_identifier(),
                &asset_amount,
            );
            self.send().direct_esdt_via_transf_exec(
                address,
                asset_token_id.as_esdt_identifier(),
                &asset_amount,
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

    fn require_community_distribution_list_not_empty(&self) -> SCResult<()> {
        require!(
            !self.community_distribution_list().is_empty(),
            "Empty community assets list"
        );
        Ok(())
    }

    fn sum_of(&self, vect: &Vec<BigUint>) -> BigUint {
        let mut sum =  BigUint::zero();
        for item in vect.iter() {
            sum += item;
        }
        sum
    }

    #[storage_mapper("global_operation_ongoing")]
    fn global_operation_ongoing(&self) -> SingleValueMapper<Self::Storage, bool>;

    #[storage_mapper("community_distribution_list")]
    fn community_distribution_list(
        &self,
    ) -> LinkedListMapper<Self::Storage, CommunityDistribution<BigUint>>;

    #[storage_mapper("user_asset")]
    fn user_asset_map(&self) -> MapMapper<Self::Storage, UserAssetKey, BigUint>;

    #[view(getDistributedTokenId)]
    #[storage_mapper("distributed_token_id")]
    fn distributed_token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;
}
