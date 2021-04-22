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
    fn set_community_distrib(&self, total_amount: BigUint, spread_epoch: u64) -> SCResult<()> {
        only_owner!(self, "Permission denied");
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
        let distrib = CommunityDistribution {
            total_amount: total_amount.clone(),
            spread_epoch,
            after_planning_amount: total_amount,
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
        self.add_all_user_assets_to_map(spread_epoch, user_assets)
    }

    #[endpoint(claimAssets)]
    fn claim_assets(&self) -> SCResult<BigUint> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_community_distribution_list_not_empty());
        let caller = self.get_caller();
        let cummulated_asset_amount = self.calculate_user_assets(&caller, true);
        self.mint_and_send_assets(&caller, &cummulated_asset_amount);
        Ok(cummulated_asset_amount)
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
        Ok(self.calculate_user_assets(&address, false))
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

    fn add_all_user_assets_to_map(
        &self,
        spread_epoch: u64,
        user_assets: VarArgs<MultiArg2<Address, BigUint>>,
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
            sc_try!(self.add_user_asset_entry(user_address, asset_amount, spread_epoch));
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
    ) -> SCResult<()> {
        let user_asset_key = UserAssetKey {
            user_address,
            spread_epoch,
        };
        require!(
            !self.user_asset_map().contains_key(&user_asset_key),
            "Vector has duplicates"
        );
        self.user_asset_map()
            .insert(user_asset_key, asset_amount);
        Ok(())
    }

    fn calculate_user_assets(&self, address: &Address, delete_after_visit: bool) -> BigUint {
        let mut amount = BigUint::zero();
        let current_epoch = self.get_block_epoch();

        for community_distrib in self
            .community_distribution_list()
            .iter()
            .take(MAX_CLAIMABLE_DISTRIBUTION_ROUNDS)
            .filter(|x| x.spread_epoch <= current_epoch)
        {
            let user_asset_key = UserAssetKey {
                user_address: address.clone(),
                spread_epoch: community_distrib.spread_epoch,
            };

            if let Some(asset_amount) = self.user_asset_map().get(&user_asset_key) {
                amount += asset_amount;

                if delete_after_visit {
                    self.user_asset_map().remove(&user_asset_key);
                }
            }
        }
        amount
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

    #[storage_mapper("global_operation_ongoing")]
    fn global_operation_ongoing(&self) -> SingleValueMapper<Self::Storage, bool>;

    #[storage_mapper("community_distribution_list")]
    fn community_distribution_list(&self) -> LinkedListMapper<Self::Storage, CommunityDistribution<BigUint>>;

    #[storage_mapper("user_asset")]
    fn user_asset_map(&self) -> MapMapper<Self::Storage, UserAssetKey, BigUint>;

    #[view(getDistributedTokenId)]
    #[storage_mapper("distributed_token_id")]
    fn distributed_token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;
}
