//! Local AI Model Manager & Dynamic Memory Budgeter
//!
//! Enforces Non-Negotiable Principles #7 & #8:
//! - AI models are dynamically loaded and unloaded on demand (never permanently consume RAM).
//! - Foreground user applications have absolute priority; background AI is preempted under memory pressure.

use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

use crate::ai::model::{ModelPackage, ModelState};
use crate::capability::{CapabilityTable, CapabilityType, Rights};
use crate::task::TaskId;

/// Dynamic AI Model Manager controlling model packaging, lifecycle, and memory budgets.
pub struct ModelManager {
    models: BTreeMap<String, ModelPackage>,
    /// Upper bound of RAM permissible for AI model residency.
    max_ram_budget_bytes: usize,
    /// Current RAM consumed by loaded models.
    current_ram_used_bytes: usize,
    /// Total inference intents processed.
    intents_processed: usize,
}

impl ModelManager {
    /// Creates a new Model Manager with the specified AI memory partition limit.
    pub const fn new(max_ram_budget_bytes: usize) -> Self {
        Self {
            models: BTreeMap::new(),
            max_ram_budget_bytes,
            current_ram_used_bytes: 0,
            intents_processed: 0,
        }
    }

    /// Registers an attested model package into the system catalog.
    pub fn register_model(&mut self, model: ModelPackage) {
        self.models.insert(model.id.clone(), model);
    }

    /// Dynamically loads a model into RAM, evicting lower-priority models if budget is exceeded.
    pub fn load_model(&mut self, model_id: &str) -> Result<(), &'static str> {
        let model = self.models.get_mut(model_id).ok_or("Model not found")?;

        if model.state == ModelState::Loaded || model.state == ModelState::Active {
            return Ok(()); // Already resident
        }

        let required_ram = model.ram_footprint_bytes;

        // Check budget; if insufficient, evict any loaded models to satisfy non-negotiable principle #7
        while self.current_ram_used_bytes + required_ram > self.max_ram_budget_bytes {
            let evict_candidate = self
                .models
                .values()
                .find(|m| m.id != model_id && m.state == ModelState::Loaded)
                .map(|m| m.id.clone());

            if let Some(candidate_id) = evict_candidate {
                self.unload_model(&candidate_id);
            } else {
                return Err("Insufficient AI RAM budget: cannot load model without exceeding threshold");
            }
        }

        if let Some(m) = self.models.get_mut(model_id) {
            m.state = ModelState::Loaded;
            self.current_ram_used_bytes += required_ram;
            Ok(())
        } else {
            Err("Model disappeared during load")
        }
    }

    /// Unloads a model and frees its RAM allocation.
    pub fn unload_model(&mut self, model_id: &str) -> bool {
        if let Some(m) = self.models.get_mut(model_id) {
            if m.state == ModelState::Loaded || m.state == ModelState::Active {
                self.current_ram_used_bytes = self.current_ram_used_bytes.saturating_sub(m.ram_footprint_bytes);
                m.state = ModelState::Unloaded;
                return true;
            }
        }
        false
    }

    /// Preempts all AI models immediately when a foreground user application requires priority memory.
    ///
    /// Principle #8: Foreground user applications have priority over background AI.
    pub fn preempt_ai_for_foreground_task(&mut self) -> usize {
        let mut freed_bytes = 0;
        for model in self.models.values_mut() {
            if model.state == ModelState::Loaded || model.state == ModelState::Active {
                freed_bytes += model.ram_footprint_bytes;
                model.state = ModelState::Preempted;
            }
        }
        self.current_ram_used_bytes = 0;
        freed_bytes
    }

    /// Executes an inference intent with zero-trust capability validation.
    pub fn execute_intent(
        &mut self,
        _caller: TaskId,
        caller_caps: &CapabilityTable,
        cap_slot: usize,
        model_id: &str,
        prompt: &str,
    ) -> Result<String, &'static str> {
        // Zero ambient authority: requires IpcSend capability token
        caller_caps
            .verify(cap_slot, CapabilityType::IpcSend, Rights::WRITE)
            .map_err(|_| "CapabilityDenied: caller lacks required IpcSend capability")?;

        // Ensure model is loaded dynamically
        self.load_model(model_id)?;

        let model = self.models.get_mut(model_id).ok_or("Model not found")?;
        model.state = ModelState::Active;
        self.intents_processed += 1;

        // Perform deterministic edge intent synthesis
        let response = alloc::format!(
            "{{\"model\":\"{}\",\"parameters\":\"{}M\",\"quant\":\"{:?}\",\"prompt\":\"{}\",\"intent_status\":\"synthesized_locally\",\"vault\":\"sovereign\"}}",
            model.id, model.parameters_millions, model.quantization, prompt
        );

        model.state = ModelState::Loaded;
        Ok(response)
    }

    /// Returns the currently consumed RAM by AI models in bytes.
    pub fn ram_used(&self) -> usize {
        self.current_ram_used_bytes
    }

    /// Returns the max RAM budget for AI models in bytes.
    pub fn ram_budget(&self) -> usize {
        self.max_ram_budget_bytes
    }
}

/// Global system-wide AI Model Manager (partitioned to 128 KiB demo budget in microkernel).
pub static MODEL_MANAGER: Mutex<ModelManager> = Mutex::new(ModelManager::new(128 * 1024));
