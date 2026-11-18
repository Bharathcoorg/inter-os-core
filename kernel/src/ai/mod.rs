// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

use core::sync::atomic::AtomicU64;

pub struct AiTelemetry {
    pub inference_count: AtomicU64,
    pub inference_latency_micros: AtomicU64,
}

pub static AI_TELEMETRY: AiTelemetry = AiTelemetry {
    inference_count: AtomicU64::new(0),
    inference_latency_micros: AtomicU64::new(0),
};
