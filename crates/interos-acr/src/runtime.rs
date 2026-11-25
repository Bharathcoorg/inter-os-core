// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Compatibility Runtime (ACR) Dalvik Virtual Machine Engine
//!
//! Provides a bounded interpreter for a deliberately limited subset of Dalvik bytecode.
//! This implementation is kernel-mediated and is not ART or a separate Ring-3 Android
//! runtime. Unsupported opcodes and framework effects fail closed; callers must not
//! treat this interpreter as general APK compatibility or a process-isolation boundary.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use crate::binder::Parcel;
use interos_android_format::dex::*;

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
    UnhandledException(u32),
    MonitorBlocked(u32),
}

const MAX_TRIES_COUNT: usize = 256;
const MAX_CATCH_HANDLERS_PER_TRY: usize = 256;

/// Checks if thrown exception is compatible with catch type descriptor according to Java exception hierarchy.
fn is_exception_compatible(dex: &DexPackage, thrown_desc: &str, catch_type_idx: u32) -> bool {
    let Some(catch_desc) = dex.types.get(catch_type_idx as usize) else {
        return false;
    };
    if catch_desc == thrown_desc {
        return true;
    }
    match catch_desc.as_str() {
        "Ljava/lang/Object;" | "Ljava/lang/Throwable;" => true,
        "Ljava/lang/Exception;" => !matches!(
            thrown_desc,
            "Ljava/lang/Error;" | "Ljava/lang/VirtualMachineError;" | "Ljava/lang/OutOfMemoryError;"
        ),
        "Ljava/lang/RuntimeException;" => matches!(
            thrown_desc,
            "Ljava/lang/RuntimeException;"
                | "Ljava/lang/NullPointerException;"
                | "Ljava/lang/IndexOutOfBoundsException;"
                | "Ljava/lang/ArrayIndexOutOfBoundsException;"
                | "Ljava/lang/StringIndexOutOfBoundsException;"
                | "Ljava/lang/IllegalArgumentException;"
                | "Ljava/lang/IllegalStateException;"
                | "Ljava/lang/ArithmeticException;"
                | "Ljava/lang/ClassCastException;"
        ),
        "Ljava/lang/IndexOutOfBoundsException;" => matches!(
            thrown_desc,
            "Ljava/lang/IndexOutOfBoundsException;"
                | "Ljava/lang/ArrayIndexOutOfBoundsException;"
                | "Ljava/lang/StringIndexOutOfBoundsException;"
        ),
        _ => false,
    }
}

/// Finds a matching catch handler offset in bytes for a given bytecode pc within the method.
/// Validates tries_size, bounded handlers, safe sleb128 abs without overflow, and instruction 2-byte alignment.
fn find_catch_handler(
    dex: &DexPackage,
    code_offset: usize,
    pc_bytes: usize,
    ex_class_desc: Option<&str>,
) -> Option<usize> {
    if code_offset == 0 || code_offset + 16 > dex.raw.len() {
        return None;
    }
    let tries_size = u16::from_le_bytes([dex.raw[code_offset + 6], dex.raw[code_offset + 7]]) as usize;
    if tries_size == 0 || tries_size > MAX_TRIES_COUNT {
        return None;
    }
    let insns_size = u32::from_le_bytes([
        dex.raw[code_offset + 12],
        dex.raw[code_offset + 13],
        dex.raw[code_offset + 14],
        dex.raw[code_offset + 15],
    ]) as usize;
    let insns_bytes_len = insns_size.checked_mul(2)?;
    let padding = if insns_size % 2 == 1 { 2 } else { 0 };
    let tries_start = code_offset.checked_add(16)?.checked_add(insns_bytes_len)?.checked_add(padding)?;
    let tries_bytes = tries_size.checked_mul(8)?;
    let handlers_list_start = tries_start.checked_add(tries_bytes)?;
    if handlers_list_start > dex.raw.len() {
        return None;
    }

    let code_unit_pc = (pc_bytes / 2) as u32;

    for i in 0..tries_size {
        let try_offset = tries_start + i * 8;
        if try_offset + 8 > dex.raw.len() {
            break;
        }
        let start_addr = u32::from_le_bytes([
            dex.raw[try_offset],
            dex.raw[try_offset + 1],
            dex.raw[try_offset + 2],
            dex.raw[try_offset + 3],
        ]);
        let insn_count = u16::from_le_bytes([
            dex.raw[try_offset + 4],
            dex.raw[try_offset + 5],
        ]) as u32;
        let handler_off = u16::from_le_bytes([
            dex.raw[try_offset + 6],
            dex.raw[try_offset + 7],
        ]) as usize;

        if code_unit_pc >= start_addr && code_unit_pc < start_addr.saturating_add(insn_count) {
            let mut cur = handlers_list_start.checked_add(handler_off)?;
            if cur >= dex.raw.len() {
                return None;
            }
            let handler_size = read_sleb128(dex.raw, &mut cur).ok()?;
            if handler_size == i32::MIN {
                return None;
            }
            let abs_size = handler_size.unsigned_abs() as usize;
            if abs_size > MAX_CATCH_HANDLERS_PER_TRY {
                return None;
            }

            for _ in 0..abs_size {
                let type_idx = read_uleb128(dex.raw, &mut cur).ok()?;
                let target_addr = read_uleb128(dex.raw, &mut cur).ok()?;

                let matches = if let Some(ex_desc) = ex_class_desc {
                    is_exception_compatible(dex, ex_desc, type_idx)
                } else {
                    true
                };

                if matches {
                    let target_byte_pc = (target_addr as usize).checked_mul(2)?;
                    if target_byte_pc < insns_bytes_len && (target_byte_pc % 2 == 0) {
                        return Some(target_byte_pc);
                    }
                }
            }

            if handler_size <= 0 {
                let catch_all_addr = read_uleb128(dex.raw, &mut cur).ok()?;
                let target_byte_pc = (catch_all_addr as usize).checked_mul(2)?;
                if target_byte_pc < insns_bytes_len && (target_byte_pc % 2 == 0) {
                    return Some(target_byte_pc);
                }
            }
        }
    }

    None
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

/// Typed value stored inside an Android Bundle or Intent extras.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleValue {
    String(String),
    Int(i32),
    Long(i64),
    Boolean(bool),
    Bundle(BTreeMap<String, BundleValue>),
}

impl BundleValue {
    pub fn estimated_size_bytes(&self) -> usize {
        core::mem::size_of::<Self>() + match self {
            BundleValue::String(s) => s.len(),
            BundleValue::Bundle(m) => m.iter().map(|(k, v)| k.len() + v.estimated_size_bytes() + 32).sum(),
            _ => 0,
        }
    }
}

/// Software square root using Newton-Raphson method for no_std targets.
pub fn soft_sqrt(val: f64) -> f64 {
    if val < 0.0 || val.is_nan() {
        f64::NAN
    } else if val == 0.0 || val.is_infinite() {
        val
    } else {
        let mut curr = val;
        let mut prev = 0.0;
        for _ in 0..64 {
            if (curr - prev).abs() < 1e-15 {
                break;
            }
            prev = curr;
            curr = 0.5 * (curr + val / curr);
        }
        curr
    }
}

/// VM-managed payload for stateful Java and Android objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectPayload {
    None,
    String(String),
    StringBuilder(String),
    BoxedBoolean(bool),
    BoxedInteger(i32),
    BoxedLong(i64),
    Class { descriptor: String },
    ArrayList(Vec<u32>),
    HashMap(BTreeMap<String, u32>),
    SharedPreferences {
        name: String,
        values: BTreeMap<String, BundleValue>,
    },
    SharedPreferencesEditor {
        pref_name: String,
        staged: BTreeMap<String, Option<BundleValue>>,
        clear_staged: bool,
    },
    File {
        path: String,
    },
    Parcel(Parcel),
    Bundle(BTreeMap<String, BundleValue>),
    Intent {
        action: Option<String>,
        categories: Vec<String>,
        data_uri: Option<String>,
        extras: BTreeMap<String, BundleValue>,
    },
    Throwable {
        message: Option<String>,
        cause_id: Option<u32>,
    },
}

/// Bounded object allocated by one isolated Dalvik VM instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DalvikObject {
    pub id: u32,
    pub class_descriptor: String,
    pub fields: Vec<(u16, u32)>,
    pub array_elements: Option<Vec<u32>>,
    pub payload: ObjectPayload,
}

impl DalvikObject {
    fn new(id: u32, class_descriptor: &str) -> Self {
        let payload = match class_descriptor {
            "Ljava/lang/String;" => ObjectPayload::String(String::new()),
            "Ljava/lang/StringBuilder;" => ObjectPayload::StringBuilder(String::new()),
            "Ljava/lang/Integer;" => ObjectPayload::BoxedInteger(0),
            "Ljava/lang/Long;" => ObjectPayload::BoxedLong(0),
            "Ljava/lang/Boolean;" => ObjectPayload::BoxedBoolean(false),
            "Ljava/lang/Class;" => ObjectPayload::Class { descriptor: String::new() },
            "Ljava/util/ArrayList;" => ObjectPayload::ArrayList(Vec::new()),
            "Ljava/util/HashMap;" | "Ljava/util/Map;" => ObjectPayload::HashMap(BTreeMap::new()),
            "Ljava/io/File;" => ObjectPayload::File { path: String::new() },
            "Landroid/os/Parcel;" => ObjectPayload::Parcel(Parcel::obtain()),
            "Landroid/content/SharedPreferences;" => ObjectPayload::SharedPreferences {
                name: String::from("default"),
                values: BTreeMap::new(),
            },
            "Landroid/content/SharedPreferences$Editor;" => ObjectPayload::SharedPreferencesEditor {
                pref_name: String::from("default"),
                staged: BTreeMap::new(),
                clear_staged: false,
            },
            "Landroid/content/Intent;" => ObjectPayload::Intent {
                action: None,
                categories: Vec::new(),
                data_uri: None,
                extras: BTreeMap::new(),
            },
            "Landroid/os/Bundle;" => ObjectPayload::Bundle(BTreeMap::new()),
            desc if desc.contains("Exception") || desc.contains("Throwable") || desc.contains("Error") => {
                ObjectPayload::Throwable { message: None, cause_id: None }
            }
            _ => ObjectPayload::None,
        };
        Self {
            id,
            class_descriptor: String::from(class_descriptor),
            fields: Vec::new(),
            array_elements: None,
            payload,
        }
    }

    fn new_array(id: u32, element_type: &str, length: usize) -> Self {
        Self {
            id,
            class_descriptor: String::from(element_type),
            fields: Vec::new(),
            array_elements: Some(alloc::vec![0u32; length]),
            payload: ObjectPayload::None,
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

    pub fn estimated_size_bytes(&self) -> usize {
        let mut size = core::mem::size_of::<Self>() + self.class_descriptor.len();
        size += self.fields.len() * (core::mem::size_of::<(u16, u32)>() + 8);
        if let Some(ref arr) = self.array_elements {
            size += arr.len() * core::mem::size_of::<u32>();
        }
        size += match &self.payload {
            ObjectPayload::None => 0,
            ObjectPayload::String(s) | ObjectPayload::StringBuilder(s) => s.len(),
            ObjectPayload::Class { descriptor } => descriptor.len(),
            ObjectPayload::ArrayList(list) => list.len() * core::mem::size_of::<u32>(),
            ObjectPayload::HashMap(map) => map.iter().map(|(k, _)| k.len() + 24).sum(),
            ObjectPayload::SharedPreferences { name, values } => {
                name.len() + values.iter().map(|(k, v)| k.len() + v.estimated_size_bytes() + 24).sum::<usize>()
            }
            ObjectPayload::SharedPreferencesEditor { pref_name, staged, .. } => {
                pref_name.len()
                    + staged
                        .iter()
                        .map(|(k, opt_v)| {
                            k.len() + opt_v.as_ref().map(|v| v.estimated_size_bytes()).unwrap_or(0) + 24
                        })
                        .sum::<usize>()
            }
            ObjectPayload::File { path } => path.len(),
            ObjectPayload::Parcel(p) => p.data_size(),
            ObjectPayload::Bundle(b) => b.iter().map(|(k, v)| k.len() + v.estimated_size_bytes() + 24).sum(),
            ObjectPayload::Intent { action, categories, data_uri, extras } => {
                action.as_ref().map(|s| s.len()).unwrap_or(0)
                    + categories.iter().map(|s| s.len()).sum::<usize>()
                    + data_uri.as_ref().map(|s| s.len()).unwrap_or(0)
                    + extras.iter().map(|(k, v)| k.len() + v.estimated_size_bytes() + 24).sum::<usize>()
            }
            ObjectPayload::Throwable { message, .. } => message.as_ref().map(|s| s.len()).unwrap_or(0),
            _ => 0,
        };
        size
    }
}

pub const DEFAULT_HEAP_MAX_BYTES: usize = 16 * 1024 * 1024; // 16 MB default quota
pub const MAX_HEAP_CEILING_BYTES: usize = 64 * 1024 * 1024; // 64 MB maximum ceiling
pub const MAX_REGISTERS_LIMIT: usize = 2048;

/// Execution state of the sovereign Dalvik Virtual Machine.
pub struct DalvikVmState {
    /// Dynamic general-purpose 32-bit Dalvik virtual registers (v0..vN), up to MAX_REGISTERS_LIMIT.
    pub registers: Vec<u32>,
    /// Shadow string registers for tracking loaded string object references.
    pub string_registers: Vec<Option<String>>,
    /// Total instruction opcodes executed.
    pub instructions_executed: u64,
    /// Last value returned by a return opcode or method invocation.
    pub last_return_value: u32,
    /// High 32 bits for wide 64-bit returns.
    pub last_return_wide_hi: u32,
    /// Shadow return string for methods returning String objects.
    pub last_return_string: Option<String>,
    /// Intercepted Android framework logging calls.
    pub captured_logs: Vec<String>,
    /// Nested DEX calls are bounded to prevent recursive bytecode exhaustion.
    pub call_depth: u8,
    /// Pending exception object ID if an exception is currently unwinding.
    pub pending_exception: Option<u32>,
    /// Task/thread identity executing in this VM instance.
    pub current_task_id: u64,
    pub current_thread_id: u64,
    /// Thread execution contexts managed by this VM.
    pub threads: BTreeMap<u64, JavaThreadState>,
    /// Cooperative green-thread execution run queue.
    pub run_queue: VecDeque<u64>,
    /// Current virtual machine monotonic tick count for timer and waiter expiration.
    pub now_ticks: u64,
    /// Caller context providing security credentials for VFS operations.
    pub caller_context: Option<super::vfs::AndroidCaller>,
    /// Monitor lock table tracking object identities, thread ownership, and recursion depth.
    pub monitors: BTreeMap<u32, MonitorRecord>,
    objects: Vec<DalvikObject>,
    next_object_id: u32,
    static_fields: Vec<(u16, u32)>,
    pub heap_max_bytes: usize,
    pub heap_used_bytes: usize,
    pub gc_trigger_threshold_bytes: usize,
    pub gc_cycles: u64,
    pub gc_reclaimed_objects: usize,
    pub gc_total_pause_ticks: u64,
}

/// Execution state of a managed Java thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JavaThreadStatus {
    Runnable,
    BlockedOnMonitor(u32),
    WaitingOnCondition(u32),
    TimedWaiting(u32, u64),
    Terminated,
}

/// Saved stack frame for inter-method Dalvik calls within a Java thread context.
#[derive(Debug, Clone)]
pub struct DalvikCallFrame {
    pub current_class: String,
    pub current_method: String,
    pub return_pc: usize,
    pub saved_registers: Vec<u32>,
    pub saved_string_registers: Vec<Option<String>>,
    pub code_offset: usize,
    pub registers_size: u16,
}

/// A managed Java thread state within the Dalvik VM holding complete execution context.
#[derive(Debug, Clone)]
pub struct JavaThreadState {
    pub thread_id: u64,
    pub name: String,
    pub status: JavaThreadStatus,
    pub interrupted: bool,
    pub owned_monitors: Vec<u32>,
    pub registers: Vec<u32>,
    pub string_registers: Vec<Option<String>>,
    pub call_depth: u8,
    pub pending_exception: Option<u32>,
    pub pc: usize,
    pub current_class: String,
    pub current_method: String,
    pub code_offset: usize,
    pub registers_size: u16,
    pub target_object: Option<u32>,
    pub call_stack: Vec<DalvikCallFrame>,
}

/// State of a synchronized Java monitor lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorRecord {
    pub owner_thread: u64,
    pub recursion_count: u32,
    pub wait_queue: Vec<u64>,
    pub cond_waiters: Vec<u64>,
}

/// Returns true if a source class descriptor is assignable to target class descriptor.
pub fn is_type_compatible(dex: &DexPackage, source_desc: &str, target_desc: &str) -> bool {
    if source_desc == target_desc {
        return true;
    }
    if target_desc == "Ljava/lang/Object;" {
        return true;
    }
    if source_desc.starts_with('[') {
        if target_desc == "Ljava/lang/Cloneable;" || target_desc == "Ljava/io/Serializable;" {
            return true;
        }
        if target_desc.starts_with('[') {
            let elem_source = &source_desc[1..];
            let elem_target = &target_desc[1..];
            if (elem_source.starts_with('L') || elem_source.starts_with('['))
                && (elem_target.starts_with('L') || elem_target.starts_with('['))
            {
                return is_type_compatible(dex, elem_source, elem_target);
            }
        }
        return false;
    }
    if class_implements_framework_interface(source_desc, target_desc) {
        return true;
    }

    let mut curr = source_desc;
    let mut depth = 0;
    while depth < 32 {
        if curr == target_desc {
            return true;
        }
        if class_implements_framework_interface(curr, target_desc) {
            return true;
        }
        if check_dex_interfaces(dex, curr, target_desc) {
            return true;
        }

        if let Some(super_desc) = find_dex_superclass(dex, curr) {
            curr = super_desc;
        } else if let Some(framework_super) = get_framework_superclass(curr) {
            curr = framework_super;
        } else {
            break;
        }
        depth += 1;
    }

    false
}

/// Zero-copy resolution of a class's declared superclass from DEX class_defs.
pub fn find_dex_superclass<'a>(dex: &'a DexPackage<'a>, class_desc: &str) -> Option<&'a str> {
    let class_defs_size = u32::from_le(dex.header.class_defs_size) as usize;
    let class_defs_off = u32::from_le(dex.header.class_defs_off) as usize;
    for i in 0..class_defs_size {
        let off = class_defs_off.checked_add(i.checked_mul(32)?)?;
        if off + 32 > dex.raw.len() { break; }
        let c_idx = u32::from_le_bytes(dex.raw[off..off + 4].try_into().ok()?);
        if let Ok(c_name) = dex.get_type_slice(c_idx) {
            if c_name == class_desc {
                let super_idx = u32::from_le_bytes(dex.raw[off + 8..off + 12].try_into().ok()?);
                if super_idx != 0xFFFF_FFFF {
                    return dex.get_type_slice(super_idx).ok();
                }
                return None;
            }
        }
    }
    None
}

/// Zero-copy check if a DEX class implements an interface directly or transitively.
pub fn check_dex_interfaces(dex: &DexPackage, class_desc: &str, target_iface: &str) -> bool {
    let class_defs_size = u32::from_le(dex.header.class_defs_size) as usize;
    let class_defs_off = u32::from_le(dex.header.class_defs_off) as usize;
    for i in 0..class_defs_size {
        let off = match class_defs_off.checked_add(i * 32) {
            Some(o) if o + 32 <= dex.raw.len() => o,
            _ => break,
        };
        let c_idx = match dex.raw[off..off + 4].try_into() {
            Ok(b) => u32::from_le_bytes(b),
            Err(_) => break,
        };
        if let Ok(c_name) = dex.get_type_slice(c_idx) {
            if c_name == class_desc {
                let if_off = match dex.raw[off + 12..off + 16].try_into() {
                    Ok(b) => u32::from_le_bytes(b) as usize,
                    Err(_) => 0,
                };
                if if_off > 0 && if_off + 4 <= dex.raw.len() {
                    let count = match dex.raw[if_off..if_off + 4].try_into() {
                        Ok(b) => u32::from_le_bytes(b) as usize,
                        Err(_) => 0,
                    };
                    for k in 0..count.min(64) {
                        let item_pos = if_off + 4 + k * 2;
                        if item_pos + 2 <= dex.raw.len() {
                            let type_idx = u16::from_le_bytes([dex.raw[item_pos], dex.raw[item_pos + 1]]) as u32;
                            if let Ok(if_name) = dex.get_type_slice(type_idx) {
                                if if_name == target_iface || is_type_compatible(dex, if_name, target_iface) {
                                    return true;
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
    }
    false
}

/// Known Android and Java standard framework class hierarchy.
pub fn get_framework_superclass(class_desc: &str) -> Option<&'static str> {
    match class_desc {
        "Landroid/app/Activity;" => Some("Landroid/view/ContextThemeWrapper;"),
        "Landroid/view/ContextThemeWrapper;" => Some("Landroid/content/ContextWrapper;"),
        "Landroid/app/Service;" | "Landroid/app/Application;" => Some("Landroid/content/ContextWrapper;"),
        "Landroid/content/ContextWrapper;" => Some("Landroid/content/Context;"),
        "Landroid/content/Context;" => Some("Ljava/lang/Object;"),
        "Landroid/view/ViewGroup;" | "Landroid/widget/TextView;" | "Landroid/widget/ImageView;" => Some("Landroid/view/View;"),
        "Landroid/widget/Button;" => Some("Landroid/widget/TextView;"),
        "Landroid/widget/LinearLayout;" | "Landroid/widget/RelativeLayout;" | "Landroid/widget/FrameLayout;" => Some("Landroid/view/ViewGroup;"),
        "Landroid/view/View;" => Some("Ljava/lang/Object;"),
        "Ljava/lang/Throwable;" => Some("Ljava/lang/Object;"),
        "Ljava/lang/Exception;" | "Ljava/lang/Error;" => Some("Ljava/lang/Throwable;"),
        "Ljava/lang/RuntimeException;" => Some("Ljava/lang/Exception;"),
        "Ljava/lang/NullPointerException;"
        | "Ljava/lang/IndexOutOfBoundsException;"
        | "Ljava/lang/IllegalArgumentException;"
        | "Ljava/lang/IllegalStateException;"
        | "Ljava/lang/IllegalMonitorStateException;"
        | "Ljava/lang/ArrayStoreException;"
        | "Ljava/lang/ClassCastException;"
        | "Ljava/lang/ArithmeticException;" => Some("Ljava/lang/RuntimeException;"),
        "Ljava/lang/ArrayIndexOutOfBoundsException;" | "Ljava/lang/StringIndexOutOfBoundsException;" => Some("Ljava/lang/IndexOutOfBoundsException;"),
        "Ljava/lang/ClassNotFoundException;" => Some("Ljava/lang/Exception;"),
        "Ljava/lang/Number;" => Some("Ljava/lang/Object;"),
        "Ljava/lang/Integer;" | "Ljava/lang/Long;" | "Ljava/lang/Float;" | "Ljava/lang/Double;" | "Ljava/lang/Byte;" | "Ljava/lang/Short;" => Some("Ljava/lang/Number;"),
        "Ljava/lang/Boolean;" | "Ljava/lang/Character;" | "Ljava/lang/String;" | "Ljava/lang/StringBuilder;" | "Ljava/lang/StringBuffer;" => Some("Ljava/lang/Object;"),
        "Ljava/util/AbstractCollection;" => Some("Ljava/lang/Object;"),
        "Ljava/util/AbstractList;" => Some("Ljava/util/AbstractCollection;"),
        "Ljava/util/ArrayList;" => Some("Ljava/util/AbstractList;"),
        "Ljava/util/AbstractMap;" => Some("Ljava/lang/Object;"),
        "Ljava/util/HashMap;" => Some("Ljava/util/AbstractMap;"),
        _ => None,
    }
}

/// Known interfaces implemented by Java standard library and Android framework classes.
pub fn class_implements_framework_interface(class_desc: &str, iface_desc: &str) -> bool {
    match iface_desc {
        "Ljava/lang/CharSequence;" => matches!(
            class_desc,
            "Ljava/lang/String;" | "Ljava/lang/StringBuilder;" | "Ljava/lang/StringBuffer;"
        ),
        "Ljava/io/Serializable;" => matches!(
            class_desc,
            "Ljava/lang/String;"
                | "Ljava/lang/StringBuilder;"
                | "Ljava/lang/Number;"
                | "Ljava/lang/Integer;"
                | "Ljava/lang/Long;"
                | "Ljava/lang/Float;"
                | "Ljava/lang/Double;"
                | "Ljava/lang/Boolean;"
                | "Ljava/lang/Character;"
                | "Ljava/lang/Byte;"
                | "Ljava/lang/Short;"
                | "Ljava/lang/Throwable;"
                | "Ljava/lang/Exception;"
                | "Ljava/lang/RuntimeException;"
                | "Ljava/lang/NullPointerException;"
                | "Ljava/lang/IndexOutOfBoundsException;"
                | "Ljava/lang/ArrayIndexOutOfBoundsException;"
                | "Ljava/lang/IllegalArgumentException;"
                | "Ljava/lang/IllegalStateException;"
                | "Ljava/lang/IllegalMonitorStateException;"
                | "Ljava/lang/ArrayStoreException;"
                | "Ljava/lang/ClassCastException;"
                | "Ljava/util/ArrayList;"
                | "Ljava/util/HashMap;"
        ),
        "Ljava/lang/Comparable;" => matches!(
            class_desc,
            "Ljava/lang/String;"
                | "Ljava/lang/Integer;"
                | "Ljava/lang/Long;"
                | "Ljava/lang/Float;"
                | "Ljava/lang/Double;"
                | "Ljava/lang/Boolean;"
                | "Ljava/lang/Character;"
                | "Ljava/lang/Byte;"
                | "Ljava/lang/Short;"
        ),
        "Ljava/lang/Cloneable;" => matches!(
            class_desc,
            "Ljava/util/ArrayList;" | "Ljava/util/HashMap;" | "Landroid/content/Intent;" | "Landroid/os/Bundle;"
        ),
        "Ljava/util/List;" => matches!(class_desc, "Ljava/util/ArrayList;" | "Ljava/util/AbstractList;"),
        "Ljava/util/Collection;" | "Ljava/lang/Iterable;" => matches!(
            class_desc,
            "Ljava/util/ArrayList;" | "Ljava/util/AbstractList;" | "Ljava/util/AbstractCollection;"
        ),
        "Ljava/util/Map;" => matches!(class_desc, "Ljava/util/HashMap;" | "Ljava/util/AbstractMap;"),
        "Landroid/os/Parcelable;" => matches!(class_desc, "Landroid/content/Intent;" | "Landroid/os/Bundle;"),
        _ => false,
    }
}

impl JavaThreadState {
    pub fn new(thread_id: u64, name: &str) -> Self {
        Self {
            thread_id,
            name: String::from(name),
            status: JavaThreadStatus::Runnable,
            interrupted: false,
            owned_monitors: Vec::new(),
            registers: alloc::vec![0u32; 32],
            string_registers: alloc::vec![None; 32],
            call_depth: 0,
            pending_exception: None,
            pc: 0,
            current_class: String::new(),
            current_method: String::new(),
            code_offset: 0,
            registers_size: 0,
            target_object: None,
            call_stack: Vec::new(),
        }
    }
}

impl DalvikVmState {
    /// Creates a fresh Dalvik Virtual Machine state with cleared registers and initial main thread.
    pub fn new() -> Self {
        let mut threads = BTreeMap::new();
        threads.insert(1, JavaThreadState::new(1, "main"));
        let mut run_queue = VecDeque::new();
        run_queue.push_back(1);

        Self {
            registers: alloc::vec![0u32; 32],
            string_registers: alloc::vec![None; 32],
            instructions_executed: 0,
            last_return_value: 0,
            last_return_wide_hi: 0,
            last_return_string: None,
            captured_logs: Vec::new(),
            call_depth: 0,
            pending_exception: None,
            current_task_id: 1,
            current_thread_id: 1,
            threads,
            run_queue,
            now_ticks: 0,
            caller_context: None,
            monitors: BTreeMap::new(),
            objects: Vec::new(),
            next_object_id: 1,
            static_fields: Vec::new(),
            heap_max_bytes: DEFAULT_HEAP_MAX_BYTES,
            heap_used_bytes: 0,
            gc_trigger_threshold_bytes: DEFAULT_HEAP_MAX_BYTES * 3 / 4,
            gc_cycles: 0,
            gc_reclaimed_objects: 0,
            gc_total_pause_ticks: 0,
        }
    }

    /// Saves the active VM execution context into the current thread state.
    pub fn save_current_thread_context(&mut self) {
        if let Some(t) = self.threads.get_mut(&self.current_thread_id) {
            t.registers = self.registers.clone();
            t.string_registers = self.string_registers.clone();
            t.call_depth = self.call_depth;
            t.pending_exception = self.pending_exception;
        }
    }

    /// Loads the active VM execution context from the specified thread state.
    pub fn load_current_thread_context(&mut self, thread_id: u64) {
        if let Some(t) = self.threads.get(&thread_id) {
            self.current_thread_id = thread_id;
            self.current_task_id = thread_id;
            self.registers = t.registers.clone();
            self.string_registers = t.string_registers.clone();
            self.call_depth = t.call_depth;
            self.pending_exception = t.pending_exception;
        }
    }

    /// Enqueues a thread into the cooperative run queue and marks it Runnable.
    pub fn enqueue_runnable(&mut self, thread_id: u64) {
        if let Some(t) = self.threads.get_mut(&thread_id) {
            t.status = JavaThreadStatus::Runnable;
        }
        if !self.run_queue.contains(&thread_id) {
            self.run_queue.push_back(thread_id);
        }
    }

    /// Pops the next runnable thread ID from the cooperative run queue.
    pub fn pop_next_runnable(&mut self) -> Option<u64> {
        while let Some(tid) = self.run_queue.pop_front() {
            if let Some(t) = self.threads.get(&tid) {
                if t.status == JavaThreadStatus::Runnable {
                    return Some(tid);
                }
            }
        }
        None
    }

    /// Wakes up threads waiting on timed monitor conditions or Thread.sleep whose timeout has elapsed.
    pub fn wake_timed_waiters(&mut self, current_ticks: u64) {
        self.now_ticks = current_ticks;
        let mut woke = Vec::new();
        for (&tid, t) in self.threads.iter() {
            if let JavaThreadStatus::TimedWaiting(obj_id, until_ticks) = t.status {
                if current_ticks >= until_ticks {
                    woke.push((tid, obj_id));
                }
            }
        }

        for (tid, obj_id) in woke {
            if obj_id == 0 {
                // Pure Thread.sleep() elapsed -> make Runnable and enqueue
                self.enqueue_runnable(tid);
            } else if let Some(rec) = self.monitors.get_mut(&obj_id) {
                rec.cond_waiters.retain(|&w| w != tid);
                if rec.owner_thread == 0 {
                    rec.owner_thread = tid;
                    rec.recursion_count = 1;
                    if let Some(t) = self.threads.get_mut(&tid) {
                        t.status = JavaThreadStatus::Runnable;
                        t.pc = t.pc.saturating_add(2);
                        if !t.owned_monitors.contains(&obj_id) {
                            t.owned_monitors.push(obj_id);
                        }
                    }
                    self.enqueue_runnable(tid);
                } else {
                    if !rec.wait_queue.contains(&tid) {
                        rec.wait_queue.push(tid);
                    }
                    if let Some(t) = self.threads.get_mut(&tid) {
                        t.status = JavaThreadStatus::BlockedOnMonitor(obj_id);
                    }
                }
            } else {
                self.enqueue_runnable(tid);
            }
        }
    }

    /// Advances the cooperative green-thread scheduler by up to `max_quanta` slices at monotonic tick `current_ticks`.
    pub fn step_scheduler_slice(&mut self, max_quanta: usize, current_ticks: u64) -> usize {
        self.wake_timed_waiters(current_ticks);

        let mut steps = 0;
        while steps < max_quanta {
            if let Some(next_tid) = self.pop_next_runnable() {
                self.save_current_thread_context();
                self.load_current_thread_context(next_tid);
                steps = steps.saturating_add(1);

                // If thread remains runnable, re-enqueue for round-robin fairness
                if let Some(t) = self.threads.get(&next_tid) {
                    if t.status == JavaThreadStatus::Runnable {
                        self.run_queue.push_back(next_tid);
                    }
                }
            } else {
                break;
            }
        }
        steps
    }

    /// Advances thread execution by slicing bytecode execution across runnable threads using DEX.
    pub fn step_scheduler_slice_with_dex(
        &mut self,
        dex: &DexPackage,
        max_quanta: usize,
        current_ticks: u64,
    ) -> usize {
        self.wake_timed_waiters(current_ticks);

        let mut steps = 0;
        while steps < max_quanta {
            if let Some(next_tid) = self.pop_next_runnable() {
                self.save_current_thread_context();
                self.load_current_thread_context(next_tid);
                steps = steps.saturating_add(1);

                let code_off = self.threads.get(&next_tid).map(|t| t.code_offset).unwrap_or(0);
                let reg_sz = self.threads.get(&next_tid).map(|t| t.registers_size).unwrap_or(0);
                if code_off > 0 && code_off + 16 <= dex.raw.len() {
                    let insns_size = u32::from_le_bytes([
                        dex.raw[code_off + 12],
                        dex.raw[code_off + 13],
                        dex.raw[code_off + 14],
                        dex.raw[code_off + 15],
                    ]) as usize;
                    let insns_bytes = insns_size.saturating_mul(2);
                    let insns_start = code_off + 16;
                    if insns_start + insns_bytes <= dex.raw.len() {
                        let insns = &dex.raw[insns_start..insns_start + insns_bytes];
                        let res = self.interpret_insns_with_code_offset(dex, reg_sz, code_off, insns);
                        match res {
                            Ok(_) => {
                                self.thread_exit(next_tid);
                            }
                            Err(DalvikVmError::MonitorBlocked(_)) => {
                                // Thread is blocked on monitor, do not re-enqueue
                            }
                            Err(_) => {
                                if let Some(t) = self.threads.get_mut(&next_tid) {
                                    t.status = JavaThreadStatus::Terminated;
                                }
                                self.thread_exit(next_tid);
                            }
                        }
                    }
                }

                // If thread remains runnable, re-enqueue for round-robin fairness
                if let Some(t) = self.threads.get(&next_tid) {
                    if t.status == JavaThreadStatus::Runnable {
                        self.run_queue.push_back(next_tid);
                    }
                }
            } else {
                break;
            }
        }
        steps
    }

    /// Sets the currently active thread identity for this VM.
    pub fn set_current_thread(&mut self, thread_id: u64) {
        self.save_current_thread_context();
        if !self.threads.contains_key(&thread_id) {
            let name = alloc::format!("Thread-{}", thread_id);
            self.threads.insert(thread_id, JavaThreadState::new(thread_id, &name));
        }
        self.load_current_thread_context(thread_id);
        self.enqueue_runnable(thread_id);
    }

    /// Spawns a new managed thread in the VM.
    pub fn spawn_thread(&mut self, thread_id: u64, name: &str) {
        self.threads.insert(thread_id, JavaThreadState::new(thread_id, name));
        self.enqueue_runnable(thread_id);
    }

    /// Releases all monitors held by a terminating thread, waking up contending waiters.
    pub fn thread_exit(&mut self, thread_id: u64) {
        self.run_queue.retain(|&tid| tid != thread_id);
        let owned = if let Some(t) = self.threads.get_mut(&thread_id) {
            t.status = JavaThreadStatus::Terminated;
            core::mem::take(&mut t.owned_monitors)
        } else {
            Vec::new()
        };

        for obj_id in owned {
            if let Some(rec) = self.monitors.get_mut(&obj_id) {
                if rec.owner_thread == thread_id {
                    rec.recursion_count = 0;
                    if !rec.wait_queue.is_empty() {
                        let next_owner = rec.wait_queue.remove(0);
                        rec.owner_thread = next_owner;
                        rec.recursion_count = 1;
                        if let Some(t) = self.threads.get_mut(&next_owner) {
                            t.status = JavaThreadStatus::Runnable;
                            t.pc = t.pc.saturating_add(2);
                            if !t.owned_monitors.contains(&obj_id) {
                                t.owned_monitors.push(obj_id);
                            }
                        }
                        self.enqueue_runnable(next_owner);
                    } else if rec.cond_waiters.is_empty() {
                        self.monitors.remove(&obj_id);
                    } else {
                        rec.owner_thread = 0;
                    }
                }
            }
        }
    }

    /// Implements Object.wait(): verifies caller owns monitor, releases lock, and enqueues thread on condition.
    pub fn monitor_wait(&mut self, obj_id: u32, timeout_millis: u64) -> Result<u32, DalvikVmError> {
        let tid = self.current_thread_id;
        let Some(rec) = self.monitors.get_mut(&obj_id) else {
            return Err(self.throw_illegal_monitor_state(Some("Object not locked")));
        };
        if rec.owner_thread != tid || rec.recursion_count == 0 {
            return Err(self.throw_illegal_monitor_state(Some("Current thread does not own monitor")));
        }

        // Release lock
        rec.recursion_count = 0;
        rec.cond_waiters.push(tid);

        if let Some(t) = self.threads.get_mut(&tid) {
            t.status = if timeout_millis > 0 {
                JavaThreadStatus::TimedWaiting(obj_id, timeout_millis)
            } else {
                JavaThreadStatus::WaitingOnCondition(obj_id)
            };
            t.owned_monitors.retain(|&m| m != obj_id);
        }

        // Wake next contending waiter if any
        if !rec.wait_queue.is_empty() {
            let next_owner = rec.wait_queue.remove(0);
            rec.owner_thread = next_owner;
            rec.recursion_count = 1;
            if let Some(t) = self.threads.get_mut(&next_owner) {
                t.status = JavaThreadStatus::Runnable;
                t.pc = t.pc.saturating_add(2);
                if !t.owned_monitors.contains(&obj_id) {
                    t.owned_monitors.push(obj_id);
                }
            }
            self.enqueue_runnable(next_owner);
        } else {
            rec.owner_thread = 0;
        }

        self.last_return_value = 0;
        self.last_return_string = None;
        Ok(0)
    }

    /// Implements Object.notify() / Object.notifyAll(): moves condition waiters to monitor acquisition wait queue.
    pub fn monitor_notify(&mut self, obj_id: u32, notify_all: bool) -> Result<u32, DalvikVmError> {
        let tid = self.current_thread_id;
        let Some(rec) = self.monitors.get_mut(&obj_id) else {
            return Err(self.throw_illegal_monitor_state(Some("Object not locked")));
        };
        if rec.owner_thread != tid || rec.recursion_count == 0 {
            return Err(self.throw_illegal_monitor_state(Some("Current thread does not own monitor")));
        }

        if notify_all {
            while !rec.cond_waiters.is_empty() {
                let waiter_tid = rec.cond_waiters.remove(0);
                if !rec.wait_queue.contains(&waiter_tid) {
                    rec.wait_queue.push(waiter_tid);
                }
                if let Some(t) = self.threads.get_mut(&waiter_tid) {
                    t.status = JavaThreadStatus::BlockedOnMonitor(obj_id);
                }
            }
        } else if !rec.cond_waiters.is_empty() {
            let waiter_tid = rec.cond_waiters.remove(0);
            if !rec.wait_queue.contains(&waiter_tid) {
                rec.wait_queue.push(waiter_tid);
            }
            if let Some(t) = self.threads.get_mut(&waiter_tid) {
                t.status = JavaThreadStatus::BlockedOnMonitor(obj_id);
            }
        }

        self.last_return_value = 0;
        self.last_return_string = None;
        Ok(0)
    }

    /// Mark-and-sweep garbage collection over the managed Dalvik object heap.
    /// Traces roots from active registers across all threads, call stacks, and static fields.
    pub fn gc_mark_and_sweep(&mut self) -> (usize, u64) {
        let mut marked = alloc::collections::BTreeSet::new();

        // Roots from currently active registers
        for reg in &self.registers {
            if *reg > 0 && *reg < self.next_object_id {
                marked.insert(*reg);
            }
        }

        // Roots from all thread contexts (registers, call stack frames, target objects)
        for thread in self.threads.values() {
            for reg in &thread.registers {
                if *reg > 0 && *reg < self.next_object_id {
                    marked.insert(*reg);
                }
            }
            if let Some(target) = thread.target_object {
                if target > 0 && target < self.next_object_id {
                    marked.insert(target);
                }
            }
            for frame in &thread.call_stack {
                for reg in &frame.saved_registers {
                    if *reg > 0 && *reg < self.next_object_id {
                        marked.insert(*reg);
                    }
                }
            }
        }

        // Roots from static fields
        for (_, val) in &self.static_fields {
            if *val > 0 && *val < self.next_object_id {
                marked.insert(*val);
            }
        }

        // Traverse fields and stateful payloads of marked objects
        let mut queue: Vec<u32> = marked.iter().copied().collect();
        while let Some(obj_id) = queue.pop() {
            if let Some(obj) = self.objects.iter().find(|o| o.id == obj_id) {
                for (_, f_val) in &obj.fields {
                    if *f_val > 0 && *f_val < self.next_object_id && marked.insert(*f_val) {
                        queue.push(*f_val);
                    }
                }
                if let Some(ref elems) = obj.array_elements {
                    for elem in elems {
                        if *elem > 0 && *elem < self.next_object_id && marked.insert(*elem) {
                            queue.push(*elem);
                        }
                    }
                }
                match &obj.payload {
                    ObjectPayload::ArrayList(list) => {
                        for item in list {
                            if *item > 0 && *item < self.next_object_id && marked.insert(*item) {
                                queue.push(*item);
                            }
                        }
                    }
                    ObjectPayload::HashMap(map) => {
                        for (_, val) in map {
                            if *val > 0 && *val < self.next_object_id && marked.insert(*val) {
                                queue.push(*val);
                            }
                        }
                    }
                    ObjectPayload::Throwable { cause_id: Some(cause), .. } => {
                        if *cause > 0 && *cause < self.next_object_id && marked.insert(*cause) {
                            queue.push(*cause);
                        }
                    }
                    _ => {}
                }
            }
        }

        let initial_count = self.objects.len();
        self.objects.retain(|o| marked.contains(&o.id));
        let reclaimed = initial_count.saturating_sub(self.objects.len());
        self.heap_used_bytes = self.objects.iter().map(|o| o.estimated_size_bytes()).sum();
        self.gc_cycles += 1;
        self.gc_reclaimed_objects += reclaimed;
        let pause_ticks = (reclaimed as u64) * 5 + 10;
        self.gc_total_pause_ticks += pause_ticks;
        (reclaimed, pause_ticks)
    }

    /// Reads a static field value, returning 0 if uninitialized.
    pub fn get_static_field(&self, field_idx: u16) -> u32 {
        self.static_fields.iter().find(|(idx, _)| *idx == field_idx).map(|(_, v)| *v).unwrap_or(0)
    }

    /// Sets a static field value in the VM's static storage table.
    pub fn set_static_field(&mut self, field_idx: u16, val: u32) -> Result<(), DalvikVmError> {
        if let Some(entry) = self.static_fields.iter_mut().find(|(idx, _)| *idx == field_idx) {
            entry.1 = val;
            Ok(())
        } else if self.static_fields.len() < 256 {
            self.static_fields.push((field_idx, val));
            Ok(())
        } else {
            Err(DalvikVmError::ObjectHeapLimitExceeded)
        }
    }

    /// Allocates an actual VM-local object identity; IDs are never reused during this VM's life.
    fn allocate_object(&mut self, class_descriptor: &str) -> Result<u32, DalvikVmError> {
        let est_size = 64 + class_descriptor.len();
        if self.heap_used_bytes.saturating_add(est_size) > self.gc_trigger_threshold_bytes {
            self.gc_mark_and_sweep();
        }
        if self.heap_used_bytes.saturating_add(est_size) > self.heap_max_bytes {
            self.gc_mark_and_sweep();
            if self.heap_used_bytes.saturating_add(est_size) > self.heap_max_bytes {
                return Err(DalvikVmError::ObjectHeapLimitExceeded);
            }
        }
        if (!class_descriptor.starts_with('L') || !class_descriptor.ends_with(';')) && !class_descriptor.starts_with('[') {
            return Err(DalvikVmError::InvalidBytecode);
        }
        let id = self.next_object_id;
        self.next_object_id = id.checked_add(1).ok_or(DalvikVmError::ObjectHeapLimitExceeded)?;
        let obj = DalvikObject::new(id, class_descriptor);
        let actual_size = obj.estimated_size_bytes();
        self.heap_used_bytes = self.heap_used_bytes.saturating_add(actual_size);
        self.objects.push(obj);
        Ok(id)
    }

    fn allocate_array(&mut self, element_type: &str, length: usize) -> Result<u32, DalvikVmError> {
        if length > 65536 {
            return Err(DalvikVmError::ObjectHeapLimitExceeded);
        }
        let slots = if element_type.starts_with("[J") || element_type.starts_with("[D") {
            length.saturating_mul(2)
        } else {
            length
        };
        let est_size = 64 + element_type.len() + slots.saturating_mul(4);
        if self.heap_used_bytes.saturating_add(est_size) > self.gc_trigger_threshold_bytes {
            self.gc_mark_and_sweep();
        }
        if self.heap_used_bytes.saturating_add(est_size) > self.heap_max_bytes {
            self.gc_mark_and_sweep();
            if self.heap_used_bytes.saturating_add(est_size) > self.heap_max_bytes {
                return Err(DalvikVmError::ObjectHeapLimitExceeded);
            }
        }
        let id = self.next_object_id;
        self.next_object_id = id.checked_add(1).ok_or(DalvikVmError::ObjectHeapLimitExceeded)?;
        let arr = DalvikObject::new_array(id, element_type, slots);
        let actual_size = arr.estimated_size_bytes();
        self.heap_used_bytes = self.heap_used_bytes.saturating_add(actual_size);
        self.objects.push(arr);
        Ok(id)
    }

    pub fn get_wide_reg(&self, reg: usize) -> u64 {
        if reg + 1 < self.registers.len() {
            (self.registers[reg] as u64) | ((self.registers[reg + 1] as u64) << 32)
        } else if reg < self.registers.len() {
            self.registers[reg] as u64
        } else {
            0
        }
    }

    pub fn set_wide_reg(&mut self, reg: usize, val: u64) -> Result<(), DalvikVmError> {
        let reg_cap = self.registers.len();
        if reg + 1 >= reg_cap {
            return Err(DalvikVmError::RegisterOutOfBounds);
        }
        self.registers[reg] = val as u32;
        self.registers[reg + 1] = (val >> 32) as u32;
        if reg < self.string_registers.len() {
            self.string_registers[reg] = None;
        }
        if reg + 1 < self.string_registers.len() {
            self.string_registers[reg + 1] = None;
        }
        Ok(())
    }

    pub fn throw_class_cast(&mut self, msg: Option<&str>) -> DalvikVmError {
        if let Ok(obj_id) = self.allocate_object("Ljava/lang/ClassCastException;") {
            if let Ok(obj) = self.get_object_mut(obj_id) {
                obj.payload = ObjectPayload::Throwable {
                    message: msg.map(String::from),
                    cause_id: None,
                };
            }
            DalvikVmError::UnhandledException(obj_id)
        } else {
            DalvikVmError::InvalidBytecode
        }
    }

    fn get_object(&self, id: u32) -> Result<&DalvikObject, DalvikVmError> {
        self.objects.iter().find(|o| o.id == id).ok_or(DalvikVmError::NullPointerDereference)
    }

    fn get_object_mut(&mut self, id: u32) -> Result<&mut DalvikObject, DalvikVmError> {
        self.objects.iter_mut().find(|o| o.id == id).ok_or(DalvikVmError::NullPointerDereference)
    }

    /// Allocates a new String object identity with the given content.
    pub fn allocate_string(&mut self, text: &str) -> Result<u32, DalvikVmError> {
        let id = self.allocate_object("Ljava/lang/String;")?;
        if let Ok(obj) = self.get_object_mut(id) {
            obj.payload = ObjectPayload::String(String::from(text));
        }
        Ok(id)
    }

    /// Retrieves string text if the given object is a String, StringBuilder, or Class.
    pub fn get_string_payload(&self, id: u32) -> Option<String> {
        if id == 0 {
            return None;
        }
        if let Ok(obj) = self.get_object(id) {
            match &obj.payload {
                ObjectPayload::String(s) => return Some(s.clone()),
                ObjectPayload::StringBuilder(s) => return Some(s.clone()),
                ObjectPayload::Class { descriptor } => {
                    let clean = descriptor.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(descriptor);
                    return Some(clean.replace('/', "."));
                }
                _ => {}
            }
        }
        None
    }

    /// Seamlessly resolves a string argument across shadow registers, object heap, and DEX string pool.
    pub fn resolve_string_arg(&self, dex: &DexPackage, reg_val: u32, shadow_str: Option<&str>) -> Option<String> {
        if let Some(s) = shadow_str {
            return Some(String::from(s));
        }
        if let Some(s) = self.get_string_payload(reg_val) {
            return Some(s);
        }
        if let Ok(s) = dex.get_string_slice(reg_val) {
            return Some(String::from(s));
        }
        None
    }

    fn resolve_string_for_receiver(&self, dex: &DexPackage, arg_registers: &[usize], arg_values: &[u32]) -> String {
        if !arg_values.is_empty() {
            let shadow = if !arg_registers.is_empty() && arg_registers[0] < self.string_registers.len() {
                self.string_registers[arg_registers[0]].as_deref()
            } else {
                None
            };
            self.resolve_string_arg(dex, arg_values[0], shadow).unwrap_or_default()
        } else {
            String::new()
        }
    }

    fn get_class_descriptor_for_receiver(&self, arg_values: &[u32]) -> String {
        if !arg_values.is_empty() {
            if let Ok(obj) = self.get_object(arg_values[0]) {
                if let ObjectPayload::Class { descriptor } = &obj.payload {
                    return descriptor.clone();
                }
                return obj.class_descriptor.clone();
            }
        }
        String::from("Ljava/lang/Object;")
    }

    /// Allocates an exception object and returns an UnhandledException error with its ID.
    pub fn throw_exception(&mut self, class_descriptor: &str, message: Option<&str>) -> DalvikVmError {
        match self.allocate_object(class_descriptor) {
            Ok(ex_id) => {
                if let Ok(obj) = self.get_object_mut(ex_id) {
                    obj.payload = ObjectPayload::Throwable {
                        message: message.map(String::from),
                        cause_id: None,
                    };
                }
                DalvikVmError::UnhandledException(ex_id)
            }
            Err(e) => e,
        }
    }

    pub fn throw_npe(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/NullPointerException;", message)
    }

    pub fn throw_oob(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/IndexOutOfBoundsException;", message)
    }

    pub fn throw_class_not_found(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/ClassNotFoundException;", message)
    }

    pub fn throw_illegal_monitor_state(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/IllegalMonitorStateException;", message)
    }

    pub fn throw_array_store(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/ArrayStoreException;", message)
    }

    pub fn throw_arithmetic(&mut self, message: Option<&str>) -> DalvikVmError {
        self.throw_exception("Ljava/lang/ArithmeticException;", message)
    }

    /// Dispatches Java standard library and Android framework methods with genuine VM-managed state.
    pub fn dispatch_framework_method(
        &mut self,
        dex: &DexPackage,
        class_name: &str,
        method_name: &str,
        arg_registers: &[usize],
        arg_values: &[u32],
    ) -> Result<u32, DalvikVmError> {
        let class = class_name.strip_prefix('L').and_then(|s| s.strip_suffix(';')).unwrap_or(class_name);
        match class {
            "java/lang/Object" => {
                match method_name {
                    "<init>" => {
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "wait" => {
                        let receiver_id = arg_values.get(0).copied().unwrap_or(0);
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null receiver in Object.wait")));
                        }
                        self.monitor_wait(receiver_id, 0)
                    }
                    "notify" => {
                        let receiver_id = arg_values.get(0).copied().unwrap_or(0);
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null receiver in Object.notify")));
                        }
                        self.monitor_notify(receiver_id, false)
                    }
                    "notifyAll" => {
                        let receiver_id = arg_values.get(0).copied().unwrap_or(0);
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null receiver in Object.notifyAll")));
                        }
                        self.monitor_notify(receiver_id, true)
                    }
                    "equals" => {
                        let a = arg_values.get(0).copied().unwrap_or(0);
                        let b = arg_values.get(1).copied().unwrap_or(0);
                        let eq = if a == b { 1 } else { 0 };
                        self.last_return_value = eq;
                        self.last_return_string = None;
                        Ok(eq)
                    }
                    "hashCode" => {
                        let a = arg_values.get(0).copied().unwrap_or(0);
                        self.last_return_value = a;
                        self.last_return_string = None;
                        Ok(a)
                    }
                    _ => {
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                }
            }
            "java/lang/Thread" => {
                match method_name {
                    "<init>" => {
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "currentThread" => {
                        self.last_return_value = self.current_thread_id as u32;
                        self.last_return_string = None;
                        Ok(self.current_thread_id as u32)
                    }
                    "getId" => {
                        self.last_return_value = self.current_thread_id as u32;
                        self.last_return_string = None;
                        Ok(self.current_thread_id as u32)
                    }
                    "isInterrupted" => {
                        let interrupted = self.threads.get(&self.current_thread_id).map(|t| t.interrupted).unwrap_or(false);
                        let res = if interrupted { 1 } else { 0 };
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "interrupt" => {
                        let target_tid = arg_values.get(0).copied().unwrap_or(0) as u64;
                        let tid = if target_tid == 0 { self.current_thread_id } else { target_tid };
                        if let Some(t) = self.threads.get_mut(&tid) {
                            t.interrupted = true;
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "start" => {
                        let new_tid = self.threads.keys().max().copied().unwrap_or(1) + 1;
                        let name = alloc::format!("Thread-{}", new_tid);
                        self.spawn_thread(new_tid, &name);
                        let target_obj = arg_values.first().copied().unwrap_or(0);
                        let target_desc = if target_obj > 0 {
                            self.get_object(target_obj).ok().map(|o| o.class_descriptor.clone())
                        } else {
                            None
                        };
                        if let Some(t) = self.threads.get_mut(&new_tid) {
                            if let Some(desc) = target_desc {
                                t.target_object = Some(target_obj);
                                t.current_class = desc.clone();
                                t.current_method = String::from("run");
                                if let Some(meth) = dex.methods.iter().find(|m| m.class_name == desc && m.method_name == "run") {
                                    t.code_offset = meth.code_offset;
                                    t.registers_size = meth.registers_size;
                                    if (meth.registers_size as usize) > t.registers.len() {
                                        t.registers.resize(meth.registers_size as usize, 0);
                                        t.string_registers.resize(meth.registers_size as usize, None);
                                    }
                                    if meth.registers_size > 0 && meth.ins_size > 0 {
                                        let this_reg = (meth.registers_size.saturating_sub(meth.ins_size)) as usize;
                                        if this_reg < t.registers.len() {
                                            t.registers[this_reg] = target_obj;
                                        }
                                    }
                                }
                            }
                        }
                        self.last_return_value = new_tid as u32;
                        self.last_return_string = None;
                        Ok(new_tid as u32)
                    }
                    "yield" => {
                        self.enqueue_runnable(self.current_thread_id);
                        if let Some(next_tid) = self.pop_next_runnable() {
                            self.current_thread_id = next_tid;
                            self.current_task_id = next_tid;
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "sleep" => {
                        let millis = arg_values.first().copied().unwrap_or(1) as u64;
                        let until_ticks = self.now_ticks.saturating_add(millis);
                        if let Some(t) = self.threads.get_mut(&self.current_thread_id) {
                            t.status = JavaThreadStatus::TimedWaiting(0, until_ticks);
                        }
                        if let Some(next_tid) = self.pop_next_runnable() {
                            self.current_thread_id = next_tid;
                            self.current_task_id = next_tid;
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => {
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                }
            }
            "java/lang/String" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let init_str = if arg_values.len() > 1 {
                                let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                    self.string_registers[arg_registers[1]].as_deref()
                                } else {
                                    None
                                };
                                self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::String(init_str.clone());
                            }
                            if let Some(r) = arg_registers.first() {
                                if *r < self.string_registers.len() {
                                    self.string_registers[*r] = Some(init_str);
                                }
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "length" => {
                        let s = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let len = s.encode_utf16().count() as u32;
                        self.last_return_value = len;
                        self.last_return_string = None;
                        Ok(len)
                    }
                    "charAt" => {
                        let s = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let idx = arg_values.get(1).copied().unwrap_or(0) as usize;
                        let utf16: Vec<u16> = s.encode_utf16().collect();
                        if idx >= utf16.len() {
                            return Err(self.throw_oob(Some("String index out of bounds")));
                        }
                        let ch = utf16[idx] as u32;
                        self.last_return_value = ch;
                        self.last_return_string = None;
                        Ok(ch)
                    }
                    "substring" => {
                        let s = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let utf16: Vec<u16> = s.encode_utf16().collect();
                        let begin = arg_values.get(1).copied().unwrap_or(0) as usize;
                        let end = if arg_values.len() > 2 { arg_values[2] as usize } else { utf16.len() };
                        if begin > end || end > utf16.len() {
                            return Err(self.throw_oob(Some("String index out of bounds")));
                        }
                        let sub = String::from_utf16_lossy(&utf16[begin..end]);
                        let new_id = self.allocate_string(&sub)?;
                        self.last_return_value = new_id;
                        self.last_return_string = Some(sub);
                        Ok(new_id)
                    }
                    "concat" => {
                        let s1 = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let s2 = if arg_values.len() > 1 {
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let mut res = s1;
                        res.push_str(&s2);
                        let new_id = self.allocate_string(&res)?;
                        self.last_return_value = new_id;
                        self.last_return_string = Some(res);
                        Ok(new_id)
                    }
                    "equals" => {
                        let s1 = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let s2_opt = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow)
                        } else {
                            None
                        };
                        let eq = match s2_opt {
                            Some(s2) => s1 == s2,
                            None => false,
                        };
                        let res = if eq { 1 } else { 0 };
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "isEmpty" => {
                        let s = self.resolve_string_for_receiver(dex, arg_registers, arg_values);
                        let is_empty = if s.is_empty() { 1 } else { 0 };
                        self.last_return_value = is_empty;
                        self.last_return_string = None;
                        Ok(is_empty)
                    }
                    "valueOf" => {
                        let s = if !arg_values.is_empty() {
                            let shadow = if !arg_registers.is_empty() && arg_registers[0] < self.string_registers.len() {
                                self.string_registers[arg_registers[0]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[0], shadow).unwrap_or_else(|| {
                                alloc::format!("{}", arg_values[0])
                            })
                        } else {
                            String::new()
                        };
                        let new_id = self.allocate_string(&s)?;
                        self.last_return_value = new_id;
                        self.last_return_string = Some(s);
                        Ok(new_id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/StringBuilder" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let init_str = if arg_values.len() > 1 {
                                let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                    self.string_registers[arg_registers[1]].as_deref()
                                } else {
                                    None
                                };
                                self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_default()
                            } else {
                                String::new()
                            };
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::StringBuilder(init_str);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "append" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null StringBuilder receiver")));
                        }
                        let append_str = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_else(|| {
                                alloc::format!("{}", arg_values[1])
                            })
                        } else {
                            String::new()
                        };
                        let current_buf = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            match &mut obj.payload {
                                ObjectPayload::StringBuilder(buf) => {
                                    buf.push_str(&append_str);
                                    buf.clone()
                                }
                                _ => {
                                    let mut buf = String::new();
                                    buf.push_str(&append_str);
                                    obj.payload = ObjectPayload::StringBuilder(buf.clone());
                                    buf
                                }
                            }
                        } else {
                            return Err(self.throw_npe(Some("Invalid StringBuilder object")));
                        };
                        self.last_return_value = receiver_id;
                        self.last_return_string = Some(current_buf);
                        Ok(receiver_id)
                    }
                    "toString" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let buf = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::StringBuilder(s) => s.clone(),
                                _ => String::new(),
                            }
                        } else {
                            String::new()
                        };
                        let str_id = self.allocate_string(&buf)?;
                        self.last_return_value = str_id;
                        self.last_return_string = Some(buf);
                        Ok(str_id)
                    }
                    "length" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let len = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::StringBuilder(s) => s.encode_utf16().count() as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = len;
                        self.last_return_string = None;
                        Ok(len)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/Integer" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let val = arg_values.get(1).copied().unwrap_or(0) as i32;
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::BoxedInteger(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "valueOf" => {
                        let val = arg_values.first().copied().unwrap_or(0) as i32;
                        let id = self.allocate_object("Ljava/lang/Integer;")?;
                        if let Ok(obj) = self.get_object_mut(id) {
                            obj.payload = ObjectPayload::BoxedInteger(val);
                        }
                        self.last_return_value = id;
                        self.last_return_string = None;
                        Ok(id)
                    }
                    "intValue" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::BoxedInteger(v) => *v as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "toString" => {
                        let val_str = if !arg_values.is_empty() {
                            if let Ok(obj) = self.get_object(arg_values[0]) {
                                match &obj.payload {
                                    ObjectPayload::BoxedInteger(v) => alloc::format!("{}", v),
                                    _ => String::from("0"),
                                }
                            } else {
                                alloc::format!("{}", arg_values[0])
                            }
                        } else {
                            String::from("0")
                        };
                        let id = self.allocate_string(&val_str)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(val_str);
                        Ok(id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/Long" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let lo = arg_values.get(1).copied().unwrap_or(0) as u64;
                            let hi = arg_values.get(2).copied().unwrap_or(0) as u64;
                            let val = ((hi << 32) | lo) as i64;
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::BoxedLong(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "valueOf" => {
                        let lo = arg_values.first().copied().unwrap_or(0) as u64;
                        let hi = arg_values.get(1).copied().unwrap_or(0) as u64;
                        let val = ((hi << 32) | lo) as i64;
                        let id = self.allocate_object("Ljava/lang/Long;")?;
                        if let Ok(obj) = self.get_object_mut(id) {
                            obj.payload = ObjectPayload::BoxedLong(val);
                        }
                        self.last_return_value = id;
                        self.last_return_string = None;
                        Ok(id)
                    }
                    "longValue" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::BoxedLong(v) => *v,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = (val as u64 & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((val as u64 >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "intValue" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::BoxedLong(v) => *v as i32 as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/Boolean" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let val = arg_values.get(1).copied().unwrap_or(0) != 0;
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::BoxedBoolean(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "valueOf" => {
                        let val = arg_values.first().copied().unwrap_or(0) != 0;
                        let id = self.allocate_object("Ljava/lang/Boolean;")?;
                        if let Ok(obj) = self.get_object_mut(id) {
                            obj.payload = ObjectPayload::BoxedBoolean(val);
                        }
                        self.last_return_value = id;
                        self.last_return_string = None;
                        Ok(id)
                    }
                    "booleanValue" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::BoxedBoolean(b) => if *b { 1 } else { 0 },
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/Class" => {
                match method_name {
                    "forName" => {
                        let shadow = if !arg_registers.is_empty() && arg_registers[0] < self.string_registers.len() {
                            self.string_registers[arg_registers[0]].as_deref()
                        } else {
                            None
                        };
                        let class_name = if !arg_values.is_empty() {
                            self.resolve_string_arg(dex, arg_values[0], shadow).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        let descriptor = if class_name.starts_with('L') && class_name.ends_with(';') {
                            class_name.clone()
                        } else {
                            alloc::format!("L{};", class_name.replace('.', "/"))
                        };
                        let exists = dex.types.iter().any(|t| t == &descriptor)
                            || dex.methods.iter().any(|m| m.class_name == descriptor)
                            || matches!(descriptor.as_str(), "Ljava/lang/String;" | "Ljava/lang/Object;" | "Ljava/lang/Integer;" | "Ljava/lang/Boolean;");
                        if !exists {
                            return Err(self.throw_class_not_found(Some(&class_name)));
                        }
                        let id = self.allocate_object("Ljava/lang/Class;")?;
                        if let Ok(obj) = self.get_object_mut(id) {
                            obj.payload = ObjectPayload::Class { descriptor };
                        }
                        self.last_return_value = id;
                        self.last_return_string = None;
                        Ok(id)
                    }
                    "getName" => {
                        let desc = self.get_class_descriptor_for_receiver(arg_values);
                        let clean = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(&desc);
                        let dotted = clean.replace('/', ".");
                        let id = self.allocate_string(&dotted)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(dotted);
                        Ok(id)
                    }
                    "getSimpleName" => {
                        let desc = self.get_class_descriptor_for_receiver(arg_values);
                        let clean = desc.strip_prefix('L').and_then(|d| d.strip_suffix(';')).unwrap_or(&desc);
                        let simple = clean.rsplit('/').next().unwrap_or(clean);
                        let id = self.allocate_string(simple)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(String::from(simple));
                        Ok(id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/util/ArrayList" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::ArrayList(Vec::new());
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "add" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null ArrayList receiver")));
                        }
                        if arg_values.len() == 2 {
                            let elem = arg_values[1];
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                match &mut obj.payload {
                                    ObjectPayload::ArrayList(list) => {
                                        list.push(elem);
                                    }
                                    _ => {
                                        obj.payload = ObjectPayload::ArrayList(alloc::vec![elem]);
                                    }
                                }
                            }
                            self.last_return_value = 1;
                            self.last_return_string = None;
                            Ok(1)
                        } else {
                            let idx = arg_values[1] as usize;
                            let elem = arg_values[2];
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                match &mut obj.payload {
                                    ObjectPayload::ArrayList(list) => {
                                        if idx > list.len() {
                                            return Err(self.throw_oob(Some("IndexOutOfBoundsException")));
                                        }
                                        list.insert(idx, elem);
                                    }
                                    _ => {
                                        if idx > 0 {
                                            return Err(self.throw_oob(Some("IndexOutOfBoundsException")));
                                        }
                                        obj.payload = ObjectPayload::ArrayList(alloc::vec![elem]);
                                    }
                                }
                            }
                            self.last_return_value = 0;
                            self.last_return_string = None;
                            Ok(0)
                        }
                    }
                    "get" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null ArrayList receiver")));
                        }
                        let idx = arg_values[1] as usize;
                        let elem = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::ArrayList(list) => {
                                    if idx >= list.len() {
                                        return Err(self.throw_oob(Some("IndexOutOfBoundsException")));
                                    }
                                    list[idx]
                                }
                                _ => return Err(self.throw_oob(Some("IndexOutOfBoundsException"))),
                            }
                        } else {
                            0
                        };
                        self.last_return_value = elem;
                        self.last_return_string = self.get_string_payload(elem);
                        Ok(elem)
                    }
                    "size" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let len = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::ArrayList(list) => list.len() as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = len;
                        self.last_return_string = None;
                        Ok(len)
                    }
                    "isEmpty" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let is_empty = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::ArrayList(list) => if list.is_empty() { 1 } else { 0 },
                                _ => 1,
                            }
                        } else {
                            1
                        };
                        self.last_return_value = is_empty;
                        self.last_return_string = None;
                        Ok(is_empty)
                    }
                    "clear" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::ArrayList(list) = &mut obj.payload {
                                list.clear();
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/content/Intent" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            let action = if arg_values.len() > 1 {
                                let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                    self.string_registers[arg_registers[1]].as_deref()
                                } else {
                                    None
                                };
                                self.resolve_string_arg(dex, arg_values[1], shadow)
                            } else {
                                None
                            };
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::Intent {
                                    action,
                                    categories: Vec::new(),
                                    data_uri: None,
                                    extras: BTreeMap::new(),
                                };
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "setAction" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let action = self.resolve_string_arg(dex, arg_values[1], shadow);
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Intent { action: act, .. } = &mut obj.payload {
                                *act = action;
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "getAction" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let action = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Intent { action: Some(a), .. } => Some(a.clone()),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        match action {
                            Some(a) => {
                                let id = self.allocate_string(&a)?;
                                self.last_return_value = id;
                                self.last_return_string = Some(a);
                                Ok(id)
                            }
                            None => {
                                self.last_return_value = 0;
                                self.last_return_string = None;
                                Ok(0)
                            }
                        }
                    }
                    "putExtra" => {
                        if arg_values.len() < 3 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let val_shadow = if arg_registers.len() > 2 && arg_registers[2] < self.string_registers.len() {
                            self.string_registers[arg_registers[2]].as_deref()
                        } else {
                            None
                        };
                        let val_bundle = if let Some(s) = self.resolve_string_arg(dex, arg_values[2], val_shadow) {
                            BundleValue::String(s)
                        } else {
                            BundleValue::Int(arg_values[2] as i32)
                        };
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Intent { extras, .. } = &mut obj.payload {
                                extras.insert(key, val_bundle);
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "getStringExtra" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Intent { extras, .. } => {
                                    match extras.get(&key) {
                                        Some(BundleValue::String(s)) => Some(s.clone()),
                                        _ => None,
                                    }
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        match val {
                            Some(s) => {
                                let id = self.allocate_string(&s)?;
                                self.last_return_value = id;
                                self.last_return_string = Some(s);
                                Ok(id)
                            }
                            None => {
                                self.last_return_value = 0;
                                self.last_return_string = None;
                                Ok(0)
                            }
                        }
                    }
                    "getIntExtra" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let default_val = arg_values.get(2).copied().unwrap_or(0);
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Intent { extras, .. } => {
                                    match extras.get(&key) {
                                        Some(BundleValue::Int(v)) => *v as u32,
                                        _ => default_val,
                                    }
                                }
                                _ => default_val,
                            }
                        } else {
                            default_val
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "putExtras" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let bundle_id = arg_values[1];
                        let bundle_extras = if let Ok(b_obj) = self.get_object(bundle_id) {
                            match &b_obj.payload {
                                ObjectPayload::Bundle(map) => Some(map.clone()),
                                _ => None,
                            }
                        } else {
                            None
                        };
                        if let Some(map) = bundle_extras {
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                if let ObjectPayload::Intent { extras, .. } = &mut obj.payload {
                                    for (k, v) in map {
                                        extras.insert(k, v);
                                    }
                                }
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "getExtras" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let extras_clone = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Intent { extras, .. } => extras.clone(),
                                _ => BTreeMap::new(),
                            }
                        } else {
                            BTreeMap::new()
                        };
                        let b_id = self.allocate_object("Landroid/os/Bundle;")?;
                        if let Ok(b_obj) = self.get_object_mut(b_id) {
                            b_obj.payload = ObjectPayload::Bundle(extras_clone);
                        }
                        self.last_return_value = b_id;
                        self.last_return_string = None;
                        Ok(b_id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/os/Bundle" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::Bundle(BTreeMap::new());
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "putString" => {
                        if arg_values.len() < 3 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let val_shadow = if arg_registers.len() > 2 && arg_registers[2] < self.string_registers.len() {
                            self.string_registers[arg_registers[2]].as_deref()
                        } else {
                            None
                        };
                        let val = self.resolve_string_arg(dex, arg_values[2], val_shadow).unwrap_or_default();
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Bundle(map) = &mut obj.payload {
                                map.insert(key, BundleValue::String(val));
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "getString" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Bundle(map) => {
                                    match map.get(&key) {
                                        Some(BundleValue::String(s)) => Some(s.clone()),
                                        _ => None,
                                    }
                                }
                                _ => None,
                            }
                        } else {
                            None
                        };
                        match val {
                            Some(s) => {
                                let id = self.allocate_string(&s)?;
                                self.last_return_value = id;
                                self.last_return_string = Some(s);
                                Ok(id)
                            }
                            None => {
                                self.last_return_value = 0;
                                self.last_return_string = None;
                                Ok(0)
                            }
                        }
                    }
                    "putInt" => {
                        if arg_values.len() < 3 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let val = arg_values[2] as i32;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Bundle(map) = &mut obj.payload {
                                map.insert(key, BundleValue::Int(val));
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "getInt" => {
                        if arg_values.len() < 2 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let key = self.resolve_string_arg(dex, arg_values[1], key_shadow).unwrap_or_default();
                        let default_val = arg_values.get(2).copied().unwrap_or(0);
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Bundle(map) => {
                                    match map.get(&key) {
                                        Some(BundleValue::Int(v)) => *v as u32,
                                        _ => default_val,
                                    }
                                }
                                _ => default_val,
                            }
                        } else {
                            default_val
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "size" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        let len = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::Bundle(map) => map.len() as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = len;
                        self.last_return_string = None;
                        Ok(len)
                    }
                    "clear" => {
                        if arg_values.is_empty() {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let receiver_id = arg_values[0];
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Bundle(map) = &mut obj.payload {
                                map.clear();
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/util/HashMap" | "java/util/Map" => {
                match method_name {
                    "<init>" => {
                        if !arg_values.is_empty() {
                            let receiver_id = arg_values[0];
                            if let Ok(obj) = self.get_object_mut(receiver_id) {
                                obj.payload = ObjectPayload::HashMap(BTreeMap::new());
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "put" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null HashMap receiver")));
                        }
                        let key = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_else(|| alloc::format!("id_{}", arg_values[1]))
                        } else {
                            String::new()
                        };
                        let val = arg_values.get(2).copied().unwrap_or(0);
                        let prev = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            match &mut obj.payload {
                                ObjectPayload::HashMap(map) => map.insert(key, val).unwrap_or(0),
                                _ => {
                                    let mut m = BTreeMap::new();
                                    m.insert(key, val);
                                    obj.payload = ObjectPayload::HashMap(m);
                                    0
                                }
                            }
                        } else {
                            0
                        };
                        self.last_return_value = prev;
                        self.last_return_string = None;
                        Ok(prev)
                    }
                    "get" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null HashMap receiver")));
                        }
                        let key = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_else(|| alloc::format!("id_{}", arg_values[1]))
                        } else {
                            String::new()
                        };
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::HashMap(map) => map.get(&key).copied().unwrap_or(0),
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = val;
                        self.last_return_string = self.get_string_payload(val);
                        Ok(val)
                    }
                    "remove" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null HashMap receiver")));
                        }
                        let key = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_else(|| alloc::format!("id_{}", arg_values[1]))
                        } else {
                            String::new()
                        };
                        let prev = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            match &mut obj.payload {
                                ObjectPayload::HashMap(map) => map.remove(&key).unwrap_or(0),
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = prev;
                        self.last_return_string = None;
                        Ok(prev)
                    }
                    "containsKey" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if receiver_id == 0 {
                            return Err(self.throw_npe(Some("Null HashMap receiver")));
                        }
                        let key = if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else {
                                None
                            };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_else(|| alloc::format!("id_{}", arg_values[1]))
                        } else {
                            String::new()
                        };
                        let contains = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::HashMap(map) => if map.contains_key(&key) { 1 } else { 0 },
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = contains;
                        self.last_return_string = None;
                        Ok(contains)
                    }
                    "size" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let len = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::HashMap(map) => map.len() as u32,
                                _ => 0,
                            }
                        } else {
                            0
                        };
                        self.last_return_value = len;
                        self.last_return_string = None;
                        Ok(len)
                    }
                    "isEmpty" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let is_empty = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::HashMap(map) => if map.is_empty() { 1 } else { 0 },
                                _ => 1,
                            }
                        } else {
                            1
                        };
                        self.last_return_value = is_empty;
                        self.last_return_string = None;
                        Ok(is_empty)
                    }
                    "clear" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::HashMap(map) = &mut obj.payload {
                                map.clear();
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/content/SharedPreferences" => {
                match method_name {
                    "edit" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let pref_name = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { name, .. } => name.clone(),
                                _ => String::from("default"),
                            }
                        } else {
                            String::from("default")
                        };
                        let ed_id = self.allocate_object("Landroid/content/SharedPreferences$Editor;")?;
                        if let Ok(obj) = self.get_object_mut(ed_id) {
                            obj.payload = ObjectPayload::SharedPreferencesEditor {
                                pref_name,
                                staged: BTreeMap::new(),
                                clear_staged: false,
                            };
                        }
                        self.last_return_value = ed_id;
                        self.last_return_string = None;
                        Ok(ed_id)
                    }
                    "getString" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let def_shadow = if arg_registers.len() > 2 && arg_registers[2] < self.string_registers.len() {
                            self.string_registers[arg_registers[2]].as_deref()
                        } else { None };
                        let def_val = self.resolve_string_arg(dex, arg_values.get(2).copied().unwrap_or(0), def_shadow);

                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { values, .. } => {
                                    match values.get(&key) {
                                        Some(BundleValue::String(s)) => Some(s.clone()),
                                        _ => def_val,
                                    }
                                }
                                _ => def_val,
                            }
                        } else {
                            def_val
                        };
                        if let Some(s) = val {
                            let s_id = self.allocate_string(&s)?;
                            self.last_return_value = s_id;
                            self.last_return_string = Some(s);
                            Ok(s_id)
                        } else {
                            self.last_return_value = 0;
                            self.last_return_string = None;
                            Ok(0)
                        }
                    }
                    "getInt" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let def_val = arg_values.get(2).copied().unwrap_or(0);
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { values, .. } => {
                                    match values.get(&key) {
                                        Some(BundleValue::Int(v)) => *v as u32,
                                        _ => def_val,
                                    }
                                }
                                _ => def_val,
                            }
                        } else {
                            def_val
                        };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "getLong" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let def_lo = arg_values.get(2).copied().unwrap_or(0) as u64;
                        let def_hi = arg_values.get(3).copied().unwrap_or(0) as u64;
                        let def_val = ((def_hi << 32) | def_lo) as i64;
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { values, .. } => {
                                    match values.get(&key) {
                                        Some(BundleValue::Long(v)) => *v,
                                        _ => def_val,
                                    }
                                }
                                _ => def_val,
                            }
                        } else {
                            def_val
                        };
                        self.last_return_value = (val as u64 & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((val as u64 >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "getBoolean" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let def_val = arg_values.get(2).copied().unwrap_or(0) != 0;
                        let val = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { values, .. } => {
                                    match values.get(&key) {
                                        Some(BundleValue::Boolean(b)) => *b,
                                        _ => def_val,
                                    }
                                }
                                _ => def_val,
                            }
                        } else {
                            def_val
                        };
                        let res = if val { 1 } else { 0 };
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "contains" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let has = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferences { values, .. } => if values.contains_key(&key) { 1 } else { 0 },
                                _ => 0,
                            }
                        } else { 0 };
                        self.last_return_value = has;
                        self.last_return_string = None;
                        Ok(has)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/content/SharedPreferences$Editor" => {
                match method_name {
                    "putString" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let val_shadow = if arg_registers.len() > 2 && arg_registers[2] < self.string_registers.len() {
                            self.string_registers[arg_registers[2]].as_deref()
                        } else { None };
                        let val = self.resolve_string_arg(dex, arg_values.get(2).copied().unwrap_or(0), val_shadow).unwrap_or_default();
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { staged, .. } = &mut obj.payload {
                                staged.insert(key, Some(BundleValue::String(val)));
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "putInt" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let val = arg_values.get(2).copied().unwrap_or(0) as i32;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { staged, .. } = &mut obj.payload {
                                staged.insert(key, Some(BundleValue::Int(val)));
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "putLong" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let lo = arg_values.get(2).copied().unwrap_or(0) as u64;
                        let hi = arg_values.get(3).copied().unwrap_or(0) as u64;
                        let val = ((hi << 32) | lo) as i64;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { staged, .. } = &mut obj.payload {
                                staged.insert(key, Some(BundleValue::Long(val)));
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "putBoolean" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        let val = arg_values.get(2).copied().unwrap_or(0) != 0;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { staged, .. } = &mut obj.payload {
                                staged.insert(key, Some(BundleValue::Boolean(val)));
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "remove" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let key_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let key = self.resolve_string_arg(dex, arg_values.get(1).copied().unwrap_or(0), key_shadow).unwrap_or_default();
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { staged, .. } = &mut obj.payload {
                                staged.insert(key, None);
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "clear" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::SharedPreferencesEditor { clear_staged, staged, .. } = &mut obj.payload {
                                *clear_staged = true;
                                staged.clear();
                            }
                        }
                        self.last_return_value = receiver_id;
                        self.last_return_string = None;
                        Ok(receiver_id)
                    }
                    "apply" | "commit" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let (target_pref, staged_muts, do_clear) = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::SharedPreferencesEditor { pref_name, staged, clear_staged } => {
                                    (pref_name.clone(), staged.clone(), *clear_staged)
                                }
                                _ => (String::new(), BTreeMap::new(), false),
                            }
                        } else {
                            (String::new(), BTreeMap::new(), false)
                        };

                        if let Some(sp_obj) = self.objects.iter_mut().find(|o| {
                            if let ObjectPayload::SharedPreferences { name, .. } = &o.payload {
                                name == &target_pref
                            } else {
                                false
                            }
                        }) {
                            if let ObjectPayload::SharedPreferences { values, .. } = &mut sp_obj.payload {
                                if do_clear {
                                    values.clear();
                                }
                                for (k, v_opt) in staged_muts {
                                    match v_opt {
                                        Some(v) => { values.insert(k, v); }
                                        None => { values.remove(&k); }
                                    }
                                }
                            }
                        }

                        let ret = if method_name == "commit" { 1 } else { 0 };
                        self.last_return_value = ret;
                        self.last_return_string = None;
                        Ok(ret)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/io/File" => {
                match method_name {
                    "<init>" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if arg_values.len() > 2 {
                            let parent = self.get_string_payload(arg_values[1]).unwrap_or_default();
                            let shadow = if arg_registers.len() > 2 && arg_registers[2] < self.string_registers.len() {
                                self.string_registers[arg_registers[2]].as_deref()
                            } else { None };
                            let child = self.resolve_string_arg(dex, arg_values[2], shadow).unwrap_or_default();
                            if parent.ends_with('/') {
                                alloc::format!("{}{}", parent, child)
                            } else {
                                alloc::format!("{}/{}", parent, child)
                            }
                        } else if arg_values.len() > 1 {
                            let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                                self.string_registers[arg_registers[1]].as_deref()
                            } else { None };
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_default()
                        } else {
                            String::new()
                        };
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            obj.payload = ObjectPayload::File { path };
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "getPath" | "getAbsolutePath" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let id = self.allocate_string(&path)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(path);
                        Ok(id)
                    }
                    "getName" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let name = path.rsplit('/').next().unwrap_or(&path);
                        let id = self.allocate_string(name)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(String::from(name));
                        Ok(id)
                    }
                    "exists" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let exists = if super::vfs::ANDROID_SANDBOX_FS.lock().exists_as(&caller, &path) { 1 } else { 0 };
                        self.last_return_value = exists;
                        self.last_return_string = None;
                        Ok(exists)
                    }
                    "isDirectory" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let is_dir = if super::vfs::ANDROID_SANDBOX_FS.lock().is_directory_as(&caller, &path) { 1 } else { 0 };
                        self.last_return_value = is_dir;
                        self.last_return_string = None;
                        Ok(is_dir)
                    }
                    "isFile" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let mut fs = super::vfs::ANDROID_SANDBOX_FS.lock();
                        let is_file = if fs.exists_as(&caller, &path) && !fs.is_directory_as(&caller, &path) { 1 } else { 0 };
                        self.last_return_value = is_file;
                        self.last_return_string = None;
                        Ok(is_file)
                    }
                    "length" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let len = super::vfs::ANDROID_SANDBOX_FS.lock().open_read_as(&caller, &path).map(|d| d.len() as u64).unwrap_or(0);
                        self.last_return_value = (len & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((len >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "delete" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let ok = match super::vfs::ANDROID_SANDBOX_FS.lock().delete_file_as(&caller, &path) {
                            Ok(deleted) => if deleted { 1 } else { 0 },
                            Err(_) => 0,
                        };
                        self.last_return_value = ok;
                        self.last_return_string = None;
                        Ok(ok)
                    }
                    "list" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let path = if let Ok(obj) = self.get_object(receiver_id) {
                            match &obj.payload {
                                ObjectPayload::File { path } => path.clone(),
                                _ => String::new(),
                            }
                        } else { String::new() };
                        let caller = self.caller_context.clone().unwrap_or_else(super::vfs::AndroidCaller::system);
                        let entries = super::vfs::ANDROID_SANDBOX_FS.lock().list_directory_as(&caller, &path).unwrap_or_default();
                        let arr_id = self.allocate_array("[Ljava/lang/String;", entries.len())?;
                        for (i, entry) in entries.into_iter().enumerate() {
                            let s_id = self.allocate_string(&entry)?;
                            if let Ok(arr_obj) = self.get_object_mut(arr_id) {
                                if let Some(elems) = arr_obj.array_elements.as_mut() {
                                    if i < elems.len() { elems[i] = s_id; }
                                }
                            }
                        }
                        self.last_return_value = arr_id;
                        self.last_return_string = None;
                        Ok(arr_id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/os/Parcel" => {
                match method_name {
                    "obtain" => {
                        let p_id = self.allocate_object("Landroid/os/Parcel;")?;
                        if let Ok(obj) = self.get_object_mut(p_id) {
                            obj.payload = ObjectPayload::Parcel(Parcel::obtain());
                        }
                        self.last_return_value = p_id;
                        self.last_return_string = None;
                        Ok(p_id)
                    }
                    "recycle" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.recycle();
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "writeInt" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let val = arg_values.get(1).copied().unwrap_or(0) as i32;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.write_int(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "readInt" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let val = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.read_int().unwrap_or(0) as u32
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "writeLong" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let lo = arg_values.get(1).copied().unwrap_or(0) as u64;
                        let hi = arg_values.get(2).copied().unwrap_or(0) as u64;
                        let val = ((hi << 32) | lo) as i64;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.write_long(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "readLong" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let val = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.read_long().unwrap_or(0)
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = (val as u64 & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((val as u64 >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "writeFloat" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let bits = arg_values.get(1).copied().unwrap_or(0);
                        let val = f32::from_bits(bits);
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.write_float(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "readFloat" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let val = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.read_float().unwrap_or(0.0).to_bits()
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = val;
                        self.last_return_string = None;
                        Ok(val)
                    }
                    "writeDouble" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let lo = arg_values.get(1).copied().unwrap_or(0) as u64;
                        let hi = arg_values.get(2).copied().unwrap_or(0) as u64;
                        let val = f64::from_bits((hi << 32) | lo);
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.write_double(val);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "readDouble" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let val = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.read_double().unwrap_or(0.0).to_bits()
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = (val & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((val >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "writeString" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else { None };
                        let s = if arg_values.len() > 1 {
                            self.resolve_string_arg(dex, arg_values[1], shadow).unwrap_or_default()
                        } else { String::new() };
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.write_string(&s);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    "readString" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let opt_s = if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.read_string()
                            } else { None }
                        } else { None };
                        if let Some(s) = opt_s {
                            let s_id = self.allocate_string(&s)?;
                            self.last_return_value = s_id;
                            self.last_return_string = Some(s);
                            Ok(s_id)
                        } else {
                            self.last_return_value = 0;
                            self.last_return_string = None;
                            Ok(0)
                        }
                    }
                    "dataSize" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let sz = if let Ok(obj) = self.get_object(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &obj.payload {
                                p.data_size() as u32
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = sz;
                        self.last_return_string = None;
                        Ok(sz)
                    }
                    "dataPosition" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let pos = if let Ok(obj) = self.get_object(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &obj.payload {
                                p.data_position() as u32
                            } else { 0 }
                        } else { 0 };
                        self.last_return_value = pos;
                        self.last_return_string = None;
                        Ok(pos)
                    }
                    "setDataPosition" => {
                        let receiver_id = *arg_values.first().ok_or(DalvikVmError::InvalidBytecode)?;
                        let pos = arg_values.get(1).copied().unwrap_or(0) as usize;
                        if let Ok(obj) = self.get_object_mut(receiver_id) {
                            if let ObjectPayload::Parcel(p) = &mut obj.payload {
                                p.set_data_position(pos);
                            }
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/Math" => {
                match method_name {
                    "min" => {
                        let a = arg_values.get(0).copied().unwrap_or(0) as i32;
                        let b = arg_values.get(1).copied().unwrap_or(0) as i32;
                        let res = a.min(b) as u32;
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "max" => {
                        let a = arg_values.get(0).copied().unwrap_or(0) as i32;
                        let b = arg_values.get(1).copied().unwrap_or(0) as i32;
                        let res = a.max(b) as u32;
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "abs" => {
                        let a = arg_values.get(0).copied().unwrap_or(0) as i32;
                        let res = a.abs() as u32;
                        self.last_return_value = res;
                        self.last_return_string = None;
                        Ok(res)
                    }
                    "sqrt" => {
                        let lo = arg_values.get(0).copied().unwrap_or(0) as u64;
                        let hi = arg_values.get(1).copied().unwrap_or(0) as u64;
                        let val = f64::from_bits((hi << 32) | lo);
                        let root = soft_sqrt(val);
                        let bits = root.to_bits();
                        self.last_return_value = (bits & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((bits >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "java/lang/System" => {
                match method_name {
                    "currentTimeMillis" => {
                        let ts = 1774300000000i64.wrapping_add(self.instructions_executed as i64);
                        self.last_return_value = (ts as u64 & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((ts as u64 >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "nanoTime" => {
                        let nt = 1774300000000000000i64.wrapping_add(self.instructions_executed as i64 * 100);
                        self.last_return_value = (nt as u64 & 0xFFFFFFFF) as u32;
                        self.last_return_wide_hi = ((nt as u64 >> 32) & 0xFFFFFFFF) as u32;
                        self.last_return_string = None;
                        Ok(self.last_return_value)
                    }
                    "arraycopy" => {
                        if arg_values.len() < 5 {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let src_id = arg_values[0];
                        let src_pos = arg_values[1] as usize;
                        let dest_id = arg_values[2];
                        let dest_pos = arg_values[3] as usize;
                        let len = arg_values[4] as usize;
                        if src_id == 0 || dest_id == 0 {
                            return Err(self.throw_npe(Some("Null array in arraycopy")));
                        }
                        let src_slice: Vec<u32> = {
                            let src_obj = self.get_object(src_id)?;
                            let elems = src_obj.array_elements.as_ref().ok_or(DalvikVmError::NullPointerDereference)?;
                            if src_pos.checked_add(len).map_or(true, |end| end > elems.len()) {
                                return Err(self.throw_oob(Some("Source arraycopy index out of bounds")));
                            }
                            elems[src_pos..src_pos + len].to_vec()
                        };
                        {
                            let dest_obj = self.get_object_mut(dest_id)?;
                            let elems = dest_obj.array_elements.as_mut().ok_or(DalvikVmError::NullPointerDereference)?;
                            if dest_pos.checked_add(len).map_or(true, |end| end > elems.len()) {
                                return Err(self.throw_oob(Some("Destination arraycopy index out of bounds")));
                            }
                            elems[dest_pos..dest_pos + len].copy_from_slice(&src_slice);
                        }
                        self.last_return_value = 0;
                        self.last_return_string = None;
                        Ok(0)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            "android/content/Context" | "android/content/ContextWrapper" => {
                match method_name {
                    "getSharedPreferences" => {
                        let name_shadow = if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                            self.string_registers[arg_registers[1]].as_deref()
                        } else {
                            None
                        };
                        let name = if arg_values.len() > 1 {
                            self.resolve_string_arg(dex, arg_values[1], name_shadow).unwrap_or_else(|| String::from("default"))
                        } else {
                            String::from("default")
                        };
                        let existing = self.objects.iter().find(|o| {
                            if let ObjectPayload::SharedPreferences { name: n, .. } = &o.payload {
                                n == &name
                            } else {
                                false
                            }
                        }).map(|o| o.id);

                        let sp_id = match existing {
                            Some(id) => id,
                            None => {
                                let id = self.allocate_object("Landroid/content/SharedPreferences;")?;
                                if let Ok(obj) = self.get_object_mut(id) {
                                    obj.payload = ObjectPayload::SharedPreferences {
                                        name,
                                        values: BTreeMap::new(),
                                    };
                                }
                                id
                            }
                        };
                        self.last_return_value = sp_id;
                        self.last_return_string = None;
                        Ok(sp_id)
                    }
                    "getFilesDir" => {
                        let f_id = self.allocate_object("Ljava/io/File;")?;
                        if let Ok(obj) = self.get_object_mut(f_id) {
                            obj.payload = ObjectPayload::File { path: String::from("/data/data/files") };
                        }
                        self.last_return_value = f_id;
                        self.last_return_string = None;
                        Ok(f_id)
                    }
                    "getCacheDir" => {
                        let f_id = self.allocate_object("Ljava/io/File;")?;
                        if let Ok(obj) = self.get_object_mut(f_id) {
                            obj.payload = ObjectPayload::File { path: String::from("/data/data/cache") };
                        }
                        self.last_return_value = f_id;
                        self.last_return_string = None;
                        Ok(f_id)
                    }
                    "getPackageName" => {
                        let pkg = String::from("org.interos.acr");
                        let id = self.allocate_string(&pkg)?;
                        self.last_return_value = id;
                        self.last_return_string = Some(pkg);
                        Ok(id)
                    }
                    _ => Err(DalvikVmError::UnsupportedFrameworkApi),
                }
            }
            _ => {
                let string_args = [
                    if !arg_registers.is_empty() && arg_registers[0] < self.string_registers.len() {
                        self.string_registers[arg_registers[0]].as_deref()
                    } else {
                        None
                    },
                    if arg_registers.len() > 1 && arg_registers[1] < self.string_registers.len() {
                        self.string_registers[arg_registers[1]].as_deref()
                    } else {
                        None
                    },
                ];
                match super::framework::dispatch(class_name, method_name, &string_args, &mut self.captured_logs) {
                    Ok(super::framework::AndroidFrameworkResult::Return(value)) => {
                        self.last_return_value = value;
                        self.last_return_string = None;
                        Ok(value)
                    }
                    Ok(super::framework::AndroidFrameworkResult::ReturnString(s)) => {
                        self.last_return_value = 0;
                        self.last_return_string = Some(alloc::string::String::from(s));
                        Ok(0)
                    }
                    Ok(super::framework::AndroidFrameworkResult::ReturnOwnedString(s)) => {
                        self.last_return_value = 0;
                        self.last_return_string = Some(s);
                        Ok(0)
                    }
                    Ok(super::framework::AndroidFrameworkResult::Service(svc)) => {
                        self.last_return_value = svc.handle();
                        self.last_return_string = None;
                        Ok(svc.handle())
                    }
                    Err(super::framework::AndroidFrameworkError::UnsupportedApi) => Err(DalvikVmError::UnsupportedFrameworkApi),
                    Err(_) => Err(DalvikVmError::SecurityViolationForbiddenApi),
                }
            }
        }
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
        if self.call_depth == 0 {
            let initial_size = (dex.methods.iter().find(|m| m.class_name == class_name && m.method_name == method_name).map(|m| m.registers_size as usize).unwrap_or(32)).max(32).min(MAX_REGISTERS_LIMIT);
            self.registers = alloc::vec![0u32; initial_size];
            self.string_registers = alloc::vec![None; initial_size];
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

        // Dynamically resize register frames to method-defined registers_size (up to MAX_REGISTERS_LIMIT)
        let frame_size = (method.registers_size as usize).max(32);
        if frame_size > MAX_REGISTERS_LIMIT {
            return Err(DalvikVmError::RegisterOutOfBounds);
        }
        if self.registers.len() < frame_size {
            self.registers.resize(frame_size, 0);
        }
        if self.string_registers.len() < frame_size {
            self.string_registers.resize(frame_size, None);
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
        self.interpret_insns_with_code_offset(dex, method.registers_size, method.code_offset, insns)
    }

    /// Resolves virtual method implementation by traversing runtime receiver class inheritance hierarchy.
    pub fn resolve_virtual_method<'a>(
        &self,
        dex: &'a DexPackage<'a>,
        receiver_class: &str,
        method_name: &str,
        proto_shorty: &str,
    ) -> Option<&'a DexMethod> {
        let mut curr = receiver_class;
        let mut depth = 0;
        while depth < 32 {
            if let Some(m) = dex.methods.iter().find(|m| {
                m.class_name == curr
                    && m.method_name == method_name
                    && m.proto_shorty == proto_shorty
                    && !m.is_direct
            }) {
                return Some(m);
            }

            if let Some(super_desc) = find_dex_superclass(dex, curr) {
                curr = super_desc;
            } else if let Some(framework_super) = get_framework_superclass(curr) {
                curr = framework_super;
            } else {
                break;
            }
            depth += 1;
        }
        None
    }

    /// Checks if a source class is compatible with/assignable to target class descriptor.
    pub fn is_type_compatible(&self, dex: &DexPackage, source_desc: &str, target_desc: &str) -> bool {
        is_type_compatible(dex, source_desc, target_desc)
    }

    /// Interprets a sequence of Dalvik bytecode instructions directly.
    pub fn interpret_insns(
        &mut self,
        dex: &DexPackage,
        registers_size: u16,
        insns: &[u8],
    ) -> Result<u32, DalvikVmError> {
        self.interpret_insns_with_code_offset(dex, registers_size, 0, insns)
    }

    /// Interprets a sequence of Dalvik bytecode instructions with method code_offset for try-catch table resolution.
    pub fn interpret_insns_with_code_offset(
        &mut self,
        dex: &DexPackage,
        registers_size: u16,
        code_offset: usize,
        insns: &[u8],
    ) -> Result<u32, DalvikVmError> {
        struct MethodFrame {
            registers_size: u16,
        }
        let method = MethodFrame { registers_size };
        let mut pc: usize = 0;
        let mut steps = 0;
        const STEP_LIMIT: usize = 4096;
        let reg_cap = self.registers.len();

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
                    if v_dest >= reg_cap || v_src >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = self.pending_exception.take().unwrap_or(0);
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_CONST_4 => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let val = ((insns[pc + 1] as i8) >> 4) as u32;
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = val;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_CONST => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = u32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]);
                    if v_dest >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = val;
                    self.string_registers[v_dest] = None;
                    pc += 6;
                }
                OP_CONST_HIGH16 => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = (u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as u32) << 16;
                    if v_dest >= reg_cap {
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
                    let val = i16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as i64;
                    if v_dest >= reg_cap || v_dest + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.set_wide_reg(v_dest, val as u64)?;
                    pc += 4;
                }
                OP_CONST_WIDE_32 => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as i64;
                    if v_dest >= reg_cap || v_dest + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.set_wide_reg(v_dest, val as u64)?;
                    pc += 6;
                }
                OP_CONST_WIDE_FULL => {
                    if pc + 10 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = u64::from_le_bytes([
                        insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5],
                        insns[pc + 6], insns[pc + 7], insns[pc + 8], insns[pc + 9],
                    ]);
                    if v_dest >= reg_cap || v_dest + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.set_wide_reg(v_dest, val)?;
                    pc += 10;
                }
                OP_CONST_WIDE_HIGH16 => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let val = (u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as u64) << 48;
                    if v_dest >= reg_cap || v_dest + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.set_wide_reg(v_dest, val)?;
                    pc += 4;
                }
                OP_CONST_STRING => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let str_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                OP_GOTO_32 => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let offset = (i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as isize) * 2;
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                OP_INVOKE_VIRTUAL | OP_INVOKE_DIRECT | OP_INVOKE_STATIC | OP_INVOKE_SUPER | OP_INVOKE_INTERFACE => {
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
                    let is_framework = target_class.starts_with("Landroid/")
                        || target_class.starts_with("Landroidx/")
                        || target_class.starts_with("Lcom/google/android/gms/")
                        || target_class.starts_with("Ljava/")
                        || target_class.starts_with("Ljavax/");
                    let arg_vals: Vec<u32> = arg_registers[..arg_count].iter().map(|&r| self.registers[r]).collect();

                    if is_framework {
                        match self.dispatch_framework_method(dex, &target_class, &target_method, &arg_registers[..arg_count], &arg_vals) {
                            Ok(value) => {
                                self.last_return_value = value;
                            }
                            Err(DalvikVmError::UnhandledException(ex_id)) => {
                                if code_offset > 0 {
                                    let ex_class = self.get_object(ex_id).map(|o| o.class_descriptor.clone()).ok();
                                    if let Some(handler_pc) = find_catch_handler(dex, code_offset, pc, ex_class.as_deref()) {
                                        self.pending_exception = Some(ex_id);
                                        pc = handler_pc;
                                        continue;
                                    }
                                }
                                return Err(DalvikVmError::UnhandledException(ex_id));
                            }
                            Err(e) => return Err(e),
                        }
                    } else {
                        // Virtual dispatch resolution for application classes
                        let mut resolved_class = target.class_name.clone();
                        let mut resolved_method = target.method_name.clone();
                        let mut target_m = target;

                        if (opcode == OP_INVOKE_VIRTUAL || opcode == OP_INVOKE_INTERFACE) && arg_count > 0 {
                            let receiver_id = arg_vals[0];
                            if receiver_id == 0 {
                                return Err(self.throw_npe(Some("Null receiver for virtual invoke")));
                            }
                            if let Ok(receiver_obj) = self.get_object(receiver_id) {
                                let receiver_class = &receiver_obj.class_descriptor;
                                if receiver_class != &target.class_name {
                                    if let Some(override_m) = self.resolve_virtual_method(dex, receiver_class, &target.method_name, &target.proto_shorty) {
                                        resolved_class = override_m.class_name.clone();
                                        resolved_method = override_m.method_name.clone();
                                        target_m = override_m;
                                    }
                                }
                            }
                        }

                        let target_registers_size = target_m.registers_size as usize;
                        let target_ins_size = target_m.ins_size as usize;
                        if arg_count != target_ins_size || target_ins_size > target_registers_size || target_registers_size > MAX_REGISTERS_LIMIT {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let caller_registers = self.registers.clone();
                        let caller_strings = self.string_registers.clone();
                        let base = target_registers_size - target_ins_size;
                        let frame_len = target_registers_size.max(32);
                        self.registers = alloc::vec![0u32; frame_len];
                        self.string_registers = alloc::vec![None; frame_len];
                        for i in 0..arg_count {
                            self.registers[base + i] = caller_registers[arg_registers[i]];
                            if arg_registers[i] < caller_strings.len() {
                                self.string_registers[base + i] = caller_strings[arg_registers[i]].clone();
                            }
                        }
                        let result = self.invoke_method(dex, &resolved_class, &resolved_method);
                        self.registers = caller_registers;
                        self.string_registers = caller_strings;
                        match result {
                            Ok(val) => {
                                self.last_return_value = val;
                                self.last_return_string = None;
                            }
                            Err(DalvikVmError::UnhandledException(ex_id)) => {
                                if code_offset > 0 {
                                    let ex_class = self.get_object(ex_id).map(|o| o.class_descriptor.clone()).ok();
                                    if let Some(handler_pc) = find_catch_handler(dex, code_offset, pc, ex_class.as_deref()) {
                                        self.pending_exception = Some(ex_id);
                                        pc = handler_pc;
                                        continue;
                                    }
                                }
                                return Err(DalvikVmError::UnhandledException(ex_id));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    pc += 6;
                }
                OP_INVOKE_VIRTUAL_RANGE
                | OP_INVOKE_SUPER_RANGE
                | OP_INVOKE_DIRECT_RANGE
                | OP_INVOKE_STATIC_RANGE
                | OP_INVOKE_INTERFACE_RANGE => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let arg_count = insns[pc + 1] as usize;
                    let method_index = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let first_reg = u16::from_le_bytes([insns[pc + 4], insns[pc + 5]]) as usize;
                    if first_reg.checked_add(arg_count).map_or(true, |end| end > method.registers_size as usize) {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let target = dex.methods.iter().find(|m| m.method_index == method_index)
                        .ok_or(DalvikVmError::MethodNotFound)?;
                    let target_class = target.class_name.clone();
                    let target_method = target.method_name.clone();
                    let is_framework = target_class.starts_with("Landroid/")
                        || target_class.starts_with("Landroidx/")
                        || target_class.starts_with("Lcom/google/android/gms/")
                        || target_class.starts_with("Ljava/")
                        || target_class.starts_with("Ljavax/");
                    let arg_regs: Vec<usize> = (first_reg..first_reg + arg_count).collect();
                    let arg_vals: Vec<u32> = arg_regs.iter().map(|&r| self.registers[r]).collect();

                    if is_framework {
                        match self.dispatch_framework_method(dex, &target_class, &target_method, &arg_regs, &arg_vals) {
                            Ok(value) => {
                                self.last_return_value = value;
                            }
                            Err(DalvikVmError::UnhandledException(ex_id)) => {
                                if code_offset > 0 {
                                    let ex_class = self.get_object(ex_id).map(|o| o.class_descriptor.clone()).ok();
                                    if let Some(handler_pc) = find_catch_handler(dex, code_offset, pc, ex_class.as_deref()) {
                                        self.pending_exception = Some(ex_id);
                                        pc = handler_pc;
                                        continue;
                                    }
                                }
                                return Err(DalvikVmError::UnhandledException(ex_id));
                            }
                            Err(e) => return Err(e),
                        }
                    } else {
                        // Virtual dispatch resolution for application classes
                        let mut resolved_class = target.class_name.clone();
                        let mut resolved_method = target.method_name.clone();
                        let mut target_m = target;

                        if (opcode == OP_INVOKE_VIRTUAL_RANGE || opcode == OP_INVOKE_INTERFACE_RANGE) && arg_count > 0 {
                            let receiver_id = arg_vals[0];
                            if receiver_id == 0 {
                                return Err(self.throw_npe(Some("Null receiver for virtual invoke range")));
                            }
                            if let Ok(receiver_obj) = self.get_object(receiver_id) {
                                let receiver_class = &receiver_obj.class_descriptor;
                                if receiver_class != &target.class_name {
                                    if let Some(override_m) = self.resolve_virtual_method(dex, receiver_class, &target.method_name, &target.proto_shorty) {
                                        resolved_class = override_m.class_name.clone();
                                        resolved_method = override_m.method_name.clone();
                                        target_m = override_m;
                                    }
                                }
                            }
                        }

                        let target_registers_size = target_m.registers_size as usize;
                        let target_ins_size = target_m.ins_size as usize;
                        if arg_count != target_ins_size || target_ins_size > target_registers_size || target_registers_size > MAX_REGISTERS_LIMIT {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        let caller_registers = self.registers.clone();
                        let caller_strings = self.string_registers.clone();
                        let base = target_registers_size - target_ins_size;
                        let frame_len = target_registers_size.max(32);
                        self.registers = alloc::vec![0u32; frame_len];
                        self.string_registers = alloc::vec![None; frame_len];
                        for i in 0..arg_count {
                            self.registers[base + i] = caller_registers[first_reg + i];
                            if (first_reg + i) < caller_strings.len() {
                                self.string_registers[base + i] = caller_strings[first_reg + i].clone();
                            }
                        }
                        let result = self.invoke_method(dex, &resolved_class, &resolved_method);
                        self.registers = caller_registers;
                        self.string_registers = caller_strings;
                        match result {
                            Ok(val) => {
                                self.last_return_value = val;
                                self.last_return_string = None;
                            }
                            Err(DalvikVmError::UnhandledException(ex_id)) => {
                                if code_offset > 0 {
                                    let ex_class = self.get_object(ex_id).map(|o| o.class_descriptor.clone()).ok();
                                    if let Some(handler_pc) = find_catch_handler(dex, code_offset, pc, ex_class.as_deref()) {
                                        self.pending_exception = Some(ex_id);
                                        pc = handler_pc;
                                        continue;
                                    }
                                }
                                return Err(DalvikVmError::UnhandledException(ex_id));
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    pc += 6;
                }
                OP_ADD_INT_2ADDR => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_dest >= reg_cap || v_b >= reg_cap || v_c >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_dest >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
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
                    if v_a >= reg_cap || v_b >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let elem_type = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    let length = self.registers[v_b] as usize;
                    let obj_id = self.allocate_array(elem_type, length)?;
                    self.registers[v_a] = obj_id;
                    self.string_registers[v_a] = None;
                    pc += 4;
                }
                OP_FILLED_NEW_ARRAY => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let (arg_regs, count) = decode_invoke_registers(insns, pc)?;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let elem_type = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    let mut vals = [0u32; 5];
                    for i in 0..count {
                        if arg_regs[i] >= reg_cap {
                            return Err(DalvikVmError::RegisterOutOfBounds);
                        }
                        vals[i] = self.registers[arg_regs[i]];
                    }
                    let obj_id = self.allocate_array(elem_type, count)?;
                    if let Ok(obj) = self.get_object_mut(obj_id) {
                        if let Some(elements) = obj.array_elements.as_mut() {
                            for i in 0..count {
                                elements[i] = vals[i];
                            }
                        }
                    }
                    self.last_return_value = obj_id;
                    self.last_return_string = None;
                    pc += 6;
                }
                OP_FILLED_NEW_ARRAY_RANGE => {
                    if pc + 6 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let count = insns[pc + 1] as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let start_reg = u16::from_le_bytes([insns[pc + 4], insns[pc + 5]]) as usize;
                    let elem_type = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                    let mut vals = Vec::with_capacity(count);
                    for i in 0..count {
                        let reg = start_reg + i;
                        if reg >= reg_cap {
                            return Err(DalvikVmError::RegisterOutOfBounds);
                        }
                        vals.push(self.registers[reg]);
                    }
                    let obj_id = self.allocate_array(elem_type, count)?;
                    if let Ok(obj) = self.get_object_mut(obj_id) {
                        if let Some(elements) = obj.array_elements.as_mut() {
                            for i in 0..count {
                                elements[i] = vals[i];
                            }
                        }
                    }
                    self.last_return_value = obj_id;
                    self.last_return_string = None;
                    pc += 6;
                }
                OP_CMP_LONG => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest >= reg_cap || v_b + 1 >= reg_cap || v_c + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let b = self.get_wide_reg(v_b) as i64;
                    let c = self.get_wide_reg(v_c) as i64;
                    let res: i32 = if b < c { -1 } else if b > c { 1 } else { 0 };
                    self.registers[v_dest] = res as u32;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_CMPL_DOUBLE | OP_CMPG_DOUBLE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest >= reg_cap || v_b + 1 >= reg_cap || v_c + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let db = f64::from_bits(self.get_wide_reg(v_b));
                    let dc = f64::from_bits(self.get_wide_reg(v_c));
                    let res: i32 = if db.is_nan() || dc.is_nan() {
                        if opcode == OP_CMPL_DOUBLE { -1 } else { 1 }
                    } else if db < dc {
                        -1
                    } else if db > dc {
                        1
                    } else {
                        0
                    };
                    self.registers[v_dest] = res as u32;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_AGET | OP_AGET_OBJECT | OP_AGET_BOOLEAN | OP_AGET_BYTE | OP_AGET_CHAR | OP_AGET_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_dest >= reg_cap || v_arr >= reg_cap || v_idx >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    let idx = self.registers[v_idx] as usize;
                    let obj = self.get_object(arr_id)?;
                    let elements = obj.array_elements.as_ref().ok_or(DalvikVmError::NullPointerDereference)?;
                    if idx >= elements.len() {
                        return Err(DalvikVmError::ArrayIndexOutOfBounds);
                    }
                    let raw = elements[idx];
                    self.registers[v_dest] = match opcode {
                        OP_AGET_BOOLEAN => raw & 0x01,
                        OP_AGET_BYTE => (raw as i8 as i32) as u32,
                        OP_AGET_CHAR => (raw as u16) as u32,
                        OP_AGET_SHORT => (raw as i16 as i32) as u32,
                        _ => raw,
                    };
                    self.string_registers[v_dest] = if opcode == OP_AGET_OBJECT {
                        self.get_string_payload(raw)
                    } else {
                        None
                    };
                    pc += 4;
                }
                OP_AGET_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_dest + 1 >= reg_cap || v_arr >= reg_cap || v_idx >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    let idx = self.registers[v_idx] as usize;
                    let (val_lo, val_hi) = {
                        let obj = self.get_object(arr_id)?;
                        let elements = obj.array_elements.as_ref().ok_or(DalvikVmError::NullPointerDereference)?;
                        let slot = idx * 2;
                        if slot + 1 >= elements.len() {
                            return Err(DalvikVmError::ArrayIndexOutOfBounds);
                        }
                        (elements[slot], elements[slot + 1])
                    };
                    self.registers[v_dest] = val_lo;
                    self.registers[v_dest + 1] = val_hi;
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 4;
                }
                OP_APUT | OP_APUT_OBJECT | OP_APUT_BOOLEAN | OP_APUT_BYTE | OP_APUT_CHAR | OP_APUT_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_val = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_val >= reg_cap || v_arr >= reg_cap || v_idx >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    if arr_id == 0 {
                        return Err(self.throw_npe(Some("Null array reference in aput")));
                    }
                    let idx = self.registers[v_idx] as usize;
                    let val = self.registers[v_val];
                    if opcode == OP_APUT_OBJECT && val != 0 {
                        let val_obj = self.get_object(val)?;
                        let val_desc = val_obj.class_descriptor.clone();
                        let arr_obj = self.get_object(arr_id)?;
                        if let Some(comp_type) = arr_obj.class_descriptor.strip_prefix('[') {
                            if !self.is_type_compatible(dex, &val_desc, comp_type) {
                                return Err(self.throw_array_store(Some("Incompatible array element type")));
                            }
                        }
                    }
                    let obj = self.get_object_mut(arr_id)?;
                    let elements = obj.array_elements.as_mut().ok_or(DalvikVmError::NullPointerDereference)?;
                    if idx >= elements.len() {
                        return Err(DalvikVmError::ArrayIndexOutOfBounds);
                    }
                    elements[idx] = match opcode {
                        OP_APUT_BOOLEAN => val & 0x01,
                        OP_APUT_BYTE => (val as u8) as u32,
                        OP_APUT_CHAR => (val as u16) as u32,
                        OP_APUT_SHORT => (val as u16) as u32,
                        _ => val,
                    };
                    pc += 4;
                }
                OP_APUT_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_val = insns[pc + 1] as usize;
                    let v_arr = insns[pc + 2] as usize;
                    let v_idx = insns[pc + 3] as usize;
                    if v_val + 1 >= reg_cap || v_arr >= reg_cap || v_idx >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let arr_id = self.registers[v_arr];
                    let idx = self.registers[v_idx] as usize;
                    let val_lo = self.registers[v_val];
                    let val_hi = self.registers[v_val + 1];
                    let obj = self.get_object_mut(arr_id)?;
                    let elements = obj.array_elements.as_mut().ok_or(DalvikVmError::NullPointerDereference)?;
                    let slot = idx * 2;
                    if slot + 1 >= elements.len() {
                        return Err(DalvikVmError::ArrayIndexOutOfBounds);
                    }
                    elements[slot] = val_lo;
                    elements[slot + 1] = val_hi;
                    pc += 4;
                }
                OP_IGET | OP_IGET_OBJECT | OP_IGET_BOOLEAN | OP_IGET_BYTE | OP_IGET_CHAR | OP_IGET_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a >= reg_cap || v_b >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object(obj_id)?;
                    if obj.array_elements.is_some() || obj.class_descriptor.starts_with('[') {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let raw = obj.get_field(field_idx);
                    self.registers[v_a] = match opcode {
                        OP_IGET_BOOLEAN => raw & 0x01,
                        OP_IGET_BYTE => (raw as i8 as i32) as u32,
                        OP_IGET_CHAR => (raw as u16) as u32,
                        OP_IGET_SHORT => (raw as i16 as i32) as u32,
                        _ => raw,
                    };
                    self.string_registers[v_a] = if opcode == OP_IGET_OBJECT {
                        self.get_string_payload(raw)
                    } else {
                        None
                    };
                    pc += 4;
                }
                OP_IGET_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a + 1 >= reg_cap || v_b >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let obj_id = self.registers[v_b];
                    let (val_lo, val_hi) = {
                        let obj = self.get_object(obj_id)?;
                        if obj.array_elements.is_some() || obj.class_descriptor.starts_with('[') {
                            return Err(DalvikVmError::InvalidBytecode);
                        }
                        (obj.get_field(field_idx), obj.get_field(field_idx.wrapping_add(1)))
                    };
                    self.registers[v_a] = val_lo;
                    self.registers[v_a + 1] = val_hi;
                    self.string_registers[v_a] = None;
                    self.string_registers[v_a + 1] = None;
                    pc += 4;
                }
                OP_IPUT | OP_IPUT_OBJECT | OP_IPUT_BOOLEAN | OP_IPUT_BYTE | OP_IPUT_CHAR | OP_IPUT_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a >= reg_cap || v_b >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let val = self.registers[v_a];
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object_mut(obj_id)?;
                    if obj.array_elements.is_some() || obj.class_descriptor.starts_with('[') {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let store_val = match opcode {
                        OP_IPUT_BOOLEAN => val & 0x01,
                        _ => val,
                    };
                    obj.set_field(field_idx, store_val);
                    pc += 4;
                }
                OP_IPUT_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_a + 1 >= reg_cap || v_b >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let val_lo = self.registers[v_a];
                    let val_hi = self.registers[v_a + 1];
                    let obj_id = self.registers[v_b];
                    let obj = self.get_object_mut(obj_id)?;
                    if obj.array_elements.is_some() || obj.class_descriptor.starts_with('[') {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    obj.set_field(field_idx, val_lo);
                    obj.set_field(field_idx.wrapping_add(1), val_hi);
                    pc += 4;
                }
                OP_SGET | OP_SGET_OBJECT | OP_SGET_BOOLEAN | OP_SGET_BYTE | OP_SGET_CHAR | OP_SGET_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_dest >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let raw = self.get_static_field(field_idx);
                    self.registers[v_dest] = match opcode {
                        OP_SGET_BOOLEAN => raw & 0x01,
                        OP_SGET_BYTE => (raw as i8 as i32) as u32,
                        OP_SGET_CHAR => (raw as u16) as u32,
                        OP_SGET_SHORT => (raw as i16 as i32) as u32,
                        _ => raw,
                    };
                    self.string_registers[v_dest] = if opcode == OP_SGET_OBJECT {
                        self.get_string_payload(raw)
                    } else {
                        None
                    };
                    pc += 4;
                }
                OP_SGET_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_dest + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.registers[v_dest] = self.get_static_field(field_idx);
                    self.registers[v_dest + 1] = self.get_static_field(field_idx.wrapping_add(1));
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 4;
                }
                OP_SPUT | OP_SPUT_OBJECT | OP_SPUT_BOOLEAN | OP_SPUT_BYTE | OP_SPUT_CHAR | OP_SPUT_SHORT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_src = insns[pc + 1] as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_src >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let val = self.registers[v_src];
                    let store_val = match opcode {
                        OP_SPUT_BOOLEAN => val & 0x01,
                        _ => val,
                    };
                    self.set_static_field(field_idx, store_val)?;
                    pc += 4;
                }
                OP_SPUT_WIDE => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_src = insns[pc + 1] as usize;
                    let field_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]);
                    if v_src + 1 >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    self.set_static_field(field_idx, self.registers[v_src])?;
                    self.set_static_field(field_idx.wrapping_add(1), self.registers[v_src + 1])?;
                    pc += 4;
                }
                OP_INT_TO_LONG => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = (self.registers[v_src] as i32) as i64;
                    self.set_wide_reg(v_dest, val as u64)?;
                    pc += 2;
                }
                OP_LONG_TO_INT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.get_wide_reg(v_src) as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_INT_TO_DOUBLE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = (self.registers[v_src] as i32) as f64;
                    self.set_wide_reg(v_dest, val.to_bits())?;
                    pc += 2;
                }
                OP_INT_TO_FLOAT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let int_val = self.registers[v_src] as i32;
                    let float_val = int_val as f32;
                    self.registers[v_dest] = float_val.to_bits();
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_LONG_TO_FLOAT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = (self.get_wide_reg(v_src) as i64) as f32;
                    self.registers[v_dest] = val.to_bits();
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_LONG_TO_DOUBLE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = (self.get_wide_reg(v_src) as i64) as f64;
                    self.set_wide_reg(v_dest, val.to_bits())?;
                    pc += 2;
                }
                OP_FLOAT_TO_INT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let float_val = f32::from_bits(self.registers[v_src]);
                    self.registers[v_dest] = (float_val as i32) as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_FLOAT_TO_LONG => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let float_val = f32::from_bits(self.registers[v_src]);
                    self.set_wide_reg(v_dest, (float_val as i64) as u64)?;
                    pc += 2;
                }
                OP_FLOAT_TO_DOUBLE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let float_val = f32::from_bits(self.registers[v_src]) as f64;
                    self.set_wide_reg(v_dest, float_val.to_bits())?;
                    pc += 2;
                }
                OP_DOUBLE_TO_INT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let d = f64::from_bits(self.get_wide_reg(v_src));
                    self.registers[v_dest] = (d as i32) as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_DOUBLE_TO_LONG => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let d = f64::from_bits(self.get_wide_reg(v_src));
                    self.set_wide_reg(v_dest, (d as i64) as u64)?;
                    pc += 2;
                }
                OP_DOUBLE_TO_FLOAT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let d = f64::from_bits(self.get_wide_reg(v_src)) as f32;
                    self.registers[v_dest] = d.to_bits();
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_INT_TO_BYTE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let byte_val = (self.registers[v_src] as i8) as i32;
                    self.registers[v_dest] = byte_val as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_INT_TO_CHAR => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = (self.registers[v_src] as u16) as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_INT_TO_SHORT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let short_val = (self.registers[v_src] as i16) as i32;
                    self.registers[v_dest] = short_val as u32;
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_CMPL_FLOAT | OP_CMPG_FLOAT => {
                    if pc + 4 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest >= reg_cap || v_b >= reg_cap || v_c >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let fb = f32::from_bits(self.registers[v_b]);
                    let fc = f32::from_bits(self.registers[v_c]);
                    let res: i32 = if fb.is_nan() || fc.is_nan() {
                        if opcode == OP_CMPL_FLOAT { -1 } else { 1 }
                    } else if fb < fc {
                        -1
                    } else if fb > fc {
                        1
                    } else {
                        0
                    };
                    self.registers[v_dest] = res as u32;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_RETURN_VOID => {
                    self.last_return_value = 0;
                    self.last_return_string = None;
                    return Ok(0);
                }
                OP_RETURN | OP_RETURN_WIDE | OP_RETURN_OBJECT => {
                    if pc + 2 > insns.len() {
                        return Err(DalvikVmError::InvalidBytecode);
                    }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= reg_cap {
                        return Err(DalvikVmError::RegisterOutOfBounds);
                    }
                    let ret_val = self.registers[v_src];
                    self.last_return_value = ret_val;
                    if opcode == OP_RETURN_WIDE {
                        if v_src + 1 >= reg_cap {
                            return Err(DalvikVmError::RegisterOutOfBounds);
                        }
                        self.last_return_wide_hi = self.registers[v_src + 1];
                        self.last_return_string = None;
                    } else if opcode == OP_RETURN_OBJECT {
                        self.last_return_wide_hi = 0;
                        if let Some(ref s) = self.string_registers[v_src] {
                            self.last_return_string = Some(s.clone());
                        } else if let Ok(obj) = self.get_object(ret_val) {
                            if let ObjectPayload::String(ref s) = obj.payload {
                                self.last_return_string = Some(s.clone());
                            } else {
                                self.last_return_string = None;
                            }
                        } else {
                            self.last_return_string = None;
                        }
                    } else {
                        self.last_return_wide_hi = 0;
                        self.last_return_string = self.string_registers[v_src].clone();
                    }
                    return Ok(ret_val);
                }
                OP_MOVE_FROM16 => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 4;
                }
                OP_MOVE_16 => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 4], insns[pc + 5]]) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 6;
                }
                OP_MOVE_WIDE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.registers[v_dest + 1] = self.registers[v_src + 1];
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 2;
                }
                OP_MOVE_WIDE_FROM16 => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.registers[v_dest + 1] = self.registers[v_src + 1];
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 4;
                }
                OP_MOVE_WIDE_16 => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 4], insns[pc + 5]]) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.registers[v_dest + 1] = self.registers[v_src + 1];
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 6;
                }
                OP_MOVE_OBJECT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 2;
                }
                OP_MOVE_OBJECT_FROM16 => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 4;
                }
                OP_MOVE_OBJECT_16 => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    let v_src = u16::from_le_bytes([insns[pc + 4], insns[pc + 5]]) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.registers[v_src];
                    self.string_registers[v_dest] = self.string_registers[v_src].clone();
                    pc += 6;
                }
                OP_MOVE_RESULT_WIDE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.last_return_value;
                    self.registers[v_dest + 1] = self.last_return_wide_hi;
                    self.string_registers[v_dest] = None;
                    self.string_registers[v_dest + 1] = None;
                    pc += 2;
                }
                OP_MOVE_RESULT_OBJECT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    if v_dest >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    self.registers[v_dest] = self.last_return_value;
                    self.string_registers[v_dest] = self.last_return_string.clone();
                    pc += 2;
                }
                OP_MONITOR_ENTER => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let obj_id = self.registers[v_src];
                    if obj_id == 0 {
                        return Err(self.throw_npe(Some("Null object reference in monitor-enter")));
                    }
                    let tid = self.current_thread_id;
                    let mut is_blocked = false;

                    if let Some(rec) = self.monitors.get_mut(&obj_id) {
                        if rec.recursion_count == 0 {
                            rec.owner_thread = tid;
                            rec.recursion_count = 1;
                        } else if rec.owner_thread == tid {
                            rec.recursion_count = rec.recursion_count.saturating_add(1);
                        } else {
                            // Contention: monitor held by another thread!
                            // Enqueue current thread on wait_queue and mark Blocked
                            if !rec.wait_queue.contains(&tid) {
                                rec.wait_queue.push(tid);
                            }
                            if let Some(t) = self.threads.get_mut(&tid) {
                                t.status = JavaThreadStatus::BlockedOnMonitor(obj_id);
                            }
                            is_blocked = true;
                        }
                    } else {
                        self.monitors.insert(obj_id, MonitorRecord {
                            owner_thread: tid,
                            recursion_count: 1,
                            wait_queue: Vec::new(),
                            cond_waiters: Vec::new(),
                        });
                    }

                    if is_blocked {
                        // Save pc on thread state for acquisition retry
                        if let Some(t) = self.threads.get_mut(&tid) {
                            t.pc = pc;
                        }
                        return Err(DalvikVmError::MonitorBlocked(obj_id));
                    }

                    if let Some(t) = self.threads.get_mut(&tid) {
                        if !t.owned_monitors.contains(&obj_id) {
                            t.owned_monitors.push(obj_id);
                        }
                    }
                    pc += 2;
                }
                OP_MONITOR_EXIT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let obj_id = self.registers[v_src];
                    if obj_id == 0 {
                        return Err(self.throw_npe(Some("Null object reference in monitor-exit")));
                    }
                    let tid = self.current_thread_id;
                    let mut should_remove = false;
                    let mut woken_thread = None;

                    if let Some(rec) = self.monitors.get_mut(&obj_id) {
                        if rec.owner_thread != tid || rec.recursion_count == 0 {
                            return Err(self.throw_illegal_monitor_state(Some("Current thread does not own monitor")));
                        }
                        rec.recursion_count -= 1;
                        if rec.recursion_count == 0 {
                            if !rec.wait_queue.is_empty() {
                                let next_owner = rec.wait_queue.remove(0);
                                rec.owner_thread = next_owner;
                                rec.recursion_count = 1;
                                woken_thread = Some(next_owner);
                            } else if rec.cond_waiters.is_empty() {
                                should_remove = true;
                            } else {
                                rec.owner_thread = 0;
                            }
                        }
                    } else {
                        return Err(self.throw_illegal_monitor_state(Some("Monitor not locked")));
                    }

                    if should_remove {
                        self.monitors.remove(&obj_id);
                    }
                    if let Some(w_tid) = woken_thread {
                        if let Some(t) = self.threads.get_mut(&w_tid) {
                            t.status = JavaThreadStatus::Runnable;
                            t.pc = t.pc.saturating_add(2);
                            if !t.owned_monitors.contains(&obj_id) {
                                t.owned_monitors.push(obj_id);
                            }
                        }
                        self.enqueue_runnable(w_tid);
                    }
                    if should_remove || woken_thread.is_some() {
                        if let Some(curr_t) = self.threads.get_mut(&tid) {
                            curr_t.owned_monitors.retain(|&m| m != obj_id);
                        }
                    }
                    pc += 2;
                }
                OP_CHECK_CAST => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let obj_id = self.registers[v_dest];
                    if obj_id > 0 {
                        let target_type = dex.get_type_slice(type_idx as u32).map_err(|_| DalvikVmError::InvalidBytecode)?;
                        let obj = self.get_object(obj_id)?;
                        if !self.is_type_compatible(dex, &obj.class_descriptor, target_type) {
                            return Err(self.throw_class_cast(Some("Incompatible cast")));
                        }
                    }
                    pc += 4;
                }
                OP_INSTANCE_OF => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let type_idx = u16::from_le_bytes([insns[pc + 2], insns[pc + 3]]) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let obj_id = self.registers[v_src];
                    let is_inst = if obj_id > 0 {
                        if let Ok(target_type) = dex.get_type_slice(type_idx as u32) {
                            if let Ok(obj) = self.get_object(obj_id) {
                                self.is_type_compatible(dex, &obj.class_descriptor, target_type)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    self.registers[v_dest] = if is_inst { 1 } else { 0 };
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_FILL_ARRAY_DATA => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_arr = insns[pc + 1] as usize;
                    if v_arr >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let offset = (i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as isize) * 2;
                    let payload_pos = pc as isize + offset;
                    if payload_pos >= 0 && (payload_pos as usize + 8) <= insns.len() {
                        let p = payload_pos as usize;
                        let elem_width = u16::from_le_bytes([insns[p + 2], insns[p + 3]]) as usize;
                        let size = u32::from_le_bytes([insns[p + 4], insns[p + 5], insns[p + 6], insns[p + 7]]) as usize;
                        let total_bytes = elem_width.saturating_mul(size);
                        if p + 8 + total_bytes <= insns.len() {
                            let arr_id = self.registers[v_arr];
                            if let Ok(obj) = self.get_object_mut(arr_id) {
                                if let Some(elements) = obj.array_elements.as_mut() {
                                    for i in 0..size.min(elements.len()) {
                                        let ep = p + 8 + i * elem_width;
                                        elements[i] = match elem_width {
                                            1 => insns[ep] as u32,
                                            2 => u16::from_le_bytes([insns[ep], insns[ep + 1]]) as u32,
                                            4 => u32::from_le_bytes([insns[ep], insns[ep + 1], insns[ep + 2], insns[ep + 3]]),
                                            _ => 0,
                                        };
                                    }
                                }
                            }
                        }
                    }
                    pc += 6;
                }
                OP_THROW => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_ex = insns[pc + 1] as usize;
                    if v_ex >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let ex_id = self.registers[v_ex];
                    if ex_id == 0 { return Err(DalvikVmError::NullPointerDereference); }
                    if code_offset > 0 {
                        let ex_class = self.get_object(ex_id).map(|o| o.class_descriptor.clone()).ok();
                        if let Some(handler_pc) = find_catch_handler(dex, code_offset, pc, ex_class.as_deref()) {
                            self.pending_exception = Some(ex_id);
                            pc = handler_pc;
                            continue;
                        }
                    }
                    return Err(DalvikVmError::UnhandledException(ex_id));
                }
                OP_PACKED_SWITCH => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let test_val = self.registers[v_src] as i32;
                    let offset = (i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as isize) * 2;
                    let payload_pos = pc as isize + offset;
                    let mut matched = false;
                    if payload_pos >= 0 && (payload_pos as usize + 8) <= insns.len() {
                        let p = payload_pos as usize;
                        let size = u16::from_le_bytes([insns[p + 2], insns[p + 3]]) as usize;
                        let first_key = i32::from_le_bytes([insns[p + 4], insns[p + 5], insns[p + 6], insns[p + 7]]);
                        if test_val >= first_key && (test_val as isize) < (first_key as isize + size as isize) {
                            let idx = (test_val - first_key) as usize;
                            let target_off_pos = p + 8 + idx * 4;
                            if target_off_pos + 4 <= insns.len() {
                                let target_offset = (i32::from_le_bytes([insns[target_off_pos], insns[target_off_pos + 1], insns[target_off_pos + 2], insns[target_off_pos + 3]]) as isize) * 2;
                                let target_pc = pc as isize + target_offset;
                                if target_pc >= 0 && (target_pc as usize) < insns.len() {
                                    pc = target_pc as usize;
                                    matched = true;
                                }
                            }
                        }
                    }
                    if !matched { pc += 6; }
                }
                OP_SPARSE_SWITCH => {
                    if pc + 6 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_src = insns[pc + 1] as usize;
                    if v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let test_val = self.registers[v_src] as i32;
                    let offset = (i32::from_le_bytes([insns[pc + 2], insns[pc + 3], insns[pc + 4], insns[pc + 5]]) as isize) * 2;
                    let payload_pos = pc as isize + offset;
                    let mut matched = false;
                    if payload_pos >= 0 && (payload_pos as usize + 4) <= insns.len() {
                        let p = payload_pos as usize;
                        let size = u16::from_le_bytes([insns[p + 2], insns[p + 3]]) as usize;
                        let keys_start = p + 4;
                        let targets_start = keys_start + size * 4;
                        if targets_start + size * 4 <= insns.len() {
                            for i in 0..size {
                                let kp = keys_start + i * 4;
                                let key = i32::from_le_bytes([insns[kp], insns[kp + 1], insns[kp + 2], insns[kp + 3]]);
                                if key == test_val {
                                    let tp = targets_start + i * 4;
                                    let target_offset = (i32::from_le_bytes([insns[tp], insns[tp + 1], insns[tp + 2], insns[tp + 3]]) as isize) * 2;
                                    let target_pc = pc as isize + target_offset;
                                    if target_pc >= 0 && (target_pc as usize) < insns.len() {
                                        pc = target_pc as usize;
                                        matched = true;
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    if !matched { pc += 6; }
                }
                OP_NEG_INT | OP_NOT_INT | OP_NEG_FLOAT => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = self.registers[v_src];
                    self.registers[v_dest] = match opcode {
                        OP_NEG_INT => (-(val as i32)) as u32,
                        OP_NOT_INT => !val,
                        OP_NEG_FLOAT => {
                            let f = f32::from_bits(val);
                            (-f).to_bits()
                        }
                        _ => 0,
                    };
                    self.string_registers[v_dest] = None;
                    pc += 2;
                }
                OP_NEG_LONG | OP_NOT_LONG | OP_NEG_DOUBLE => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_dest + 1 >= reg_cap || v_src + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let val = self.get_wide_reg(v_src);
                    let res = match opcode {
                        OP_NEG_LONG => (-(val as i64)) as u64,
                        OP_NOT_LONG => !val,
                        OP_NEG_DOUBLE => {
                            let d = f64::from_bits(val);
                            (-d).to_bits()
                        }
                        _ => 0,
                    };
                    self.set_wide_reg(v_dest, res)?;
                    pc += 2;
                }
                OP_ADD_INT_LIT8 | OP_RSUB_INT_LIT8 | OP_MUL_INT_LIT8 | OP_DIV_INT_LIT8 | OP_REM_INT_LIT8
                | OP_AND_INT_LIT8 | OP_OR_INT_LIT8 | OP_XOR_INT_LIT8 | OP_SHL_INT_LIT8 | OP_SHR_INT_LIT8 | OP_USHR_INT_LIT8 => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_src = insns[pc + 2] as usize;
                    let lit8 = insns[pc + 3] as i8 as i32;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let src_val = self.registers[v_src] as i32;
                    let res: u32 = match opcode {
                        OP_ADD_INT_LIT8 => src_val.wrapping_add(lit8) as u32,
                        OP_RSUB_INT_LIT8 => lit8.wrapping_sub(src_val) as u32,
                        OP_MUL_INT_LIT8 => src_val.wrapping_mul(lit8) as u32,
                        OP_DIV_INT_LIT8 => {
                            if lit8 == 0 { return Err(DalvikVmError::DivideByZero); }
                            src_val.wrapping_div(lit8) as u32
                        }
                        OP_REM_INT_LIT8 => {
                            if lit8 == 0 { return Err(DalvikVmError::DivideByZero); }
                            src_val.wrapping_rem(lit8) as u32
                        }
                        OP_AND_INT_LIT8 => (src_val & lit8) as u32,
                        OP_OR_INT_LIT8 => (src_val | lit8) as u32,
                        OP_XOR_INT_LIT8 => (src_val ^ lit8) as u32,
                        OP_SHL_INT_LIT8 => (src_val.wrapping_shl((lit8 & 0x1F) as u32)) as u32,
                        OP_SHR_INT_LIT8 => (src_val.wrapping_shr((lit8 & 0x1F) as u32)) as u32,
                        OP_USHR_INT_LIT8 => (src_val as u32).wrapping_shr((lit8 & 0x1F) as u32),
                        _ => 0,
                    };
                    self.registers[v_dest] = res;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_ADD_INT_LIT16 | OP_RSUB_INT | OP_MUL_INT_LIT16 | OP_DIV_INT_LIT16 | OP_REM_INT_LIT16
                | OP_AND_INT_LIT16 | OP_OR_INT_LIT16 | OP_XOR_INT_LIT16 => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = (insns[pc + 1] & 0x0F) as usize;
                    let v_src = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    let lit16 = (i16::from_le_bytes([insns[pc + 2], insns[pc + 3]])) as i32;
                    if v_dest >= reg_cap || v_src >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let src_val = self.registers[v_src] as i32;
                    let res: u32 = match opcode {
                        OP_ADD_INT_LIT16 => src_val.wrapping_add(lit16) as u32,
                        OP_RSUB_INT => lit16.wrapping_sub(src_val) as u32,
                        OP_MUL_INT_LIT16 => src_val.wrapping_mul(lit16) as u32,
                        OP_DIV_INT_LIT16 => {
                            if lit16 == 0 { return Err(DalvikVmError::DivideByZero); }
                            src_val.wrapping_div(lit16) as u32
                        }
                        OP_REM_INT_LIT16 => {
                            if lit16 == 0 { return Err(DalvikVmError::DivideByZero); }
                            src_val.wrapping_rem(lit16) as u32
                        }
                        OP_AND_INT_LIT16 => (src_val & lit16) as u32,
                        OP_OR_INT_LIT16 => (src_val | lit16) as u32,
                        OP_XOR_INT_LIT16 => (src_val ^ lit16) as u32,
                        _ => 0,
                    };
                    self.registers[v_dest] = res;
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_ADD_LONG | OP_SUB_LONG | OP_MUL_LONG | OP_DIV_LONG | OP_REM_LONG
                | OP_AND_LONG | OP_OR_LONG | OP_XOR_LONG | OP_SHL_LONG | OP_SHR_LONG | OP_USHR_LONG => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest + 1 >= reg_cap || v_b + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let b = self.get_wide_reg(v_b) as i64;
                    let res: i64 = match opcode {
                        OP_SHL_LONG | OP_SHR_LONG | OP_USHR_LONG => {
                            if v_c >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                            let shift = (self.registers[v_c] & 0x3F) as u32;
                            match opcode {
                                OP_SHL_LONG => b.wrapping_shl(shift),
                                OP_SHR_LONG => b.wrapping_shr(shift),
                                OP_USHR_LONG => ((b as u64).wrapping_shr(shift)) as i64,
                                _ => 0,
                            }
                        }
                        _ => {
                            if v_c + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                            let c = self.get_wide_reg(v_c) as i64;
                            match opcode {
                                OP_ADD_LONG => b.wrapping_add(c),
                                OP_SUB_LONG => b.wrapping_sub(c),
                                OP_MUL_LONG => b.wrapping_mul(c),
                                OP_DIV_LONG => {
                                    if c == 0 { return Err(DalvikVmError::DivideByZero); }
                                    b.wrapping_div(c)
                                }
                                OP_REM_LONG => {
                                    if c == 0 { return Err(DalvikVmError::DivideByZero); }
                                    b.wrapping_rem(c)
                                }
                                OP_AND_LONG => b & c,
                                OP_OR_LONG => b | c,
                                OP_XOR_LONG => b ^ c,
                                _ => 0,
                            }
                        }
                    };
                    self.set_wide_reg(v_dest, res as u64)?;
                    pc += 4;
                }
                OP_ADD_LONG_2ADDR | OP_SUB_LONG_2ADDR | OP_MUL_LONG_2ADDR | OP_DIV_LONG_2ADDR | OP_REM_LONG_2ADDR
                | OP_AND_LONG_2ADDR | OP_OR_LONG_2ADDR | OP_XOR_LONG_2ADDR | OP_SHL_LONG_2ADDR | OP_SHR_LONG_2ADDR | OP_USHR_LONG_2ADDR => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let a = self.get_wide_reg(v_a) as i64;
                    let res: i64 = match opcode {
                        OP_SHL_LONG_2ADDR | OP_SHR_LONG_2ADDR | OP_USHR_LONG_2ADDR => {
                            if v_b >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                            let shift = (self.registers[v_b] & 0x3F) as u32;
                            match opcode {
                                OP_SHL_LONG_2ADDR => a.wrapping_shl(shift),
                                OP_SHR_LONG_2ADDR => a.wrapping_shr(shift),
                                OP_USHR_LONG_2ADDR => ((a as u64).wrapping_shr(shift)) as i64,
                                _ => 0,
                            }
                        }
                        _ => {
                            if v_b + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                            let b = self.get_wide_reg(v_b) as i64;
                            match opcode {
                                OP_ADD_LONG_2ADDR => a.wrapping_add(b),
                                OP_SUB_LONG_2ADDR => a.wrapping_sub(b),
                                OP_MUL_LONG_2ADDR => a.wrapping_mul(b),
                                OP_DIV_LONG_2ADDR => {
                                    if b == 0 { return Err(DalvikVmError::DivideByZero); }
                                    a.wrapping_div(b)
                                }
                                OP_REM_LONG_2ADDR => {
                                    if b == 0 { return Err(DalvikVmError::DivideByZero); }
                                    a.wrapping_rem(b)
                                }
                                OP_AND_LONG_2ADDR => a & b,
                                OP_OR_LONG_2ADDR => a | b,
                                OP_XOR_LONG_2ADDR => a ^ b,
                                _ => 0,
                            }
                        }
                    };
                    self.set_wide_reg(v_a, res as u64)?;
                    pc += 2;
                }
                OP_ADD_FLOAT | OP_SUB_FLOAT | OP_MUL_FLOAT | OP_DIV_FLOAT | OP_REM_FLOAT => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest >= reg_cap || v_b >= reg_cap || v_c >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let fb = f32::from_bits(self.registers[v_b]);
                    let fc = f32::from_bits(self.registers[v_c]);
                    let res = match opcode {
                        OP_ADD_FLOAT => fb + fc,
                        OP_SUB_FLOAT => fb - fc,
                        OP_MUL_FLOAT => fb * fc,
                        OP_DIV_FLOAT => fb / fc,
                        OP_REM_FLOAT => fb % fc,
                        _ => 0.0,
                    };
                    self.registers[v_dest] = res.to_bits();
                    self.string_registers[v_dest] = None;
                    pc += 4;
                }
                OP_ADD_FLOAT_2ADDR | OP_SUB_FLOAT_2ADDR | OP_MUL_FLOAT_2ADDR | OP_DIV_FLOAT_2ADDR | OP_REM_FLOAT_2ADDR => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a >= reg_cap || v_b >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let fa = f32::from_bits(self.registers[v_a]);
                    let fb = f32::from_bits(self.registers[v_b]);
                    let res = match opcode {
                        OP_ADD_FLOAT_2ADDR => fa + fb,
                        OP_SUB_FLOAT_2ADDR => fa - fb,
                        OP_MUL_FLOAT_2ADDR => fa * fb,
                        OP_DIV_FLOAT_2ADDR => fa / fb,
                        OP_REM_FLOAT_2ADDR => fa % fb,
                        _ => 0.0,
                    };
                    self.registers[v_a] = res.to_bits();
                    self.string_registers[v_a] = None;
                    pc += 2;
                }
                OP_ADD_DOUBLE | OP_SUB_DOUBLE | OP_MUL_DOUBLE | OP_DIV_DOUBLE | OP_REM_DOUBLE => {
                    if pc + 4 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_dest = insns[pc + 1] as usize;
                    let v_b = insns[pc + 2] as usize;
                    let v_c = insns[pc + 3] as usize;
                    if v_dest + 1 >= reg_cap || v_b + 1 >= reg_cap || v_c + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let db = f64::from_bits(self.get_wide_reg(v_b));
                    let dc = f64::from_bits(self.get_wide_reg(v_c));
                    let res = match opcode {
                        OP_ADD_DOUBLE => db + dc,
                        OP_SUB_DOUBLE => db - dc,
                        OP_MUL_DOUBLE => db * dc,
                        OP_DIV_DOUBLE => db / dc,
                        OP_REM_DOUBLE => db % dc,
                        _ => 0.0,
                    };
                    self.set_wide_reg(v_dest, res.to_bits())?;
                    pc += 4;
                }
                OP_ADD_DOUBLE_2ADDR | OP_SUB_DOUBLE_2ADDR | OP_MUL_DOUBLE_2ADDR | OP_DIV_DOUBLE_2ADDR | OP_REM_DOUBLE_2ADDR => {
                    if pc + 2 > insns.len() { return Err(DalvikVmError::InvalidBytecode); }
                    let v_a = (insns[pc + 1] & 0x0F) as usize;
                    let v_b = ((insns[pc + 1] >> 4) & 0x0F) as usize;
                    if v_a + 1 >= reg_cap || v_b + 1 >= reg_cap { return Err(DalvikVmError::RegisterOutOfBounds); }
                    let da = f64::from_bits(self.get_wide_reg(v_a));
                    let db = f64::from_bits(self.get_wide_reg(v_b));
                    let res = match opcode {
                        OP_ADD_DOUBLE_2ADDR => da + db,
                        OP_SUB_DOUBLE_2ADDR => da - db,
                        OP_MUL_DOUBLE_2ADDR => da * db,
                        OP_DIV_DOUBLE_2ADDR => da / db,
                        OP_REM_DOUBLE_2ADDR => da % db,
                        _ => 0.0,
                    };
                    self.set_wide_reg(v_a, res.to_bits())?;
                    pc += 2;
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
    use crate::framework::*;
    use interos_android_format::dex::create_test_dex;

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
        vm.registers[0] = first;
        vm.registers[1] = second;
        vm.heap_max_bytes = vm.heap_used_bytes;
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
            _ => panic!("Expected Return(1)"),
        }
        assert_eq!(vm.last_return_value, 1);
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

    #[test]
    fn test_dalvik_static_fields() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // sput v0 into static field 42
        vm.registers[0] = 8888;
        let insns = [OP_SPUT, 0x00, 0x2A, 0x00, OP_RETURN_VOID];
        vm.interpret_insns(&dex, 1, &insns).unwrap();

        // sget v1 from static field 42
        let insns = [OP_SGET, 0x01, 0x2A, 0x00, OP_RETURN, 0x01];
        let val = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(val, 8888);
    }

    #[test]
    fn test_dalvik_conversions_and_float_comparisons() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // int-to-float: v1 = (float)v0 (42 -> 42.0f)
        vm.registers[0] = 42;
        let insns = [OP_INT_TO_FLOAT, 0x01, OP_RETURN, 0x01];
        let f_bits = vm.interpret_insns(&dex, 2, &insns).unwrap();
        assert_eq!(f32::from_bits(f_bits), 42.0f32);

        // float-to-int: v2 = (int)v1
        let insns = [OP_FLOAT_TO_INT, 0x12, OP_RETURN, 0x02];
        let int_val = vm.interpret_insns(&dex, 3, &insns).unwrap();
        assert_eq!(int_val, 42);

        // int-to-byte: v3 = (byte)v0 (258 -> 2)
        vm.registers[0] = 258;
        let insns = [OP_INT_TO_BYTE, 0x03, OP_RETURN, 0x03];
        let byte_val = vm.interpret_insns(&dex, 4, &insns).unwrap();
        assert_eq!(byte_val, 2);

        // int-to-char: v4 = (char)v0 (0x10041 -> 0x0041)
        vm.registers[0] = 0x10041;
        let insns = [OP_INT_TO_CHAR, 0x04, OP_RETURN, 0x04];
        let char_val = vm.interpret_insns(&dex, 5, &insns).unwrap();
        assert_eq!(char_val, 0x0041);

        // int-to-short: v5 = (short)v0 (0x10005 -> 5)
        vm.registers[0] = 0x10005;
        let insns = [OP_INT_TO_SHORT, 0x05, OP_RETURN, 0x05];
        let short_val = vm.interpret_insns(&dex, 6, &insns).unwrap();
        assert_eq!(short_val, 5);

        // cmpl-float: v0 = 1.5f, v1 = 2.5f -> v2 = -1
        vm.registers[0] = 1.5f32.to_bits();
        vm.registers[1] = 2.5f32.to_bits();
        let insns = [OP_CMPL_FLOAT, 0x02, 0x00, 0x01, OP_RETURN, 0x02];
        let cmp_res = vm.interpret_insns(&dex, 3, &insns).unwrap();
        assert_eq!(cmp_res as i32, -1);

        // cmpg-float: v0 = 3.0f, v1 = 2.0f -> v2 = 1
        vm.registers[0] = 3.0f32.to_bits();
        vm.registers[1] = 2.0f32.to_bits();
        let insns = [OP_CMPG_FLOAT, 0x02, 0x00, 0x01, OP_RETURN, 0x02];
        let cmp_res2 = vm.interpret_insns(&dex, 3, &insns).unwrap();
        assert_eq!(cmp_res2 as i32, 1);
    }

    #[test]
    fn test_dalvik_literals_and_moves() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        vm.registers[1] = 10;
        let insns = [OP_ADD_INT_LIT8, 0x00, 0x01, 0x05, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns).unwrap(), 15);

        let insns = [OP_RSUB_INT_LIT8, 0x00, 0x01, 0x32, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns).unwrap(), 40);

        let insns = [OP_MUL_INT_LIT16, 0x10, 0x64, 0x00, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns).unwrap(), 1000);

        vm.registers[1] = 999;
        let insns = [OP_MOVE_FROM16, 0x00, 0x01, 0x00, OP_RETURN, 0x00];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns).unwrap(), 999);
    }

    #[test]
    fn test_dalvik_gc_mark_and_sweep() {
        let mut vm = DalvikVmState::new();
        let obj1 = vm.allocate_object("Ljava/lang/Object;").expect("alloc 1");
        let obj2 = vm.allocate_object("Ljava/lang/Object;").expect("alloc 2");
        let _obj3 = vm.allocate_object("Ljava/lang/Object;").expect("alloc 3");
        assert_eq!(vm.objects.len(), 3);

        vm.registers[0] = obj1;
        let _ = vm.set_static_field(10, obj2);

        let (reclaimed, pause_ticks) = vm.gc_mark_and_sweep();
        assert_eq!(reclaimed, 1);
        assert!(pause_ticks > 0);
        assert_eq!(vm.objects.len(), 2);
        assert_eq!(vm.gc_cycles, 1);
        assert_eq!(vm.gc_reclaimed_objects, 1);
    }

    #[test]
    fn test_security_wide_opcode_at_reg_cap_minus_one_rejected() {
        let mut vm = DalvikVmState::new();
        let insns = [OP_RETURN_WIDE, 31];
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        assert_eq!(
            vm.interpret_insns(&dex, 32, &insns),
            Err(DalvikVmError::RegisterOutOfBounds)
        );
    }

    #[test]
    fn test_security_oversized_register_count_fails_closed() {
        let mut vm = DalvikVmState::new();
        let mut dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let initial_dex = DexPackage::parse(&dex_bytes).unwrap();
        let code_offset = initial_dex.methods[0].code_offset;
        let oversized = (MAX_REGISTERS_LIMIT + 1) as u16;
        dex_bytes[code_offset..code_offset + 2].copy_from_slice(&oversized.to_le_bytes());
        let checksum = interos_android_format::dex::compute_adler32(&dex_bytes[12..]);
        dex_bytes[8..12].copy_from_slice(&checksum.to_le_bytes());
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let res = vm.invoke_method(&dex, "Lorg/interos/Test;", "onCreate");
        assert_eq!(res, Err(DalvikVmError::RegisterOutOfBounds));
    }

    #[test]
    fn test_dalvik_range_invoke_opcodes() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // Test range invoke with 3 arguments starting at register 4
        let insns = [
            OP_INVOKE_STATIC_RANGE, 0x03, 0x00, 0x00, 0x04, 0x00,
            OP_RETURN, 0x00,
        ];
        let res = vm.interpret_insns(&dex, 16, &insns);
        assert_eq!(res, Err(DalvikVmError::InvalidBytecode));

        // Range invoke with first_reg + count exceeding registers_size fails closed with RegisterOutOfBounds
        let out_of_bounds = [
            OP_INVOKE_STATIC_RANGE, 0x05, 0x00, 0x00, 0x0C, 0x00,
            OP_RETURN, 0x00,
        ];
        let res = vm.interpret_insns(&dex, 16, &out_of_bounds);
        assert_eq!(res, Err(DalvikVmError::RegisterOutOfBounds));
    }

    #[test]
    fn test_dalvik_exception_handling_and_throw() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Throwing null (reg = 0) returns NullPointerDereference
        let throw_null = [OP_THROW, 0x01, OP_RETURN_VOID];
        vm.registers[1] = 0;
        assert_eq!(vm.interpret_insns(&dex, 8, &throw_null), Err(DalvikVmError::NullPointerDereference));

        // 2. Throwing a valid object without try-catch returns UnhandledException
        let obj_id = vm.allocate_object("Ljava/lang/RuntimeException;").unwrap();
        vm.registers[1] = obj_id;
        assert_eq!(vm.interpret_insns(&dex, 8, &throw_null), Err(DalvikVmError::UnhandledException(obj_id)));

        // 3. Move exception clears pending_exception into destination register
        vm.pending_exception = Some(obj_id);
        let move_ex = [OP_MOVE_EXCEPTION, 0x02, OP_RETURN, 0x02];
        let caught = vm.interpret_insns(&dex, 8, &move_ex).unwrap();
        assert_eq!(caught, obj_id);
        assert_eq!(vm.registers[2], obj_id);
        assert_eq!(vm.pending_exception, None);
    }

    #[test]
    fn test_dalvik_java_standard_library_framework_dispatch() {
        let mut logs = Vec::new();
        // 1. java/lang/String length & equals
        assert_eq!(
            dispatch("Ljava/lang/String;", "length", &[Some("InterOS")], &mut logs),
            Ok(AndroidFrameworkResult::Return(7))
        );
        assert_eq!(
            dispatch("Ljava/lang/String;", "isEmpty", &[Some("")], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );
        assert_eq!(
            dispatch("Ljava/lang/String;", "equals", &[Some("abc"), Some("abc")], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );
        assert_eq!(
            dispatch("Ljava/lang/String;", "equals", &[Some("abc"), Some("xyz")], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );

        // 2. java/lang/StringBuilder append & toString
        assert_eq!(
            dispatch("Ljava/lang/StringBuilder;", "<init>", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(
            dispatch("Ljava/lang/StringBuilder;", "append", &[Some("Hello "), Some("World")], &mut logs),
            Ok(AndroidFrameworkResult::ReturnOwnedString(String::from("Hello World")))
        );

        // 3. java/lang/Class reflection
        assert_eq!(
            dispatch("Ljava/lang/Class;", "forName", &[Some("java.lang.String")], &mut logs),
            Ok(AndroidFrameworkResult::Return(1))
        );
        assert_eq!(
            dispatch("Ljava/lang/Class;", "getSimpleName", &[Some("java.lang.String")], &mut logs),
            Ok(AndroidFrameworkResult::ReturnOwnedString(String::from("String")))
        );

        // 4. android/content/Intent
        assert_eq!(
            dispatch("Landroid/content/Intent;", "<init>", &[], &mut logs),
            Ok(AndroidFrameworkResult::Return(0))
        );
        assert_eq!(
            dispatch("Landroid/content/Intent;", "getAction", &[], &mut logs),
            Ok(AndroidFrameworkResult::ReturnString("android.intent.action.MAIN"))
        );
    }

    #[test]
    fn test_vm_stateful_collections_retain_elements() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        let list_id = vm.allocate_object("Ljava/util/ArrayList;").unwrap();
        // <init>
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "<init>", &[0], &[list_id]), Ok(0));

        // Initial isEmpty is true (1), size is 0
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "isEmpty", &[0], &[list_id]), Ok(1));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "size", &[0], &[list_id]), Ok(0));

        // add element 42 and element 99
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "add", &[0, 1], &[list_id, 42]), Ok(1));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "add", &[0, 1], &[list_id, 99]), Ok(1));

        // size is now 2, isEmpty is 0
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "size", &[0], &[list_id]), Ok(2));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "isEmpty", &[0], &[list_id]), Ok(0));

        // get(0) == 42, get(1) == 99
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "get", &[0, 1], &[list_id, 0]), Ok(42));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "get", &[0, 1], &[list_id, 1]), Ok(99));

        // get(2) throws IndexOutOfBoundsException
        let oob = vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "get", &[0, 1], &[list_id, 2]);
        match oob {
            Err(DalvikVmError::UnhandledException(ex_id)) => {
                assert_eq!(vm.get_object(ex_id).unwrap().class_descriptor, "Ljava/lang/IndexOutOfBoundsException;");
            }
            _ => panic!("Expected IndexOutOfBoundsException"),
        }

        // clear()
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "clear", &[0], &[list_id]), Ok(0));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "size", &[0], &[list_id]), Ok(0));
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/ArrayList;", "isEmpty", &[0], &[list_id]), Ok(1));
    }

    #[test]
    fn test_vm_stateful_intent_and_bundle_typed_extras() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Intent setAction and getAction
        let intent_id = vm.allocate_object("Landroid/content/Intent;").unwrap();
        assert_eq!(vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "<init>", &[0], &[intent_id]), Ok(0));

        let act_str = vm.allocate_string("android.intent.action.VIEW").unwrap();
        let set_res = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "setAction", &[0, 1], &[intent_id, act_str]).unwrap();
        assert_eq!(set_res, intent_id);

        let get_act = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "getAction", &[0], &[intent_id]).unwrap();
        assert_eq!(vm.get_string_payload(get_act).as_deref(), Some("android.intent.action.VIEW"));

        // 2. Intent putExtra int and getIntExtra
        let k_id = vm.allocate_string("user_id").unwrap();
        let put_res = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "putExtra", &[0, 1, 2], &[intent_id, k_id, 1337]).unwrap();
        assert_eq!(put_res, intent_id);
        let get_int = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "getIntExtra", &[0, 1, 2], &[intent_id, k_id, 0]).unwrap();
        assert_eq!(get_int, 1337);

        // 3. Bundle putString and getString
        let bundle_id = vm.allocate_object("Landroid/os/Bundle;").unwrap();
        assert_eq!(vm.dispatch_framework_method(&dex, "Landroid/os/Bundle;", "<init>", &[0], &[bundle_id]), Ok(0));
        let b_key = vm.allocate_string("session_token").unwrap();
        let b_val = vm.allocate_string("tok_sec_999").unwrap();
        assert_eq!(vm.dispatch_framework_method(&dex, "Landroid/os/Bundle;", "putString", &[0, 1, 2], &[bundle_id, b_key, b_val]), Ok(0));
        let b_ret = vm.dispatch_framework_method(&dex, "Landroid/os/Bundle;", "getString", &[0, 1], &[bundle_id, b_key]).unwrap();
        assert_eq!(vm.get_string_payload(b_ret).as_deref(), Some("tok_sec_999"));

        // 4. putExtras copies bundle into intent
        assert_eq!(vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "putExtras", &[0, 1], &[intent_id, bundle_id]), Ok(intent_id));
        let from_intent = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "getStringExtra", &[0, 1], &[intent_id, b_key]).unwrap();
        assert_eq!(vm.get_string_payload(from_intent).as_deref(), Some("tok_sec_999"));

        // 5. getExtras returns a new Bundle object with copied extras
        let extracted_bundle = vm.dispatch_framework_method(&dex, "Landroid/content/Intent;", "getExtras", &[0], &[intent_id]).unwrap();
        assert!(extracted_bundle > 0);
        let ex_str = vm.dispatch_framework_method(&dex, "Landroid/os/Bundle;", "getString", &[0, 1], &[extracted_bundle, b_key]).unwrap();
        assert_eq!(vm.get_string_payload(ex_str).as_deref(), Some("tok_sec_999"));
    }

    #[test]
    fn test_vm_stateful_string_and_stringbuilder_utf16() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. String UTF-16 code units length: "InterOS 🚀" has 8 ASCII + 2 surrogate code units = 10
        let s_id = vm.allocate_string("InterOS 🚀").unwrap();
        let len = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "length", &[0], &[s_id]).unwrap();
        assert_eq!(len, 10);

        // charAt(0) == 'I' (0x49 = 73)
        let ch0 = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "charAt", &[0, 1], &[s_id, 0]).unwrap();
        assert_eq!(ch0, 73);

        // charAt(15) throws IndexOutOfBoundsException
        let oob = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "charAt", &[0, 1], &[s_id, 15]);
        match oob {
            Err(DalvikVmError::UnhandledException(ex_id)) => {
                assert_eq!(vm.get_object(ex_id).unwrap().class_descriptor, "Ljava/lang/IndexOutOfBoundsException;");
            }
            _ => panic!("Expected IndexOutOfBoundsException"),
        }

        // substring(0, 7) == "InterOS"
        let sub_id = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "substring", &[0, 1, 2], &[s_id, 0, 7]).unwrap();
        assert_eq!(vm.get_string_payload(sub_id).as_deref(), Some("InterOS"));

        // concat
        let world_id = vm.allocate_string(" World").unwrap();
        let cat_id = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "concat", &[0, 1], &[sub_id, world_id]).unwrap();
        assert_eq!(vm.get_string_payload(cat_id).as_deref(), Some("InterOS World"));

        // equals
        let eq_same = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "equals", &[0, 1], &[sub_id, sub_id]).unwrap();
        assert_eq!(eq_same, 1);
        let eq_diff = vm.dispatch_framework_method(&dex, "Ljava/lang/String;", "equals", &[0, 1], &[sub_id, world_id]).unwrap();
        assert_eq!(eq_diff, 0);

        // 2. StringBuilder mutating append & chaining
        let sb_id = vm.allocate_object("Ljava/lang/StringBuilder;").unwrap();
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/lang/StringBuilder;", "<init>", &[0], &[sb_id]), Ok(0));

        let part1 = vm.allocate_string("Pure ").unwrap();
        let chain1 = vm.dispatch_framework_method(&dex, "Ljava/lang/StringBuilder;", "append", &[0, 1], &[sb_id, part1]).unwrap();
        assert_eq!(chain1, sb_id);

        let part2 = vm.allocate_string("Rust").unwrap();
        let chain2 = vm.dispatch_framework_method(&dex, "Ljava/lang/StringBuilder;", "append", &[0, 1], &[sb_id, part2]).unwrap();
        assert_eq!(chain2, sb_id);

        let res_str_id = vm.dispatch_framework_method(&dex, "Ljava/lang/StringBuilder;", "toString", &[0], &[sb_id]).unwrap();
        assert_eq!(vm.get_string_payload(res_str_id).as_deref(), Some("Pure Rust"));
    }

    #[test]
    fn test_vm_stateful_reflection_and_exceptions() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "onCreate", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Class.forName on known DEX class Lorg/interos/Test;
        let c_test = vm.allocate_string("org.interos.Test").unwrap();
        let cls_id = vm.dispatch_framework_method(&dex, "Ljava/lang/Class;", "forName", &[0], &[c_test]).unwrap();
        assert!(cls_id > 0);

        // Class.getName and getSimpleName
        let name_id = vm.dispatch_framework_method(&dex, "Ljava/lang/Class;", "getName", &[0], &[cls_id]).unwrap();
        assert_eq!(vm.get_string_payload(name_id).as_deref(), Some("org.interos.Test"));
        let simple_id = vm.dispatch_framework_method(&dex, "Ljava/lang/Class;", "getSimpleName", &[0], &[cls_id]).unwrap();
        assert_eq!(vm.get_string_payload(simple_id).as_deref(), Some("Test"));

        // 2. Class.forName on non-existent class throws ClassNotFoundException
        let c_fake = vm.allocate_string("com.unknown.NonExistent").unwrap();
        let err = vm.dispatch_framework_method(&dex, "Ljava/lang/Class;", "forName", &[0], &[c_fake]);
        match err {
            Err(DalvikVmError::UnhandledException(ex_id)) => {
                assert_eq!(vm.get_object(ex_id).unwrap().class_descriptor, "Ljava/lang/ClassNotFoundException;");
            }
            _ => panic!("Expected ClassNotFoundException"),
        }

        // 3. Exception hierarchy matching: IndexOutOfBoundsException is caught by RuntimeException catch block
        assert!(is_exception_compatible(&dex, "Ljava/lang/IndexOutOfBoundsException;", 1)); // 1 is Object
        assert!(is_exception_compatible(&dex, "Ljava/lang/NullPointerException;", 1));
    }

    #[test]
    fn test_vm_virtual_dispatch_subclass_override() {
        let dex_bytes = create_test_dex("Lorg/interos/Base;", "compute", "test");
        let mut dex = DexPackage::parse(&dex_bytes).unwrap();

        // Add a subclass method override to DEX package:
        // Lorg/interos/Derived;->compute() overriding Base->compute()
        let mut derived_method = dex.methods[0].clone();
        derived_method.class_name = String::from("Lorg/interos/Derived;");
        derived_method.method_index = 100;
        dex.methods.push(derived_method);

        let mut vm = DalvikVmState::new();

        // Allocate a Derived instance
        let derived_obj_id = vm.allocate_object("Lorg/interos/Derived;").unwrap();
        vm.registers[0] = derived_obj_id;

        // Bytecode invoking virtual method 0 (Base->compute) with Derived receiver in v0
        // Format 35c: OP_INVOKE_VIRTUAL, count=1 in high nibble (0x10), method_index=0 (0x00, 0x00), reg 0 (0x00, 0x00)
        let insns = [
            OP_INVOKE_VIRTUAL, 0x10, 0x00, 0x00, 0x00, 0x00,
            OP_RETURN, 0x00,
        ];

        // Should resolve to Derived->compute (and successfully execute) rather than failing
        let res = vm.interpret_insns(&dex, 4, &insns);
        assert!(res.is_ok(), "Virtual dispatch must execute Derived override");
    }

    #[test]
    fn test_dalvik_long_and_double_arithmetic() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "compute", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Long addition: v0:v1 (1_000_000_000_000L) + v2:v3 (2_000_000_000_000L) -> v4:v5
        vm.set_wide_reg(0, 1_000_000_000_000u64).unwrap();
        vm.set_wide_reg(2, 2_000_000_000_000u64).unwrap();
        let insns = [
            OP_ADD_LONG, 0x04, 0x00, 0x02,
            OP_RETURN_WIDE, 0x04,
        ];
        let res = vm.interpret_insns(&dex, 6, &insns).unwrap();
        let wide_res = vm.get_wide_reg(4);
        assert_eq!(wide_res, 3_000_000_000_000u64);
        assert_eq!(res, (3_000_000_000_000u64 & 0xFFFFFFFF) as u32);

        // 2. Long 2addr subtraction: v0:v1 -= v2:v3
        let insns_sub = [
            OP_SUB_LONG_2ADDR, 0x20, // v_a = 0, v_b = 2
            OP_RETURN_VOID,
        ];
        vm.interpret_insns(&dex, 4, &insns_sub).unwrap();
        assert_eq!(vm.get_wide_reg(0) as i64, -1_000_000_000_000i64);

        // 3. Double multiplication: v0:v1 (2.5) * v2:v3 (4.0) -> v4:v5 (10.0)
        vm.set_wide_reg(0, 2.5f64.to_bits()).unwrap();
        vm.set_wide_reg(2, 4.0f64.to_bits()).unwrap();
        let insns_mul = [
            OP_MUL_DOUBLE, 0x04, 0x00, 0x02,
            OP_RETURN_VOID,
        ];
        vm.interpret_insns(&dex, 6, &insns_mul).unwrap();
        assert_eq!(f64::from_bits(vm.get_wide_reg(4)), 10.0f64);

        // 4. Double comparisons (cmpl-double and cmpg-double)
        vm.set_wide_reg(0, 1.5f64.to_bits()).unwrap();
        vm.set_wide_reg(2, 3.5f64.to_bits()).unwrap();
        let insns_cmp = [
            OP_CMPL_DOUBLE, 0x04, 0x00, 0x02,
            OP_RETURN, 0x04,
        ];
        let cmp_val = vm.interpret_insns(&dex, 6, &insns_cmp).unwrap() as i32;
        assert_eq!(cmp_val, -1);

        // 5. Long comparison (cmp-long)
        vm.set_wide_reg(0, 500u64).unwrap();
        vm.set_wide_reg(2, 500u64).unwrap();
        let insns_cmpl = [
            OP_CMP_LONG, 0x04, 0x00, 0x02,
            OP_RETURN, 0x04,
        ];
        let cmp_long_val = vm.interpret_insns(&dex, 6, &insns_cmpl).unwrap() as i32;
        assert_eq!(cmp_long_val, 0);
    }

    #[test]
    fn test_dalvik_full_conversions_and_casts() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "compute", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. int-to-long: v0 (12345) -> v2:v3
        vm.registers[0] = 12345;
        let insns = [
            OP_INT_TO_LONG, 0x02, // v_dest=2, v_src=0
            OP_RETURN_VOID,
        ];
        vm.interpret_insns(&dex, 4, &insns).unwrap();
        assert_eq!(vm.get_wide_reg(2), 12345u64);

        // 2. long-to-int: v2:v3 (12345) -> v4
        let insns2 = [
            OP_LONG_TO_INT, 0x24, // v_dest=4, v_src=2
            OP_RETURN, 0x04,
        ];
        let int_val = vm.interpret_insns(&dex, 5, &insns2).unwrap();
        assert_eq!(int_val, 12345);

        // 3. int-to-double: v0 (42) -> v2:v3 (42.0)
        vm.registers[0] = 42;
        let insns3 = [
            OP_INT_TO_DOUBLE, 0x02,
            OP_RETURN_VOID,
        ];
        vm.interpret_insns(&dex, 4, &insns3).unwrap();
        assert_eq!(f64::from_bits(vm.get_wide_reg(2)), 42.0f64);

        // 4. check-cast valid: HashMap to Map
        let map_id = vm.allocate_object("Ljava/util/HashMap;").unwrap();
        vm.registers[0] = map_id;
        let insns_cast = [
            OP_CHECK_CAST, 0x00, 0x01, 0x00, // v0 against type 1 (Ljava/lang/Object;)
            OP_RETURN, 0x00,
        ];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns_cast).unwrap(), map_id);

        // 5. instance-of: v0 is instance of Object -> v1 = 1
        let insns_inst = [
            OP_INSTANCE_OF, 0x01, 0x01, 0x00, // v_dest=1, v_src=0, type 1 (Object)
            OP_RETURN, 0x01,
        ];
        assert_eq!(vm.interpret_insns(&dex, 2, &insns_inst).unwrap(), 1);
    }

    #[test]
    fn test_dalvik_typed_arrays_and_filled_new_array() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "compute", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. filled-new-array with 3 elements: v0=10, v1=20, v2=30
        vm.registers[0] = 10;
        vm.registers[1] = 20;
        vm.registers[2] = 30;
        let insns = [
            OP_FILLED_NEW_ARRAY, 0x30, 0x00, 0x00, 0x10, 0x02,
            OP_MOVE_RESULT, 0x03,
            OP_RETURN, 0x03,
        ];
        let arr_id = vm.interpret_insns(&dex, 4, &insns).unwrap();
        assert!(arr_id > 0);
        let obj = vm.get_object(arr_id).unwrap();
        let elems = obj.array_elements.as_ref().unwrap();
        assert_eq!(elems.len(), 3);
        assert_eq!(elems[0], 10);
        assert_eq!(elems[1], 20);
        assert_eq!(elems[2], 30);

        // 2. Wide array aput-wide and aget-wide
        let wide_arr = vm.allocate_array("[J", 4).unwrap();
        vm.registers[0] = wide_arr;
        vm.registers[1] = 2; // index 2
        vm.set_wide_reg(2, 0x0123_4567_89AB_CDEFu64).unwrap(); // val in v2:v3
        let insns_wide = [
            OP_APUT_WIDE, 0x02, 0x00, 0x01, // aput-wide v2 into arr v0 at idx v1
            OP_AGET_WIDE, 0x04, 0x00, 0x01, // aget-wide v4 from arr v0 at idx v1
            OP_RETURN_VOID,
        ];
        vm.interpret_insns(&dex, 6, &insns_wide).unwrap();
        assert_eq!(vm.get_wide_reg(4), 0x0123_4567_89AB_CDEFu64);
    }

    #[test]
    fn test_vm_framework_dispatch_math_and_system() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "compute", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. Math.abs(-999) -> 999
        let res = vm.dispatch_framework_method(&dex, "Ljava/lang/Math;", "abs", &[0], &[(-999i32) as u32]).unwrap();
        assert_eq!(res, 999);

        // 2. Math.max(15, 88) -> 88
        let res = vm.dispatch_framework_method(&dex, "Ljava/lang/Math;", "max", &[0, 1], &[15, 88]).unwrap();
        assert_eq!(res, 88);

        // 3. System.currentTimeMillis() -> timestamp > 0
        let res = vm.dispatch_framework_method(&dex, "Ljava/lang/System;", "currentTimeMillis", &[], &[]).unwrap();
        assert!(res > 0);

        // 4. System.arraycopy
        let src = vm.allocate_array("[I", 5).unwrap();
        let dst = vm.allocate_array("[I", 5).unwrap();
        {
            let s_obj = vm.get_object_mut(src).unwrap();
            s_obj.array_elements.as_mut().unwrap().copy_from_slice(&[10, 20, 30, 40, 50]);
        }
        vm.dispatch_framework_method(&dex, "Ljava/lang/System;", "arraycopy", &[0, 1, 2, 3, 4], &[src, 1, dst, 2, 3]).unwrap();
        let d_obj = vm.get_object(dst).unwrap();
        let d_elems = d_obj.array_elements.as_ref().unwrap();
        assert_eq!(d_elems, &[0, 0, 20, 30, 40]);
    }

    #[test]
    fn test_vm_framework_dispatch_collections_and_parcel() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "compute", "test");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. HashMap operations: new, put, get, containsKey, size, remove
        let map_id = vm.allocate_object("Ljava/util/HashMap;").unwrap();
        vm.dispatch_framework_method(&dex, "Ljava/util/HashMap;", "<init>", &[0], &[map_id]).unwrap();
        let k1 = vm.allocate_string("itemKey").unwrap();
        let v1 = vm.allocate_string("itemValue").unwrap();
        vm.dispatch_framework_method(&dex, "Ljava/util/HashMap;", "put", &[0, 1, 2], &[map_id, k1, v1]).unwrap();

        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/HashMap;", "size", &[0], &[map_id]).unwrap(), 1);
        assert_eq!(vm.dispatch_framework_method(&dex, "Ljava/util/HashMap;", "containsKey", &[0, 1], &[map_id, k1]).unwrap(), 1);
        let retrieved = vm.dispatch_framework_method(&dex, "Ljava/util/HashMap;", "get", &[0, 1], &[map_id, k1]).unwrap();
        assert_eq!(retrieved, v1);
        assert_eq!(vm.get_string_payload(retrieved).as_deref(), Some("itemValue"));

        // 2. SharedPreferences & Editor
        let sp_id = vm.allocate_object("Landroid/content/SharedPreferences;").unwrap();
        let ed_id = vm.dispatch_framework_method(&dex, "Landroid/content/SharedPreferences;", "edit", &[0], &[sp_id]).unwrap();
        let pref_key = vm.allocate_string("theme").unwrap();
        let pref_val = vm.allocate_string("dark").unwrap();
        vm.dispatch_framework_method(&dex, "Landroid/content/SharedPreferences$Editor;", "putString", &[0, 1, 2], &[ed_id, pref_key, pref_val]).unwrap();
        vm.dispatch_framework_method(&dex, "Landroid/content/SharedPreferences$Editor;", "apply", &[0], &[ed_id]).unwrap();

        let got_val = vm.dispatch_framework_method(&dex, "Landroid/content/SharedPreferences;", "getString", &[0, 1, 2], &[sp_id, pref_key, 0]).unwrap();
        assert_eq!(vm.get_string_payload(got_val).as_deref(), Some("dark"));

        // 3. Parcel serialization and deserialization roundtrip
        let p_id = vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "obtain", &[], &[]).unwrap();
        vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "writeInt", &[0, 1], &[p_id, 98765]).unwrap();
        let p_str = vm.allocate_string("ParcelTestString").unwrap();
        vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "writeString", &[0, 1], &[p_id, p_str]).unwrap();

        // Rewind parcel position to 0
        vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "setDataPosition", &[0, 1], &[p_id, 0]).unwrap();
        let read_i = vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "readInt", &[0], &[p_id]).unwrap();
        assert_eq!(read_i, 98765);
        let read_s = vm.dispatch_framework_method(&dex, "Landroid/os/Parcel;", "readString", &[0], &[p_id]).unwrap();
        assert_eq!(vm.get_string_payload(read_s).as_deref(), Some("ParcelTestString"));
    }

    #[test]
    fn test_dalvik_monitor_synchronization_reentrancy_and_mismatched_unlock() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "test", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        let obj_id = vm.allocate_object("Ljava/lang/Object;").unwrap();
        vm.registers[0] = obj_id;

        // Bytecode: monitor-enter v0, monitor-enter v0, monitor-exit v0, monitor-exit v0, return-void
        let insns = [
            OP_MONITOR_ENTER, 0x00,
            OP_MONITOR_ENTER, 0x00,
            OP_MONITOR_EXIT, 0x00,
            OP_MONITOR_EXIT, 0x00,
            OP_RETURN_VOID, 0x00,
        ];

        let res = vm.interpret_insns(&dex, 2, &insns);
        assert!(res.is_ok(), "Reentrant monitor enter/exit must succeed");
        assert!(vm.monitors.is_empty(), "All monitors must be cleared after matching exits");

        // Test mismatched unlock throws IllegalMonitorStateException
        let bad_insns = [
            OP_MONITOR_EXIT, 0x00,
            OP_RETURN_VOID, 0x00,
        ];
        let err = vm.interpret_insns(&dex, 2, &bad_insns).unwrap_err();
        match err {
            DalvikVmError::UnhandledException(ex_id) => {
                let obj = vm.get_object(ex_id).unwrap();
                assert_eq!(obj.class_descriptor, "Ljava/lang/IllegalMonitorStateException;");
            }
            other => panic!("Expected IllegalMonitorStateException, got {:?}", other),
        }

        // Test null monitor enter throws NullPointerException
        vm.registers[0] = 0;
        let null_insns = [
            OP_MONITOR_ENTER, 0x00,
            OP_RETURN_VOID, 0x00,
        ];
        let err2 = vm.interpret_insns(&dex, 2, &null_insns).unwrap_err();
        match err2 {
            DalvikVmError::UnhandledException(ex_id) => {
                let obj = vm.get_object(ex_id).unwrap();
                assert_eq!(obj.class_descriptor, "Ljava/lang/NullPointerException;");
            }
            other => panic!("Expected NullPointerException, got {:?}", other),
        }
    }

    #[test]
    fn test_dalvik_runtime_type_checking_checkcast_and_instanceof_hierarchy() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "test", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let vm = DalvikVmState::new();

        // Exact match
        assert!(vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/lang/String;"));
        // Superclass Object
        assert!(vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/lang/Object;"));
        // Interfaces for String
        assert!(vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/lang/CharSequence;"));
        assert!(vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/io/Serializable;"));
        assert!(vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/lang/Comparable;"));

        // Interfaces and superclass for ArrayList
        assert!(vm.is_type_compatible(&dex, "Ljava/util/ArrayList;", "Ljava/util/List;"));
        assert!(vm.is_type_compatible(&dex, "Ljava/util/ArrayList;", "Ljava/util/Collection;"));
        assert!(vm.is_type_compatible(&dex, "Ljava/util/ArrayList;", "Ljava/lang/Iterable;"));
        assert!(vm.is_type_compatible(&dex, "Ljava/util/ArrayList;", "Ljava/lang/Object;"));

        // Incompatible types fail closed
        assert!(!vm.is_type_compatible(&dex, "Ljava/lang/String;", "Ljava/util/List;"));
        assert!(!vm.is_type_compatible(&dex, "Ljava/util/HashMap;", "Ljava/util/List;"));
        assert!(!vm.is_type_compatible(&dex, "Landroid/content/Context;", "Ljava/util/Map;"));
    }

    #[test]
    fn test_dalvik_array_store_exception_and_wide_register_bounds() {
        let dex_bytes = create_test_dex("Lorg/interos/Test;", "test", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // Create String array: [Ljava/lang/String;
        let arr_id = vm.allocate_array("[Ljava/lang/String;", 2).unwrap();
        let str_id = vm.allocate_string("valid_str").unwrap();
        let int_id = vm.allocate_object("Ljava/lang/Integer;").unwrap();

        vm.registers[0] = str_id;
        vm.registers[1] = arr_id;
        vm.registers[2] = 0; // index 0

        // aput-object v0, v1, v2
        let valid_aput = [
            OP_APUT_OBJECT, 0x00, 0x01, 0x02,
            OP_RETURN_VOID, 0x00,
        ];
        assert!(vm.interpret_insns(&dex, 4, &valid_aput).is_ok());

        // Now attempt storing Integer into String array: aput-object v0 (with Integer), v1, v2
        vm.registers[0] = int_id;
        let err = vm.interpret_insns(&dex, 4, &valid_aput).unwrap_err();
        match err {
            DalvikVmError::UnhandledException(ex_id) => {
                let obj = vm.get_object(ex_id).unwrap();
                assert_eq!(obj.class_descriptor, "Ljava/lang/ArrayStoreException;");
            }
            other => panic!("Expected ArrayStoreException, got {:?}", other),
        }
    }

    #[test]
    fn test_dalvik_multi_level_virtual_dispatch() {
        let dex_bytes = create_test_dex("Landroid/view/View;", "getId", "()I");
        let mut dex = DexPackage::parse(&dex_bytes).unwrap();

        // Grandparent defines getId() -> returns 42
        dex.methods.push(DexMethod {
            method_index: dex.methods.len(),
            class_name: String::from("Landroid/view/View;"),
            method_name: String::from("getId"),
            proto_shorty: String::from("I"),
            is_direct: false,
            code_offset: 0,
            registers_size: 1,
            ins_size: 0,
            outs_size: 0,
            instruction_count: 2,
        });

        let vm = DalvikVmState::new();
        // Landroid/widget/Button; -> TextView -> View (defined in framework hierarchy)
        let resolved = vm.resolve_virtual_method(&dex, "Landroid/widget/Button;", "getId", "I");
        assert!(resolved.is_some(), "Multi-level virtual dispatch must find View::getId from Button receiver");
        assert_eq!(resolved.unwrap().class_name, "Landroid/view/View;");
    }

    #[test]
    fn test_dalvik_monitor_reentrant_and_contention() {
        let dex_bytes = create_test_dex("Lcom/example/SyncTest;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        let obj_id = vm.allocate_string("monitor_target").unwrap();
        vm.registers[0] = obj_id;

        // Thread 1 acquires monitor
        vm.set_current_thread(1);
        let insns_enter = [OP_MONITOR_ENTER, 0x00, OP_RETURN_VOID, 0x00];
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());

        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 1);
        assert_eq!(mon.recursion_count, 1);

        // Reentrant enter: count becomes 2
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());
        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.recursion_count, 2);

        // Reentrant exit: count becomes 1, still owned by Thread 1
        let insns_exit = [OP_MONITOR_EXIT, 0x00, OP_RETURN_VOID, 0x00];
        assert!(vm.interpret_insns(&dex, 2, &insns_exit).is_ok());
        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 1);
        assert_eq!(mon.recursion_count, 1);

        // Thread 2 attempts to acquire: contention -> MonitorBlocked(obj_id)
        vm.spawn_thread(2, "Worker-1");
        vm.set_current_thread(2);
        vm.registers[0] = obj_id;
        let err = vm.interpret_insns(&dex, 2, &insns_enter).unwrap_err();
        assert_eq!(err, DalvikVmError::MonitorBlocked(obj_id));

        // Thread 2 status must be BlockedOnMonitor(obj_id)
        let t2 = vm.threads.get(&2).unwrap();
        assert_eq!(t2.status, JavaThreadStatus::BlockedOnMonitor(obj_id));

        // Thread 1 exits: ownership transfers to Thread 2!
        vm.set_current_thread(1);
        assert!(vm.interpret_insns(&dex, 2, &insns_exit).is_ok());

        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 2);
        assert_eq!(mon.recursion_count, 1);

        let t2 = vm.threads.get(&2).unwrap();
        assert_eq!(t2.status, JavaThreadStatus::Runnable);
    }

    #[test]
    fn test_dalvik_object_wait_and_notify() {
        let mut vm = DalvikVmState::new();
        let obj_id = vm.allocate_string("cond_var").unwrap();

        vm.spawn_thread(10, "Producer");
        vm.spawn_thread(20, "Consumer");

        // Consumer enters monitor
        vm.set_current_thread(20);
        vm.registers[0] = obj_id;
        let dex_bytes = create_test_dex("Lcom/example/CondTest;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let insns_enter = [OP_MONITOR_ENTER, 0x00, OP_RETURN_VOID, 0x00];
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());

        // Consumer calls monitor_wait: releases lock and joins cond_waiters
        assert!(vm.monitor_wait(obj_id, 0).is_ok());
        let consumer = vm.threads.get(&20).unwrap();
        assert_eq!(consumer.status, JavaThreadStatus::WaitingOnCondition(obj_id));

        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 0); // Lock is free
        assert_eq!(mon.cond_waiters, alloc::vec![20]);

        // Producer enters monitor
        vm.set_current_thread(10);
        vm.registers[0] = obj_id;
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());

        // Producer notifies: Consumer moves from cond_waiters to wait_queue
        assert_eq!(vm.monitor_notify(obj_id, false), Ok(0));
        let mon = vm.monitors.get(&obj_id).unwrap();
        assert!(mon.cond_waiters.is_empty());
        assert_eq!(mon.wait_queue, alloc::vec![20]);

        // Producer exits: Consumer acquires lock and becomes Runnable
        let insns_exit = [OP_MONITOR_EXIT, 0x00, OP_RETURN_VOID, 0x00];
        assert!(vm.interpret_insns(&dex, 2, &insns_exit).is_ok());

        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 20);
        let consumer = vm.threads.get(&20).unwrap();
        assert_eq!(consumer.status, JavaThreadStatus::Runnable);
    }

    #[test]
    fn test_dalvik_thread_exit_cleanup_releases_monitors() {
        let mut vm = DalvikVmState::new();
        let obj_id = vm.allocate_string("shared_resource").unwrap();

        vm.spawn_thread(100, "Worker");
        vm.spawn_thread(200, "Waiter");

        vm.set_current_thread(100);
        vm.registers[0] = obj_id;
        let dex_bytes = create_test_dex("Lcom/example/ExitTest;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let insns_enter = [OP_MONITOR_ENTER, 0x00, OP_RETURN_VOID, 0x00];
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());

        // Waiter attempts acquisition and gets blocked
        vm.set_current_thread(200);
        vm.registers[0] = obj_id;
        assert_eq!(vm.interpret_insns(&dex, 2, &insns_enter), Err(DalvikVmError::MonitorBlocked(obj_id)));

        // Worker terminates unexpectedly
        vm.thread_exit(100);

        // Lock must have transferred to Waiter
        let mon = vm.monitors.get(&obj_id).unwrap();
        assert_eq!(mon.owner_thread, 200);
        let waiter = vm.threads.get(&200).unwrap();
        assert_eq!(waiter.status, JavaThreadStatus::Runnable);
    }

    #[test]
    fn test_cooperative_scheduler_slices_and_round_robin() {
        let mut vm = DalvikVmState::new();
        // Initial thread is main (tid 1)
        assert_eq!(vm.current_thread_id, 1);
        assert_eq!(vm.run_queue.len(), 1);

        // Spawn two additional worker threads
        vm.spawn_thread(2, "Worker-1");
        vm.spawn_thread(3, "Worker-2");
        assert_eq!(vm.run_queue.len(), 3);

        // Step 3 quanta: threads 1, 2, 3 take turns in round-robin order
        let steps = vm.step_scheduler_slice(3, 100);
        assert_eq!(steps, 3);
        assert_eq!(vm.current_thread_id, 3);
        assert_eq!(vm.now_ticks, 100);

        // All 3 threads remain runnable and re-enqueued
        assert_eq!(vm.run_queue.len(), 3);
    }

    #[test]
    fn test_timed_waiting_and_scheduler_wakeup() {
        let mut vm = DalvikVmState::new();
        vm.spawn_thread(10, "SleepingThread");
        vm.set_current_thread(10);

        // Thread 10 sleeps until tick 500
        vm.threads.get_mut(&10).unwrap().status = JavaThreadStatus::TimedWaiting(0, 500);
        vm.run_queue.retain(|&tid| tid != 10);

        // Tick 200: thread 10 should still be sleeping
        vm.wake_timed_waiters(200);
        assert_eq!(vm.threads.get(&10).unwrap().status, JavaThreadStatus::TimedWaiting(0, 500));
        assert!(!vm.run_queue.contains(&10));

        // Tick 500: thread 10 should wake up and become Runnable in run_queue
        vm.wake_timed_waiters(500);
        assert_eq!(vm.threads.get(&10).unwrap().status, JavaThreadStatus::Runnable);
        assert!(vm.run_queue.contains(&10));
    }

    #[test]
    fn test_thread_framework_dispatch_lifecycle() {
        let mut vm = DalvikVmState::new();
        let dex_bytes = create_test_dex("Lcom/example/ThreadTest;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();

        // 1. Thread.start creates new thread and enqueues it
        let start_res = vm.dispatch_framework_method(&dex, "java/lang/Thread", "start", &[], &[]);
        assert!(start_res.is_ok());
        let new_tid = start_res.unwrap() as u64;
        assert!(new_tid > 1);
        assert!(vm.threads.contains_key(&new_tid));
        assert!(vm.run_queue.contains(&new_tid));

        // 2. Thread.sleep puts current thread to TimedWaiting
        let sleep_res = vm.dispatch_framework_method(&dex, "java/lang/Thread", "sleep", &[], &[50]);
        assert_eq!(sleep_res, Ok(0));
        assert!(matches!(vm.threads.get(&1).unwrap().status, JavaThreadStatus::TimedWaiting(0, _)));

        // 3. Thread.yield re-enqueues runnable
        vm.threads.get_mut(&1).unwrap().status = JavaThreadStatus::Runnable;
        let yield_res = vm.dispatch_framework_method(&dex, "java/lang/Thread", "yield", &[], &[]);
        assert_eq!(yield_res, Ok(0));
    }

    #[test]
    fn test_dalvik_extended_opcodes() {
        let dex_bytes = create_test_dex("Lorg/interos/ExtendedOps;", "test", "()I");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        // 1. OP_CONST (0x14) loads full 32-bit integer: v0 = 0x12345678
        let insns_const = [
            OP_CONST, 0x00, 0x78, 0x56, 0x34, 0x12,
            OP_RETURN, 0x00,
        ];
        let res = vm.interpret_insns(&dex, 2, &insns_const).unwrap();
        assert_eq!(res, 0x1234_5678);

        // 2. OP_GOTO_32 (0x2A): jumps 4 code units (+8 bytes) ahead past an unreachable trap
        let insns_goto32 = [
            OP_CONST_4, 0x10, // v0 = 1 (2 bytes, pc: 0..2)
            OP_GOTO_32, 0x00, 0x04, 0x00, 0x00, 0x00, // jump +4 code units (+8 bytes) from pc=2 to pc=10
            OP_CONST_4, 0x90, // v0 = -7 (skipped, 2 bytes, pc: 8..10)
            OP_RETURN, 0x00,  // pc: 10..12
        ];
        let res = vm.interpret_insns(&dex, 2, &insns_goto32).unwrap();
        assert_eq!(res, 1);

        // 3. OP_RETURN_OBJECT (0x11): returns object ID and shadow string
        let obj_id = vm.allocate_string("test").unwrap();
        vm.registers[0] = obj_id;
        let insns_ret_obj = [
            OP_RETURN_OBJECT, 0x00,
        ];
        let ret_obj = vm.interpret_insns(&dex, 2, &insns_ret_obj).unwrap();
        assert_eq!(ret_obj, obj_id);
        assert_eq!(vm.last_return_string.as_deref(), Some("test"));

        // 4. OP_MOVE_WIDE_FROM16 (0x05) and OP_MOVE_WIDE_16 (0x06)
        let insns_wide_moves = [
            OP_CONST_WIDE_32, 0x02, 0xEF, 0xBE, 0xAD, 0xDE, // v2:v3 = 0xDEADBEEF
            OP_MOVE_WIDE_FROM16, 0x00, 0x02, 0x00,          // v0:v1 = v2:v3
            OP_MOVE_WIDE_16, 0x00, 0x04, 0x00, 0x00, 0x00,  // v4:v5 = v0:v1
            OP_RETURN_WIDE, 0x04,
        ];
        let ret_wide = vm.interpret_insns(&dex, 6, &insns_wide_moves).unwrap();
        assert_eq!(ret_wide, 0xDEAD_BEEF);

        // 5. OP_MOVE_OBJECT_FROM16 (0x08) and OP_MOVE_OBJECT_16 (0x09)
        vm.registers[2] = obj_id;
        let insns_obj_moves = [
            OP_MOVE_OBJECT_FROM16, 0x00, 0x02, 0x00,       // v0 = v2
            OP_MOVE_OBJECT_16, 0x00, 0x04, 0x00, 0x00, 0x00, // v4 = v0
            OP_RETURN_OBJECT, 0x04,
        ];
        let ret_obj2 = vm.interpret_insns(&dex, 6, &insns_obj_moves).unwrap();
        assert_eq!(ret_obj2, obj_id);
        assert_eq!(vm.last_return_string.as_deref(), Some("test"));

        // 6. Wide unary ops: OP_NEG_LONG (0x7D), OP_NOT_LONG (0x7E), OP_NEG_DOUBLE (0x80)
        let insns_neg_long = [
            OP_CONST_WIDE_32, 0x00, 0x2A, 0x00, 0x00, 0x00, // v0:v1 = 42
            OP_NEG_LONG, 0x02,                              // v2:v3 = -42
            OP_RETURN_WIDE, 0x02,
        ];
        vm.interpret_insns(&dex, 4, &insns_neg_long).unwrap();
        assert_eq!(vm.get_wide_reg(2) as i64, -42);

        let insns_not_long = [
            OP_CONST_WIDE_32, 0x00, 0x00, 0x00, 0x00, 0x00, // v0:v1 = 0
            OP_NOT_LONG, 0x02,                              // v2:v3 = !0 = ~0
            OP_RETURN_WIDE, 0x02,
        ];
        vm.interpret_insns(&dex, 4, &insns_not_long).unwrap();
        assert_eq!(vm.get_wide_reg(2), !0u64);
    }

    #[test]
    fn test_heap_quota_and_gc_scaling() {
        let mut vm = DalvikVmState::new();

        // Under the old 256 limit, allocating 500 objects would fail.
        // With the 16MB quota, allocating 500 objects succeeds cleanly!
        let mut live_ids = Vec::new();
        for _ in 0..500 {
            let id = vm.allocate_object("Ljava/lang/String;").unwrap();
            live_ids.push(id);
        }
        assert_eq!(vm.objects.len(), 500);
        assert!(vm.heap_used_bytes > 0 && vm.heap_used_bytes < vm.heap_max_bytes);

        // Keep 10 roots in static fields, rest become garbage
        for i in 0..10 {
            vm.set_static_field(i as u16, live_ids[i]).unwrap();
        }
        // Force GC
        let (reclaimed, pause) = vm.gc_mark_and_sweep();
        assert_eq!(reclaimed, 490);
        assert_eq!(vm.objects.len(), 10);
        assert!(pause > 0);
        assert!(vm.heap_used_bytes < 5000);

        // Verify ceiling enforcement
        vm.heap_max_bytes = 100; // Artificially constrain quota
        let err = vm.allocate_object("Ljava/lang/String;").unwrap_err();
        assert_eq!(err, DalvikVmError::ObjectHeapLimitExceeded);
    }

    #[test]
    fn test_dalvik_two_thread_producer_consumer_wait_notify_and_isolation() {
        let dex_bytes = create_test_dex("Lorg/interos/SyncTest;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let mut vm = DalvikVmState::new();

        let lock_obj = vm.allocate_object("Ljava/lang/Object;").unwrap();

        // Main thread (tid=1) acquires lock
        vm.set_current_thread(1);
        let insns_enter = [OP_MONITOR_ENTER, 0x00, OP_RETURN_VOID, 0x00];
        vm.registers[0] = lock_obj;
        assert!(vm.interpret_insns(&dex, 2, &insns_enter).is_ok());

        // Spawn consumer thread (tid=2)
        vm.spawn_thread(2, "Consumer");
        vm.set_current_thread(2);
        vm.registers[0] = lock_obj;

        // Thread 2 attempts to enter held monitor -> must fail with MonitorBlocked and save PC
        let err = vm.interpret_insns(&dex, 2, &insns_enter).unwrap_err();
        assert_eq!(err, DalvikVmError::MonitorBlocked(lock_obj));
        assert!(matches!(vm.threads.get(&2).unwrap().status, JavaThreadStatus::BlockedOnMonitor(id) if id == lock_obj));

        // Switch back to thread 1 and exit monitor
        vm.set_current_thread(1);
        let insns_exit = [OP_MONITOR_EXIT, 0x00, OP_RETURN_VOID, 0x00];
        vm.registers[0] = lock_obj;
        assert!(vm.interpret_insns(&dex, 2, &insns_exit).is_ok());

        // Lock ownership has transferred to thread 2, and thread 2 is now Runnable with advanced PC
        let mon = vm.monitors.get(&lock_obj).unwrap();
        assert_eq!(mon.owner_thread, 2);
        assert_eq!(vm.threads.get(&2).unwrap().status, JavaThreadStatus::Runnable);
        assert_eq!(vm.threads.get(&2).unwrap().pc, 2);

        // Step scheduler: thread 2 executes its slice in complete isolation
        vm.step_scheduler_slice(2, 50);
        assert_eq!(vm.current_thread_id, 2);
    }

    #[test]
    fn scheduler_with_dex_executes_bytecode_while_context_only_does_not() {
        // WHAT THIS IS FOR: the ACR service loop must drive the bytecode-executing
        // scheduler, not the context-only one. This pins the difference on the
        // same runnable thread: `step_scheduler_slice` switches context but never
        // advances the PC, while `step_scheduler_slice_with_dex` interprets the
        // thread's `return-void` and terminates it.
        let dex_bytes = create_test_dex("Lorg/interos/Sched;", "run", "()V");
        let dex = DexPackage::parse(&dex_bytes).unwrap();
        let code_off = dex.methods[0].code_offset;
        let reg_sz = dex.methods[0].registers_size;
        assert!(code_off > 0, "the fixture method must carry a code item");

        // Context-only path: the thread is switched but its PC never moves and
        // it stays Runnable -- no bytecode executed.
        let mut ctx_only = DalvikVmState::new();
        ctx_only.spawn_thread(1, "T1");
        {
            let t = ctx_only.threads.get_mut(&1).unwrap();
            t.code_offset = code_off;
            t.registers_size = reg_sz;
        }
        assert_eq!(ctx_only.step_scheduler_slice(1, 10), 1);
        assert_eq!(ctx_only.threads.get(&1).unwrap().pc, 0);
        assert!(matches!(
            ctx_only.threads.get(&1).unwrap().status,
            JavaThreadStatus::Runnable
        ));

        // DEX path: the same thread interprets its instruction and terminates.
        let mut with_dex = DalvikVmState::new();
        with_dex.spawn_thread(1, "T1");
        {
            let t = with_dex.threads.get_mut(&1).unwrap();
            t.code_offset = code_off;
            t.registers_size = reg_sz;
        }
        assert_eq!(with_dex.step_scheduler_slice_with_dex(&dex, 1, 10), 1);
        assert!(
            matches!(
                with_dex.threads.get(&1).unwrap().status,
                JavaThreadStatus::Terminated
            ),
            "the DEX scheduler must execute the thread's return-void and terminate it"
        );
    }
}


