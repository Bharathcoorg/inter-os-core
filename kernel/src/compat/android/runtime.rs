//! Android Compatibility Runtime (ACR) Dalvik Virtual Machine Engine
//!
//! Provides a zero-trust, capability-bounded bytecode execution engine that runs
//! genuine Android Dalvik Executable (`classes.dex`) opcodes in Ring 3 isolated memory
//! without copying Android Linux kernels, ART runtime, or ambient authority.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use super::dex::{
    DexPackage, OP_NOP, OP_MOVE, OP_MOVE_RESULT, OP_MOVE_EXCEPTION,
    OP_RETURN_VOID, OP_RETURN, OP_RETURN_WIDE,
    OP_CONST_4, OP_CONST_16, OP_CONST_WIDE, OP_CONST_STRING, OP_CONST_STRING_JUMBO, OP_CONST_CLASS,
    OP_NEW_INSTANCE, OP_GOTO, OP_GOTO_16, OP_IF_EQ, OP_IF_NE, OP_IF_EQZ, OP_IF_NEZ,
    OP_INVOKE_VIRTUAL, OP_INVOKE_DIRECT, OP_INVOKE_STATIC,
    OP_ADD_INT_2ADDR, OP_SUB_INT_2ADDR, OP_MUL_INT_2ADDR,
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
}

/// Bounded object allocated by one isolated Dalvik VM instance.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DalvikObject {
    id: u32,
    class_descriptor: String,
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
        if !class_descriptor.starts_with('L') || !class_descriptor.ends_with(';') {
            return Err(DalvikVmError::InvalidBytecode);
        }
        let id = self.next_object_id;
        self.next_object_id = id.checked_add(1).ok_or(DalvikVmError::ObjectHeapLimitExceeded)?;
        self.objects.push(DalvikObject { id, class_descriptor: String::from(class_descriptor) });
        Ok(id)
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

        // 3. Dalvik Register Bytecode Interpreter Loop
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
                    self.string_registers[v_dest] = None;
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
                    let text = dex.strings.get(str_idx).ok_or(DalvikVmError::InvalidBytecode)?;
                    self.string_registers[v_dest] = Some(text.clone());
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
                    let text = dex.strings.get(str_idx).ok_or(DalvikVmError::InvalidBytecode)?;
                    self.string_registers[v_dest] = Some(text.clone());
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
                    if dex.types.get(type_idx).is_none() {
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
                    let class_descriptor = dex.types.get(type_idx).ok_or(DalvikVmError::InvalidBytecode)?;
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
                    // Dalvik 35c invoke format: opcode, register-count/register-list, method_idx.
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let method_index = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let target = dex.methods.iter().find(|m| m.method_index == method_index)
                        .ok_or(DalvikVmError::MethodNotFound)?;
                    let string_args = [
                        self.string_registers[(insns[pc + 4] & 0x0F) as usize].as_deref(),
                        self.string_registers[((insns[pc + 4] >> 4) & 0x0F) as usize].as_deref(),
                    ];
                    if target.class_name.starts_with("Landroid/") || target.class_name.starts_with("Landroidx/") {
                        match super::framework::dispatch(&target.class_name, &target.method_name, &string_args, &mut self.captured_logs) {
                            Ok(super::framework::AndroidFrameworkResult::Return(value)) => self.last_return_value = value,
                            // A service descriptor is not a service invocation. Do not turn
                            // an unimplemented bridge into a synthetic successful return.
                            Ok(super::framework::AndroidFrameworkResult::Service(_)) => return Err(DalvikVmError::UnsupportedFrameworkApi),
                            Err(super::framework::AndroidFrameworkError::UnsupportedApi) => return Err(DalvikVmError::UnsupportedFrameworkApi),
                            Err(_) => return Err(DalvikVmError::SecurityViolationForbiddenApi),
                        }
                    } else {
                        self.last_return_value = self.invoke_method(dex, &target.class_name, &target.method_name)?;
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
                OP_RETURN_VOID => {
                    self.last_return_value = 0;
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
}
