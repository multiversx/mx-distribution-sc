#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
pub use crate::asset::*;
pub use crate::locked_asset::*;
pub use crate::proxy_pair::*;

use elrond_wasm::{contract_call, only_owner, require, sc_error, sc_try};

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct WrappedFarmTokenAttributes {
    farm_token_id: TokenIdentifier,
    farm_token_nonce: Nonce,
    farmed_token_id: TokenIdentifier,
    farmed_token_nonce: Nonce,
}

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct ProxyFarmParams {
    pub simulate_enter_farm_gas_limit: u64,
    pub simulate_exit_farm_gas_limit: u64,
    pub claim_rewards_gas_limit: u64,
    pub enter_farm_gas_limit: u64,
    pub exit_farm_gas_limit: u64,
    pub burn_tokens_gas_limit: u64,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct SftTokenAmountPair<BigUint: BigUintApi> {
    token_id: TokenIdentifier,
    token_nonce: Nonce,
    amount: BigUint,
}

type SimulateExitFarmResultType<BigUint> =
    MultiResult2<TokenAmountPair<BigUint>, TokenAmountPair<BigUint>>;

#[elrond_wasm_derive::callable(FarmContractProxy)]
pub trait FarmContract {
    fn simulateEnterFarm(
        &self,
        token_in: TokenIdentifier,
        amount_in: BigUint,
    ) -> ContractCall<BigUint, SftTokenAmountPair<BigUint>>;
    fn simulateExitFarm(
        &self,
        token_id: TokenIdentifier,
        token_nonce: Nonce,
        amount: BigUint,
    ) -> ContractCall<BigUint, SimulateExitFarmResultType<BigUint>>;
}
const ENTER_FARM_FUNC_NAME: &[u8] = b"enterFarm";
const EXIT_FARM_FUNC_NAME: &[u8] = b"exitFarm";
const CLAIM_REWARDS_FUNC_NAME: &[u8] = b"claimRewards";

#[elrond_wasm_derive::module(ProxyFarmModuleImpl)]
pub trait ProxyFarmModule {
    #[module(AssetModuleImpl)]
    fn asset(&self) -> AssetModuleImpl<T, BigInt, BigUint>;

    #[module(LockedAssetModuleImpl)]
    fn locked_asset(&self) -> LockedAssetModuleImpl<T, BigInt, BigUint>;

    #[module(ProxyPairModuleImpl)]
    fn proxy_pair(&self) -> ProxyPairModuleImpl<T, BigInt, BigUint>;

    #[endpoint(setProxyParams)]
    fn set_proxy_params(&self, proxy_params: ProxyFarmParams) -> SCResult<()> {
        sc_try!(self.require_permissions());
        self.params().set(&proxy_params);
        Ok(())
    }

    #[endpoint(addFarmToIntermediate)]
    fn add_farm_to_intermediate(&self, farm_address: Address) -> SCResult<()> {
        sc_try!(self.require_permissions());
        self.intermediated_farms().insert(farm_address);
        Ok(())
    }

    #[endpoint(removeIntermediatedFarm)]
    fn remove_intermediated_farm(&self, farm_address: Address) -> SCResult<()> {
        sc_try!(self.require_permissions());
        sc_try!(self.require_is_intermediated_farm(&farm_address));
        self.intermediated_farms().remove(&farm_address);
        Ok(())
    }

    #[payable("*")]
    #[endpoint(enterFarmProxy)]
    fn enter_farm_proxy(&self, farm_address: &Address) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_farm(&farm_address));
        sc_try!(self.require_params_not_empty());
        let proxy_params = self.params().get();

        let token_nonce = self.call_value().esdt_token_nonce();
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Payment amount cannot be zero");
        require!(
            token_id == self.proxy_pair().token_id().get(),
            "Should only be used with wrapped LP tokens"
        );

        let wrapped_lp_token_attrs =
            sc_try!(self.proxy_pair().get_attributes(&token_id, token_nonce));

        let lp_token_id = wrapped_lp_token_attrs.lp_token_id;

        let farm_result =
            self.simulate_enter_farm(&farm_address, &lp_token_id, &amount, &proxy_params);
        let farm_token_id = farm_result.token_id;
        let farm_token_nonce = farm_result.token_nonce;
        let farm_token_total_amount = farm_result.amount;
        require!(
            farm_token_total_amount > 0,
            "Farm token amount received should be greater than 0"
        );
        sc_try!(self.actual_enter_farm(&farm_address, &lp_token_id, &amount, &proxy_params));

        let attributes = WrappedFarmTokenAttributes {
            farm_token_id,
            farm_token_nonce,
            farmed_token_id: token_id,
            farmed_token_nonce: token_nonce,
        };
        let caller = self.blockchain().get_caller();
        self.create_and_send_wrapped_farm_token(&attributes, &farm_token_total_amount, &caller);

        Ok(())
    }

    #[payable("*")]
    #[endpoint(exitFarmProxy)]
    fn exit_farm_proxy(&self, farm_address: &Address) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_farm(&farm_address));
        sc_try!(self.require_params_not_empty());
        let proxy_params = self.params().get();

        let token_nonce = self.call_value().esdt_token_nonce();
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Payment amount cannot be zero");
        require!(
            token_id == self.token_id().get(),
            "Should only be used with wrapped farm tokens"
        );

        let wrapped_farm_token_attrs = sc_try!(self.get_attributes(&token_id, token_nonce));
        let farm_token_id = wrapped_farm_token_attrs.farm_token_id;
        let farm_token_nonce = wrapped_farm_token_attrs.farm_token_nonce;

        let farm_result = self.simulate_exit_farm(
            &farm_address,
            &farm_token_id,
            farm_token_nonce,
            &amount,
            &proxy_params,
        );
        let lp_token_returned = farm_result.0;
        let reward_token_returned = farm_result.1;
        sc_try!(self.actual_exit_farm(
            &farm_address,
            &farm_token_id,
            farm_token_nonce,
            &amount,
            &proxy_params
        ));

        let caller = self.blockchain().get_caller();
        self.send().transfer_tokens(
            &wrapped_farm_token_attrs.farmed_token_id,
            wrapped_farm_token_attrs.farmed_token_nonce,
            &lp_token_returned.amount,
            &caller,
        );

        self.send().transfer_tokens(
            &reward_token_returned.token_id,
            0,
            &reward_token_returned.amount,
            &caller,
        );
        self.send().burn_tokens(
            &token_id,
            token_nonce,
            &amount,
            self.blockchain().get_gas_left(),
        );

        Ok(())
    }

    #[payable("*")]
    #[endpoint(claimRewardsProxy)]
    fn claim_rewards_proxy(&self, farm_address: Address) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_farm(&farm_address));
        sc_try!(self.require_params_not_empty());
        let proxy_params = self.params().get();

        let token_nonce = self.call_value().esdt_token_nonce();
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Payment amount cannot be zero");
        require!(
            token_id == self.token_id().get(),
            "Should only be used with wrapped farm tokens"
        );

        // Read info about wrapped farm token and then burn it.
        let wrapped_farm_token_attrs = sc_try!(self.get_attributes(&token_id, token_nonce));
        let farm_token_id = wrapped_farm_token_attrs.farm_token_id;
        let farm_token_nonce = wrapped_farm_token_attrs.farm_token_nonce;
        self.send().burn_tokens(
            &token_id,
            token_nonce,
            &amount,
            proxy_params.burn_tokens_gas_limit,
        );

        // Simulate an exit farm and get the returned tokens.
        let exit_farm_result = self.simulate_exit_farm(
            &farm_address,
            &farm_token_id,
            farm_token_nonce,
            &amount,
            &proxy_params,
        );
        let lp_token_returned = exit_farm_result.0;
        let reward_token_returned = exit_farm_result.1;

        // Simulate an enter farm and get the returned tokens.
        let enter_farm_result = self.simulate_enter_farm(
            &farm_address,
            &lp_token_returned.token_id,
            &lp_token_returned.amount,
            &proxy_params,
        );
        let new_farm_token_id = enter_farm_result.token_id;
        let new_farm_token_nonce = enter_farm_result.token_nonce;
        let new_farm_token_total_amount = enter_farm_result.amount;
        require!(
            new_farm_token_total_amount > 0,
            "Farm token amount received should be greater than 0"
        );

        // Do the actual claiming of rewards.
        sc_try!(self.actual_exit_farm(
            &farm_address,
            &farm_token_id,
            farm_token_nonce,
            &amount,
            &proxy_params
        ));

        // Send the reward to the caller.
        let caller = self.blockchain().get_caller();
        self.send().transfer_tokens(
            &reward_token_returned.token_id,
            0,
            &reward_token_returned.amount,
            &caller,
        );

        // Create new Wrapped tokens and send them.
        let new_wrapped_farm_token_attributes = WrappedFarmTokenAttributes {
            farm_token_id: new_farm_token_id,
            farm_token_nonce: new_farm_token_nonce,
            farmed_token_id: wrapped_farm_token_attrs.farmed_token_id,
            farmed_token_nonce: wrapped_farm_token_attrs.farmed_token_nonce,
        };
        self.create_and_send_wrapped_farm_token(
            &new_wrapped_farm_token_attributes,
            &new_farm_token_total_amount,
            &caller,
        );

        Ok(())
    }

    fn get_attributes(
        &self,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
    ) -> SCResult<WrappedFarmTokenAttributes> {
        let token_info = self.blockchain().get_esdt_token_data(
            &self.blockchain().get_sc_address(),
            token_id.as_esdt_identifier(),
            token_nonce,
        );

        let attributes = token_info.decode_attributes::<WrappedFarmTokenAttributes>();
        match attributes {
            Result::Ok(decoded_obj) => Ok(decoded_obj),
            Result::Err(_) => {
                return sc_error!("Decoding error");
            }
        }
    }

    fn create_and_send_wrapped_farm_token(
        &self,
        attributes: &WrappedFarmTokenAttributes,
        amount: &BigUint,
        address: &Address,
    ) {
        let wrapped_farm_token_id = self.token_id().get();
        self.create_tokens(&wrapped_farm_token_id, attributes, amount);
        let nonce = self.token_nonce().get();
        self.send()
            .transfer_tokens(&wrapped_farm_token_id, nonce, amount, address);
    }

    fn create_tokens(
        &self,
        token_id: &TokenIdentifier,
        attributes: &WrappedFarmTokenAttributes,
        amount: &BigUint,
    ) {
        self.send().esdt_nft_create::<WrappedFarmTokenAttributes>(
            self.blockchain().get_gas_left(),
            token_id.as_esdt_identifier(),
            amount,
            &BoxedBytes::empty(),
            &BigUint::zero(),
            &H256::zero(),
            &attributes,
            &[BoxedBytes::empty()],
        );
        self.increase_nonce();
    }

    fn simulate_enter_farm(
        &self,
        farm_address: &Address,
        lp_token_id: &TokenIdentifier,
        amount: &BigUint,
        proxy_params: &ProxyFarmParams,
    ) -> SftTokenAmountPair<BigUint> {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.simulate_enter_farm_gas_limit,
        );
        contract_call!(self, farm_address.clone(), FarmContractProxy)
            .simulateEnterFarm(lp_token_id.clone(), amount.clone())
            .execute_on_dest_context_custom_range(
                gas_limit,
                |_, after| (after - 1, after),
                self.send(),
            )
    }

    fn simulate_exit_farm(
        &self,
        farm_address: &Address,
        farm_token_id: &TokenIdentifier,
        farm_token_nonce: Nonce,
        amount: &BigUint,
        proxy_params: &ProxyFarmParams,
    ) -> (TokenAmountPair<BigUint>, TokenAmountPair<BigUint>) {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.simulate_exit_farm_gas_limit,
        );
        let result = contract_call!(self, farm_address.clone(), FarmContractProxy)
            .simulateExitFarm(farm_token_id.clone(), farm_token_nonce, amount.clone())
            .execute_on_dest_context(gas_limit, self.send());
        result.0
    }

    fn actual_enter_farm(
        &self,
        farm_address: &Address,
        lp_token_id: &TokenIdentifier,
        amount: &BigUint,
        proxy_params: &ProxyFarmParams,
    ) -> SCResult<()> {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.enter_farm_gas_limit,
        );
        let result = self.send().direct_esdt_execute(
            farm_address,
            lp_token_id.as_esdt_identifier(),
            amount,
            gas_limit,
            ENTER_FARM_FUNC_NAME,
            &ArgBuffer::new(),
        );
        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn actual_exit_farm(
        &self,
        farm_address: &Address,
        farm_token_id: &TokenIdentifier,
        farm_token_nonce: Nonce,
        amount: &BigUint,
        proxy_params: &ProxyFarmParams,
    ) -> SCResult<()> {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.exit_farm_gas_limit,
        );
        let result = self.send().direct_esdt_nft_execute(
            farm_address,
            farm_token_id.as_esdt_identifier(),
            farm_token_nonce,
            amount,
            gas_limit,
            EXIT_FARM_FUNC_NAME,
            &ArgBuffer::new(),
        );
        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn actual_claim_rewards(
        &self,
        farm_address: &Address,
        farm_token_id: &TokenIdentifier,
        farm_token_nonce: Nonce,
        amount: &BigUint,
        proxy_params: &ProxyFarmParams,
    ) -> SCResult<()> {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.claim_rewards_gas_limit,
        );
        let result = self.send().direct_esdt_nft_execute(
            farm_address,
            farm_token_id.as_esdt_identifier(),
            farm_token_nonce,
            amount,
            gas_limit,
            CLAIM_REWARDS_FUNC_NAME,
            &ArgBuffer::new(),
        );
        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn increase_nonce(&self) -> Nonce {
        let new_nonce = self.token_nonce().get() + 1;
        self.token_nonce().set(&new_nonce);
        new_nonce
    }

    fn require_is_intermediated_farm(&self, address: &Address) -> SCResult<()> {
        require!(
            self.intermediated_farms().contains(address),
            "Not an intermediated farm"
        );
        Ok(())
    }

    fn require_permissions(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        Ok(())
    }

    fn require_params_not_empty(&self) -> SCResult<()> {
        require!(!self.params().is_empty(), "Empty params");
        Ok(())
    }

    #[view(getIntermediatedFarms)]
    #[storage_mapper("intermediated_farms")]
    fn intermediated_farms(&self) -> SetMapper<Self::Storage, Address>;

    #[view(getWrappedFarmTokenId)]
    #[storage_mapper("wrapped_farm_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("wrapped_farm_token_nonce")]
    fn token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;

    #[storage_mapper("proxy_farm_params")]
    fn params(&self) -> SingleValueMapper<Self::Storage, ProxyFarmParams>;
}
