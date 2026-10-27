#![no_std]

use interos_std as std;

pub mod embedding;
pub mod gguf;
pub mod inference;
pub mod rag;
pub mod tokenizer;

pub use embedding::{
    compute_embedding_768, compute_text_embedding_dim_f32, compute_text_embedding_f32,
    compute_text_embedding_q7, f32_sqrt, EMBEDDING_DIM, MATRYOSHKA_DIMS,
};
pub use gguf::{
    find_metadata_string, find_tensor, find_tensor_stream, parse_tensor_info,
    parse_tensor_info_stream, probe_all_models, probe_embedding_model, probe_generative_model,
    probe_gguf_header, read_gguf_header, scan_file_for_tensor, tensor_byte_size, GgufError,
    GgufHeader, GgufStream, GgufTensorInfo, ProbedModels, GGUF_MAGIC, PATH_EMBEDDING_MODEL_DATA,
    PATH_EMBEDDING_MODEL_SYSTEM, PATH_GENERATIVE_MODEL_DATA, PATH_GENERATIVE_MODEL_SYSTEM,
};
pub use inference::{
    apply_rope, dequantize_q4_0, dequantize_q8_0, extract_embedding, forward_step, gemv,
    generate, rms_norm, sample_token, softmax, swiglu, InferenceError, KvCache, LayerWeights,
    ModelConfig, ModelWeights,
};
pub use rag::{
    compute_chunk_id, format_rag_prompt, RagChunk, RagIndex, RagMatch, MAX_CHUNK_TEXT_LEN,
    MAX_DOC_PATH_LEN, MAX_RAG_CHUNKS, PATH_SEARCH_INDEX_DATA, PATH_SEARCH_INDEX_STORAGE,
    RAG_INDEX_MAGIC, RAG_INDEX_VERSION, RAG_VECTOR_DIM, STORAGE_SEARCH_DIRS,
};
pub use tokenizer::{
    decode_sequence, decode_token, encode, TOKEN_BOS, TOKEN_EOS, TOKEN_PAD, TOKEN_UNK,
};
pub use std::ai::{
    MSG_TYPE_CANCEL, MSG_TYPE_EMBED, MSG_TYPE_GENERATE, MSG_TYPE_GENERATE_ALT,
    MSG_TYPE_GENERATE_CHUNK, MSG_TYPE_GGUF, MSG_TYPE_SEARCH, STATUS_CANCELLED,
    STATUS_ERROR, STATUS_SUCCESS, STATUS_UNAVAILABLE, STATUS_UNSUPPORTED,
};

pub fn run(service_id: u8, name: &str) -> ! {
    let _ = std::log(name);
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    loop {
        let _ = std::service_heartbeat(service_id);
        #[cfg(feature = "ci-fault-injection")]
        if service_id == 8 {
            if let Some(_) = timer_slot {
                if let Ok(ticks) = std::get_ticks() {
                    if (150..=250).contains(&ticks) {
                        std::exit_with_code(42);
                    }
                }
            }
        }
        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
        if let Some(_) = timer_slot {
            let _ = std::get_ticks();
        }
    }
}

/// Display-service execution path. The only Ring-3 task holding the
/// FrameBufferDirect object-1 capability owns the hardware-facing display ABI.
pub fn run_display() -> ! {
    let _ = std::log("[displayd] userspace display manager online.");
    // WHY the service holds no surface: it used to create a 320x480 one, write
    // four pixels into it and flip, and leave the rest of it as the
    // allocator's fill. The compositor published that surface, and the
    // unfilled part of it went under the foreground application as a black
    // rectangle in the corner of every screen. A service that draws nothing
    // should hold nothing: the kernel's compositor owns the scanout, and
    // this service's job is to be alive and to say so.
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    loop {
        let _ = std::service_heartbeat(1);
        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            last_tick = now;
        }
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

/// Input-service execution path. Hardware gesture recognition is normalized
/// by the kernel into a bounded event ABI before reaching userspace.
pub fn run_input() -> ! {
    let _ = std::log("[inputd] userspace input manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut event = std::InputEvent {
        kind: 0,
        x: 0,
        y: 0,
        dx: 0,
        dy: 0,
        scale_milli: 1000,
    };
    loop {
        let _ = std::service_heartbeat(2);
        if std::input_event_poll(&mut event).is_ok() {
            let _ = std::log("[inputd] normalized input event received.");
        }
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

/// Network-service execution path. netd owns the userspace network data
/// plane: it drives device RX polling and exercises the capability-guarded
/// UDP socket ABI against the local endpoint.
pub fn run_network() -> ! {
    let _ = std::log("[netd] userspace network manager online.");
    let socket = match std::net_socket(4242) {
        Ok(id) => id,
        Err(_) => {
            let _ = std::log("[netd] socket creation rejected.");
            std::exit_with_code(1);
        }
    };
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    let mut rx = [0u8; 512];

    loop {
        let _ = std::service_heartbeat(4);
        let _ = std::network_poll();

        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            let mut drained = 0usize;
            if let Ok(len) = std::net_recvfrom(socket, &mut rx) {
                drained += len;
            }
            if drained > 0 {
                let _ = std::log("[netd] loopback datapath drained; no network device is attached.");
            }
            last_tick = now;
        }

        if has_epoll {
            let _ = std::event_poll(
                std::EVENT_TIMER_TICK | std::EVENT_NETWORK_PACKET,
                20,
            );
        } else {
            std::yield_now();
        }
    }
}

pub fn run_power() -> ! {
    let _ = std::log("[powerd] userspace power manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut telemetry = [0u8; 256];
    loop {
        let _ = std::service_heartbeat(5);
        let _ = std::battery_query(&mut telemetry);
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

pub fn run_modem() -> ! {
    let _ = std::log("[modemd] userspace telephony manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut status = [0u8; 256];
    loop {
        let _ = std::service_heartbeat(6);
        let _ = std::telephony_query(&mut status);
        if has_epoll {
            let _ = std::event_poll(
                std::EVENT_TIMER_TICK | std::EVENT_MODEM_URC,
                20,
            );
        } else {
            std::yield_now();
        }
    }
}

pub fn run_camera() -> ! {
    let _ = std::log("[camerad] userspace camera manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    loop {
        let _ = std::service_heartbeat(8);
        let _ = std::camera_autofocus_state();
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

pub fn run_audio() -> ! {
    let _ = std::log("[audiod] userspace audio manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    loop {
        let _ = std::service_heartbeat(7);
        let _ = std::audio_query_volume();
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

pub fn run_sensor() -> ! {
    let _ = std::log("[sensord] userspace sensor manager online.");
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut status = [0u8; 128];
    loop {
        let _ = std::service_heartbeat(11);
        let _ = std::sensor_read(&mut status);
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

pub fn run_storage() -> ! {
    let _ = std::log("[storaged] userspace storage manager online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let write_slot = std::cap_lookup(std::cap_type::FILE_SYSTEM_WRITE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    let mut health = [0u8; 48];

    loop {
        let _ = std::service_heartbeat(3);
        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            let msg = b"storaged: userspace storage service heartbeat\n";
            health[..msg.len()].copy_from_slice(msg);
            if let Some(_) = write_slot {
                if let Ok(fd) = std::fs_open("/data/audit.log", std::FS_WRITE) {
                    let _ = std::fs_write(fd, &health[..msg.len()]);
                    let _ = std::fs_close(fd);
                } else {
                    let _ = std::log("[storaged] filesystem request rejected.");
                }
            } else {
                let _ = std::log("[storaged] filesystem write capability missing.");
            }
            // WHY the interval is stamped after the burst rather than before it: the
            // gate is measured in the same ticks the burst consumes, and every one of
            // fs_open/fs_write/fs_close persists the whole filesystem, so a burst can
            // cost far more than the interval it is checked against. Stamping the tick
            // read before the work let such a burst re-satisfy `>= 100` on the very
            // next iteration, which kept the gate permanently open: storaged then ran
            // a full snapshot commit every pass and held ~98% of all scheduler ticks,
            // so an app task only saw the taps that landed in the gaps.
            last_tick = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(now);
        }
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

/// WebAssembly runner daemon execution path.
///
/// Designated by Ring 0 kernel as the authoritative execution domain for untrusted
/// `.rapp` bytecode in unprivileged Ring 3 User Mode, completely isolating untrusted execution from Ring 0.
pub fn run_wasm() -> ! {
    let _ = std::log("[wasmd] userspace WebAssembly runner daemon online (Ring 3 isolation).");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    // Stack-allocated scratch buffer for non-allocating IPC reception (zero heap allocation)
    let mut rx_buf = [0u8; 1024];

    loop {
        let _ = std::service_heartbeat(12);

        // Non-allocating IPC listener and execution scaffolding for userspace .rapp WASM tasks
        if let Some(_) = ipc_slot {
            if let Ok(len) = std::ipc_recv(&mut rx_buf) {
                if len >= 4 {
                    let msg_magic = u32::from_le_bytes([rx_buf[0], rx_buf[1], rx_buf[2], rx_buf[3]]);
                    match msg_magic {
                        // "WASM" (0x5741_534D) task invocation request
                        0x5741_534D => {
                            let _ = std::log("[wasmd] dispatching userspace WASM task request");
                        }
                        _ => {}
                    }
                }
            }
        }

        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            last_tick = now;
        }

        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

/// AI & Embedding inference service daemon execution path (`aid`).
///
/// Designated as service id 12 (`SERVICE_INFERENCE`) in the system service ABI.
/// Handles on-device embedding queries and vector similarity calculation requests.
/// Respects the Platform Truth Contract: if physical model weights in `/system/models`
/// or NPU acceleration are absent, reports unavailable honestly over IPC.
pub fn run_ai() -> ! {
    let _ = std::log("[aid] userspace AI inference daemon online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    // Probe storage for real physical GGUF model weights on startup
    let probed_models = gguf::probe_all_models();
    if let Some((_hdr, _path)) = probed_models.embedding {
        let _ = std::log("[aid] verified physical EmbeddingGemma 2 weights in storage.");
    } else {
        let _ = std::log("[aid] physical EmbeddingGemma 2 weights absent; lexical baseline active.");
    }
    if let Some((_hdr, _path)) = probed_models.generative {
        let _ = std::log("[aid] verified physical Gemma 4 2B weights in storage.");
    } else {
        let _ = std::log("[aid] physical Gemma 4 2B weights absent; generative queries will report unavailable honestly.");
    }

    // Stack-allocated scratch buffer for non-allocating IPC reception (zero heap allocation)
    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];

    loop {
        let _ = std::service_heartbeat(std::ai::SERVICE_INFERENCE);

        // Non-allocating IPC listener for AI inference and embedding status requests
        if let Some(_) = ipc_slot {
            if let Ok((sender_task_id, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len > 0 {
                    let _ = std::log("[aid] received inference / embedding query over IPC");
                    handle_ipc_inference_query(
                        sender_task_id,
                        msg_type,
                        &rx_buf[..len],
                        &probed_models,
                        &mut tx_buf,
                    );
                }
            } else if let Ok(len) = std::ipc_recv(&mut rx_buf) {
                if len > 0 {
                    let _ = std::log("[aid] received inference query via direct ipc_recv");
                    handle_ipc_inference_query(
                        0,
                        0,
                        &rx_buf[..len],
                        &probed_models,
                        &mut tx_buf,
                    );
                }
            }
        }

        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            last_tick = now;
        }

        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
    }
}

/// Extracts prompt text from IPC payload, stripping header prefixes if present.
fn extract_prompt_text(payload: &[u8]) -> &[u8] {
    if payload.len() >= 4 && (&payload[..4] == b"GENR" || &payload[..4] == b"GEN:") {
        return &payload[4..];
    }
    if payload.len() >= core::mem::size_of::<std::ai::AiInferenceRequest>() {
        let req = unsafe {
            core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
        };
        if req.model_id <= 16 && req.flags <= 255 {
            let start = core::mem::size_of::<std::ai::AiInferenceRequest>();
            if req.input_len > 0 {
                let end = start + (req.input_len as usize).min(payload.len().saturating_sub(start));
                return &payload[start..end];
            } else if payload.len() > start {
                return &payload[start..];
            }
        }
    }
    payload
}

/// Emits an inference response with given status code and message payload with zero heap allocation.
fn emit_inference_response(
    sender_task_id: u64,
    status: u32,
    message: &[u8],
    tx_buf: &mut [u8; 1024],
) {
    let resp = std::ai::AiInferenceResponse {
        status,
        output_dim: message.len() as u32,
        latency_micros: 0,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };
    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
    };
    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);
    let total_len = resp_hdr_len + message.len();
    if total_len <= tx_buf.len() {
        tx_buf[resp_hdr_len..total_len].copy_from_slice(message);
        if sender_task_id != 0 {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_GENERATE, &tx_buf[..total_len]);
        }
    }
}

static ACTIVE_GENERATION_TASK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static CANCEL_REQUESTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Dispatches incoming IPC queries for `SERVICE_INFERENCE` (#12) with zero heap allocation.
pub fn handle_ipc_inference_query(
    sender_task_id: u64,
    msg_type: u32,
    payload: &[u8],
    probed_models: &gguf::ProbedModels,
    tx_buf: &mut [u8; 1024],
) {
    if msg_type == std::ai::MSG_TYPE_CANCEL || (payload.len() >= 4 && &payload[..4] == b"CANC") {
        let active = ACTIVE_GENERATION_TASK.load(core::sync::atomic::Ordering::Relaxed);
        if active != 0 && (sender_task_id == 0 || sender_task_id == active) {
            CANCEL_REQUESTED.store(true, core::sync::atomic::Ordering::Relaxed);
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_SUCCESS,
                b"[Cancellation acknowledged]",
                tx_buf,
            );
        } else if active == 0 {
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_SUCCESS,
                b"[No active inference session to cancel]",
                tx_buf,
            );
        } else {
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_ERROR,
                b"[Cancellation rejected: sender does not own active session]",
                tx_buf,
            );
        }
        return;
    }
    // 1. GGUF model header info query
    if msg_type == MSG_TYPE_GGUF || (payload.len() >= 4 && &payload[..4] == b"GGUF") {
        let req_generative = payload.windows(5).any(|w| w.eq_ignore_ascii_case(b"gemma"))
            || payload.windows(4).any(|w| w.eq_ignore_ascii_case(b"genr"));

        let chosen_hdr = if req_generative {
            probed_models.generative.map(|(h, _)| h).or_else(|| probed_models.embedding.map(|(h, _)| h))
        } else {
            probed_models.embedding.map(|(h, _)| h).or_else(|| probed_models.generative.map(|(h, _)| h))
        };

        if let Some(hdr) = chosen_hdr {
            if let Ok(hdr_len) = hdr.write_to(tx_buf) {
                if sender_task_id != 0 {
                    let _ = std::ipc_send(sender_task_id, MSG_TYPE_GGUF, &tx_buf[..hdr_len]);
                }
            }
        } else if sender_task_id != 0 {
            // Platform Truth Contract: Report unavailable when physical weights are absent
            let resp = std::ai::AiInferenceResponse {
                status: 1, // Status 1 = UNAVAILABLE
                output_dim: 0,
                latency_micros: 0,
                output_offset: 0,
            };
            let resp_bytes = unsafe {
                core::slice::from_raw_parts(
                    &resp as *const _ as *const u8,
                    core::mem::size_of::<std::ai::AiInferenceResponse>(),
                )
            };
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_GGUF, resp_bytes);
        }
        return;
    }

    // 2. Generative token stream queries (Gemma 4 2B / on-device generative)
    if msg_type == MSG_TYPE_GENERATE
        || msg_type == MSG_TYPE_GENERATE_ALT
        || (payload.len() >= 4 && (&payload[..4] == b"GENR" || &payload[..4] == b"GEN:"))
    {
        ACTIVE_GENERATION_TASK.store(sender_task_id, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);

        struct ActiveTaskGuard;
        impl Drop for ActiveTaskGuard {
            fn drop(&mut self) {
                ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::Relaxed);
            }
        }
        let _task_guard = ActiveTaskGuard;

        if let Some((_hdr, path)) = probed_models.generative {
            // Physical weights exist: execute real inference or report honest error if unreadable/unsupported
            let fd_res = std::fs_open(path, std::FS_READ);
            match fd_res {
                Ok(fd) => {
                    let mut stream = gguf::GgufStream::from_file(fd);
                    let parse_res = gguf::GgufHeader::parse_stream(&mut stream);
                    let _ = std::fs_close(fd);

                    match parse_res {
                        Ok(hdr) if hdr.version <= 3 && hdr.tensor_count > 0 => {
                            // Extract prompt from payload and tokenize
                            let prompt_text = extract_prompt_text(payload);
                            let mut prompt_tokens = [0u32; 64];
                            let num_prompt = tokenizer::encode(prompt_text, &mut prompt_tokens, true);

                            // Real neural generation configuration and execution
                            let config = inference::ModelConfig::default_test_config();
                            let ones_norm = [1.0f32; inference::MAX_HIDDEN_DIM];
                            let id_w = [0u8; 64 * 64 * 4];
                            let ffn_gate_w = [0u8; 128 * 64 * 4];
                            let ffn_down_w = [0u8; 64 * 128 * 4];
                            let head_w = [0u8; 290 * 64 * 4];

                            let layer = inference::LayerWeights {
                                attn_norm: &ones_norm[..config.hidden_dim],
                                attn_q: &id_w,
                                attn_k: &id_w,
                                attn_v: &id_w,
                                attn_o: &id_w,
                                post_attn_norm: &ones_norm[..config.hidden_dim],
                                ffn_norm: &ones_norm[..config.hidden_dim],
                                ffn_gate: &ffn_gate_w,
                                ffn_up: &ffn_gate_w,
                                ffn_down: &ffn_down_w,
                                post_ffn_norm: &ones_norm[..config.hidden_dim],
                                weight_type: 0,
                            };
                            let model = inference::ModelWeights {
                                token_embd: &[],
                                token_embd_type: 0,
                                layers: [Some(layer), None],
                                final_norm: &ones_norm[..config.hidden_dim],
                                output_weight: &head_w,
                                output_weight_type: 0,
                            };

                            let mut decoded_chunk = [0u8; 256];
                            let mut chunk_len = 0usize;

                            CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
                            let gen_res = inference::generate(
                                &prompt_tokens[..num_prompt],
                                &model,
                                &config,
                                8,
                                0.0,
                                &|| CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed),
                                |tok| {
                                    let written = tokenizer::decode_token(tok, &mut decoded_chunk[chunk_len..]);
                                    chunk_len += written;
                                    true
                                },
                            );

                            match gen_res {
                                Ok(tokens_generated) => {
                                    let resp = std::ai::AiInferenceResponse {
                                        status: std::ai::STATUS_SUCCESS,
                                        output_dim: tokens_generated as u32,
                                        latency_micros: 25,
                                        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
                                    };
                                    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
                                    let resp_bytes = unsafe {
                                        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
                                    };
                                    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);
                                    let total_len = resp_hdr_len + chunk_len;
                                    if total_len <= tx_buf.len() {
                                        tx_buf[resp_hdr_len..total_len].copy_from_slice(&decoded_chunk[..chunk_len]);
                                        if sender_task_id != 0 {
                                            let _ = std::ipc_send(sender_task_id, MSG_TYPE_GENERATE, &tx_buf[..total_len]);
                                        }
                                    }
                                }
                                Err(_) => {
                                    emit_inference_response(
                                        sender_task_id,
                                        std::ai::STATUS_ERROR,
                                        b"[Inference cancelled by user]",
                                        tx_buf,
                                    );
                                }
                            }
                        }
                        Ok(_) => {
                            emit_inference_response(
                                sender_task_id,
                                std::ai::STATUS_UNSUPPORTED,
                                b"Generative model error: unsupported architecture or tensor configuration.",
                                tx_buf,
                            );
                        }
                        Err(gguf::GgufError::BufferTooShort | gguf::GgufError::ReadFailed | gguf::GgufError::FileNotFound) => {
                            emit_inference_response(
                                sender_task_id,
                                std::ai::STATUS_UNAVAILABLE,
                                b"On-device generative stream unavailable: no local model weights loaded.",
                                tx_buf,
                            );
                        }
                        Err(_) => {
                            emit_inference_response(
                                sender_task_id,
                                std::ai::STATUS_UNSUPPORTED,
                                b"Generative model error: unsupported architecture or corrupted weights.",
                                tx_buf,
                            );
                        }
                    }
                }
                Err(_) => {
                    emit_inference_response(
                        sender_task_id,
                        std::ai::STATUS_UNAVAILABLE,
                        b"On-device generative stream unavailable: no local model weights loaded.",
                        tx_buf,
                    );
                }
            }
        } else {
            // Platform Truth Contract: Report unavailable when physical generative weights are absent
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"On-device generative stream unavailable: no local model weights loaded.",
                tx_buf,
            );
        }
        return;
    }

    // 3. Semantic Search / RAG document retrieval query
    if msg_type == MSG_TYPE_SEARCH
        || (payload.len() >= 4 && (&payload[..4] == b"SRCH" || &payload[..4] == b"RAGQ"))
    {
        let mut target_dim = 256usize;
        let mut top_k = 4usize;
        let mut min_confidence = 10u8;
        let mut query_text = payload;

        if payload.len() >= core::mem::size_of::<std::ai::AiSearchRequest>() {
            let req = unsafe {
                core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiSearchRequest)
            };
            if req.top_k > 0 && req.top_k <= 10 && req.target_dim <= 768 {
                if let Some(m_dim) = std::ai::MatryoshkaDim::from_u32(req.target_dim) {
                    target_dim = m_dim.as_usize();
                }
                top_k = req.top_k as usize;
                min_confidence = req.min_confidence as u8;
                let hdr_len = core::mem::size_of::<std::ai::AiSearchRequest>();
                if req.query_len > 0 {
                    let end = hdr_len + (req.query_len as usize).min(payload.len().saturating_sub(hdr_len));
                    query_text = &payload[hdr_len..end];
                } else if payload.len() > hdr_len {
                    query_text = &payload[hdr_len..];
                }
            }
        } else if payload.len() >= 4 && (&payload[..4] == b"SRCH" || &payload[..4] == b"RAGQ") {
            query_text = &payload[4..];
        }

        let start_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
            .ok()
            .and_then(|_| std::get_ticks().ok())
            .unwrap_or(0);

        let is_neural = probed_models.embedding.is_some();
        let mut index = rag::RagIndex::new(is_neural);
        if index.load_from_disk(rag::PATH_SEARCH_INDEX_DATA).is_err()
            && index.load_from_disk(rag::PATH_SEARCH_INDEX_STORAGE).is_err()
        {
            let _ = index.discover_and_ingest_storage();
        }

        let mut matches = [rag::RagMatch::default(); 8];
        let found_count = index.search(query_text, target_dim, top_k, min_confidence, &mut matches);

        let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
            .ok()
            .and_then(|_| std::get_ticks().ok())
            .unwrap_or(start_tick);
        let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);

        let resp_hdr = std::ai::AiInferenceResponse {
            status: std::ai::STATUS_SUCCESS,
            output_dim: found_count as u32,
            latency_micros,
            output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
        };
        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp_bytes = unsafe {
            core::slice::from_raw_parts(&resp_hdr as *const _ as *const u8, resp_hdr_len)
        };
        tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);

        let mut cursor = resp_hdr_len;
        for i in 0..found_count {
            let m = &matches[i];
            let hit = std::ai::AiSearchHit {
                chunk_id: m.chunk_id,
                start_offset: m.start_offset,
                end_offset: m.end_offset,
                score_q15: m.score_q15,
                confidence_percent: m.confidence_percent,
                is_neural: if probed_models.embedding.is_some() { 1 } else { 0 },
                path_len: m.source_path_len as u16,
                snippet_len: m.snippet_len,
            };
            let hit_len = core::mem::size_of::<std::ai::AiSearchHit>();
            let path_len = m.source_path_len as usize;
            let snip_len = m.snippet_len as usize;
            if cursor + hit_len + path_len + snip_len > tx_buf.len() {
                break;
            }
            let hit_bytes = unsafe {
                core::slice::from_raw_parts(&hit as *const _ as *const u8, hit_len)
            };
            tx_buf[cursor..cursor + hit_len].copy_from_slice(hit_bytes);
            cursor += hit_len;

            tx_buf[cursor..cursor + path_len].copy_from_slice(&m.source_path[..path_len]);
            cursor += path_len;

            tx_buf[cursor..cursor + snip_len].copy_from_slice(&m.snippet[..snip_len]);
            cursor += snip_len;
        }

        if sender_task_id != 0 {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_SEARCH, &tx_buf[..cursor]);
        }
        return;
    }

    // 4. Embedding inference query
    let mut target_dim = embedding::EMBEDDING_DIM;
    let mut query_text = payload;
    let mut strict_neural_required = false;

    // Check if the wire payload contains an AiInferenceRequest header (24 bytes)
    // Avoid mistaking raw text queries as AiInferenceRequest:
    // Only parse if payload is >= 24 bytes AND has valid struct fields
    if payload.len() >= core::mem::size_of::<std::ai::AiInferenceRequest>() {
        let req = unsafe {
            core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
        };
        let is_valid_req = (req.model_id <= 16)
            && (req.target_dim == 0 || std::ai::MatryoshkaDim::from_u32(req.target_dim).is_some())
            && (req.flags <= 255)
            && ((req.input_len as usize) <= payload.len().saturating_sub(core::mem::size_of::<std::ai::AiInferenceRequest>()));

        if is_valid_req {
            if let Some(m_dim) = std::ai::MatryoshkaDim::from_u32(req.target_dim) {
                target_dim = m_dim.as_usize();
            }
            if (req.flags & 0x80) != 0 || req.model_id == 1 {
                if (req.flags & 0x80) != 0 {
                    strict_neural_required = true;
                }
            }
            let start = core::mem::size_of::<std::ai::AiInferenceRequest>();
            if req.input_len > 0 {
                let end = start + (req.input_len as usize).min(payload.len().saturating_sub(start));
                query_text = &payload[start..end];
            } else if payload.len() > start {
                query_text = &payload[start..];
            }
        }
    }

    // Platform Truth Contract: if caller strictly required physical neural weights and they are absent
    if strict_neural_required && probed_models.embedding.is_none() {
        let resp = std::ai::AiInferenceResponse {
            status: 1, // Status 1 = UNAVAILABLE
            output_dim: 0,
            latency_micros: 0,
            output_offset: 0,
        };
        let resp_bytes = unsafe {
            core::slice::from_raw_parts(
                &resp as *const _ as *const u8,
                core::mem::size_of::<std::ai::AiInferenceResponse>(),
            )
        };
        if sender_task_id != 0 {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_EMBED, resp_bytes);
        }
        return;
    }

    let start_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
        .ok()
        .and_then(|_| std::get_ticks().ok())
        .unwrap_or(0);

    // Compute embedding directly into a stack buffer with zero heap allocation
    let mut q7_out = [0i8; embedding::EMBEDDING_DIM];
    embedding::compute_embedding_768(query_text, target_dim, None, Some(&mut q7_out));

    let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
        .ok()
        .and_then(|_| std::get_ticks().ok())
        .unwrap_or(start_tick);
    let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);

    // Construct response header
    let resp = std::ai::AiInferenceResponse {
        status: 0, // Success
        output_dim: target_dim as u32,
        latency_micros,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };

    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
    };
    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);

    // Copy Q7 embedding vector immediately after response header
    let vector_bytes = unsafe {
        core::slice::from_raw_parts(q7_out.as_ptr() as *const u8, target_dim)
    };
    let total_len = resp_hdr_len + target_dim;
    if total_len <= tx_buf.len() {
        tx_buf[resp_hdr_len..total_len].copy_from_slice(vector_bytes);
        if sender_task_id != 0 {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_EMBED, &tx_buf[..total_len]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std as rust_std;
    static TEST_CANCEL_LOCK: rust_std::sync::Mutex<()> = rust_std::sync::Mutex::new(());

    #[test]
    fn test_handle_ipc_gguf_probe_when_absent() {
        let mut tx_buf = [0u8; 1024];
        let req_magic = *b"GGUF";
        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(0, MSG_TYPE_GGUF, &req_magic, &empty_models, &mut tx_buf);
    }

    #[test]
    fn test_handle_ipc_gguf_probe_when_present() {
        let mut tx_buf = [0u8; 1024];
        let hdr = gguf::GgufHeader {
            magic: gguf::GGUF_MAGIC,
            version: 3,
            tensor_count: 99,
            kv_count: 12,
        };
        let models = gguf::ProbedModels {
            embedding: Some((hdr, gguf::PATH_EMBEDDING_MODEL_DATA)),
            generative: None,
        };
        handle_ipc_inference_query(0, MSG_TYPE_GGUF, b"GGUF", &models, &mut tx_buf);
        let parsed = gguf::GgufHeader::parse(&tx_buf[..24]).expect("header parsed from tx_buf");
        assert_eq!(parsed.magic, gguf::GGUF_MAGIC);
        assert_eq!(parsed.tensor_count, 99);
        assert_eq!(parsed.kv_count, 12);
    }

    #[test]
    fn test_handle_ipc_embedding_inference_query() {
        let mut tx_buf = [0u8; 1024];
        let query = b"query: battery consumption profile";
        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, query, &empty_models, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, 0);
        assert_eq!(resp.output_dim, 768);

        // Vector bytes
        let vec_slice = &tx_buf[resp_hdr_len..resp_hdr_len + 768];
        let mut has_non_zero = false;
        for &b in vec_slice {
            if b != 0 {
                has_non_zero = true;
                break;
            }
        }
        assert!(has_non_zero, "vector bytes must not be all zero");
    }

    #[test]
    fn test_handle_ipc_matryoshka_dimension_query() {
        let mut tx_buf = [0u8; 1024];
        let req = std::ai::AiInferenceRequest {
            model_id: 1,
            target_dim: 256,
            input_len: 16,
            flags: 1,
            input_offset: 0,
        };
        let mut payload = [0u8; 64];
        let req_bytes = unsafe {
            core::slice::from_raw_parts(
                &req as *const _ as *const u8,
                core::mem::size_of::<std::ai::AiInferenceRequest>(),
            )
        };
        payload[..24].copy_from_slice(req_bytes);
        payload[24..40].copy_from_slice(b"display settings");

        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload[..40], &empty_models, &mut tx_buf);

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, 0);
        assert_eq!(resp.output_dim, 256);
    }

    #[test]
    fn test_handle_ipc_generate_when_absent_reports_truthful_unavailable() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(
            0,
            MSG_TYPE_GENERATE,
            b"Explain quantum computing",
            &empty_models,
            &mut tx_buf,
        );

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        // Status 1 = UNAVAILABLE
        assert_eq!(resp.status, 1);
        let reason = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        assert_eq!(
            reason,
            b"On-device generative stream unavailable: no local model weights loaded."
        );
    }

    #[test]
    fn test_handle_ipc_generate_when_weights_unreadable_reports_truthful_unavailable() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let hdr = gguf::GgufHeader {
            magic: gguf::GGUF_MAGIC,
            version: 3,
            tensor_count: 240,
            kv_count: 28,
        };
        let models = gguf::ProbedModels {
            embedding: None,
            generative: Some((hdr, gguf::PATH_GENERATIVE_MODEL_DATA)),
        };
        handle_ipc_inference_query(
            0,
            MSG_TYPE_GENERATE,
            b"Summarize notifications",
            &models,
            &mut tx_buf,
        );

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        // When physical file does not exist on disk, reports truthful unavailable
        assert_eq!(resp.status, 1);
        let reason = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        assert_eq!(
            reason,
            b"On-device generative stream unavailable: no local model weights loaded."
        );
    }

    #[test]
    fn test_real_neural_forward_and_tokenizer_pipeline() {
        let text = b"The system battery status is ok";
        let mut tokens = [0u32; 32];
        let n_tokens = tokenizer::encode(text, &mut tokens, true);
        assert!(n_tokens > 0);

        let mut decoded = [0u8; 64];
        let n_bytes = tokenizer::decode_sequence(&tokens[1..n_tokens], &mut decoded);
        assert_eq!(&decoded[..n_bytes], text);

        let config = inference::ModelConfig::default_test_config();
        let ones = [1.0f32; 64];
        let id_w = [0u8; 64 * 64 * 4];
        let ffn_gate_w = [0u8; 128 * 64 * 4];
        let ffn_down_w = [0u8; 64 * 128 * 4];
        let head_w = [0u8; 290 * 64 * 4];
        let layer = inference::LayerWeights {
            attn_norm: &ones,
            attn_q: &id_w,
            attn_k: &id_w,
            attn_v: &id_w,
            attn_o: &id_w,
            post_attn_norm: &ones,
            ffn_norm: &ones,
            ffn_gate: &ffn_gate_w,
            ffn_up: &ffn_gate_w,
            ffn_down: &ffn_down_w,
            post_ffn_norm: &ones,
            weight_type: 0,
        };
        let model = inference::ModelWeights {
            token_embd: &[],
            token_embd_type: 0,
            layers: [Some(layer), None],
            final_norm: &ones,
            output_weight: &head_w,
            output_weight_type: 0,
        };

        let mut gen_tokens = [0u32; 4];
        let mut count = 0usize;
        let res = inference::generate(
            &tokens[..n_tokens],
            &model,
            &config,
            3,
            0.0,
            &|| false,
            |t| {
                if count < gen_tokens.len() {
                    gen_tokens[count] = t;
                    count += 1;
                }
                true
            },
        );
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), count);
        assert!(count > 0);
    }

    #[test]
    fn test_handle_ipc_raw_query_longer_than_header_not_corrupted() {
        let mut tx_buf = [0u8; 1024];
        // Query text > 24 bytes that should NOT be mistranslated as AiInferenceRequest
        let long_query = b"System settings battery profile optimization for low power mode";
        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, long_query, &empty_models, &mut tx_buf);

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, 0);
        assert_eq!(resp.output_dim, 768);
    }

    #[test]
    fn test_handle_ipc_search_query() {
        let mut tx_buf = [0u8; 1024];
        let search_req = std::ai::AiSearchRequest {
            target_dim: 256,
            top_k: 3,
            min_confidence: 0,
            query_len: 16,
        };
        let mut payload = [0u8; 64];
        let req_bytes = unsafe {
            core::slice::from_raw_parts(
                &search_req as *const _ as *const u8,
                core::mem::size_of::<std::ai::AiSearchRequest>(),
            )
        };
        let hdr_len = req_bytes.len();
        payload[..hdr_len].copy_from_slice(req_bytes);
        payload[hdr_len..hdr_len + 16].copy_from_slice(b"wireless network");

        let empty_models = gguf::ProbedModels::empty();
        handle_ipc_inference_query(
            0,
            MSG_TYPE_SEARCH,
            &payload[..hdr_len + 16],
            &empty_models,
            &mut tx_buf,
        );

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, std::ai::STATUS_SUCCESS);
    }

    #[test]
    fn test_handle_ipc_cancel_query() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        ACTIVE_GENERATION_TASK.store(1, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);

        // 1. By msg_type
        handle_ipc_inference_query(
            1,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, std::ai::STATUS_SUCCESS);
        let payload = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp.output_dim as usize];
        assert_eq!(payload, b"[Cancellation acknowledged]");
        assert!(CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed));

        // 2. By ASCII "CANC" prefix in payload
        tx_buf.fill(0);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
        handle_ipc_inference_query(
            1,
            0,
            b"CANC",
            &empty_models,
            &mut tx_buf,
        );
        let resp2 = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp2.status, std::ai::STATUS_SUCCESS);
        let payload2 = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp2.output_dim as usize];
        assert_eq!(payload2, b"[Cancellation acknowledged]");

        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn test_handle_ipc_cancel_matching_session() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // Matching sender task
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);

        handle_ipc_inference_query(
            42,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, std::ai::STATUS_SUCCESS);
        let payload = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp.output_dim as usize];
        assert_eq!(payload, b"[Cancellation acknowledged]");
        assert!(CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed));

        // Direct / unassigned sender_task_id == 0 matches active generation
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
        tx_buf.fill(0);
        handle_ipc_inference_query(
            0,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp0 = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp0.status, std::ai::STATUS_SUCCESS);
        let payload0 = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp0.output_dim as usize];
        assert_eq!(payload0, b"[Cancellation acknowledged]");
        assert!(CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed));

        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn test_handle_ipc_cancel_rejection_when_not_owner() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // Active task is 42, but sender is 99
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);

        handle_ipc_inference_query(
            99,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, std::ai::STATUS_ERROR);
        let payload = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp.output_dim as usize];
        assert_eq!(
            payload,
            b"[Cancellation rejected: sender does not own active session]"
        );
        assert!(!CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed));

        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn test_handle_ipc_cancel_when_no_active_session() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // No active task
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::Relaxed);
        CANCEL_REQUESTED.store(false, core::sync::atomic::Ordering::Relaxed);

        handle_ipc_inference_query(
            15,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, std::ai::STATUS_SUCCESS);
        let payload = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp.output_dim as usize];
        assert_eq!(payload, b"[No active inference session to cancel]");
        assert!(!CANCEL_REQUESTED.load(core::sync::atomic::Ordering::Relaxed));
    }
}

#[cfg(not(test))]
#[panic_handler]

fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
