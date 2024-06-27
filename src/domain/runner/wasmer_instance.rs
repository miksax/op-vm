use std::sync::Arc;

use napi::bindgen_prelude::{Buffer, Promise};
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction};
use tokio::runtime::Runtime;
use wasmer::{CompilerConfig, ExportError, Function, FunctionEnv, FunctionEnvMut, imports, Imports, Instance, Memory, MemoryAccessError, MemoryView, Module, RuntimeError, Store, StoreMut, Value};
use wasmer::sys::{BaseTunables, EngineBuilder};
use wasmer_compiler_singlepass::Singlepass;
use wasmer_middlewares::metering::{get_remaining_points, MeteringPoints, set_remaining_points};
use wasmer_middlewares::Metering;
use wasmer_types::Target;

use crate::domain::contract::{AbortData, CustomEnv};
use crate::domain::runner::RunnerInstance;
use crate::domain::vm::{get_op_cost, LimitingTunables};
use crate::interfaces::ThreadSafeJsImportResponse;

pub struct WasmerInstance {
    store: Store,
    instance: Instance,
    env: FunctionEnv<CustomEnv>,
}

impl WasmerInstance {
    pub fn new(bytecode: &[u8], max_gas: u64, load_function: ThreadsafeFunction<ThreadSafeJsImportResponse, ErrorStrategy::CalleeHandled>) -> anyhow::Result<Self> {
        let metering = Arc::new(Metering::new(max_gas, get_op_cost));

        let mut compiler = Singlepass::default();
        compiler.canonicalize_nans(true);
        compiler.push_middleware(metering);
        compiler.enable_verifier();

        let base = BaseTunables::for_target(&Target::default());
        let tunables = LimitingTunables::new(base, 16, 1024 * 1024);

        let mut engine = EngineBuilder::new(compiler).set_features(None).engine();
        engine.set_tunables(tunables);

        let mut store = Store::new(engine);
        let instance = CustomEnv::new(load_function)?;
        let env = FunctionEnv::new(&mut store, instance);

        fn abort(
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

            return Err(RuntimeError::new("Execution aborted"));
        }

        fn deploy_from_address_mut(mut context: FunctionEnvMut<CustomEnv>, ptr: u32) -> Result<u32, RuntimeError> {
            let (env, mut store): (&mut CustomEnv, StoreMut) = context.data_and_store_mut();

            let memory: Memory = env.memory.clone().unwrap();
            let instance: Instance = env.instance.clone().unwrap();

            let view: MemoryView = memory.view(&store);

            let data: Vec<u8> = env.read_buffer(&view, ptr).map_err(|_e| {
                RuntimeError::new("Error lifting typed array")
            })?;

            let load_func: &ThreadsafeFunction<ThreadSafeJsImportResponse> = &env.load_function;

            let deploy = {
                async move {

                    let response: ThreadSafeJsImportResponse = ThreadSafeJsImportResponse {
                        buffer: data,
                    };

                    let response: Result<Promise<Buffer>, RuntimeError> = load_func.call_async(Ok(response)).await.map_err(|_e| {
                        RuntimeError::new("Error calling load function")
                    });

                    let promise: Promise<Buffer> = response?;

                    let data: Buffer = promise.await.map_err(|_e| {
                        RuntimeError::new("Error awaiting promise")
                    })?;

                    let data: Vec<u8> = data.into();
                    Ok(data)
                }
            };

            let rt: Runtime = Runtime::new().unwrap();
            let response: Result<Vec<u8>, RuntimeError> = rt.block_on(deploy);

            let data: Vec<u8> = response?;

            let value: i64 = env.write_buffer(&instance, &mut store, &data, 13, 0).map_err(|_e| {
                RuntimeError::new("Error writing buffer")
            })?;

            Ok(value as u32)
        }

        let abort_typed = Function::new_typed_with_env(&mut store, &env, abort);
        let deploy_from_address_typed = Function::new_typed_with_env(&mut store, &env, deploy_from_address_mut);

        let import_object: Imports = imports! {
            "env" => {
                "abort" => abort_typed,
                "deployFromAddress" => deploy_from_address_typed,
            }
        };

        let module: Module = Module::new(&store, &bytecode)?;
        let instance: Instance = Instance::new(&mut store, &module, &import_object)?;

        env.as_mut(&mut store).memory = Some(Self::get_memory(&instance).clone());
        env.as_mut(&mut store).instance = Some(instance.clone());

        Ok(Self {
            store,
            instance,
            env,
        })
    }

    fn get_memory(instance: &Instance) -> &Memory {
        instance.exports.get_memory("memory").unwrap()
    }

    fn get_function<'a>(
        instance: &'a Instance,
        function: &str,
    ) -> Result<&'a Function, ExportError> {
        instance.exports.get_function(function)
    }
}

impl RunnerInstance for WasmerInstance {
    fn call(&mut self, function: &str, params: &[Value]) -> anyhow::Result<Box<[Value]>> {
        let export = Self::get_function(&self.instance, function)?;
        let result = export.call(&mut self.store, params)?;
        Ok(result)
    }

    fn read_memory(&self, offset: u64, length: u64) -> Result<Vec<u8>, RuntimeError> {
        let memory = Self::get_memory(&self.instance);
        let view = memory.view(&self.store);

        let mut buffer: Vec<u8> = vec![0; length as usize];
        view.read(offset, &mut buffer).unwrap();

        Ok(buffer)
    }

    fn write_memory(&self, offset: u64, data: &[u8]) -> Result<(), MemoryAccessError> {
        let memory = Self::get_memory(&self.instance);
        let view = memory.view(&self.store);
        view.write(offset, data)
    }

    fn get_remaining_gas(&mut self) -> u64 {
        let remaining_points = get_remaining_points(&mut self.store, &self.instance);
        match remaining_points {
            MeteringPoints::Remaining(remaining) => remaining,
            MeteringPoints::Exhausted => 0,
        }
    }

    fn set_remaining_gas(&mut self, gas: u64) {
        set_remaining_points(&mut self.store, &self.instance, gas);
    }

    fn get_abort_data(&self) -> Option<AbortData> {
        self.env.as_ref(&self.store).abort_data
    }
}
