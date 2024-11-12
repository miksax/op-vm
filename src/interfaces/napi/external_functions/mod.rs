pub use self::{
    call_other_contract_external_function::*, console_log_external_function::*,
    deploy_from_address_external_function::*, emit_external_function::*, external_function::*,
    generic_external_function::*, inputs_external_function::*, next_pointer_value_greater_than_external_function::*, outputs_external_function::*,
    storage_load_external_function::*, storage_store_external_function::*,
};

mod call_other_contract_external_function;
mod console_log_external_function;
mod deploy_from_address_external_function;
mod external_function;
mod generic_external_function;
mod storage_load_external_function;
mod storage_store_external_function;
mod emit_external_function;
mod inputs_external_function;
mod outputs_external_function;
mod next_pointer_value_greater_than_external_function;
