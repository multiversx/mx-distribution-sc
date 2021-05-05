#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::clippy::comparison_chain)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
use elrond_wasm::{contract_call, only_owner, require, sc_error, sc_try};

pub use crate::asset::*;
pub use crate::locked_asset::*;
use core::cmp::min;

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct ProxyPairParams {
    pub add_liquidity_gas_limit: u64,
    pub accept_esdt_payment_gas_limit: u64,
    pub ask_for_lp_token_gas_limit: u64,
    pub ask_for_tokens_gas_limit: u64,
    pub remove_liquidity_gas_limit: u64,
    pub burn_tokens_gas_limit: u64,
    pub mint_tokens_gas_limit: u64,
}

type AddLiquidityResultType<BigUint> =
    MultiResult3<TokenAmountPair<BigUint>, TokenAmountPair<BigUint>, TokenAmountPair<BigUint>>;

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct TokenAmountPair<BigUint: BigUintApi> {
    pub token_id: TokenIdentifier,
    pub amount: BigUint,
}

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct WrappedLpTokenAttributes<BigUint: BigUintApi> {
    pub lp_token_id: TokenIdentifier,
    pub lp_token_total_amount: BigUint,
    locked_assets_token_id: TokenIdentifier,
    locked_assets_invested: BigUint,
    locked_assets_nonce: Nonce,
}

#[elrond_wasm_derive::callable(PairContractProxy)]
pub trait PairContract {
    fn addLiquidity(
        &self,
        first_token_amount_desired: BigUint,
        first_token_amount_min: BigUint,
        second_token_amount_desired: BigUint,
        second_token_amount_min: BigUint,
    ) -> ContractCall<BigUint, AddLiquidityResultType<BigUint>>;
    fn getLpTokenIdentifier(&self) -> ContractCall<BigUint, TokenIdentifier>;
    fn getTokensForGivenPosition(
        &self,
        amount: BigUint,
    ) -> ContractCall<BigUint, MultiResult2<TokenAmountPair<BigUint>, TokenAmountPair<BigUint>>>;
}
const ACCEPT_ESDT_PAYMENT_FUNC_NAME: &[u8] = b"acceptEsdtPayment";
const REMOVE_LIQUIDITY_FUNC_NAME: &[u8] = b"removeLiquidity";

#[elrond_wasm_derive::module(ProxyPairModuleImpl)]
pub trait ProxyPairModule {
    #[module(AssetModuleImpl)]
    fn asset(&self) -> AssetModuleImpl<T, BigInt, BigUint>;

    #[endpoint(setProxyParams)]
    fn set_proxy_params(&self, proxy_params: ProxyPairParams) -> SCResult<()> {
        sc_try!(self.require_permissions());
        self.params().set(&proxy_params);
        Ok(())
    }

    #[endpoint(addPairToIntermediate)]
    fn add_pair_to_intermediate(&self, pair_address: Address) -> SCResult<()> {
        sc_try!(self.require_permissions());
        self.intermediated_pairs().insert(pair_address);
        Ok(())
    }

    #[endpoint(removeIntermediatedPair)]
    fn remove_intermediated_pair(&self, pair_address: Address) -> SCResult<()> {
        sc_try!(self.require_permissions());
        sc_try!(self.require_is_intermediated_pair(&pair_address));
        self.intermediated_pairs().remove(&pair_address);
        Ok(())
    }

    #[endpoint(addAcceptedLockedAssetTokenId)]
    fn add_accepted_locked_asset_token_id(&self, token_id: TokenIdentifier) -> SCResult<()> {
        sc_try!(self.require_permissions());
        self.accepted_locked_assets().insert(token_id);
        Ok(())
    }

    #[endpoint(removeAcceptedLockedAssetTokenId)]
    fn remove_accepted_locked_asset_token_id(&self, token_id: TokenIdentifier) -> SCResult<()> {
        sc_try!(self.require_permissions());
        sc_try!(self.require_is_accepted_locked_asset(&token_id));
        self.accepted_locked_assets().remove(&token_id);
        Ok(())
    }

    #[payable("*")]
    #[endpoint(acceptEsdtPaymentProxy)]
    fn accept_esdt_payment_proxy(&self, pair_address: Address) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let token_nonce = self.call_value().esdt_token_nonce();
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Payment amount cannot be zero");

        let caller = self.blockchain().get_caller();
        self.increase_temporary_funds_amount(&caller, &token_id, token_nonce, &amount);
        Ok(())
    }

    #[endpoint(reclaimTemporaryFundsProxy)]
    fn reclaim_temporary_funds_proxy(
        &self,
        first_token_id: TokenIdentifier,
        first_token_nonce: Nonce,
        second_token_id: TokenIdentifier,
        second_token_nonce: Nonce,
    ) -> SCResult<()> {
        let caller = self.blockchain().get_caller();
        self.send_temporary_funds_back(&caller, &first_token_id, first_token_nonce);
        self.send_temporary_funds_back(&caller, &second_token_id, second_token_nonce);
        Ok(())
    }

    #[endpoint(addLiquidityProxy)]
    fn add_liquidity_proxy(
        &self,
        pair_address: Address,
        first_token_id: TokenIdentifier,
        first_token_nonce: Nonce,
        first_token_amount_min: BigUint,
        second_token_id: TokenIdentifier,
        second_token_nonce: Nonce,
        second_token_amount_min: BigUint,
    ) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_pair(&pair_address));
        sc_try!(self.require_params_not_empty());
        let proxy_params = self.params().get();

        let caller = self.blockchain().get_caller();
        require!(first_token_id != second_token_id, "Identical tokens");
        require!(
            (first_token_nonce == 0 && second_token_nonce != 0)
                || (first_token_nonce != 0 && second_token_nonce == 0),
            "This endpoint accepts one Fungible and one SemiFungible"
        );
        let locked_asset_token_id = if self.accepted_locked_assets().contains(&first_token_id) {
            first_token_id.clone()
        } else if self.accepted_locked_assets().contains(&second_token_id) {
            second_token_id.clone()
        } else {
            return sc_error!("One token should be an accepted locked asset token")
        };
        let first_token_amount = self
            .temporary_funds(&caller, &first_token_id, first_token_nonce)
            .get();
        require!(first_token_amount > 0, "First token amount is zero");
        let second_token_amount = self
            .temporary_funds(&caller, &second_token_id, second_token_nonce)
            .get();
        require!(second_token_amount > 0, "Second token amount is zero");

        // Actual 2x acceptEsdtPayment
        sc_try!(self.forward_to_pair(
            &pair_address,
            &first_token_id,
            first_token_nonce,
            &first_token_amount,
            &proxy_params,
        ));
        sc_try!(self.forward_to_pair(
            &pair_address,
            &second_token_id,
            second_token_nonce,
            &second_token_amount,
            &proxy_params,
        ));

        // Actual adding of liquidity
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.add_liquidity_gas_limit,
        );
        let result = contract_call!(self, pair_address, PairContractProxy)
            .addLiquidity(
                first_token_amount.clone(),
                second_token_amount.clone(),
                first_token_amount_min,
                second_token_amount_min,
            )
            .execute_on_dest_context(gas_limit, self.send());

        let result_tuple = result.0;
        let lp_received = result_tuple.0;
        let first_token_used = result_tuple.1;
        let second_token_used = result_tuple.2;
        require!(
            lp_received.amount > 0,
            "LP token amount should be greater than 0"
        );
        require!(
            first_token_used.token_id == first_token_id
                || second_token_used.token_id == second_token_id,
            "Bad token order"
        );

        //Recalculate temporary funds and burn unused
        let locked_asset_token_nonce: Nonce;
        let consumed_locked_tokens: BigUint;
        let asset_token_id = self.asset().token_id().get();
        if first_token_used.token_id == asset_token_id {
            consumed_locked_tokens = first_token_used.amount;
            let unused_minted_assets = first_token_amount - consumed_locked_tokens.clone();
            self.send().burn_tokens(
                &asset_token_id,
                0,
                &unused_minted_assets,
                proxy_params.burn_tokens_gas_limit,
            );
            locked_asset_token_nonce = first_token_nonce;

            self.decrease_temporary_funds_amount(
                &caller,
                &first_token_id,
                first_token_nonce,
                &consumed_locked_tokens,
            );
            self.decrease_temporary_funds_amount(
                &caller,
                &second_token_used.token_id,
                second_token_nonce,
                &second_token_used.amount,
            );
        } else if second_token_used.token_id == asset_token_id {
            consumed_locked_tokens = second_token_used.amount;
            let unused_minted_assets = second_token_amount - consumed_locked_tokens.clone();
            self.send().burn_tokens(
                &asset_token_id,
                0,
                &unused_minted_assets,
                proxy_params.burn_tokens_gas_limit,
            );
            locked_asset_token_nonce = second_token_nonce;

            self.decrease_temporary_funds_amount(
                &caller,
                &first_token_used.token_id,
                first_token_nonce,
                &first_token_used.amount,
            );
            self.decrease_temporary_funds_amount(
                &caller,
                &second_token_id,
                second_token_nonce,
                &consumed_locked_tokens,
            );
        } else {
            return sc_error!("Add liquidity did not return asset token id");
        }

        self.create_and_send_wrapped_lp_token(
            &lp_received.token_id,
            &lp_received.amount,
            &locked_asset_token_id,
            &consumed_locked_tokens,
            locked_asset_token_nonce,
            &caller,
        );

        Ok(())
    }

    #[payable("*")]
    #[endpoint(removeLiquidityProxy)]
    fn remove_liquidity_proxy(
        &self,
        pair_address: Address,
        first_token_amount_min: BigUint,
        second_token_amount_min: BigUint,
    ) -> SCResult<()> {
        sc_try!(self.require_is_intermediated_pair(&pair_address));
        sc_try!(self.require_params_not_empty());
        let proxy_params = self.params().get();

        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_nonce != 0, "Can only be called with an SFT");
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Payment amount cannot be zero");

        let wrapped_lp_token_id = self.token_id().get();
        require!(token_id == wrapped_lp_token_id, "Wrong input token");

        let caller = self.blockchain().get_caller();
        let lp_token_id = self.ask_for_lp_token_id(&pair_address, &proxy_params);
        let attributes = sc_try!(self.get_attributes(&token_id, token_nonce));
        require!(lp_token_id == attributes.lp_token_id, "Bad input address");

        let locked_asset_token_id = attributes.locked_assets_token_id;
        let asset_token_id = self.asset().token_id().get();
        let tokens_for_position =
            self.ask_for_tokens_for_position(&pair_address, &amount, &proxy_params);
        sc_try!(self.actual_remove_liquidity(
            &pair_address,
            &lp_token_id,
            &amount,
            &first_token_amount_min,
            &second_token_amount_min,
            &proxy_params
        ));

        let fungible_token_id: TokenIdentifier;
        let fungible_token_amount: BigUint;
        let assets_received: BigUint;
        let locked_assets_invested =
            amount.clone() * attributes.locked_assets_invested / attributes.lp_token_total_amount;
        require!(
            locked_assets_invested > 0,
            "Not enough wrapped lp token provided"
        );
        if tokens_for_position.0.token_id == asset_token_id {
            assets_received = tokens_for_position.0.amount;
            fungible_token_id = tokens_for_position.1.token_id;
            fungible_token_amount = tokens_for_position.1.amount;
        } else if tokens_for_position.1.token_id == asset_token_id {
            assets_received = tokens_for_position.1.amount;
            fungible_token_id = tokens_for_position.0.token_id;
            fungible_token_amount = tokens_for_position.0.amount;
        } else {
            return sc_error!("Bad tokens received from pair SC");
        }

        //Send back the tokens removed from pair sc.
        self.send()
            .transfer_tokens(&fungible_token_id, 0, &fungible_token_amount, &caller);
        let locked_assets_to_send =
            core::cmp::min(assets_received.clone(), locked_assets_invested.clone());
        self.send().transfer_tokens(
            &locked_asset_token_id,
            attributes.locked_assets_nonce,
            &locked_assets_to_send,
            &caller,
        );

        //Do cleanup
        if assets_received > locked_assets_invested {
            let difference = assets_received - locked_assets_invested.clone();
            self.send()
                .transfer_tokens(&asset_token_id, 0, &difference, &caller);
            self.send().burn_tokens(
                &asset_token_id,
                0,
                &locked_assets_invested,
                proxy_params.burn_tokens_gas_limit,
            );
        } else if assets_received < locked_assets_invested {
            let difference = locked_assets_invested - assets_received.clone();
            self.send().burn_tokens(
                &locked_asset_token_id,
                attributes.locked_assets_nonce,
                &difference,
                proxy_params.burn_tokens_gas_limit,
            );
            self.send().burn_tokens(
                &asset_token_id,
                0,
                &assets_received,
                proxy_params.burn_tokens_gas_limit,
            );
        } else {
            self.send().burn_tokens(
                &asset_token_id,
                0,
                &assets_received,
                proxy_params.burn_tokens_gas_limit,
            );
        }

        self.send().burn_tokens(
            &wrapped_lp_token_id,
            token_nonce,
            &amount,
            proxy_params.burn_tokens_gas_limit,
        );
        Ok(())
    }

    fn actual_remove_liquidity(
        &self,
        pair_address: &Address,
        lp_token_id: &TokenIdentifier,
        liquidity: &BigUint,
        first_token_amount_min: &BigUint,
        second_token_amount_min: &BigUint,
        proxy_params: &ProxyPairParams,
    ) -> SCResult<()> {
        let mut arg_buffer = ArgBuffer::new();
        arg_buffer.push_argument_bytes(&first_token_amount_min.to_bytes_be());
        arg_buffer.push_argument_bytes(&second_token_amount_min.to_bytes_be());
        let result = self.send().direct_esdt_execute(
            pair_address,
            lp_token_id.as_esdt_identifier(),
            liquidity,
            min(
                self.blockchain().get_gas_left(),
                proxy_params.remove_liquidity_gas_limit,
            ),
            REMOVE_LIQUIDITY_FUNC_NAME,
            &arg_buffer,
        );

        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn ask_for_lp_token_id(
        &self,
        pair_address: &Address,
        proxy_params: &ProxyPairParams,
    ) -> TokenIdentifier {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.ask_for_lp_token_gas_limit,
        );
        contract_call!(self, pair_address.clone(), PairContractProxy)
            .getLpTokenIdentifier()
            .execute_on_dest_context(gas_limit, self.send())
    }

    fn ask_for_tokens_for_position(
        &self,
        pair_address: &Address,
        liquidity: &BigUint,
        proxy_params: &ProxyPairParams,
    ) -> (TokenAmountPair<BigUint>, TokenAmountPair<BigUint>) {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            proxy_params.ask_for_lp_token_gas_limit,
        );
        let result = contract_call!(self, pair_address.clone(), PairContractProxy)
            .getTokensForGivenPosition(liquidity.clone())
            .execute_on_dest_context(gas_limit, self.send());
        result.0
    }

    fn get_attributes(
        &self,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
    ) -> SCResult<WrappedLpTokenAttributes<BigUint>> {
        let token_info = self.blockchain().get_esdt_token_data(
            &self.blockchain().get_sc_address(),
            token_id.as_esdt_identifier(),
            token_nonce,
        );

        let attributes = token_info.decode_attributes::<WrappedLpTokenAttributes<BigUint>>();
        match attributes {
            Result::Ok(decoded_obj) => Ok(decoded_obj),
            Result::Err(_) => {
                return sc_error!("Decoding error");
            }
        }
    }

    fn create_and_send_wrapped_lp_token(
        &self,
        lp_token_id: &TokenIdentifier,
        lp_token_amount: &BigUint,
        locked_token_id: &TokenIdentifier,
        locked_tokens_consumed: &BigUint,
        locked_tokens_nonce: Nonce,
        caller: &Address,
    ) {
        let wrapped_lp_token_id = self.token_id().get();
        self.create_wrapped_lp_token(
            &wrapped_lp_token_id,
            lp_token_id,
            lp_token_amount,
            locked_token_id,
            locked_tokens_consumed,
            locked_tokens_nonce,
        );
        let nonce = self.token_nonce().get();
        self.send()
            .transfer_tokens(&wrapped_lp_token_id, nonce, lp_token_amount, caller);
    }

    fn create_wrapped_lp_token(
        &self,
        wrapped_lp_token_id: &TokenIdentifier,
        lp_token_id: &TokenIdentifier,
        lp_token_amount: &BigUint,
        locked_token_id: &TokenIdentifier,
        locked_tokens_consumed: &BigUint,
        locked_tokens_nonce: Nonce,
    ) {
        let attributes = WrappedLpTokenAttributes::<BigUint> {
            lp_token_id: lp_token_id.clone(),
            lp_token_total_amount: lp_token_amount.clone(),
            locked_assets_token_id: locked_token_id.clone(),
            locked_assets_invested: locked_tokens_consumed.clone(),
            locked_assets_nonce: locked_tokens_nonce,
        };
        self.send()
            .esdt_nft_create::<WrappedLpTokenAttributes<BigUint>>(
                self.blockchain().get_gas_left(),
                wrapped_lp_token_id.as_esdt_identifier(),
                lp_token_amount,
                &BoxedBytes::empty(),
                &BigUint::zero(),
                &H256::zero(),
                &attributes,
                &[BoxedBytes::empty()],
            );
        self.increase_nonce();
    }

    fn send_temporary_funds_back(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
    ) {
        let amount = self.temporary_funds(caller, token_id, token_nonce).get();
        self.send()
            .transfer_tokens(token_id, token_nonce, &amount, caller);
        self.temporary_funds(caller, token_id, token_nonce).clear();
    }

    fn forward_to_pair(
        &self,
        pair_address: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        amount: &BigUint,
        proxy_params: &ProxyPairParams,
    ) -> SCResult<()> {
        let token_to_send: TokenIdentifier;
        if token_nonce == 0 {
            token_to_send = token_id.clone();
        } else {
            let asset_token_id = self.asset().token_id().get();
            self.send().esdt_local_mint(
                min(
                    self.blockchain().get_gas_left(),
                    proxy_params.mint_tokens_gas_limit,
                ),
                &asset_token_id.as_esdt_identifier(),
                amount,
            );
            token_to_send = asset_token_id;
        };
        let result = self.send().direct_esdt_execute(
            pair_address,
            token_to_send.as_esdt_identifier(),
            amount,
            min(
                self.blockchain().get_gas_left(),
                proxy_params.accept_esdt_payment_gas_limit,
            ),
            ACCEPT_ESDT_PAYMENT_FUNC_NAME,
            &ArgBuffer::new(),
        );

        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn increase_temporary_funds_amount(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        increase_amount: &BigUint,
    ) {
        let old_amount = self.temporary_funds(caller, token_id, token_nonce).get();
        let new_amount = old_amount + increase_amount.clone();
        self.temporary_funds(caller, token_id, token_nonce)
            .set(&new_amount);
    }

    fn increase_nonce(&self) -> Nonce {
        let new_nonce = self.token_nonce().get() + 1;
        self.token_nonce().set(&new_nonce);
        new_nonce
    }

    fn decrease_temporary_funds_amount(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        decrease_amount: &BigUint,
    ) {
        let old_amount = self.temporary_funds(caller, token_id, token_nonce).get();
        let new_amount = old_amount - decrease_amount.clone();
        if new_amount > 0 {
            self.temporary_funds(caller, token_id, token_nonce)
                .set(&new_amount);
        } else {
            self.temporary_funds(caller, token_id, token_nonce).clear();
        }
    }

    fn require_is_intermediated_pair(&self, address: &Address) -> SCResult<()> {
        require!(
            self.intermediated_pairs().contains(address),
            "Not an intermediated pair"
        );
        Ok(())
    }

    fn require_is_accepted_locked_asset(&self, token_id: &TokenIdentifier) -> SCResult<()> {
        require!(
            self.accepted_locked_assets().contains(token_id),
            "Not an accepted locked asset"
        );
        Ok(())
    }

    fn require_permissions(&self) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        Ok(())
    }

    fn require_params_not_empty(&self) -> SCResult<()> {
        require!(!self.params().is_empty(), "Empty proxy_params");
        Ok(())
    }

    #[view(getTemporaryFunds)]
    #[storage_mapper("funds")]
    fn temporary_funds(
        &self,
        caller: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
    ) -> SingleValueMapper<Self::Storage, BigUint>;

    #[view(getIntermediatedPairs)]
    #[storage_mapper("intermediated_pairs")]
    fn intermediated_pairs(&self) -> SetMapper<Self::Storage, Address>;

    #[view(getAcceptedLockedAssetsTokenIds)]
    #[storage_mapper("accepted_locked_assets")]
    fn accepted_locked_assets(&self) -> SetMapper<Self::Storage, TokenIdentifier>;

    #[view(getWrappedLpTokenId)]
    #[storage_mapper("wrapped_lp_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("wrapped_tp_token_nonce")]
    fn token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;

    #[storage_mapper("proxy_farm_params")]
    fn params(&self) -> SingleValueMapper<Self::Storage, ProxyPairParams>;
}
