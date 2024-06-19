use anyhow::anyhow;
use napi::Error;
use wasmer::{MemoryAccessError, Value};

use crate::domain::runner::RunnerInstance;

pub struct AssemblyScript;

impl AssemblyScript {
    pub fn __new(runner: &mut Box<dyn RunnerInstance>, size: i32, id: i32) -> anyhow::Result<i32> {
        let params = &[Value::I32(size), Value::I32(id)];
        let result = runner.call("__new", params)?;

        let pointer = result
            .get(0)
            .ok_or(anyhow!("can't get pointer"))?
            .i32()
            .ok_or(anyhow!("can't get pointer"))?;

        return Ok(pointer);
    }

    pub fn __pin(
        runner: &mut Box<dyn RunnerInstance>,
        pointer: i32,
    ) -> anyhow::Result<Box<[Value]>> {
        runner.call("__pin", &[Value::I32(pointer)])
    }

    pub fn __unpin(
        runner: &mut Box<dyn RunnerInstance>,
        pointer: i32,
    ) -> anyhow::Result<Box<[Value]>> {
        runner.call("__unpin", &[Value::I32(pointer)])
    }

    pub fn write_buffer(
        mut runner: &mut Box<dyn RunnerInstance>,
        value: &[u8],
        id: i32,
        align: u32,
    ) -> Result<i64, Error> {
        // Calculate the length and create a new buffer
        let length = value.len();
        let buffer_size = length << align;
        let buffer = Self::__new(&mut runner, buffer_size as i32, 1);
        if buffer.is_err() {
            return Err(Error::from_reason(format!(
                "Failed to get buffer from __new: {:?}",
                buffer.unwrap_err()
            )));
        }

        let buffer_value = buffer.unwrap();

        // Pin the buffer
        let pinned_buffer = Self::__pin(runner, buffer_value);
        if pinned_buffer.is_err() {
            return Err(Error::from_reason(format!(
                "Failed to pin buffer: {:?}",
                pinned_buffer.unwrap_err()
            )));
        }

        let pin_value: Value = pinned_buffer.unwrap()[0].clone();
        let pinned_buffer_value = pin_value.unwrap_i32() as u32;

        // Create the header
        let header = Self::__new(runner, 12, id);
        if header.is_err() {
            return Err(Error::from_reason(format!(
                "Failed to get header from __new: {:?}",
                header.unwrap_err()
            )));
        }

        let header_value = header.unwrap();

        // Set the header values
        Self::set_u32(&mut runner, header_value, pinned_buffer_value).unwrap();
        Self::set_u32(&mut runner, header_value + 4, pinned_buffer_value).unwrap();
        Self::set_u32(&mut runner, header_value + 8, buffer_size as u32).unwrap();

        // Write the buffer value to the contract's memory
        runner.write_memory(pinned_buffer_value as u64, &value).unwrap();

        // Unpin the buffer
        Self::__unpin(runner, pinned_buffer_value as i32).unwrap();

        return Ok(header_value as i64);
    }

    pub fn set_u32(runner: &mut Box<dyn RunnerInstance>, offset: i32, value: u32) -> Result<(), MemoryAccessError> {
        runner.write_memory(offset as u64, &value.to_le_bytes())
    }
}
