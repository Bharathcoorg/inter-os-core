// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Compatibility Runtime (ACR) Dalvik Virtual Machine Engine
//!
//! Provides a bounded interpreter for a deliberately limited subset of Dalvik bytecode.
//! This implementation is kernel-mediated and is not ART or a separate Ring-3 Android
//! runtime. Unsupported opcodes and framework effects fail closed; callers must not
//! treat this interpreter as general APK compatibility or a process-isolation boundary.

use alloc::string::String;
use alloc::vec::Vec;
use super::dex::{
    DexPackage, OP_NOP, OP_MOVE, OP_MOVE_RESULT, OP_MOVE_EXCEPTION,
    OP_RETURN_VOID, OP_RETURN, OP_RETURN_WIDE,
    OP_CONST_4, OP_CONST_16, OP_CONST_WIDE, OP_CONST_STRING, OP_CONST_STRING_JUMBO, OP_CONST_CLASS,
    OP_NEW_INSTANCE, OP_GOTO, OP_GOTO_16, OP_IF_EQ, OP_IF_NE, OP_IF_EQZ, OP_IF_NEZ,
    OP_INVOKE_VIRTUAL, OP_INVOKE_DIRECT, OP_INVOKE_STATIC,
    OP_ADD_INT_2ADDR, OP_SUB_INT_2ADDR, OP_MUL_INT_2ADDR,
    OP_DIV_INT_2ADDR, OP_REM_INT_2ADDR, OP_AND_INT_2ADDR, OP_OR_INT_2ADDR, OP_XOR_INT_2ADDR,
    OP_SHL_INT_2ADDR, OP_SHR_INT_2ADDR, OP_USHR_INT_2ADDR,
    OP_ARRAY_LENGTH, OP_NEW_ARRAY,
    OP_IF_LT, OP_IF_GE, OP_IF_GT, OP_IF_LE,
    OP_IF_LTZ, OP_IF_GEZ, OP_IF_GTZ, OP_IF_LEZ,
    OP_AGET, OP_APUT, OP_IGET, OP_IPUT,
    OP_ADD_INT, OP_SUB_INT, OP_MUL_INT, OP_DIV_INT, OP_REM_INT,
    OP_AND_INT, OP_OR_INT, OP_XOR_INT, OP_SHL_INT, OP_SHR_INT, OP_USHR_INT,
};

/// Errors encountered during Dalvik bytecode execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DalvikVmError {
    MethodNotFound,
    InvalidBytecode,
    RegisterOutOfBounds,
    ExecutionStepLimit,
    IllegalInstruction(u8),
    SecurityViolationForbiddenApi,
    UnsupportedFrameworkApi,
    ObjectHeapLimitExceeded,
    DivideByZero,
    ArrayIndexOutOfBounds,
    NullPointerDereference,
}

/// Decodes Dalvik format 35c's C/D/E/F/G register list and validates the count.
fn decode_invoke_registers(insns: &[u8], pc: usize) -> Result<([usize; 5], usize), DalvikVmError> {
    if pc.checked_add(6).filter(|end| *end <= insns.len()).is_none() { return Err(DalvikVmError::InvalidBytecode); }
    let count = (insns[pc + 1] >> 4) as usize;
    if count > 5 { return Err(DalvikVmError::InvalidBytecode); }
    let regs = [
        (insns[pc + 4] & 0x0f) as usize,
        (insns[pc + 4] >> 4) as usize,
        (insns[pc + 5] & 0x0f) as usize,
        (insns[pc + 5] >> 4) as usize,
        (insns[pc + 1] & 0x0f) as usize,
    ];
    Ok((regs, count))
}

/// Bounded object allocated by one isolated Dalvik VM instance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DalvikObject {
    id: u32,
    class_descriptor: String,
    fields: Vec<(u16, u32)>,
    array_elements: Option<Vec<u32>>,
}

impl DalvikObject {
    fn new(id: u32, class_descriptor: &str) -> Self {
        Self {
            id,
            class_descriptor: String::from(class_descriptor),
            fields: Vec::new(),
            array_elements: None,
        }
    }

    fn new_array(id: u32, element_type: &str, length: usize) -> Self {
        Self {
            id,
            class_descriptor: String::from(element_type),
            fields: Vec::new(),
            array_elements: Some(alloc::vec![0u32; length]),
        }
    }

    fn get_field(&self, field_idx: u16) -> u32 {
        self.fields.iter().find(|(k, _)| *k == field_idx).map(|(_, v)| *v).unwrap_or(0)
    }

    fn set_field(&mut self, field_idx: u16, val: u32) {
        if let Some(entry) = self.fields.iter_mut().find(|(k, _)| *k == field_idx) {
            entry.1 = val;
        } else {
            self.fields.push((field_idx, val));
        }
    }
}

const MAX_DALVIK_OBJECTS: usize = 256;

/// Execution state of the sovereign Dalvik Virtual Machine.
pub struct DalvikVmState {
    /// 32 general-purpose 32-bit Dalvik virtual registers (v0..v31).
    pub registers: [u32; 32],
    /// Shadow string registers for tracking loaded string object references.
    pub string_registers: [Option<String>; 32],
    /// Total instruction opcodes executed.
    pub instructions_executed: u64,
    /// Last value returned by a return opcode or method invocation.
    pub last_return_value: u32,
    /// Shadow return string for methods returning String objects.
    pub last_return_string: Option<String>,
    /// Intercepted Android framework logging calls.
    pub captured_logs: Vec<String>,
    /// Nested DEX calls are bounded to prevent recursive bytecode exhaustion.
    pub call_depth: u8,
    objects: Vec<DalvikObject>,
    next_object_id: u32,
}

impl DalvikVmState {
    /// Creates a fresh Dalvik Virtual Machine state with cleared registers.
    pub fn new() -> Self {
        const INIT_STR: Option<String> = None;
        Self {
            registers: [0u32; 32],
            string_registers: [INIT_STR; 32],
            instructions_executed: 0,
            last_return_value: 0,
            last_return_string: None,
            captured_logs: Vec::new(),
            call_depth: 0,
            objects: Vec::new(),
            next_object_id: 1,
        }
    }

    /// Allocates an actual VM-local object identity; IDs are never reused during this VM's life.
    fn allocate_object(&mut self, class_descriptor: &str) -> Result<u32, DalvikVmError> {
        if self.objects.len() >= MAX_DALVIK_OBJECTS {
            return Err(DalvikVmError::ObjectHeapLimitExceeded);
        }
        if (!class_descriptor.starts_with('L') || !class_descriptor.ends_with(';')) && !class_descriptor.starts_with('[') {
            return Err(DalvikVmError::InvalidBytecode);
        }
        let id = self.next_object_id;
        self.next_object_id = id.checked_add(1).ok_or(DalvikVmError::ObjectHeapLimitExceeded)?;
        self.objects.push(DalvikObject::new(id, class_descriptor));
        Ok(id)
    }

    fn allocate_array(&mut self, element_type: &str, length: usize) -> Result<u32, DalvikVmError> {
        if self.objects.len() >= MAX_DALVIK_OBJECTS {
            return Err(DalvikVmError::ObjectHeapLimitExceeded);
        }
        if length > 1024 {
            return Err(DalvikVmError::ObjectHeapLimitExceeded);
        }
        let id = self.next_object_id;
        self.next_object_id = id.checked_add(1).ok_or(DalvikVmError::ObjectHeapLimitExceeded)?;
        self.objects.push(DalvikObject::new_array(id, element_type, length));
        Ok(id)
    }

    fn get_object(&self, id: u32) -> Result<&DalvikObject, DalvikVmError> {
        self.objects.iter().find(|o| o.id == id).ok_or(DalvikVmError::NullPointerDereference)
    }

    fn get_object_mut(&mut self, id: u32) -> Result<&mut DalvikObject, DalvikVmError> {
        self.objects.iter_mut().find(|o| o.id == id).ok_or(DalvikVmError::NullPointerDereference)
    }

    /// Invokes a specific method on a class by name, executing its Dalvik bytecode.
    pub fn invoke_method(
        &mut self,
        dex: &DexPackage,
        class_name: &str,
        method_name: &str,
    ) -> Result<u32, DalvikVmError> {
        if self.call_depth >= 32 {
            return Err(DalvikVmError::ExecutionStepLimit);
        }
        self.call_depth += 1;
        let result = self.invoke_method_inner(dex, class_name, method_name);
        self.call_depth -= 1;
        result
    }

    fn invoke_method_inner(
        &mut self,
        dex: &DexPackage,
        class_name: &str,
        method_name: &str,
    ) -> Result<u32, DalvikVmError> {
        // 1. Locate the method definition in the parsed DEX binary
        let method = dex
            .methods
            .iter()
            .find(|m| m.class_name == class_name && m.method_name == method_name)
            .ok_or(DalvikVmError::MethodNotFound)?;

        let Some(code_header_end) = method.code_offset.checked_add(16) else {
            return Err(DalvikVmError::InvalidBytecode);
        };
        if method.code_offset == 0 || code_header_end > dex.raw.len() {
            return Err(DalvikVmError::InvalidBytecode);
        }

        // The VM has a fixed register file. Reject DEX methods that require more
        // registers instead of silently aliasing them into another app register.
        if method.registers_size as usize > self.registers.len() {
            return Err(DalvikVmError::RegisterOutOfBounds);
        }

        // 2. Read instruction stream parameters from code_item
        let insns_count = u32::from_le_bytes(
            dex.raw[method.code_offset + 12..method.code_offset + 16]
                .try_into()
                .map_err(|_| DalvikVmError::InvalidBytecode)?,
        ) as usize;

        let Some(insns_bytes_len) = insns_count.checked_mul(2) else {
            return Err(DalvikVmError::InvalidBytecode);
        };
        let insns_start = code_header_end;
        let Some(insns_end) = insns_start.checked_add(insns_bytes_len) else {
            return Err(DalvikVmError::InvalidBytecode);
        };
        if insns_end > dex.raw.len() {
            return Err(DalvikVmError::InvalidBytecode);
        }

        let insns = &dex.raw[insns_start..insns_end];
        self.interpret_insns(dex, method.registers_size, insns)
    }

    /// Interprets a sequence of Dalvik bytecode instructions directly.
    pub fn interpret_insns(
        &mut self,
        dex: &DexPackage,
        registers_size: u16,
        insns: &[u8],
    ) -> Result<u32, DalvikVmError> {
        struct MethodFrame {
            registers_size: u16,
        }
        let method = MethodFrame { registers_size };
        let mut pc: usize = 0;
        let mut steps = 0;
        const STEP_LIMIT: usize = 4096;

        const TOTAL_INSTRUCTION_LIMIT: u64 = 1_000_000;
        while pc < insns.len() && steps < STEP_LIMIT {
            if self.instructions_executed >= TOTAL_INSTRUCTION_LIMIT {
                return Err(DalvikVmError::ExecutionStepLimit);
            }
            steps += 1;
            self.instructions_executed = self.instructions_executed.saturating_add(1);

            let opcode = insns[pc];

            match opcode {
                OP_NOP => {
                    pc += 2;
                }
                OP_MOVE => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= 32 || v_src >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 2;
                }
                OP_MOVE_RESULT => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = self.last_return_value;
                    self.string_registers[v_dest] = self.last_return_string.clone();
                    pc += 2;
                }
                OP_MOVE_EXCEPTION => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = 0;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_CONST_4 => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let val = ((insns[pc + 1] as i8) >> 4) as u32;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = val;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_CONST_16 => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as i32 as u32;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = val;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_CONST_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as i32 as u32;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = val;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_CONST_STRING => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let str_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let text = dex.get_string_slice(str_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    self.string_registers[v_dest] = Some(alloc::string::String::from(text));
                    self.registers[v_dest] = str_idx as u32;
                    pc += 4;
                }
                OP_CONST_STRING_JUMBO => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let str_idx = u32::from_le_bytes([
                        insns[pc + 2],
                        insns[pc + 3],
                        insns[pc + 4],
                        insns[pc + 5],
                    ]) as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let text = dex.get_string_slice(str_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    self.string_registers[v_dest] = Some(alloc::string::String::from(text));
                    self.registers[v_dest] = str_idx as u32;
                    pc += 6;
                }
                OP_CONST_CLASS => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    if dex.get_type_slice(type_idx as u32).is_err() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    self.registers[v_dest] = type_idx as u32;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_NEW_INSTANCE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let class_descriptor = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    let object_id = self.allocate_object(class_descriptor)?;
                    self.registers[v_dest] = object_id;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_GOTO => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let offset = (insns[pc + 1] as i8 as isize) * 2;
                    let next_pc = pc as isize + offset;
                    if next_pc < 0 || next_pc as usize > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    pc = next_pc as usize;
                }
                OP_GOTO_16 => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                    let next_pc = pc as isize + offset;
                    if next_pc < 0 || next_pc as usize > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    pc = next_pc as usize;
                }
                OP_IF_EQ => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    if self.registers[v_a] == self.registers[v_b] {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_IF_NE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    if self.registers[v_a] != self.registers[v_b] {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_IF_EQZ => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    if self.registers[v_dest] == 0 {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_IF_NEZ => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    if self.registers[v_dest] != 0 {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_INVOKE_VIRTUAL | OP_INVOKE_DIRECT | OP_INVOKE_STATIC => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let (arg_registers, arg_count) = decode_invoke_registers(insns, pc)?;
                    if arg_registers[..arg_count].iter().any(|register| *register >= method.registers_size as usize) {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let method_index = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let target = dex.methods.iter().find(|m| m.method_index == method_index)
                        .ok_or(DalvikVmError::MethodNotFound)?;
                    let target_class = target.class_name.clone();
                    let target_method = target.method_name.clone();
                    let target_registers_size = target.registers_size as usize;
                    let target_ins_size = target.ins_size as usize;
                    let is_framework = target_class.starts_with("Landroid/")
                        || target_class.starts_with("Landroidx/")
                        || target_class.starts_with("Lcom/google/android/gms/");
                    let string_args = [
                        if arg_count > 0 { self.string_registers[arg_registers[0]].as_deref() } else { None },
                        if arg_count > 1 { self.string_registers[arg_registers[1]].as_deref() } else { None },
                    ];
                    if is_framework {
                        match super::framework::dispatch(&target_class, &target_method, &string_args, &mut self.captured_logs) {
                            Ok(super::framework::AndroidFrameworkResult::Return(value)) => {
                                self.last_return_value = value;
                                self.last_return_string = None;
                            }
                            Ok(super::framework::AndroidFrameworkResult::ReturnString(s)) => {
                                self.last_return_value = 0;
                                self.last_return_string = Some(alloc::string::String::from(s));
                            }
                            Ok(super::framework::AndroidFrameworkResult::Service(svc)) => {
                                self.last_return_value = svc.handle();
                                self.last_return_string = None;
                            }
                            Err(super::framework::AndroidFrameworkError::UnsupportedApi) => return Err(DalvikVmError::UnsupportedFrameworkApi),
                            Err(_) => return Err(DalvikVmError::SecurityViolationForbiddenApi),
                        }
                    } else {
                        if arg_count != target_ins_size || target_ins_size > target_registers_size || target_registers_size > self.registers.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let caller_registers = self.registers;
                        let caller_strings = self.string_registers.clone();
                        let base = target_registers_size - target_ins_size;
                        self.registers = [0; 32];
                        self.string_registers = [const { None }; 32];
                        for i in 0..arg_count {
                            self.registers[base + i] = caller_registers[arg_registers[i]];
                            self.string_registers[base + i] = caller_strings[arg_registers[i]].clone();
                        }
                        let result = self.invoke_method(dex, &target.class_name, &target.method_name);
                        self.registers = caller_registers;
                        self.string_registers = caller_strings;
                        self.last_return_value = result?;
                        self.last_return_string = None;
                    }
                    pc += 6;
                }
                OP_ADD_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] = self.registers[v_a].wrapping_add(self.registers[v_b]);
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_SUB_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] = self.registers[v_a].wrapping_sub(self.registers[v_b]);
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_MUL_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] = self.registers[v_a].wrapping_mul(self.registers[v_b]);
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_DIV_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let divisor = self.registers[v_b] as i32;
                    if divisor == 0 {
                        return Err(DalvikVmError::DivideByZero);
                    }
                    let dividend = self.registers[v_a] as i32;
                    self.registers[v_a] = dividend.wrapping_div(divisor) as u32;
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_REM_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let divisor = self.registers[v_b] as i32;
                    if divisor == 0 {
                        return Err(DalvikVmError::DivideByZero);
                    }
                    let dividend = self.registers[v_a] as i32;
                    self.registers[v_a] = dividend.wrapping_rem(divisor) as u32;
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_AND_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] &= self.registers[v_b];
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_OR_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] |= self.registers[v_b];
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_XOR_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_a] ^= self.registers[v_b];
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_SHL_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let shift = self.registers[v_b] & 0x1F;
                    self.registers[v_a] = self.registers[v_a].wrapping_shl(shift);
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_SHR_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let shift = self.registers[v_b] & 0x1F;
                    self.registers[v_a] = (self.registers[v_a] as i32).wrapping_shr(shift) as u32;
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_USHR_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let shift = self.registers[v_b] & 0x1F;
                    self.registers[v_a] = self.registers[v_a].wrapping_shr(shift);
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_ADD_INT | OP_SUB_INT | OP_MUL_INT | OP_DIV_INT | OP_REM_INT
                | OP_AND_INT | OP_OR_INT | OP_XOR_INT | OP_SHL_INT | OP_SHR_INT | OP_USHR_INT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest >= 32 || v_b >= 32 || v_c >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let rb = self.registers[v_b];
                    let rc = self.registers[v_c];
                    let res = match opcode {
                        OP_ADD_INT => rb.wrapping_add(rc),
                        OP_SUB_INT => rb.wrapping_sub(rc),
                        OP_MUL_INT => rb.wrapping_mul(rc),
                        OP_DIV_INT => {
                            let divisor = rc as i32;
                            if divisor == 0 { return Err(DalvikVmError::DivideByZero); }
                            (rb as i32).wrapping_div(divisor) as u32
                        }
                        OP_REM_INT => {
                            let divisor = rc as i32;
                            if divisor == 0 { return Err(DalvikVmError::DivideByZero); }
                            (rb as i32).wrapping_rem(divisor) as u32
                        }
                        OP_AND_INT => rb & rc,
                        OP_OR_INT => rb | rc,
                        OP_XOR_INT => rb ^ rc,
                        OP_SHL_INT => rb.wrapping_shl(rc & 0x1F),
                        OP_SHR_INT => ((rb as i32).wrapping_shr(rc & 0x1F)) as u32,
                        OP_USHR_INT => rb.wrapping_shr(rc & 0x1F),
                        _ => 0,
                    };
                    self.registers[v_dest] = res;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_IF_LT | OP_IF_GE | OP_IF_GT | OP_IF_LE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let va = self.registers[v_a] as i32;
                    let vb = self.registers[v_b] as i32;
                    let cond = match opcode {
                        OP_IF_LT => va < vb,
                        OP_IF_GE => va >= vb,
                        OP_IF_GT => va > vb,
                        OP_IF_LE => va <= vb,
                        _ => false,
                    };
                    if cond {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_IF_LTZ | OP_IF_GEZ | OP_IF_GTZ | OP_IF_LEZ => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let val = self.registers[v_dest] as i32;
                    let cond = match opcode {
                        OP_IF_LTZ => val < 0,
                        OP_IF_GEZ => val >= 0,
                        OP_IF_GTZ => val > 0,
                        OP_IF_LEZ => val <= 0,
                        _ => false,
                    };
                    if cond {
                        let offset = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as isize) * 2;
                        let next_pc = pc as isize + offset;
                        if next_pc < 0 || next_pc as usize > insns.len() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        pc = next_pc as usize;
                    } else {
                        pc += 4;
                    }
                }
                OP_ARRAY_LENGTH => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object(obj_id)?;
                    let len = obj.array_elements.as_ref().map(|a| a.len()).ok_or(DalvikVmError::NullPointerDereference)?;
                    self.registers[v_a] = len as u32;
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_NEW_ARRAY => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let elem_type = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    let length = self.registers[v_b] as usize;
                    let obj_id = self.allocate_array(elem_type, length)?;
                    self.registers[v_a] = obj_id;
                    self.string_registers[v_a] = None;
                    pc += 4;
                }
                OP_AGET => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_dest >= 32 || v_arr >= 32 || v_idx >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    let idx = self.registers[v_idx] as usize;
                    let obj = self.get_object(arr_id)?;
                    let elements = obj.array_elements.as_ref().ok_or(DalvikVmError::NullPointerDereference)?;
                    if idx >= elements.len() {
                        return Err(DalvikVmError::ArrayIndexOutOfBounds);
                    }
                    self.registers[v_dest] = elements[idx];
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_APUT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_val = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_val >= 32 || v_arr >= 32 || v_idx >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    let idx = self.registers[v_idx] as usize;
                    let val = self.registers[v_val];
                    let obj = self.get_object_mut(arr_id)?;
                    let elements = obj.array_elements.as_mut().ok_or(DalvikVmError::NullPointerDereference)?;
                    if idx >= elements.len() {
                        return Err(DalvikVmError::ArrayIndexOutOfBounds);
                    }
                    elements[idx] = val;
                    pc += 4;
                }
                OP_IGET => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object(obj_id)?;
                    self.registers[v_a] = obj.get_field(field_idx);
                    self.string_registers[v_a] = None;
                    pc += 4;
                }
                OP_IPUT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a >= 32 || v_b >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let val = self.registers[v_a];
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object_mut(obj_id)?;
                    obj.set_field(field_idx, val);
                    pc += 4;
                }
                OP_RETURN_VOID => {
                    self.last_return_value = 0;
                    self.last_return_string = None;
                    return Ok(0);
                }
                OP_RETURN | OP_RETURN_WIDE => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= 32 {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let ret_val = self.registers[v_src];
                    self.last_return_value = ret_val;
                    self.last_return_string = self.string_registers[v_src].clone();
                    return Ok(ret_val);
                }
                unknown_op => {
                    return Err(DalvikVmError::IllegalInstruction(unknown_op));
                }
            }
        }

        if steps >= STEP_LIMIT || pc < insns.len() {
            Err(DalvikVmError::ExecutionStepLimit)
        } else {
            Ok(self.last_return_value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::dex::create_test_dex;

    #[test]
    fn invoke_format_35c_decodes_all_register_slots_and_rejects_bad_counts() {
        let bytes = [OP_INVOKE_STATIC, 0x53, 0x02, 0x00, 0x10, 0x43];
        let (registers, count) = decode_invoke_registers(&bytes, 0).unwrap();
        assert_eq!(count, 5);
        assert_eq!(registers, [0, 1, 3, 4, 3]);
        let malformed = [OP_INVOKE_STATIC, 0x63, 0, 0, 0, 0];
        assert_eq!(decode_invoke_registers(&malformed, 0), Err(DalvikVmError::InvalidBytecode));
    }

    #[test]
    fn object_allocations_have_distinct_vm_local_identity_and_a_hard_limit() {
        let mut vm = DalvikVmState::new();
        let first = vm.allocate_object("Lorg/interos/First;").unwrap();
        let second = vm.allocate_object("Lorg/interos/Second;").unwrap();
        assert_ne!(first, second);
        assert_eq!(vm.objects[0].id, first);
        assert_eq!(vm.objects[0].class_descriptor, "Lorg/interos/First;");
        for _ in 2..MAX_DALVIK_OBJECTS {
            vm.allocate_object("Lorg/interos/Bounded;").unwrap();
        }
        assert_eq!(vm.objects.len(), MAX_DALVIK_OBJECTS);
        assert_eq!(vm.allocate_object("Lorg/interos/Overflow;"), Err(DalvikVmError::ObjectHeapLimitExceeded));
    }

    #[test]
    fn object_allocator_rejects_non_class_descriptors() {
        let mut vm = DalvikVmState::new();
        assert_eq!(vm.allocate_object("not-a-class"), Err(DalvikVmError::InvalidBytecode));
        assert!(vm.objects.is_empty());
    }

    #[test]
    fn cumulative_instruction_budget_fails_closed() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "budget");
        let dex = DexPackage::parse(&dex_bytes).expect("test DEX should parse");
        let mut vm = DalvikVmState::new();
        vm.instructions_executed = 1_000_000;
        assert_eq!(
            vm.invoke_method(&dex, "Lorg/interos/Test;", "onCreate"),
            Err(DalvikVmError::ExecutionStepLimit)
        );
    }

    #[test]
    fn microg_framework_dispatch_updates_vm_return_state() {
        let mut vm = DalvikVmState::new();
        // Invoke GoogleApiAvailability: isGooglePlayServicesAvailable
        let res = super::super::framework::dispatch(
            "Lcom/google/android/gms/common/GoogleApiAvailability;",
            "isGooglePlayServicesAvailable",
            &[],
            &mut vm.captured_logs,
        ).unwrap();
        match res {
            super::super::framework::AndroidFrameworkResult::Return(v) => {
                vm.last_return_value = v;
                vm.last_return_string = None;
            }
            _ => panic!("Expected Return(0)"),
        }
        assert_eq!(vm.last_return_value, 0);
        assert_eq!(vm.last_return_string, None);

        // Invoke AdvertisingIdClient$Info: getId
        let res = super::super::framework::dispatch(
            "Lcom/google/android/gms/ads/identifier/AdvertisingIdClient$Info;",
            "getId",
            &[],
            &mut vm.captured_logs,
        ).unwrap();
        match res {
            super::super::framework::AndroidFrameworkResult::ReturnString(s) => {
                vm.last_return_value = 0;
                vm.last_return_string = Some(alloc::string::String::from(s));
            }
            _ => panic!("Expected ReturnString"),
        }
        assert_eq!(vm.last_return_value, 0);
        assert_eq!(vm.last_return_string.as_deref(), Some("00000000-0000-0000-0000-000000000000"));
    }

    #[test]
    fn framework_service_and_lifecycle_dispatch_updates_vm_state() {
        let mut vm = DalvikVmState::new();

        // 1. Context getSystemService returns service handle
        let res = super::super::framework::dispatch(
            "Landroid/content/Context;",
            "getSystemService",
            &[Some("window")],
            &mut vm.captured_logs,
        ).unwrap();
        match res {
            super::super::framework::AndroidFrameworkResult::Service(svc) => {
                vm.last_return_value = svc.handle();
                vm.last_return_string = None;
            }
            _ => panic!("Expected Service"),
        }
        assert_eq!(vm.last_return_value, 2); // Window handle is 2
        assert_eq!(vm.last_return_string, None);

        // 2. Activity lifecycle dispatch
        let res = super::super::framework::dispatch(
            "Landroid/app/Activity;",
            "onCreate",
            &[],
            &mut vm.captured_logs,
        ).unwrap();
        match res {
            super::super::framework::AndroidFrameworkResult::Return(v) => {
                vm.last_return_value = v;
                vm.last_return_string = None;
            }
            _ => panic!("Expected Return"),
        }
        assert_eq!(vm.last_return_value, 0);

        // 3. Unimplemented method fails closed
        let res = super::super::framework::dispatch(
            "Landroid/widget/TextView;",
            "setText",
            &[],
            &mut vm.captured_logs,
        );
        assert_eq!(res, Err(super::super::framework::AndroidFrameworkError::UnsupportedApi));
    }

    #[test]
    fn test_dalvik_arithmetic_2addr_operations() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // v0 = 100, v1 = 20
        vm.registers[0] = 100;
        vm.registers[1] = 20;

        // div-int/2addr v0, v1 -> 100 / 20 = 5
        let insns = [OP_DIV_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 5);

        // rem-int/2addr v0, v1 -> 17 % 5 = 2
        vm.registers[0] = 17;
        vm.registers[1] = 5;
        let insns = [OP_REM_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 2);

        // and-int/2addr v0, v1 -> 0xFF & 0x0F = 0x0F
        vm.registers[0] = 0xFF;
        vm.registers[1] = 0x0F;
        let insns = [OP_AND_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 0x0F);

        // or-int/2addr v0, v1 -> 0xF0 | 0x0F = 0xFF
        vm.registers[0] = 0xF0;
        vm.registers[1] = 0x0F;
        let insns = [OP_OR_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 0xFF);

        // xor-int/2addr v0, v1 -> 0xAA ^ 0xFF = 0x55
        vm.registers[0] = 0xAA;
        vm.registers[1] = 0xFF;
        let insns = [OP_XOR_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 0x55);

        // shl-int/2addr v0, v1 -> 1 << 4 = 16
        vm.registers[0] = 1;
        vm.registers[1] = 4;
        let insns = [OP_SHL_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 16);

        // shr-int/2addr v0, v1 -> -16 >> 2 = -4
        vm.registers[0] = (-16i32) as u32;
        vm.registers[1] = 2;
        let insns = [OP_SHR_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res as i32, -4);

        // ushr-int/2addr v0, v1 -> 0xF0000000 >>> 4 = 0x0F000000
        vm.registers[0] = 0xF000_0000;
        vm.registers[1] = 4;
        let insns = [OP_USHR_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 0x0F00_0000);
    }

    #[test]
    fn test_dalvik_arithmetic_3reg_operations() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        vm.registers[1] = 40;
        vm.registers[2] = 2;

        // add-int v0, v1, v2 -> 42
        let insns = [OP_ADD_INT, 0x00, 0x01, 0x02, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 3, &insns).unwrap(), 42);

        // sub-int v0, v1, v2 -> 38
        let insns = [OP_SUB_INT, 0x00, 0x01, 0x02, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 3, &insns).unwrap(), 38);

        // mul-int v0, v1, v2 -> 80
        let insns = [OP_MUL_INT, 0x00, 0x01, 0x02, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 3, &insns).unwrap(), 80);

        // div-int v0, v1, v2 -> 20
        let insns = [OP_DIV_INT, 0x00, 0x01, 0x02, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 3, &insns).unwrap(), 20);

        // rem-int v0, v1, v2 -> 0
        let insns = [OP_REM_INT, 0x00, 0x01, 0x02, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 3, &insns).unwrap(), 0);
    }

    #[test]
    fn test_dalvik_division_by_zero_fails_closed() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        vm.registers[0] = 50;
        vm.registers[1] = 0;

        let insns = [OP_DIV_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns), Err(DalvikVmError::DivideByZero));

        let insns = [OP_REM_INT_2ADDR, 0x10, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns), Err(DalvikVmError::DivideByZero));

        let insns = [OP_DIV_INT, 0x00, 0x00, 0x01, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns), Err(DalvikVmError::DivideByZero));
    }

    #[test]
    fn test_dalvik_comparison_and_branch_operations() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // Test if-lt: v0 = 5, v1 = 10 -> branch +4 words (+8 bytes) to return 99
        vm.registers[0] = 5;
        vm.registers[1] = 10;
        let insns = [
            OP_IF_LT, 0x10, 0x04, 0x00,       // if v0 < v1 goto pc+8
            OP_CONST_4, 0x00,                  // v0 = 0
            OP_RETURN, 0x00,                   // return v0
            OP_NOP, 0x00,                      // padding
            OP_CONST_4, 0x30,                  // v0 = 3
            OP_RETURN, 0x00,                   // return v0
        ];
        let res = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(res, 3);

        // Test if-gtz: v0 = 10 -> branch to return 77
        vm.registers[0] = 10;
        let insns = [
            OP_IF_GTZ, 0x00, 0x04, 0x00,      // if v0 > 0 goto pc+8
            OP_CONST_4, 0x00,                 // v0 = 0
            OP_RETURN, 0x00,                  // return v0
            OP_NOP, 0x00,                     // padding
            OP_CONST_4, 0x70,                 // v0 = 7
            OP_RETURN, 0x00,                  // return v0
        ];
        let res = vm.interpret_insns(&dex, 1, &insns).unwrap();
        assert_eq!(res, 7);
    }

    #[test]
    fn test_dalvik_array_and_field_lifecycle() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Array allocation and indexing
        let arr_id = vm.allocate_array("[I", 5).unwrap();
        vm.registers[0] = arr_id;
        // length opcode
        let insns = [OP_ARRAY_LENGTH, 0x01, OP_RETURN, 0x01]; // v1 = length(v0), return v1
        let len = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(len, 5);

        // aput v2 into array[v0][v1]: v2 = 777, v1 = 3
        vm.registers[0] = arr_id;
        vm.registers[1] = 3;
        vm.registers[2] = 777;
        let insns = [OP_APUT, 0x02, 0x00, 0x01, OP_RETURN_VOID];
        vm.interpret_insns(&dex, 3, &insns).unwrap();

        // aget v3 from array[v0][v1]
        let insns = [OP_AGET, 0x03, 0x00, 0x01, OP_RETURN, 0x03];
        let val = vm.interpret_insns(&dex, 4, &insns).unwrap();
        assert_eq!(val, 777);

        // 2. Object field access (iput / iget)
        let obj_id = vm.allocate_object("Lorg/interos/Test;").unwrap();
        vm.registers[0] = 12345;
        vm.registers[1] = obj_id;
        let insns = [OP_IPUT, 0x10, 0x0A, 0x00, OP_RETURN_VOID]; // iput v0 into obj v1 at field 10
        vm.interpret_insns(&dex, 2, &insns).unwrap();

        // iget into v2 from obj v1 at field 10
        let insns = [OP_IGET, 0x12, 0x0A, 0x00, OP_RETURN, 0x02];
        let val = vm.interpret_insns(&dex, 3, &insns).unwrap();
        assert_eq!(val, 12345);
    }
}

