//! Local Agentic AI Subsystem & Edge Model Runtime
//!
//! Provides the execution environment for local open-weight models as specified
//! in MASTER_PROMPT.md. Models are treated as unprivileged packages governed by
//! capability tokens, dynamic RAM budgets, and foreground task preemption.

pub mod manager;
pub mod model;
pub mod intent_parser;
pub mod embedding;
pub mod context_broker;
pub mod planner;
pub mod shield;

pub use manager::{ModelManager, MODEL_MANAGER};
pub use model::{ModelFamily, ModelPackage, ModelState, Quantization};
pub use intent_parser::{ActionDomain, ParsedIntent, parse_intent};
pub use embedding::{cosine_similarity, find_nearest, EmbeddingError, SemanticAnchor, EMBEDDING_DIM};
pub use context_broker::{ContextBroker, ContextEntry, PrivacyTier, CONTEXT_BROKER};
pub use planner::{AgentPlan, AgentPlanner, PlanError, PlanStatus, PlanStep, AGENT_PLANNER};
pub use shield::{PrivacyShield, PiiCategory};

use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::serial_println;
use crate::task::TaskId;

/// Initializes the local AI model manager and runs validation self-tests.
pub fn init() {
    serial_println!("\n--- [Initializing Local AI Model Manager & Edge Runtime] ---");

    let mut manager = MODEL_MANAGER.lock();

    // 1. Register local open-weight model packages
    let qwen_model = ModelPackage::new(
        "qwen2.5-0.5b-instruct",
        ModelFamily::Qwen,
        500, // 0.5B
        Quantization::Q4_K_M,
        32768,
        48 * 1024, // 48 KiB demo partition
        "pubkey:ed25519:alibaba_qwen_verified",
        "Apache-2.0",
    );

    let llama_model = ModelPackage::new(
        "llama3.2-1b-instruct",
        ModelFamily::Llama,
        1000, // 1.0B
        Quantization::Q4_K_M,
        131072,
        64 * 1024, // 64 KiB demo partition
        "pubkey:ed25519:meta_llama_verified",
        "Llama-3.2-Community",
    );

    manager.register_model(qwen_model);
    manager.register_model(llama_model);
    serial_println!("[OK] Registered 2 local open-weight models: 'qwen2.5-0.5b-instruct', 'llama3.2-1b-instruct'.");

    // 2. Test Dynamic Model Loading & RAM Budget Accounting
    assert_eq!(manager.ram_used(), 0, "Initial AI RAM usage must be 0");
    let load_res = manager.load_model("qwen2.5-0.5b-instruct");
    assert!(load_res.is_ok(), "Failed to dynamically load Qwen model");
    assert_eq!(manager.ram_used(), 48 * 1024, "RAM used must reflect Qwen allocation");
    serial_println!("[TEST] Dynamic AI Model Loading: 'qwen2.5-0.5b-instruct' loaded (48 KiB RAM allocated).");

    // 3. Test Multi-Model Loading within Budget
    let load_llama = manager.load_model("llama3.2-1b-instruct");
    assert!(load_llama.is_ok(), "Failed to load second model within budget");
    assert_eq!(manager.ram_used(), 112 * 1024, "Total RAM used must equal 112 KiB");
    serial_println!("[TEST] Multi-Model Memory Budgeting: Total RAM 112 KiB / 128 KiB ceiling.");

    // 4. Test Principle #8: Preempt AI for Foreground User Applications
    let freed = manager.preempt_ai_for_foreground_task();
    assert_eq!(freed, 112 * 1024, "Preemption must free all active AI RAM");
    assert_eq!(manager.ram_used(), 0, "Post-preemption RAM usage must return to 0");
    serial_println!("[TEST] Principle #8 Preemption: 112 KiB AI RAM freed immediately for foreground user task.");

    // 5. Test Zero-Trust Capability-Governed Intent Synthesis
    let mut caps = CapabilityTable::new(4);
    caps.insert(Capability {
        id: CapabilityId(501),
        cap_type: CapabilityType::IpcSend,
        rights: Rights::WRITE,
        object_id: 0,
    })
    .ok();

    let inference = manager.execute_intent(
        TaskId(1),
        &caps,
        0,
        "qwen2.5-0.5b-instruct",
        "Summarize daily telemetry status",
    );
    assert!(inference.is_ok(), "Authorized intent synthesis must succeed");
    serial_println!("[TEST] Zero-Trust AI Inference Intent: Successfully synthesized with local sovereign model.");

    // 5. Initialize On-Device Natural Language Intent Parser
    intent_parser::init();

    // 6. Initialize On-Device Vector Embedding Similarity Engine
    embedding::init();

    // 7. Initialize Privacy-Preserving AI Context Broker
    context_broker::init();

    // 8. Initialize Agentic Plan Execution Engine
    planner::init();

    // 9. Initialize Sovereign On-Device Privacy Shield
    shield::init();

    serial_println!("--- [Local AI Subsystem 100% Attested & Enforced] ---\n");
}
