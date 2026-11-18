use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub struct AiTelemetry {
    pub inference_count: AtomicU64,
    pub inference_latency_micros: AtomicU64,
}

pub static AI_TELEMETRY: AiTelemetry = AiTelemetry {
    inference_count: AtomicU64::new(0),
    inference_latency_micros: AtomicU64::new(0),
};
