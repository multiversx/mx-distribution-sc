#![no_std]

elrond_wasm::imports!();
elrond_wasm::derive_imports!();

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct UserAssetKey {
    pub user_address: Address,
    pub spread_epoch: u64,
    pub locked_asset: bool,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi, NestedEncode, NestedDecode, Clone, Copy)]
pub struct UnlockMilestone {
    pub unlock_epoch: u64,
    pub unlock_precent: u8,
}

#[derive(TopEncode, TopDecode, PartialEq, TypeAbi)]
pub struct CommunityDistribution<BigUint: BigUintApi> {
    pub total_amount: BigUint,
    pub spread_epoch: u64,
    pub after_planning_amount: BigUint,
    pub unlock_milestones: Vec<UnlockMilestone>,
}

#[derive(TopEncode, TopDecode, TypeAbi)]
pub struct LockedTokenAttributes {
    pub unlock_milestones: Vec<UnlockMilestone>,
}
