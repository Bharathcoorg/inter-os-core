//! Android Compatibility Runtime & Zone 4 Isolation
//!
//! Provides the execution boundary for untrusted Android application packages (.apk).
//! Employs a zero-trust mediator that strictly walls off Android workloads from
//! native Inter OS private storage, sovereign cryptographic keys, and raw microkernel IPC.
pub use interos_android_shared;
pub mod apk;
pub mod binder;
pub mod bridge;
pub mod dex;
pub mod manifest;
pub mod runtime;
pub mod vfs;

pub use apk::{AcrAuditStatus, AcrContainerProfile, ApkError, ApkPackage, create_test_apk};
pub use bridge::{AndroidBridgeError, AndroidIntent, AndroidIntentBridge, SovereignIntent};
pub use dex::{DexError, DexMethod, DexPackage, compute_adler32, create_test_dex};
pub use runtime::{DalvikVmError, DalvikVmState};

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

pub use binder::{BinderCommand, BinderMediator, BinderReturn, BinderTransaction, BINDER_MEDIATOR};
pub use manifest::AndroidManifest;
pub use vfs::{AndroidFsError, AndroidSandboxFs, ANDROID_SANDBOX_FS};

use crate::capability::CapabilityTable;
use crate::serial_println;
use crate::task::TaskId;

/// Represents an isolated Android guest application instance.
#[derive(Debug, Clone)]
pub struct AndroidAppInstance {
    pub pid: u32,
    pub package_name: String,
    pub target_sdk: u32,
    pub task_id: TaskId,
    pub cap_table: CapabilityTable,
    pub memory_quota_bytes: usize,
}

/// System-wide Android container runtime manager.
pub struct AndroidRuntime {
    next_pid: u32,
    active_apps: BTreeMap<u32, AndroidAppInstance>,
}

impl AndroidRuntime {
    /// Creates an empty Android runtime.
    pub const fn new() -> Self {
        Self {
            next_pid: 1000,
            active_apps: BTreeMap::new(),
        }
    }

    /// Spawns a sandboxed Android application inside the container runtime.
    pub fn spawn_app(
        &mut self,
        manifest: &AndroidManifest,
        task_id: TaskId,
        cap_table: CapabilityTable,
    ) -> u32 {
        let pid = self.next_pid;
        self.next_pid += 1;

        // Populate app-specific sandboxed directories in Android virtual VFS
        let mut fs = ANDROID_SANDBOX_FS.lock();
        let app_data_dir = alloc::format!("/data/data/{}", manifest.package_name);
        let app_files_dir = alloc::format!("/sdcard/Android/data/{}/files", manifest.package_name);
        fs.create_directory(&app_data_dir);
        fs.create_directory(&app_files_dir);
        drop(fs);

        self.active_apps.insert(
            pid,
            AndroidAppInstance {
                pid,
                package_name: manifest.package_name.clone(),
                target_sdk: manifest.target_sdk,
                task_id,
                cap_table,
                memory_quota_bytes: 64 * 1024 * 1024, // 64 MiB container ceiling
            },
        );

        pid
    }

    /// Terminates an active Android application instance.
    pub fn terminate_app(&mut self, pid: u32) -> bool {
        self.active_apps.remove(&pid).is_some()
    }

    /// Returns the number of currently running Android container applications.
    pub fn active_app_count(&self) -> usize {
        self.active_apps.len()
    }
}

/// Global system-wide Android container runtime.
pub static ANDROID_RUNTIME: Mutex<AndroidRuntime> = Mutex::new(AndroidRuntime::new());

/// Initializes the Android compatibility runtime and executes self-tests.
pub fn init() {
    serial_println!("\n--- [Initializing Android Compatibility Runtime (Zone 4 Isolation)] ---");

    // 1. Initialize Virtual Filesystem
    let mut fs = ANDROID_SANDBOX_FS.lock();
    fs.populate_defaults();
    let framework_jar = fs.open_read("/system/framework/framework.jar");
    assert!(framework_jar.is_ok(), "Android framework.jar must be present in virtual VFS");
    serial_println!("[OK] Android virtual filesystem hierarchy mounted (/system, /data, /sdcard).");

    // 2. Test Non-Leakage Boundary: Access to native host private storage MUST FAIL
    let leak_test = fs.open_read("/system/etc/policy.json");
    assert_eq!(
        leak_test,
        Err(AndroidFsError::SecurityViolation),
        "Android process must NEVER access host policy partition!"
    );
    serial_println!("[TEST] Android Sandbox Boundary: Native host file access (/system/etc/policy.json) REJECTED.");
    drop(fs);

    // 3. Register standard Android system services in Binder Mediator
    let mut binder = BINDER_MEDIATOR.lock();
    binder.register_service("servicemanager", 0);
    binder.register_service("activity", 1);
    binder.register_service("package", 2);
    binder.register_service("telephony", 3);
    serial_println!("[OK] Android Binder IPC Mediator online: 4 virtual services registered.");

    // 4. Test Binder Transaction Capability Mediation
    let empty_caps = CapabilityTable::new(4);
    let unauthorized_tx = BinderTransaction {
        target_handle: 1, // 'activity' service
        code: 10,
        flags: 0,
        data: Vec::new(),
    };
    let tx_res = binder.dispatch_transaction(TaskId(99), &empty_caps, 0, &unauthorized_tx);
    assert_eq!(
        tx_res,
        Err(BinderReturn::BrSecurityDenied),
        "Unauthorized Binder transaction must be blocked by capability check!"
    );
    serial_println!("[TEST] Android Binder Capability Boundary: Unauthorized service transaction BLOCKED (BrSecurityDenied).");
    drop(binder);

    // 5. Test APK Manifest to Capability Mapping
    let mut manifest = AndroidManifest::new("com.swiggy.android", 101, 34);
    manifest.add_permission("android.permission.INTERNET");
    manifest.add_permission("android.permission.READ_EXTERNAL_STORAGE");
    let mapped_caps = manifest.map_to_capabilities(900);
    assert_eq!(mapped_caps.len(), 2, "Manifest should map 2 capabilities");
    serial_println!("[TEST] Android Manifest Permission Mapping: 2 capabilities strictly bounded (Zero Ambient Authority).");

    // 6. Test Cross-Zone Intent Bridge Translation & Security Traps
    let payment_intent = AndroidIntent::new("android.intent.action.VIEW", "upi://pay?pa=blinkit@axis&am=299");
    let mapped_pay = AndroidIntentBridge::translate_intent(&payment_intent);
    assert_eq!(mapped_pay, Ok("INTENT_PAY"));

    let dial_intent = AndroidIntent::new("android.intent.action.DIAL", "tel:112");
    let mapped_dial = AndroidIntentBridge::translate_intent(&dial_intent);
    assert_eq!(mapped_dial, Ok("INTENT_TELEPHONY_DIAL"));

    let exploit_intent = AndroidIntent::new("android.intent.action.VIEW", "file:///system/etc/policy.json");
    let exploit_res = AndroidIntentBridge::translate_intent(&exploit_intent);
    assert_eq!(exploit_res, Err(AndroidBridgeError::SecurityViolation), "Host filesystem paths must be blocked");
    serial_println!("[TEST] Android ACR Intent Bridge: 'android.intent.action.VIEW' -> sovereign intent mapped, host path exploit BLOCKED.");

    // 7. Test PKZip Container & Binary AndroidManifest.xml Parser
    let test_apk_data = create_test_apk("org.whispersystems.signal", &["android.permission.INTERNET", "android.permission.CAMERA"]);
    let parsed_apk = ApkPackage::parse(&test_apk_data).expect("APK parsing must succeed");
    assert_eq!(parsed_apk.manifest.package_name, "org.whispersystems.signal");
    assert!(parsed_apk.has_classes_dex);
    assert!(parsed_apk.is_signed);
    assert_eq!(parsed_apk.manifest.permissions.len(), 2);
    let signal_profile = parsed_apk.audit_and_create_container(900);
    assert_eq!(signal_profile.status, AcrAuditStatus::Approved);
    assert_eq!(signal_profile.granted_caps_count, 2);
    serial_println!("[TEST] Android APK Parser: Decoded binary AndroidManifest.xml for 'org.whispersystems.signal' (VERIFIED).");

    // 8. Test Ambient Authority Rejection on Malicious APK
    let rogue_apk_data = create_test_apk("com.rogue.exploit", &["android.permission.INSTALL_PACKAGES"]);
    let rogue_apk = ApkPackage::parse(&rogue_apk_data).expect("Rogue APK parsing must succeed");
    let rogue_profile = rogue_apk.audit_and_create_container(950);
    assert_eq!(rogue_profile.status, AcrAuditStatus::AmbientAuthorityDenied);
    serial_println!("[TEST] Android ACR Attestation: Rogue APK requesting ambient 'INSTALL_PACKAGES' REJECTED (Zero Ambient Authority).");

    // 9. Test Dalvik Executable (classes.dex) Extraction, Parsing & Bytecode Auditing
    let dex_payload = parsed_apk.extract_dex_bytes(&test_apk_data).expect("DEX extraction must succeed");
    let parsed_dex = DexPackage::parse(dex_payload).expect("DEX binary parsing must succeed");
    assert_eq!(parsed_dex.methods.len(), 1);
    assert_eq!(parsed_dex.methods[0].class_name, "Lorg/whispersystems/signal/MainActivity;");
    assert_eq!(parsed_dex.methods[0].method_name, "onCreate");
    let verified_insns = parsed_dex.audit_bytecode().expect("Bytecode audit must pass");
    assert!(verified_insns > 0, "Must have verified bytecode instructions");
    serial_println!("[TEST] Android DEX Parser: Decoded classes.dex, Adler-32 verified, 1 class, 1 method, bytecode audited (VERIFIED).");

    // 10. Test Rogue Dalvik Bytecode Rejection (Zero Ambient Authority)
    let rogue_dex_bytes = create_test_dex(
        "Lcom/exploit/Rogue;",
        "run",
        "java/lang/ProcessBuilder;->start()",
    );
    let rogue_dex = DexPackage::parse(&rogue_dex_bytes).expect("Rogue DEX parse must succeed");
    let audit_res = rogue_dex.audit_bytecode();
    assert_eq!(audit_res, Err(DexError::SecurityViolationForbiddenApi));
    serial_println!("[TEST] Android DEX Verifier: Rogue Dalvik bytecode invoking 'ProcessBuilder' REJECTED (Zero Ambient Authority).");

    // 11. Test Dalvik Virtual Machine Bytecode Execution Engine
    let mut vm = runtime::DalvikVmState::new();
    let exec_res = vm.invoke_method(&parsed_dex, "Lorg/whispersystems/signal/MainActivity;", "onCreate");
    assert!(exec_res.is_ok(), "Dalvik VM method execution must succeed");
    assert_eq!(vm.instructions_executed, 2, "Must have executed 2 Dalvik instructions (const-string, return-void)");
    assert_eq!(
        vm.string_registers[0].as_deref(),
        Some("Hello from Signal on Inter OS!"),
        "Register v0 must be bound to Signal greeting string"
    );
    serial_println!(
        "[TEST] Android Dalvik Runtime VM: Executed 'Lorg/whispersystems/signal/MainActivity;->onCreate()', {} opcodes, register v0 bound (VERIFIED).",
        vm.instructions_executed
    );

    serial_println!("--- [Android Compatibility Boundary Verified: APK/DEX/Binder/VFS] ---\n");
}
