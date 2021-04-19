elrond_wasm::imports!();
elrond_wasm::derive_imports!();

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct UserRewardKey {
    pub user_address: Address,
    pub unlock_epoch: u64,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct CommunityReward<BigUint: BigUintApi> {
    pub total_amount: BigUint,
    pub unlock_epoch: u64,
    pub after_planning_amount: BigUint,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct UserReward<BigUint: BigUintApi> {
    pub user_address: Address,
    pub amount: BigUint,
}
