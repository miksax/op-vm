use bech32::{segwit, Hrp};
use ripemd::{Digest, Ripemd160};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::runtime::Runtime;
use wasmer::{FunctionEnvMut, RuntimeError, StoreMut};

use crate::domain::assembly_script::AssemblyScript;
use crate::domain::runner::{exported_import_functions, AbortData, CustomEnv, InstanceWrapper, CALL_COST, DEPLOY_COST, EMIT_COST, ENCODE_ADDRESS_COST, INPUTS_COST, IS_VALID_BITCOIN_ADDRESS_COST, LOAD_COST, NEXT_POINTER_GREATER_THAN_COST, OUTPUTS_COST, RIMD160_COST, SHA256_COST, STORE_COST, STORE_REFUND_ZERO};
use crate::interfaces::ExternalFunction;

fn safe_slice(vec: &[u8], start: usize, end: usize) -> Option<&[u8]> {
    vec.get(start..end)
}

pub fn abort_import(
    mut env: FunctionEnvMut<CustomEnv>,
    message: u32,
    file_name: u32,
    line: u32,
    column: u32,
) -> Result<(), RuntimeError> {
    let data = env.data_mut();
    data.abort_data = Some(AbortData {
        message,
        file_name,
        line,
        column,
    });

    Err(RuntimeError::new("Execution aborted"))
}

pub fn storage_load_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, store) = context.data_and_store_mut();
    load_pointer_external_import(env, store, &env.storage_load_external, ptr, LOAD_COST, &env.runtime, &env.refunded_pointers)
}

pub fn storage_next_pointer_greater_than_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, store) = context.data_and_store_mut();
    load_pointer_external_import(env, store, &env.next_pointer_value_greater_than_external, ptr, NEXT_POINTER_GREATER_THAN_COST, &env.runtime, &env.refunded_pointers)
}

pub fn storage_store_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, store) = context.data_and_store_mut();
    store_pointer_external_import(env, store, &env.storage_store_external, ptr, STORE_COST, &env.runtime, STORE_REFUND_ZERO)
}

pub fn call_other_contract_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, CALL_COST);

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let result = &env.call_other_contract_external.execute(&data, &env.runtime)?;

    let call_execution_cost_bytes = safe_slice(&result, 0, 8).ok_or(RuntimeError::new("Invalid buffer"))?;
    let response = safe_slice(&result, 8, result.len()).ok_or(RuntimeError::new("Invalid buffer"))?;

    let value = AssemblyScript::write_buffer(&mut store, &instance, &response, 13, 0).map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    let bytes = call_execution_cost_bytes.try_into().map_err(|_e| RuntimeError::new("Error converting bytes"))?;
    let call_execution_cost = u64::from_le_bytes(bytes);
    instance.use_gas(&mut store, call_execution_cost);

    Ok(value as u32)
}

pub fn inputs_import(
    mut context: FunctionEnvMut<CustomEnv>,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, INPUTS_COST);

    let result = &env.inputs_external.execute(&env.runtime)?;
    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0).map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    Ok(value as u32)
}

pub fn outputs_import(
    mut context: FunctionEnvMut<CustomEnv>,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, OUTPUTS_COST);

    let result = &env.outputs_external.execute(&env.runtime)?;
    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0).map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    Ok(value as u32)
}

pub fn deploy_from_address_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, store) = context.data_and_store_mut();
    import_external_call(
        env,
        store,
        &env.deploy_from_address_external,
        ptr,
        DEPLOY_COST,
        &env.runtime,
    )
}

pub fn encode_address_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = &env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    let network = &env.network;

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    if data.len() != 36 {
        return Err(RuntimeError::new(format!(
            "Invalid data length. Expected 32, got {}",
            data.len()
        )));
    }

    // skip 4 bytes for length
    let data = data[4..].to_vec();

    let mut ripemd = Ripemd160::new();
    ripemd.update(&data);
    let data = ripemd.finalize();

    let hrp = Hrp::parse(&network.contract_address_prefix()).expect("Valid hrp");
    let address = segwit::encode_v0(hrp, &data)
        .map_err(|e| RuntimeError::new(format!("Failed to encode address: {:?}", e)))?;

    let mut result = address.as_bytes().to_vec();
    result.push(0);

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    instance.use_gas(&mut store, ENCODE_ADDRESS_COST);

    Ok(value as u32)
}

pub fn sha256_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let result = sha256(&data)?;

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    instance.use_gas(&mut store, SHA256_COST);

    Ok(value as u32)
}

fn vec8_to_string(vec: Vec<u8>) -> String {
    String::from_utf8(vec).unwrap()
}

pub fn is_valid_bitcoin_address_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let string_data = vec8_to_string(data);
    let result = exported_import_functions::validate_bitcoin_address(&string_data, &env.network).map_err(|e| RuntimeError::new(e))?;

    let result_vec_buffer = vec![result as u8];

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result_vec_buffer, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    instance.use_gas(&mut store, IS_VALID_BITCOIN_ADDRESS_COST);

    Ok(value as u32)
}

pub fn ripemd160_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<u32, RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();

    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let result = rimemd160(&data)?;

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    instance.use_gas(&mut store, RIMD160_COST);

    Ok(value as u32)
}

fn sha256(data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let hash = Sha256::digest(data);
    let hash_as_vec: Vec<u8> = hash.to_vec();

    Ok(hash_as_vec)
}

fn rimemd160(data: &[u8]) -> Result<Vec<u8>, RuntimeError> {
    let mut ripemd = Ripemd160::new();
    ripemd.update(data);

    let hash = ripemd.finalize();
    let hash_as_vec: Vec<u8> = hash.to_vec();

    Ok(hash_as_vec)
}

pub fn console_log_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<(), RuntimeError> {
    let (env, store) = context.data_and_store_mut();
    let instance = &env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Memory not found"))?;

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    env.console_log_external.execute(&data, &env.runtime)
}

pub fn emit_import(
    mut context: FunctionEnvMut<CustomEnv>,
    ptr: u32,
) -> Result<(), RuntimeError> {
    let (env, mut store) = context.data_and_store_mut();
    let instance = &env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Memory not found"))?;

    instance.use_gas(&mut store, EMIT_COST);

    let data = AssemblyScript::read_buffer(&store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    env.emit_external.execute(&data, &env.runtime)
}

fn have_only_zero_bytes(data: &[u8]) -> bool {
    data.iter().all(|&x| x == 0)
}

fn verify_gas_refund_eligibility(
    refunded_pointers: &Mutex<HashMap<Vec<u8>, bool>>,
    instance: &InstanceWrapper,
    store: &mut StoreMut,
    refund_if_zero_result: u64,
    pointer: Vec<u8>,
) -> Result<(), RuntimeError> {
    let mut map = refunded_pointers.lock()
        .map_err(|_e| RuntimeError::new("Failed to lock refunded pointers"))?;

    if let Some(&is_refunded) = map.get(&pointer) {
        if !is_refunded {
            map.insert(pointer, true);

            instance.refund_gas(store, refund_if_zero_result);
        }
    }

    Ok(())
}

fn load_pointer_external_import(
    env: &CustomEnv,
    mut store: StoreMut,
    external_function: &impl ExternalFunction,
    ptr: u32,
    gas_cost: u64,
    runtime: &Runtime,
    refunded_pointers: &Mutex<HashMap<Vec<u8>, bool>>,
) -> Result<u32, RuntimeError> {
    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, gas_cost);

    let data = AssemblyScript::read_buffer(&mut store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let result = external_function.execute(&data, runtime)?;

    // Mutate the HashMap
    let mut map = refunded_pointers.lock()
        .map_err(|e| RuntimeError::new(format!("Error locking refunded pointers: {}", e)))?;

    if !have_only_zero_bytes(&result) {
        let pointer = safe_slice(&data, 0, 32).ok_or(RuntimeError::new("Invalid buffer"))?.to_vec();

        if !map.contains_key(&pointer) {
            map.insert(pointer, false);
        }
    }

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    Ok(value as u32)
}

fn import_external_call(
    env: &CustomEnv,
    mut store: StoreMut,
    external_function: &impl ExternalFunction,
    ptr: u32,
    gas_cost: u64,
    runtime: &Runtime,
) -> Result<u32, RuntimeError> {
    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, gas_cost);

    let data = AssemblyScript::read_buffer(&mut store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    let result = external_function.execute(&data, runtime)?;
    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    Ok(value as u32)
}

fn store_pointer_external_import(
    env: &CustomEnv,
    mut store: StoreMut,
    external_function: &impl ExternalFunction,
    ptr: u32,
    gas_cost: u64,
    runtime: &Runtime,
    refund_if_zero_result: u64,
) -> Result<u32, RuntimeError> {
    let instance = env
        .instance
        .clone()
        .ok_or(RuntimeError::new("Instance not found"))?;

    instance.use_gas(&mut store, gas_cost);

    let data = AssemblyScript::read_buffer(&mut store, &instance, ptr)
        .map_err(|_e| RuntimeError::new("Error lifting typed array"))?;

    if data.len() != 64 {
        return Err(RuntimeError::new("Invalid data length. Expected 64 bytes"));
    }

    let pointer = safe_slice(&data, 0, 32).ok_or(RuntimeError::new("Invalid buffer"))?.to_vec();
    let value = safe_slice(&data, 32, 64).ok_or(RuntimeError::new("Invalid buffer"))?.to_vec();

    let result = external_function.execute(&data, runtime)?;

    // Optionally verify refund eligibility
    if refund_if_zero_result > 0 && have_only_zero_bytes(&value) {
        verify_gas_refund_eligibility(
            &env.refunded_pointers,
            &instance,
            &mut store,
            refund_if_zero_result,
            pointer.clone(),
        )?;
    }

    let value = AssemblyScript::write_buffer(&mut store, &instance, &result, 13, 0)
        .map_err(|e| RuntimeError::new(format!("Error writing buffer: {}", e)))?;

    Ok(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hashes_number_correctly() {
        let data_to_hash = vec![9];
        let expected_hash = hex::decode("2b4c342f5433ebe591a1da77e013d1b72475562d48578dca8b84bac6651c3cb9").unwrap();

        let result = sha256(&data_to_hash).unwrap();

        assert_eq!(result, expected_hash);
    }

    #[test]
    fn sha256_hashes_hex_data_correctly() {
        let data_to_hash = hex::decode("e3b0c44298fc1c149afbf4c8").unwrap().to_vec();
        let expected_hash = hex::decode("10dac508c2a7d7f0f3474c6ecc23f2a4d9ddbabec1009c4810f2ff677f4c1a83").unwrap();

        let result = sha256(&data_to_hash).unwrap();

        assert_eq!(result, expected_hash);
    }
}
