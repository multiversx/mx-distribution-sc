#![allow(non_snake_case)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::clippy::comparison_chain)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
use elrond_wasm::{contract_call, only_owner, require, sc_error, sc_try};

pub use crate::asset::*;
pub use crate::global_op::*;
pub use crate::locked_asset::*;
use core::cmp::min;

const ADD_LIQUIDITY_GAS_LIMIT: u64 = 30000000;
const ACCEPT_ESDT_PAYMENT_GAS_LIMIT: u64 = 25000000;
const ASK_FOR_LP_TOKEN_ID_GAS_LIMIT: u64 = 25000000;
const ASK_FOR_TOKENS_GAS_LIMIT: u64 = 25000000;
const REMOVE_LIQUIDITY_GAS_LIMIT: u64 = 40000000;

const ACCEPT_ESDT_PAYMENT_FUNC_NAME: &[u8] = b"acceptEsdtPayment";
const REMOVE_LIQUIDITY_FUNC_NAME: &[u8] = b"removeLiquidity";

type AddLiquidityResultType<BigUint> =
    MultiResult3<TokenAmountPair<BigUint>, TokenAmountPair<BigUint>, TokenAmountPair<BigUint>>;

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct TokenAmountPair<BigUint: BigUintApi> {
    pub token_id: TokenIdentifier,
    pub amount: BigUint,
}

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct WrappedLpTokenAttributes<BigUint: BigUintApi> {
    lp_token_id: TokenIdentifier,
    lp_token_total_amount: BigUint,
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

#[elrond_wasm_derive::module(ProxyPairModuleImpl)]
pub trait ProxyPairModule {
    #[module(AssetModuleImpl)]
    fn asset(&self) -> AssetModuleImpl<T, BigInt, BigUint>;

    #[module(LockedAssetModuleImpl)]
    fn locked_asset(&self) -> LockedAssetModuleImpl<T, BigInt, BigUint>;

    #[module(GlobalOperationModuleImpl)]
    fn global_operation(&self) -> GlobalOperationModuleImpl<T, BigInt, BigUint>;

    #[endpoint(addPairToIntermediate)]
    fn add_pair_to_intermediate(&self, pair_address: Address) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        self.intermediated_pairs().insert(pair_address);
        Ok(())
    }

    #[endpoint(removeIntermediatedPair)]
    fn remove_intermediated_pair(&self, pair_address: Address) -> SCResult<()> {
        only_owner!(self, "Permission denied");
        sc_try!(self.require_is_intermediated_pair(&pair_address));
        self.intermediated_pairs().remove(&pair_address);
        Ok(())
    }

    #[payable("*")]
    #[endpoint(acceptEsdtPaymentProxy)]
    fn accept_esdt_payment_proxy(&self, pair_address: Address) -> SCResult<()> {
        sc_try!(self.global_operation().require_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let token_nonce = self.call_value().esdt_token_nonce();
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Paymend amount cannot be zero");

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
        sc_try!(self.global_operation().require_not_ongoing());
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
        sc_try!(self.global_operation().require_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let caller = self.blockchain().get_caller();
        require!(first_token_id != second_token_id, "Identical tokens");
        require!(
            (first_token_nonce == 0 && second_token_nonce != 0)
                || (first_token_nonce != 0 && second_token_nonce == 0),
            "This endpoint accepts one Fungible and one SemiFungible"
        );
        let locked_asset_token_id = self.locked_asset().token_id().get();
        require!(
            first_token_id == locked_asset_token_id || second_token_id == locked_asset_token_id,
            "One token should be the locked asset token"
        );
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
        ));
        sc_try!(self.forward_to_pair(
            &pair_address,
            &second_token_id,
            second_token_nonce,
            &second_token_amount,
        ));

        // Actual adding of liquidity
        let gas_limit = core::cmp::min(self.blockchain().get_gas_left(), ADD_LIQUIDITY_GAS_LIMIT);
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
            self.asset().burn(&asset_token_id, &unused_minted_assets);
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
            self.asset().burn(&asset_token_id, &unused_minted_assets);
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
        sc_try!(self.global_operation().require_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_nonce != 0, "Can only be called with an SFT");
        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Paymend amount cannot be zero");

        let wrapped_lp_token_id = self.token_id().get();
        require!(token_id == wrapped_lp_token_id, "Wrong input token");

        let caller = self.blockchain().get_caller();
        let lp_token_id = self.ask_for_lp_token_id(&pair_address);
        let attributes = sc_try!(self.get_attributes(&token_id, token_nonce));
        require!(lp_token_id == attributes.lp_token_id, "Bad input address");

        let locked_asset_token_id = self.locked_asset().token_id().get();
        let asset_token_id = self.asset().token_id().get();
        let tokens_for_position = self.ask_for_tokens_for_position(&pair_address, &amount);
        sc_try!(self.actual_remove_liquidity(
            &pair_address,
            &lp_token_id,
            &amount,
            &first_token_amount_min,
            &second_token_amount_min,
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
        self.direct_transfer_fungible(&caller, &fungible_token_id, &fungible_token_amount);
        let locked_assets_to_send =
            core::cmp::min(assets_received.clone(), locked_assets_invested.clone());
        self.locked_asset().send_tokens(
            &locked_asset_token_id,
            attributes.locked_assets_nonce,
            &locked_assets_to_send,
            &caller,
        );

        //Do cleanup
        if assets_received > locked_assets_invested {
            let difference = assets_received - locked_assets_invested.clone();
            self.direct_transfer_fungible(&caller, &asset_token_id, &difference);
            self.asset().burn(&asset_token_id, &locked_assets_invested);
        } else if assets_received < locked_assets_invested {
            let difference = locked_assets_invested - assets_received.clone();
            self.locked_asset().burn_tokens(
                &locked_asset_token_id,
                attributes.locked_assets_nonce,
                &difference,
            );
            self.asset().burn(&asset_token_id, &assets_received);
        } else {
            self.asset().burn(&asset_token_id, &assets_received);
        }

        self.burn_tokens(&wrapped_lp_token_id, token_nonce, &amount);
        Ok(())
    }

    fn burn_tokens(&self, token: &TokenIdentifier, nonce: Nonce, amount: &BigUint) {
        self.send().esdt_nft_burn(
            self.blockchain().get_gas_left(),
            token.as_esdt_identifier(),
            nonce,
            amount,
        );
    }

    fn actual_remove_liquidity(
        &self,
        pair_address: &Address,
        lp_token_id: &TokenIdentifier,
        liquidity: &BigUint,
        first_token_amount_min: &BigUint,
        second_token_amount_min: &BigUint,
    ) -> SCResult<()> {
        let mut arg_buffer = ArgBuffer::new();
        arg_buffer.push_argument_bytes(&first_token_amount_min.to_bytes_be());
        arg_buffer.push_argument_bytes(&second_token_amount_min.to_bytes_be());
        let result = self.send().direct_esdt_execute(
            pair_address,
            lp_token_id.as_esdt_identifier(),
            liquidity,
            min(self.blockchain().get_gas_left(), REMOVE_LIQUIDITY_GAS_LIMIT),
            REMOVE_LIQUIDITY_FUNC_NAME,
            &arg_buffer,
        );

        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn ask_for_lp_token_id(&self, pair_address: &Address) -> TokenIdentifier {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            ASK_FOR_LP_TOKEN_ID_GAS_LIMIT,
        );
        contract_call!(self, pair_address.clone(), PairContractProxy)
            .getLpTokenIdentifier()
            .execute_on_dest_context(gas_limit, self.send())
    }

    fn ask_for_tokens_for_position(
        &self,
        pair_address: &Address,
        liquidity: &BigUint,
    ) -> (TokenAmountPair<BigUint>, TokenAmountPair<BigUint>) {
        let gas_limit = core::cmp::min(self.blockchain().get_gas_left(), ASK_FOR_TOKENS_GAS_LIMIT);
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
        locked_tokens_consumed: &BigUint,
        locked_tokens_nonce: Nonce,
        caller: &Address,
    ) {
        let wrapped_lp_token_id = self.token_id().get();
        self.create_wrapped_lp_token(
            &wrapped_lp_token_id,
            lp_token_id,
            lp_token_amount,
            locked_tokens_consumed,
            locked_tokens_nonce,
        );
        let nonce = self.token_nonce().get();
        self.send_wrapped_lp_token(&wrapped_lp_token_id, nonce, lp_token_amount, caller);
    }

    fn send_wrapped_lp_token(
        &self,
        wrapped_lp_token_id: &TokenIdentifier,
        wrapped_lp_token_nonce: Nonce,
        amount: &BigUint,
        caller: &Address,
    ) {
        let _ = self.send().direct_esdt_nft_via_transfer_exec(
            caller,
            wrapped_lp_token_id.as_esdt_identifier(),
            wrapped_lp_token_nonce,
            &amount,
            &[],
        );
    }

    fn create_wrapped_lp_token(
        &self,
        wrapped_lp_token_id: &TokenIdentifier,
        lp_token_id: &TokenIdentifier,
        lp_token_amount: &BigUint,
        locked_tokens_consumed: &BigUint,
        locked_tokens_nonce: Nonce,
    ) {
        let attributes = WrappedLpTokenAttributes::<BigUint> {
            lp_token_id: lp_token_id.clone(),
            lp_token_total_amount: lp_token_amount.clone(),
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
        self.direct_transfer_esdt(caller, token_id, token_nonce, &amount);
        self.temporary_funds(caller, token_id, token_nonce).clear();
    }

    fn forward_to_pair(
        &self,
        pair_address: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        amount: &BigUint,
    ) -> SCResult<()> {
        let token_to_send: TokenIdentifier;
        if token_nonce == 0 {
            token_to_send = token_id.clone();
        } else {
            let asset_token_id = self.asset().token_id().get();
            self.asset().mint_tokens(&asset_token_id, amount);
            token_to_send = asset_token_id;
        };
        let result = self.send().direct_esdt_execute(
            pair_address,
            token_to_send.as_esdt_identifier(),
            amount,
            min(
                self.blockchain().get_gas_left(),
                ACCEPT_ESDT_PAYMENT_GAS_LIMIT,
            ),
            ACCEPT_ESDT_PAYMENT_FUNC_NAME,
            &ArgBuffer::new(),
        );

        match result {
            Result::Ok(()) => Ok(()),
            Result::Err(_) => sc_error!("Failed to transfer to pair"),
        }
    }

    fn direct_transfer_esdt(
        &self,
        address: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        amount: &BigUint,
    ) {
        if token_nonce == 0 {
            self.direct_transfer_fungible(address, token_id, amount);
        } else {
            self.direct_transfer_non_fungible(address, token_id, token_nonce, amount);
        }
    }

    fn direct_transfer_fungible(
        &self,
        address: &Address,
        token_id: &TokenIdentifier,
        amount: &BigUint,
    ) {
        if amount > &0 {
            let _ = self.send().direct_esdt_via_transf_exec(
                address,
                token_id.as_esdt_identifier(),
                amount,
                &[],
            );
        }
    }

    fn direct_transfer_non_fungible(
        &self,
        address: &Address,
        token_id: &TokenIdentifier,
        token_nonce: Nonce,
        amount: &BigUint,
    ) {
        if amount > &0 {
            let _ = self.send().direct_esdt_nft_via_transfer_exec(
                address,
                token_id.as_esdt_identifier(),
                token_nonce,
                amount,
                &[],
            );
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

    #[view(getWrappedLpTokenId)]
    #[storage_mapper("wrapped_lp_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("wrapped_tp_token_nonce")]
    fn token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;
}
