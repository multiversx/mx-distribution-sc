elrond_wasm::imports!();
elrond_wasm::derive_imports!();

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct UserAssetKey {
    pub user_address: Address,
    pub spread_epoch: u64,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct CommunityDistribution<BigUint: BigUintApi> {
    pub total_amount: BigUint,
    pub spread_epoch: u64,
    pub after_planning_amount: BigUint,
}
