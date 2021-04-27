#![allow(non_snake_case)]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

type Nonce = u64;
pub use crate::asset::*;
pub use crate::global_op::*;
pub use crate::locked_asset::*;
use core::cmp::min;

const ADD_LIQUIDITY_GAS_LIMIT: u64 = 25000000;
const ACCEPT_ESDT_PAYMENT_GAS_LIMIT: u64 = 15000000;
const RECLAIM_TEMPORARY_FUNDS_GAS_LIMIT: u64 = 25000000;

const ACCEPT_ESDT_PAYMENT_FUNC_NAME: &[u8] = b"acceptEsdtPayment";

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
    locked_assets_invested: BigUint,
    locked_assets_unlock_milestones: Vec<UnlockMilestone>,
}

#[elrond_wasm_derive::callable(PairContractProxy)]
pub trait PairContract {
    fn reclaimTemporaryFunds(&self) -> ContractCall<BigUint, ()>;
    fn addLiquidity(
        &self,
        first_token_amount_desired: BigUint,
        first_token_amount_min: BigUint,
        second_token_amount_desired: BigUint,
        second_token_amount_min: BigUint,
    ) -> ContractCall<BigUint, AddLiquidityResultType<BigUint>>;
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

    #[payable("*")]
    #[endpoint(acceptEsdtPaymentProxy)]
    fn accept_esdt_payment_proxy(&self, pair_address: Address) -> SCResult<()> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_nonce == 0, "Only fungible tokens are accepted");

        let (amount, token_id) = self.call_value().payment_token_pair();
        require!(amount != 0, "Paymend amount cannot be zero");

        let caller = self.blockchain().get_caller();
        sc_try!(self.forward_to_pair(&pair_address, &token_id, token_nonce, &amount));
        self.increase_temporary_funds_amount(&caller, &token_id, token_nonce, &amount);
        Ok(())
    }

    #[endpoint(reclaimTemporaryFundsProxy)]
    fn reclaim_temporary_funds_proxy(
        &self,
        pair_address: Address,
        first_token_id: TokenIdentifier,
        first_token_nonce: Nonce,
        second_token_id: TokenIdentifier,
        second_token_nonce: Nonce,
    ) -> SCResult<()> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        self.reclaim_all_temporary_funds_from_pair(pair_address);
        let caller = self.blockchain().get_caller();
        self.send_temporary_funds_back(&caller, &first_token_id, first_token_nonce);
        self.send_temporary_funds_back(&caller, &second_token_id, second_token_nonce);
        self.asset().burn_balance();
        Ok(())
    }

    #[payable("*")]
    #[endpoint(addLiquidityProxy)]
    fn add_liquidity_proxy(
        &self,
        pair_address: Address,
        first_token_amount: BigUint,
        first_token_amount_min: BigUint,
        second_token_amount: BigUint,
        second_token_amount_min: BigUint,
    ) -> SCResult<()> {
        sc_try!(self.require_global_operation_not_ongoing());
        sc_try!(self.require_is_intermediated_pair(&pair_address));

        let token_nonce = self.call_value().esdt_token_nonce();
        require!(token_nonce != 0, "Only semi-fungible tokens are accepted");

        let (amount, token_id) = self.call_value().payment_token_pair();
        let locked_asset_token_id = self.locked_asset().token_id().get();
        require!(
            token_id == locked_asset_token_id,
            "Payment should be locked asset token id"
        );
        require!(amount != 0, "Paymend amount cannot be zero");

        //Read the attributes of locked asset that was received
        let locked_asset_attr =
            sc_try!(self.locked_asset().get_attributes(&token_id, token_nonce));

        //Transfers the locked assets as an asset to the pair sc.
        let caller = self.blockchain().get_caller();
        sc_try!(self.forward_to_pair(&pair_address, &token_id, token_nonce, &amount));
        self.increase_temporary_funds_amount(&caller, &token_id, token_nonce, &amount);

        // Actual adding of liquidity
        let gas_limit = core::cmp::min(self.blockchain().get_gas_left(), ADD_LIQUIDITY_GAS_LIMIT);
        let result = contract_call!(self, pair_address, PairContractProxy)
            .addLiquidity(
                first_token_amount,
                first_token_amount_min,
                second_token_amount,
                second_token_amount_min,
            )
            .execute_on_dest_context(gas_limit, self.send());

        let result_tuple = result.0;
        let lp_received = result_tuple.0;
        let first_token_used = result_tuple.1;
        let second_token_used = result_tuple.2;

        //Recalculate temporary funds and burn unused
        let consumed_locked_tokens: BigUint;
        let asset_token_id = self.asset().token_id().get();
        if first_token_used.token_id == asset_token_id {
            consumed_locked_tokens = first_token_used.amount;
            let unused_minted_assets = amount - consumed_locked_tokens.clone();
            self.asset().burn(&asset_token_id, &unused_minted_assets);
            self.locked_asset()
                .burn_tokens(&token_id, token_nonce, &consumed_locked_tokens);
            self.decrease_temporary_funds_amount(
                &caller,
                &token_id,
                token_nonce,
                &consumed_locked_tokens,
            );
            self.decrease_temporary_funds_amount(
                &caller,
                &second_token_used.token_id,
                0u64,
                &second_token_used.amount,
            );
        } else if second_token_used.token_id == asset_token_id {
            consumed_locked_tokens = second_token_used.amount;
            let unused_minted_assets = amount - consumed_locked_tokens.clone();
            self.asset().burn(&asset_token_id, &unused_minted_assets);
            self.locked_asset()
                .burn_tokens(&token_id, token_nonce, &consumed_locked_tokens);
            self.decrease_temporary_funds_amount(
                &caller,
                &first_token_used.token_id,
                0u64,
                &first_token_used.amount,
            );
            self.decrease_temporary_funds_amount(
                &caller,
                &token_id,
                token_nonce,
                &consumed_locked_tokens,
            );
        } else {
            return sc_error!("Add liquidity did not return asset token id");
        }

        self.create_and_send_wrapped_lp_token(
            &lp_received.token_id,
            &lp_received.amount,
            &consumed_locked_tokens,
            &locked_asset_attr.unlock_milestones,
            &caller,
        );

        Ok(())
    }

    fn create_and_send_wrapped_lp_token(
        &self,
        lp_token_id: &TokenIdentifier,
        lp_token_amount: &BigUint,
        locked_tokens_consumed: &BigUint,
        unlock_milestones: &[UnlockMilestone],
        caller: &Address,
    ) {
        let wrapped_lp_token_id = self.token_id().get();
        self.create_wrapped_lp_token(
            &wrapped_lp_token_id,
            lp_token_id,
            lp_token_amount,
            locked_tokens_consumed,
            unlock_milestones,
        );
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
            &lp_token_amount,
            &[],
        );
    }

    fn create_wrapped_lp_token(
        &self,
        wrapped_lp_token_id: &TokenIdentifier,
        lp_token_id: &TokenIdentifier,
        lp_token_amount: &BigUint,
        locked_tokens_consumed: &BigUint,
        unlock_milestones: &[UnlockMilestone],
    ) {
        let attributes = WrappedLpTokenAttributes::<BigUint> {
            lp_token_id: lp_token_id.clone(),
            locked_assets_invested: locked_tokens_consumed.clone(),
            locked_assets_unlock_milestones: unlock_milestones.to_vec(),
        };

        self.send().esdt_nft_create::<WrappedLpTokenAttributes<BigUint>>(
            self.blockchain().get_gas_left(),
            wrapped_lp_token_id.as_esdt_identifier(),
            lp_token_amount,
            &BoxedBytes::empty(),
            &BigUint::zero(),
            &H256::zero(),
            &attributes,
            &[BoxedBytes::empty()],
        );
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

    fn reclaim_all_temporary_funds_from_pair(&self, pair_address: Address) {
        let gas_limit = core::cmp::min(
            self.blockchain().get_gas_left(),
            RECLAIM_TEMPORARY_FUNDS_GAS_LIMIT,
        );
        contract_call!(self, pair_address, PairContractProxy)
            .reclaimTemporaryFunds()
            .execute_on_dest_context(gas_limit, self.send());
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
        self.temporary_funds(caller, token_id, token_nonce)
            .set(&new_amount);
    }

    fn require_global_operation_not_ongoing(&self) -> SCResult<()> {
        require!(
            self.global_operation().is_ongoing().get(),
            "Global operation ongoing"
        );
        Ok(())
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
    #[storage_mapper("wrapped_tp_token_id")]
    fn token_id(&self) -> SingleValueMapper<Self::Storage, TokenIdentifier>;

    #[storage_mapper("wrapped_tp_token_nonce")]
    fn token_nonce(&self) -> SingleValueMapper<Self::Storage, Nonce>;
}
