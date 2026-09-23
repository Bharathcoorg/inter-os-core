//! Agentic Plan Execution Engine & Multi-Step Intent Pipeline
//!
//! Implements the Post-App Capability Mesh (Intent-First Computing).
//! Complex user requests and conversational tasks are parsed into multi-step
//! deterministic plans (`AgentPlan`), audited against capability boundaries,
//! and executed step-by-step with instant hardware force-takeover safety.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;
use crate::serial_println;

/// Execution status of an agentic plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Pending = 0,
    Executing = 1,
    AwaitingConsent = 2,
    Succeeded = 3,
    AbortedByForceTakeover = 4,
    Failed = 5,
}

/// Errors produced during plan creation or step execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    PlanNotFound,
    AlreadyCompleted,
    AwaitingConsentRequired,
    ForceTakeoverAborted,
    StepExecutionFailed,
}

/// A deterministic, capability-bounded step in an agent plan.
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub step_id: u32,
    pub description: String,
    pub action_type: u32, // 1: ParseIntent, 2: CapabilityAudit, 3: KeystoreUnlock, 4: AmcpDispatch, 5: StateCommit
    pub target: String,
    pub requires_consent: bool,
    pub completed: bool,
}

/// An active multi-step autonomous agent workflow.
#[derive(Debug, Clone)]
pub struct AgentPlan {
    pub id: u32,
    pub name: String,
    pub steps: Vec<PlanStep>,
    pub current_step: usize,
    pub status: PlanStatus,
}

/// Global Agentic Plan Execution Engine.
pub struct AgentPlanner {
    plans: BTreeMap<u32, AgentPlan>,
    next_id: u32,
    active_plan_id: Option<u32>,
}

impl AgentPlanner {
    pub const fn new() -> Self {
        Self {
            plans: BTreeMap::new(),
            next_id: 1,
            active_plan_id: None,
        }
    }

    /// Submits a new multi-step agent plan to the engine.
    pub fn submit_plan(&mut self, name: &str, steps: Vec<PlanStep>) -> u32 {
        let plan_id = self.next_id;
        self.next_id += 1;

        let plan = AgentPlan {
            id: plan_id,
            name: name.to_string(),
            steps,
            current_step: 0,
            status: PlanStatus::Pending,
        };

        self.plans.insert(plan_id, plan);
        self.active_plan_id = Some(plan_id);
        plan_id
    }

    /// Advances the specified plan to its next step.
    /// If the step requires explicit user consent and `user_consented` is false,
    /// the plan pauses with `PlanStatus::AwaitingConsent`.
    pub fn step_next(
        &mut self,
        plan_id: u32,
        user_consented: bool,
    ) -> Result<(usize, PlanStatus), PlanError> {
        let plan = self.plans.get_mut(&plan_id).ok_or(PlanError::PlanNotFound)?;

        if plan.status == PlanStatus::AbortedByForceTakeover {
            return Err(PlanError::ForceTakeoverAborted);
        }

        if plan.status == PlanStatus::Succeeded || plan.status == PlanStatus::Failed {
            return Err(PlanError::AlreadyCompleted);
        }

        if plan.current_step >= plan.steps.len() {
            plan.status = PlanStatus::Succeeded;
            if self.active_plan_id == Some(plan_id) {
                self.active_plan_id = None;
            }
            return Ok((plan.current_step, PlanStatus::Succeeded));
        }

        let step = &mut plan.steps[plan.current_step];

        // Check user consent boundary
        if step.requires_consent && !user_consented {
            plan.status = PlanStatus::AwaitingConsent;
            return Err(PlanError::AwaitingConsentRequired);
        }

        // Execute step: mark completed and record in AI Context Broker
        step.completed = true;
        plan.current_step += 1;

        // Log step advancement in working memory
        let mut desc_buf = [0u8; 128];
        let desc_len = core::cmp::min(step.description.len(), desc_buf.len());
        desc_buf[..desc_len].copy_from_slice(&step.description.as_bytes()[..desc_len]);
        if let Ok(desc_str) = core::str::from_utf8(&desc_buf[..desc_len]) {
            crate::ai::CONTEXT_BROKER.lock().append(
                crate::ai::context_broker::PrivacyTier::PrivateUser,
                desc_str,
            );
        }

        if plan.current_step >= plan.steps.len() {
            plan.status = PlanStatus::Succeeded;
            if self.active_plan_id == Some(plan_id) {
                self.active_plan_id = None;
            }
        } else {
            plan.status = PlanStatus::Executing;
        }

        Ok((plan.current_step, plan.status))
    }

    /// Triggers immediate hardware force-takeover (Non-Negotiable Safety Principle #4).
    /// Aborts any actively executing agent plan and restores manual control.
    pub fn trigger_force_takeover(&mut self) -> bool {
        if let Some(active_id) = self.active_plan_id {
            if let Some(plan) = self.plans.get_mut(&active_id) {
                if plan.status == PlanStatus::Executing
                    || plan.status == PlanStatus::AwaitingConsent
                    || plan.status == PlanStatus::Pending
                {
                    plan.status = PlanStatus::AbortedByForceTakeover;
                    self.active_plan_id = None;
                    serial_println!(
                        "[PLANNER] Force-takeover triggered! Plan {} ('{}') ABORTED immediately.",
                        active_id,
                        plan.name
                    );
                    return true;
                }
            }
        }
        false
    }

    /// Retrieves status summary of a plan formatted as JSON.
    pub fn format_json(&self, plan_id: u32, out_buf: &mut [u8]) -> usize {
        let plan = match self.plans.get(&plan_id) {
            Some(p) => p,
            None => {
                let err_msg = b"{\"error\":\"PlanNotFound\"}";
                let copy_len = core::cmp::min(err_msg.len(), out_buf.len());
                out_buf[..copy_len].copy_from_slice(&err_msg[..copy_len]);
                return copy_len;
            }
        };

        let status_str = match plan.status {
            PlanStatus::Pending => "Pending",
            PlanStatus::Executing => "Executing",
            PlanStatus::AwaitingConsent => "AwaitingConsent",
            PlanStatus::Succeeded => "Succeeded",
            PlanStatus::AbortedByForceTakeover => "AbortedByForceTakeover",
            PlanStatus::Failed => "Failed",
        };

        let mut pos = 0;
        let mut write_str = |s: &str| {
            let b = s.as_bytes();
            let rem = out_buf.len().saturating_sub(pos);
            let n = core::cmp::min(b.len(), rem);
            out_buf[pos..pos + n].copy_from_slice(&b[..n]);
            pos += n;
        };

        write_str("{\"plan_id\":");
        // write plan id
        let mut num_buf = [0u8; 10];
        let mut val = plan.id;
        let mut i = num_buf.len();
        if val == 0 {
            write_str("0");
        } else {
            while val > 0 && i > 0 {
                i -= 1;
                num_buf[i] = b'0' + (val % 10) as u8;
                val /= 10;
            }
            if let Ok(s) = core::str::from_utf8(&num_buf[i..]) {
                write_str(s);
            }
        }

        write_str(",\"name\":\"");
        write_str(&plan.name);
        write_str("\",\"status\":\"");
        write_str(status_str);
        write_str("\",\"current_step\":");

        let mut step_val = plan.current_step as u32;
        let mut j = num_buf.len();
        if step_val == 0 {
            write_str("0");
        } else {
            while step_val > 0 && j > 0 {
                j -= 1;
                num_buf[j] = b'0' + (step_val % 10) as u8;
                step_val /= 10;
            }
            if let Ok(s) = core::str::from_utf8(&num_buf[j..]) {
                write_str(s);
            }
        }

        write_str(",\"total_steps\":");
        let mut tot_val = plan.steps.len() as u32;
        let mut k = num_buf.len();
        if tot_val == 0 {
            write_str("0");
        } else {
            while tot_val > 0 && k > 0 {
                k -= 1;
                num_buf[k] = b'0' + (tot_val % 10) as u8;
                tot_val /= 10;
            }
            if let Ok(s) = core::str::from_utf8(&num_buf[k..]) {
                write_str(s);
            }
        }
        write_str("}");

        pos
    }
}

/// Global synchronized instance of the Agentic Plan Execution Engine.
pub static AGENT_PLANNER: Mutex<AgentPlanner> = Mutex::new(AgentPlanner::new());

/// Initializes the Agentic Plan Execution Engine and verifies multi-step execution.
pub fn init() {
    let mut planner = AGENT_PLANNER.lock();

    // 1. Construct a multi-step sovereign intent plan:
    // "Reorder milk on Blinkit with UPI payment & delivery notification"
    let steps = alloc::vec![
        PlanStep {
            step_id: 1,
            description: "Parse conversational intent & resolve cart items".to_string(),
            action_type: 1,
            target: "blinkit.cart_checkout".to_string(),
            requires_consent: false,
            completed: false,
        },
        PlanStep {
            step_id: 2,
            description: "Authorize financial transaction via Keystore Slot 2".to_string(),
            action_type: 3,
            target: "keystore.slot2".to_string(),
            requires_consent: true, // Demands user consent
            completed: false,
        },
        PlanStep {
            step_id: 3,
            description: "Dispatch attested AMCP intent datagram over network".to_string(),
            action_type: 4,
            target: "blinkit.cart_checkout".to_string(),
            requires_consent: false,
            completed: false,
        },
    ];

    let plan_id = planner.submit_plan("Blinkit Express Milk Reorder", steps);
    assert_eq!(plan_id, 1, "First plan ID must be 1");

    // 2. Step 1: No consent needed -> Squeaks through to InProgress
    let step1_res = planner.step_next(plan_id, false);
    assert!(step1_res.is_ok(), "Step 1 must execute without consent");
    let (step_num, status) = step1_res.unwrap();
    assert_eq!(step_num, 1);
    assert_eq!(status, PlanStatus::Executing);

    // 3. Step 2: Consent requested -> Without consent, must fail with AwaitingConsentRequired
    let step2_no_consent = planner.step_next(plan_id, false);
    assert_eq!(step2_no_consent, Err(PlanError::AwaitingConsentRequired));

    // 4. Step 2: With consent granted -> Advances to Step 2
    let step2_consent = planner.step_next(plan_id, true);
    assert!(step2_consent.is_ok(), "Step 2 must execute with consent");

    // 5. Test Hardware Force-Takeover: aborts plan immediately
    let aborted = planner.trigger_force_takeover();
    assert!(aborted, "Active plan must be aborted by force takeover");

    // Verify subsequent step attempt is rejected
    let step3_after_abort = planner.step_next(plan_id, true);
    assert_eq!(step3_after_abort, Err(PlanError::ForceTakeoverAborted));

    serial_println!(
        "[TEST] Agentic Plan Pipeline: Multi-step plan #1 created, consent-gated, and force-takeover safety VERIFIED."
    );
}
