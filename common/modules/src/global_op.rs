elrond_wasm::imports!();
elrond_wasm::derive_imports!();

#[elrond_wasm_derive::module(GlobalOperationModuleImpl)]
pub trait GlobalOperationModule {
    fn start(&self) {
        self.is_ongoing().set(&true);
    }

    fn stop(&self) {
        self.is_ongoing().set(&false);
    }

    #[storage_mapper("global_operation_ongoing")]
    fn is_ongoing(&self) -> SingleValueMapper<Self::Storage, bool>;
}
