// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

#![no_std]

// WHY `alloc` is linked here and nowhere else in this crate's dependants: the
// inference engine's scratch and KV cache are cut from the service's own
// `CapType::MemoryHeap` mapping, so `inference` needs `Vec`.
extern crate alloc;

// WHY the allocator is declared in the library and not in `src/bin/aid.rs`:
// a `#[global_allocator]` has to be in scope for the binary that links an
// allocating library, so putting it in `aid.rs` leaves the other ten daemons of
// this crate without one and they fail to compile -- which is the guarantee that
// keeps them honest, and it is kept by the kernel rather than by the link step:
// the grant lives in `kernel/src/services.rs::service_capability_policy`, where
// only `ServiceId::Inference` receives `MemoryHeap`. A daemon that reached an
// allocating path would get `PermissionDenied` from `sys_mem_map`, a null block
// from `InterOsHeap`, and an aborted task; none of them has such a path.
//
// `#[cfg(not(test))]` because the host unit suite links std and takes std's
// allocator, so the same engine code runs there; on the device the same code
// runs against the mapping-backed heap. Nothing about the budget is asserted by
// the host that the device allocator does not also have to satisfy.
#[cfg(not(test))]
#[global_allocator]
static HEAP: interos_std::heap::InterOsHeap = interos_std::heap::InterOsHeap::new();

use interos_std as std;

pub mod embedding;
pub mod gguf;
pub mod inference;
pub mod rag;
pub mod tokenizer;
pub mod weights;

pub use embedding::{
    compute_embedding_768, compute_text_embedding_dim_f32, compute_text_embedding_f32,
    compute_text_embedding_q7, f32_sqrt, EMBEDDING_DIM, MATRYOSHKA_DIMS,
};
pub use gguf::{
    find_installable_model, find_metadata_string, find_tensor, find_tensor_stream, listed_size,
    parse_tensor_info, parse_tensor_info_stream, probe_all_models, probe_embedding_model,
    probe_generative_model, probe_gguf_header, read_gguf_header, read_model_info,
    read_tokenizer_info, scan_file_for_tensor, tensor_byte_size, BufferedSource, GgufError,
    GgufHeader, GgufStream, GgufTensorInfo, GgufTokenizerInfo, ProbedModels, StringReading,
    TemplateSource, VocabReading, DEFAULT_CHAT_TEMPLATE, GGUF_MAGIC, MAX_CHAT_TEMPLATE_LEN,
    MAX_STRING_ARRAY_ELEMENTS, METADATA_WINDOW_BYTES, MODEL_SOURCE_TREES, PATH_EMBEDDING_MODEL_DATA,
    PATH_EMBEDDING_MODEL_SYSTEM, PATH_GENERATIVE_MODEL_DATA, PATH_GENERATIVE_MODEL_SYSTEM,
    PATH_LOCAL_MODEL_DATA, PROMPT_MARKER,
};
pub use inference::{
    apply_rope, dequantize_q4_0, dequantize_q8_0, extract_embedding, forward_step, gemv, generate,
    rms_norm, sample_token, softmax, swiglu, DEFAULT_SEQ_LEN, InferenceError, KvCache,
    LayerWeights, ModelConfig, ModelWeights, Scratch, FLOAT_BYTES, SCRATCH_BYTE_BUDGET,
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
    MSG_TYPE_GENERATE_CHUNK, MSG_TYPE_GGUF, MSG_TYPE_INSTALL, MSG_TYPE_INSTALL_CHUNK,
    MSG_TYPE_SEARCH, MSG_TYPE_SEARCH_CHUNK, STATUS_BASELINE_LEXICAL, STATUS_CANCELLED,
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

/// How many bytes of a model file an install moves per filesystem call.
///
/// WHY the copy steps rather than taking the whole file at once: the kernel's
/// per-call ceiling is far below a model file, and a copy that stopped at that
/// ceiling would report a fragment as a whole model.
const INSTALL_CHUNK_BYTES: usize = 2048;

/// Moves the model file the device already holds as a document into the model
/// namespace, and reports the bytes that reached the device.
///
/// WHY this service does the copy: `aid` is the service a model's words reach
/// and it is given the filesystem read-only, so an install that widened that
/// authority would put a write door in front of everything the model is fed.
/// The storage service is the one service the platform grants filesystem write
/// to, and it holds no IPC send authority, so it can neither be told an
/// arbitrary path nor answer anyone -- which is what keeps this operation to
/// moving one named file out of the person's own documents.
///
/// WHY nothing is reported as installed unless the whole file moved: the record
/// the model loader reads is a length, and a length that stops part-way through
/// a copy claims bytes the device never received.
pub fn install_local_model() -> Option<u64> {
    let mut source = [0u8; 96];
    let (path_len, stated_size) = gguf::find_installable_model(&mut source)?;
    let Ok(path) = core::str::from_utf8(&source[..path_len]) else {
        return None;
    };
    let Ok(source_fd) = std::fs_open(path, std::FS_READ) else {
        return None;
    };
    let Ok(target_fd) = std::fs_open(gguf::PATH_LOCAL_MODEL_DATA, std::FS_WRITE) else {
        let _ = std::fs_close(source_fd);
        let _ = std::log("[storaged] the model namespace would not open for writing.");
        return None;
    };

    let mut copied = 0u64;
    loop {
        let mut chunk = [0u8; INSTALL_CHUNK_BYTES];
        let read = match std::fs_read(source_fd, &mut chunk) {
            Ok(n) => n,
            Err(_) => break,
        };
        if read == 0 {
            break;
        }
        match std::fs_write(target_fd, &chunk[..read]) {
            Ok(written) if written == read => copied += written as u64,
            _ => break,
        }
    }
    let _ = std::fs_close(source_fd);
    // WHY the close is where the file becomes visible: the record naming a
    // device-held payload carries no bytes of its own, and the snapshot that
    // publishes it is written as the descriptor closes.
    let _ = std::fs_close(target_fd);

    if copied == 0 || copied < stated_size {
        let _ = std::log("[storaged] the install stopped part-way through the copy.");
        return None;
    }
    let _ = std::log("[storaged] installed the model file the device holds.");
    Some(copied)
}

pub fn run_storage() -> ! {
    let _ = std::log("[storaged] userspace storage manager online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let write_slot = std::cap_lookup(std::cap_type::FILE_SYSTEM_WRITE, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    let mut health = [0u8; 48];
    let mut request = [0u8; 64];

    loop {
        let _ = std::service_heartbeat(3);
        // WHY an install is taken and not answered: this service holds the
        // filesystem write the request needs and no send authority, so it has
        // no way to reply. The requester reads the result off the model
        // directory, which is the one measurement that says whether the bytes
        // are on the device.
        if ipc_slot.is_some() {
            while let Ok((_, msg_type, _)) = std::ipc::receive_any(&mut request) {
                if msg_type == std::ai::MSG_TYPE_INSTALL {
                    let _ = install_local_model();
                }
            }
        }
        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 1000 || last_tick == 0 {
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
            let _ = std::event_poll(std::EVENT_TIMER_TICK | std::EVENT_IPC_MSG, 20);
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
    let mut probed_models = gguf::probe_all_models();
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

        // WHY the probe is re-run here: an install puts a file in the namespace
        // this probe reads, and a probe only taken at startup keeps answering
        // "no weights" about a model the service itself just installed.
        if MODEL_NAMESPACE_CHANGED.swap(false, core::sync::atomic::Ordering::Relaxed) {
            probed_models = gguf::probe_all_models();
            let _ = std::log("[aid] re-probed the model namespace after an install.");
        }

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
///
/// WHY the header counts bytes and not tokens: every answer this service sends
/// reports the length of what it attached, and a reader can tell a whole answer
/// from a cut one only by comparing that number with what arrived.
fn emit_inference_response(
    sender_task_id: u64,
    status: u32,
    message: &[u8],
    tx_buf: &mut [u8; 1024],
) {
    emit_response_as(sender_task_id, status, MSG_TYPE_GENERATE, message, tx_buf);
}

/// Sends a notice in the answer layout under a message type the reader chose.
///
/// WHY the type is a parameter: a reader that cannot tell an install report from
/// a model's words would paint "installed" as though the model said so.
fn emit_response_as(
    sender_task_id: u64,
    status: u32,
    msg_type: u32,
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
            let _ = std::ipc_send(sender_task_id, msg_type, &tx_buf[..total_len]);
        }
    }
}

/// The claim that an answer is waiting on a shared-memory channel.
///
/// WHY a channel and not the mailbox: an IPC message carries 64 bytes and the
/// kernel truncates rather than refusing, so an answer of the size a model
/// produces reached the app cut off with no sign that it had been -- and the
/// service's own failure reasons are already past that limit. The bytes go over
/// one of the dynamic pipes the platform provides; only this 16-byte notice
/// travels by IPC, and it names the channel, the answer's length, and how many
/// bytes actually made it.
///
/// Returns false when there is no app to answer, no channel to claim, or the
/// notice itself was refused; the caller then replies inline as it always did.
/// A short stream is reported as delivered, because the notice carries the
/// shortfall, which is the honest answer where an inline reply is a silent cut.
///
/// WHY the notice type is a parameter: an answer and a retrieval frame are read
/// by different readers, and a reader that cannot tell which it was handed will
/// paint a document path as the model's words.
fn stream_answer(sender_task_id: u64, status: u32, notice_type: u32, message: &[u8]) -> bool {
    if sender_task_id == 0 {
        return false;
    }
    let pipe_id = match claim_answer_pipe(sender_task_id as u32) {
        Some(pipe_id) => pipe_id,
        None => return false,
    };

    let total = message.len();
    let mut streamed = 0usize;
    while streamed < total {
        let end = (streamed + std::ai::ANSWER_SLOT_BYTES).min(total);
        if std::shm_pipe_transfer(pipe_id, &message[streamed..end], 0).is_err() {
            break;
        }
        streamed = end;
    }

    let notice = std::ai::AiStreamNotice {
        status,
        pipe_id,
        streamed_len: streamed as u32,
        total_len: total as u32,
    };
    let mut wire = [0u8; std::ai::AiStreamNotice::WIRE_LEN];
    if notice.to_bytes(&mut wire) == 0 {
        return false;
    }
    if std::ipc_send(sender_task_id, notice_type, &wire).is_err() {
        return false;
    }
    if streamed != total {
        let _ = std::log("[aid] answer channel filled before the whole answer streamed.");
    }
    true
}

/// Claims one of the dynamic answer channels for this service-and-app pair.
///
/// WHY a search rather than a fixed id: the channels are few and the pair that
/// holds one keeps it until either endpoint exits, so the next app to ask is
/// answered by whichever channel is free.
fn claim_answer_pipe(consumer_task_id: u32) -> Option<u32> {
    let last = std::ai::ANSWER_PIPE_FIRST + std::ai::ANSWER_PIPE_COUNT;
    (std::ai::ANSWER_PIPE_FIRST..last).find(|pipe_id| std::shm_pipe_create(*pipe_id, 1, consumer_task_id).is_ok())
}

/// Bounded writer for the plain-text notices the service answers with.
///
/// WHY it exists: an honest notice names numbers -- the dimensions a model file
/// states and the bounds the engine is compiled to -- and formatting those by
/// hand at each call site is one chance per site to get a unit wrong.
struct Notice<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Notice<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn word(&mut self, text: &[u8]) {
        let take = text
            .len()
            .min(self.buf.len().saturating_sub(self.len));
        self.buf[self.len..self.len + take].copy_from_slice(&text[..take]);
        self.len += take;
    }

    fn number(&mut self, value: u32) {
        let mut digits = [0u8; 10];
        let written = rag::format_u32(value, &mut digits);
        self.word(&digits[..written]);
    }

    /// Writes a byte count, which is larger than what `number` can hold.
    ///
    /// WHY a second width: a model file is measured in the millions of bytes and
    /// a number that overflowed its digits would report a smaller install than
    /// the device actually holds.
    fn byte_count(&mut self, value: u64) {
        let mut digits = [0u8; 20];
        let mut filled = digits.len();
        let mut remaining = value;
        loop {
            filled -= 1;
            digits[filled] = b'0' + (remaining % 10) as u8;
            remaining /= 10;
            if remaining == 0 {
                break;
            }
        }
        self.word(&digits[filled..]);
    }

    /// Hands the buffer back with its lifetime, not the writer's.
    fn finish(self) -> &'a [u8] {
        let Notice { buf, len } = self;
        &buf[..len]
    }
}
/// Why the generative path reached the model file and still ran nothing.
enum ModelGap {
    /// The file configured the engine, and this reply read no tensor of it.
    ///
    /// WHY the sentence is about the reply and not about the engine: `weights` now
    /// reads an installed file's tensors block by block and runs the graph over them
    /// -- measured against the 333 MB file by
    /// `weights::tests::test_the_installed_file_is_read_block_by_block_and_run` --
    /// so "the engine does not read its tensors" stopped being true. This reply still
    /// reads none of them, which is the fact the screen has to carry, and is the
    /// narrower claim a reader can check from this call site.
    Configured,
    /// The file parsed and declares no tensor at all -- metadata and nothing else,
    /// which is what a placeholder install leaves on the disk.
    NoTensors,
    /// The file states a dimension -- or omits one -- the engine cannot be built
    /// from. Carries the key, what the file stated, and the engine's bound.
    Declaration(&'static str, Option<u32>, usize),
    /// The shape the file declares costs more scratch than the device's byte
    /// budget. Carries both numbers -- what the run would have taken and the
    /// budget it was refused against -- because after the budget there is no
    /// per-dimension bound left to name, and "too big" without a byte count is
    /// not an answer the next stage can act on.
    Scratch(usize, usize),
    /// The budget said the shape fits and the memory capability still would not
    /// hand the mapping out.
    ScratchUnavailable,
}

/// Writes what the tokenizer walk reached in the same model file.
///
/// WHY it names what the file did not declare: the engine has a built-in template
/// and a built-in default vocabulary width, and a screen that showed only the
/// dimensions would leave a person guessing which of the two the answer ran on.
/// Every clause here is a statement about the file -- read, counted, refused for
/// length, or absent -- and never a restating of what the engine fell back to as
/// though the file had said it.
fn tokenizer_clause(tok: &gguf::GgufTokenizerInfo, notice: &mut Notice) {
    if tok.declares_no_tokenizer() {
        notice.word(b", tokenizer absent, built-in template in use");
        if let Some(key) = tok.truncated_in {
            notice.word(b", file ended inside ");
            notice.word(key);
        }
        return;
    }
    if tok.vocab_counted_not_loaded() {
        notice.word(b", vocabulary counted not loaded");
    }
    match tok.template_reading {
        gguf::StringReading::Read(len) => {
            notice.word(b", template ");
            notice.number(len as u32);
            notice.word(b" bytes from file");
        }
        gguf::StringReading::Absent => notice.word(b", template built-in default"),
        gguf::StringReading::RefusedTooLong(stated) => {
            notice.word(b", template ");
            notice.byte_count(stated);
            notice.word(b" bytes refused");
        }
    }
    if tok.bos_token_id.is_some() || tok.eos_token_id.is_some() || tok.pad_token_id.is_some() {
        notice.word(b", bos ");
        notice.number(tok.bos_token_id.unwrap_or(u32::MAX));
        notice.word(b" eos ");
        notice.number(tok.eos_token_id.unwrap_or(u32::MAX));
        notice.word(b" pad ");
        notice.number(tok.pad_token_id.unwrap_or(u32::MAX));
    } else {
        notice.word(b", no token ids");
    }
    if let Some(declared) = tok.add_bos_token {
        notice.word(if declared { b", add-bos 1" } else { b", add-bos 0" });
    }
    if let Some(pre) = tok.pre_name() {
        notice.word(b", pre ");
        notice.word(pre.as_bytes());
    }
    if let Some(key) = tok.truncated_in {
        notice.word(b", file ended inside ");
        notice.word(key);
    }
}

/// Writes what the file said about itself, field by field.
///
/// WHY every clause is a statement about one reading: the name a model is
/// reported by used to come from the path the bytes were opened at, so a file
/// renamed `embeddinggemma-2.gguf` was answered as EmbeddingGemma 2 whatever its
/// own metadata claimed. Each clause here has one of five answers and they are not
/// the same answer: the file declared it, the file named no such key, the file
/// stated a value this reader will not hold, the value it stated is not text, or
/// the key sits past where the walk stops. Printing a filename stem, an
/// architecture's vendor name or the dimensions of a test fixture in place of any
/// of those is the invented model this repository's truth rule forbids.
///
/// WHY the name is the clause that carries the model's name: `general.name` is the
/// field its author filled in, and the architecture is the family the tensors are
/// laid out for. They are both quoted, and neither is inferred.
fn identity_clause(identity: &inference::ModelIdentity<'_>, notice: &mut Notice) {
    use inference::{NumField, StrField};

    // The first clause joins the subject with a space; every clause after it is a
    // member of the same list and carries its own comma, so the sentence stays
    // readable whichever of the five readings each field answers with.
    match identity.name {
        StrField::Declared("") => notice.word(b" named nothing"),
        StrField::Declared(name) => {
            notice.word(b" named ");
            notice.word(name.as_bytes());
        }
        StrField::Absent => notice.word(b" names no model"),
        StrField::RefusedTooLong(stated) => {
            notice.word(b" name ");
            notice.byte_count(stated);
            notice.word(b" bytes refused");
        }
        StrField::NotText(stated) => {
            notice.word(b" name ");
            notice.byte_count(stated);
            notice.word(b" bytes not text");
        }
        StrField::NotReached => notice.word(b" name past the walk's stop"),
    }
    match identity.architecture {
        StrField::Declared("") => notice.word(b", states an empty architecture"),
        StrField::Declared(arch) => {
            notice.word(b", declares ");
            notice.word(arch.as_bytes());
        }
        StrField::Absent => notice.word(b", states no architecture"),
        StrField::RefusedTooLong(stated) => {
            notice.word(b", architecture ");
            notice.byte_count(stated);
            notice.word(b" bytes refused");
        }
        StrField::NotText(stated) => {
            notice.word(b", architecture ");
            notice.byte_count(stated);
            notice.word(b" bytes not text");
        }
        StrField::NotReached => notice.word(b", architecture past the walk's stop"),
    }
    match identity.tokenizer_model {
        StrField::Declared("") => notice.word(b", tokenizer model empty"),
        StrField::Declared(model) => {
            notice.word(b", tokenizer model ");
            notice.word(model.as_bytes());
        }
        StrField::Absent => notice.word(b", tokenizer model undeclared"),
        StrField::RefusedTooLong(stated) => {
            notice.word(b", tokenizer model ");
            notice.byte_count(stated);
            notice.word(b" bytes refused");
        }
        StrField::NotText(stated) => {
            notice.word(b", tokenizer model ");
            notice.byte_count(stated);
            notice.word(b" bytes not text");
        }
        StrField::NotReached => notice.word(b", tokenizer model past the walk's stop"),
    }
    // The quantisation the file declares for its tensors, as the number the file
    // states it by. WHY the number and not a name: the reader has no table here
    // that would turn a `7` into "Q8_0" and report that word as the file's own.
    match identity.quantization_type {
        NumField::Declared(kind) => {
            notice.word(b", quantisation type ");
            notice.byte_count(kind);
        }
        NumField::Absent => notice.word(b", quantisation type undeclared"),
        NumField::NotReached => notice.word(b", quantisation type past the walk's stop"),
    }
    match identity.tensor_count {
        NumField::Declared(tensors) => {
            notice.word(b", ");
            notice.byte_count(tensors);
            notice.word(b" tensors");
        }
        NumField::Absent | NumField::NotReached => notice.word(b", tensors uncounted"),
    }
    match identity.embedding_length {
        NumField::Declared(width) => {
            notice.word(b", embedding length ");
            notice.byte_count(width);
        }
        NumField::Absent => notice.word(b", embedding length undeclared"),
        NumField::NotReached => notice.word(b", embedding length past the walk's stop"),
    }
}

/// The window every `model_gap_notice` is written into.
///
/// WHY one number for every arm: each arm used to carry its own literal, and the
/// literal that survived was the one someone had measured against the sentence a
/// test fixture produced, not the widest sentence the reader can be handed. The
/// identity clause quotes the name, the architecture and the tokenizer family at
/// the lengths `gguf` caps them to, so the worst case is set by those caps and by
/// the digit counts of every number the notice can write -- and it is the same
/// worst case whichever arm writes it, because the identity clause opens all of
/// them. `test_the_longest_notice_still_fits_the_window_it_is_written_in` measures
/// it, per arm, from the widest file the reader can report.
///
/// WHY 768 and not "as small as possible": the answer travels the shared-memory
/// pipe in one `ANSWER_SLOT_BYTES` slot and the reader holds a slot, so a window
/// at or under 1024 reaches the app whole. A narrower window that cut a sentence
/// mid-word would be the defect this notice exists to fix.
const NOTICE_WINDOW_BYTES: usize = 768;

/// Builds the notice that answers a generative query when the model file is on
/// the disk and nothing was run from it.
///
/// WHY it quotes the file: "no local model weights loaded" was the reply a real
/// Gemma got too, because the code that loads weights was never reached. Naming
/// the identity the file declares and the dimensions it states says which of the
/// cases the user is in, and writes down the number the next stage has to satisfy.
/// Counting the prompt's bytes says the words arrived, which is the one thing
/// the user can no longer infer from an answer that never came.
///
/// WHY the tokenizer is a parameter and not a field of `info`: the two walks stop
/// for different reasons -- one at the vocabulary, one past it -- and a notice
/// built without the tokenizer walk still says everything it knows, which is what
/// the `None` callers are.
fn model_gap_notice<'a>(
    info: &gguf::GgufModelInfo,
    gap: ModelGap,
    prompt: &[u8],
    tokenizer: Option<&gguf::GgufTokenizerInfo>,
    out: &'a mut [u8],
) -> &'a [u8] {
    let mut notice = Notice::new(out);
    notice.word(b"Model file");
    identity_clause(&inference::ModelIdentity::from_gguf(info), &mut notice);
    match gap {
        ModelGap::Configured | ModelGap::NoTensors => {
            // WHY no `unwrap_or(0)`: a file that states no block count is not a
            // file with zero blocks, and printing the engine's own empty number
            // as though the file had given it is the invention this notice is for.
            match info.num_layers {
                Some(blocks) => {
                    notice.word(b", over ");
                    notice.number(blocks);
                    notice.word(b" blocks");
                }
                None => notice.word(b", block count undeclared"),
            }
            // WHY the vocabulary is named as undeclared when the file never stated
            // it: "vocabulary 0 tokens" is a number the file did not give, and a
            // stripped embedding model is the case that reads it.
            match info.vocab_size {
                Some(tokens) => {
                    notice.word(b", vocabulary ");
                    notice.number(tokens);
                    notice.word(b" tokens");
                }
                None => notice.word(b", vocabulary undeclared"),
            }
            // WHY two endings for one shape: a file whose tensors the engine cannot
            // read and a file that holds no tensors at all call for different next
            // moves, and only one of them is a placeholder install.
            //
            // WHY the first of them names the reply and not the engine: while no
            // loader existed, "the engine does not read its tensors" was the honest
            // reason this arm answers nothing. `weights` now reads an installed
            // file's tensors block by block and runs the graph over what it read, so
            // that sentence is a false claim about capability. What this call site
            // can still attest is that this reply reads none of them, and saying less
            // than that would be deleting the lie so a gate could pass.
            if matches!(gap, ModelGap::NoTensors) {
                notice.word(b", and holds no tensors");
            } else {
                notice.word(b", none of whose tensors this reply reads");
            }
        }
        ModelGap::Declaration(key, Some(stated), limit) => {
            notice.word(b", and states ");
            notice.word(key.as_bytes());
            notice.word(b" as ");
            notice.number(stated);
            notice.word(b" against the engine's bound of ");
            notice.number(limit as u32);
        }
        ModelGap::Declaration(key, None, _) => {
            notice.word(b", and states no ");
            notice.word(key.as_bytes());
        }
        ModelGap::Scratch(needed, budget) => {
            notice.word(b", whose run needs ");
            notice.byte_count(needed as u64);
            notice.word(b" scratch bytes against the ");
            notice.byte_count(budget as u64);
            notice.word(b" byte budget, over by ");
            notice.byte_count(needed.saturating_sub(budget) as u64);
            notice.word(b" bytes");
        }
        ModelGap::ScratchUnavailable => {
            notice.word(b", whose scratch the byte budget prices as fitting but the device would not map");
        }
    }
    if let Some(tok) = tokenizer {
        tokenizer_clause(tok, &mut notice);
    }
    notice.word(b"; the prompt's ");
    notice.number(prompt.len() as u32);
    notice.word(b" bytes were not answered.");
    notice.finish()
}

/// Answers a generative query, streaming the bytes when the mailbox cannot hold them.
fn reply_with_answer(
    sender_task_id: u64,
    status: u32,
    message: &[u8],
    tx_buf: &mut [u8; 1024],
) {
    if !stream_answer(sender_task_id, status, MSG_TYPE_GENERATE_CHUNK, message) {
        emit_inference_response(sender_task_id, status, message, tx_buf);
    }
}

static ACTIVE_GENERATION_TASK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static CANCEL_REQUESTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Set when a model file has reached the model namespace since the last probe.
///
/// WHY a flag and not a re-probe inside the answer: the probe parses a header at
/// each candidate path, and the install answer is a measurement of a directory,
/// not of an engine. The service loop re-probes on its next pass, so an install
/// takes effect without the service being restarted -- which is the difference
/// between the screen saying "no weights" about a file it installed a moment ago.
static MODEL_NAMESPACE_CHANGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// How many times an install answer re-reads the model namespace before it
/// reports what it found.
///
/// WHY a bound: the service that copies the file holds no send authority and
/// cannot acknowledge, so the only sign that bytes arrived is the directory's own
/// listing -- and this service answers every other query from the same loop, so
/// waiting for one install cannot become an unbounded wait.
const INSTALL_WAIT_PASSES: usize = 512;

/// Asks the storage service to move a model file, then answers with what the
/// model namespace is measured to hold.
///
/// WHY the answer is a measurement rather than a relay: nothing comes back from
/// the storage service, so any reason given here -- no space, permission denied,
/// a bad path -- would be invented. What can be stated is how many bytes the
/// model namespace holds against how many the source file's listing stated, which
/// is the whole difference between "the file is on the device" and "the copy
/// stopped part-way through".
fn install_model_file(sender_task_id: u64, tx_buf: &mut [u8; 1024]) {
    let mut source = [0u8; 96];
    let outcome = match gguf::find_installable_model(&mut source) {
        None => InstallOutcome::NothingToInstall,
        Some((_path_len, stated_size)) => {
            if std::ipc_send(std::ai::SERVICE_STORAGE_TASK_ID, MSG_TYPE_INSTALL, b"INSL").is_err()
            {
                InstallOutcome::RequestRefused
            } else {
                let mut measured = gguf::local_model_size();
                for _ in 1..INSTALL_WAIT_PASSES {
                    if measured == Some(stated_size) {
                        break;
                    }
                    std::yield_now();
                    measured = gguf::local_model_size();
                }
                match measured {
                    Some(size) if size == stated_size => InstallOutcome::Installed(size),
                    Some(size) => InstallOutcome::Short(size, stated_size),
                    None => InstallOutcome::Absent(stated_size),
                }
            }
        }
    };

    if matches!(outcome, InstallOutcome::Installed(_)) {
        MODEL_NAMESPACE_CHANGED.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    let mut text = [0u8; 256];
    let (status, message) = install_notice(outcome, &mut text);
    if !stream_answer(sender_task_id, status, MSG_TYPE_INSTALL_CHUNK, message) {
        emit_response_as(sender_task_id, status, MSG_TYPE_INSTALL, message, tx_buf);
    }
}

/// What an install can honestly say about the model namespace.
enum InstallOutcome {
    /// No source tree holds a file whose header parses as a model.
    NothingToInstall,
    /// The kernel would not deliver the request to the storage service.
    RequestRefused,
    /// The namespace reports exactly the bytes the source file stated.
    Installed(u64),
    /// The namespace reports fewer bytes than the source file stated.
    Short(u64, u64),
    /// The namespace reports no model file at all.
    Absent(u64),
}

/// Pairs an install outcome with the status code and sentence it is reported as.
///
/// WHY the pairing is one function: the two numbers it puts next to each other --
/// what the source file's listing stated and what the model namespace now reports
/// -- are the entire claim, and a sentence that said "installed" over a copy that
/// stopped part-way is exactly the lie the Platform Truth Contract forbids.
fn install_notice(outcome: InstallOutcome, out: &mut [u8]) -> (u32, &[u8]) {
    let mut notice = Notice::new(out);
    let status = match &outcome {
        InstallOutcome::NothingToInstall => {
            notice.word(
                b"Nothing to install: the download and incoming folders hold no readable .gguf.",
            );
            std::ai::STATUS_UNAVAILABLE
        }
        InstallOutcome::RequestRefused => {
            notice.word(b"Install refused: the storage service would not take the request.");
            std::ai::STATUS_ERROR
        }
        InstallOutcome::Installed(size) => {
            notice.word(b"Model file installed: ");
            notice.byte_count(*size);
            notice.word(b" bytes at /data/models/local_model.gguf.");
            std::ai::STATUS_SUCCESS
        }
        InstallOutcome::Short(measured, stated) => {
            notice.word(b"Install incomplete: the model namespace holds ");
            notice.byte_count(*measured);
            notice.word(b" of the ");
            notice.byte_count(*stated);
            notice.word(b" bytes the file states.");
            std::ai::STATUS_ERROR
        }
        InstallOutcome::Absent(stated) => {
            notice.word(
                b"Install did not reach the device: the model namespace holds no file of the ",
            );
            notice.byte_count(*stated);
            notice.word(b" bytes the source states.");
            std::ai::STATUS_ERROR
        }
    };
    (status, notice.finish())
}

// ---------------------------------------------------------------------------
// The embedding arm's branches: which one runs, and what its reply may claim.
//
// WHY the decision is a function of inputs rather than a chain of `if`s inside
// the arm: the arm used to answer the lexical projection with `status: 0` and a
// caller could not tell it from weights. A status chosen at the point a vector
// is emitted is a status a reader cannot check ahead of time, so the three
// outcomes are named here, the decision that picks between them reads no file
// and makes no syscall, and `host_test_the_branch_selection` proves the mapping
// from inputs to branch. The arm then reports whichever branch it executed.
// ---------------------------------------------------------------------------

/// The longest token-id stream one embedding request is answered with.
///
/// WHY a bound: the streamed run steps the whole graph once per position, so an
/// unbounded stream is an unbounded request -- and `aid` answers every other
/// query from the same loop that request blocks.
const EMBED_TOKEN_IDS_MAX: usize = 32;

/// Which of the three answers an embedding request gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingPlan {
    /// The installed file's tensors, read block by block and run over the token
    /// ids the caller supplied. Answers `STATUS_SUCCESS`.
    RunInstalledFile,
    /// `embedding::compute_embedding_768`, the lexical hash projection. Answers
    /// `STATUS_BASELINE_LEXICAL` -- the status the projection always deserved and
    /// never carried.
    LexicalBaseline,
    /// Nothing runs; the notice names the gap.
    Refuse(EmbeddingGap),
}

/// The gap that stopped an embedding reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingGap {
    /// The caller demanded physical weights and the probe found no file.
    NoWeights,
    /// The file is on the device and the caller demanded it, but the request
    /// carries text and no ids. `tokenizer::encode` matches thirty hard-coded
    /// subwords and otherwise emits `4 + byte`: an id table nothing ever checked
    /// against the `tokenizer.ggml.tokens` array this file states, which the
    /// reader deliberately does not walk. Running the graph on ids from that
    /// guess would return real arithmetic over real weights for tokens that mean
    /// something else, and label it the model's embedding of the prompt. The gap
    /// is the vocabulary-backed mapping, and the reply says so instead of paying
    /// it with a fabricated one.
    NoVocabTokenizer,
}

/// Picks the branch from the inputs, before anything is opened.
///
/// `token_id_count` is `Some(n)` only when the request carried a whole number of
/// little-endian `u32` ids under the token-id flag and the bound accepted them.
fn plan_embedding_run(
    weights_present: bool,
    token_id_count: Option<usize>,
    strict_neural_required: bool,
) -> EmbeddingPlan {
    match (weights_present, token_id_count, strict_neural_required) {
        // Ids the caller owns plus a file the probe read: the engine runs.
        (true, Some(_), _) => EmbeddingPlan::RunInstalledFile,
        // Weights demanded, text supplied, and nothing maps one to the other.
        (true, None, true) => EmbeddingPlan::Refuse(EmbeddingGap::NoVocabTokenizer),
        (false, Some(_), true) => EmbeddingPlan::Refuse(EmbeddingGap::NoWeights),
        (false, None, true) => EmbeddingPlan::Refuse(EmbeddingGap::NoWeights),
        // Nothing demanded weights, so a labelled baseline still answers the
        // query -- which is the difference between refusing and lying.
        _ => EmbeddingPlan::LexicalBaseline,
    }
}

/// A bounded token-id stream, held on the stack rather than the heap.
#[derive(Clone, Copy)]
struct TokenIds {
    ids: [u32; EMBED_TOKEN_IDS_MAX],
    len: usize,
}

impl TokenIds {
    fn as_slice(&self) -> &[u32] {
        &self.ids[..self.len]
    }
}

/// Reads the caller's token ids, or reports that these bytes are not a stream.
///
/// `None` for an empty input, a byte count that is not a whole number of `u32`s,
/// and a stream past `EMBED_TOKEN_IDS_MAX`: all three are refusals to run rather
/// than reasons to invent ids, and none of them is text the arm should tokenize.
fn read_token_ids(input: &[u8]) -> Option<TokenIds> {
    if input.is_empty() || input.len() % 4 != 0 {
        return None;
    }
    let count = input.len() / 4;
    if count > EMBED_TOKEN_IDS_MAX {
        return None;
    }
    let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];
    for slot in 0..count {
        let chunk = &input[slot * 4..slot * 4 + 4];
        ids[slot] = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Some(TokenIds { ids, len: count })
}

/// What one file-backed embedding run measured, so the reply is a measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileEmbedReport {
    /// Hidden dimension of the graph the file's own metadata configured.
    hidden_dim: usize,
    /// Positions stepped.
    positions: usize,
    /// Blocks run across every position.
    blocks_run: usize,
    /// Tensor payloads read for the whole run.
    tensors_read: usize,
    /// Bytes of the file the run read.
    bytes_read: u64,
    /// Whether the file's `dense_2`/`dense_3` pooling head was applied. `false`
    /// means the vector is the pooled pre-head residual stream, which is not the
    /// published model's final embedding and must not be reported as one.
    head_applied: bool,
}

/// Why the installed file did not answer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FileEmbedError {
    /// The path the probe read is not openable now.
    Open,
    /// The engine could not be built from the file's own declaration.
    Config(InferenceError),
    /// The descriptor table or a tensor read refused.
    Load(weights::LoadError),
}

/// Reads the installed file one block at a time and runs it over `tokens`.
///
/// This is the hop the service had never taken on the device: `weights` had been
/// called from nowhere but its own `#[cfg(test)]` block. Everything below is the
/// loader's own code path, driven through `weights::ModelFileDescriptor` and the
/// kernel `fs_read_at` the guest answers, not through the host's `File`.
fn run_installed_embedding(
    path: &str,
    tokens: &[u32],
    out: &mut [f32],
) -> Result<FileEmbedReport, FileEmbedError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| FileEmbedError::Open)?;
    let source = weights::ModelFileDescriptor { fd };

    let walked = {
        let mut stream = GgufStream::from_file(fd);
        weights::TensorTable::walk(&mut stream)
    };
    let table = match walked {
        Ok(table) => table,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Load(error));
        }
    };

    let read = {
        let mut stream = GgufStream::from_file(fd);
        gguf::read_model_info_stream(&mut stream)
    };
    let info = match read {
        Ok(info) => info,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Load(weights::LoadError::Reader(error)));
        }
    };
    let config = match ModelConfig::from_gguf(&info) {
        Ok(config) => config,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Config(error));
        }
    };
    if config.hidden_dim > out.len() {
        let _ = std::fs_close(fd);
        return Err(FileEmbedError::Config(InferenceError::BufferTooSmall));
    }

    let mut reader = weights::WeightReader::new(table, source);
    let final_norm = match reader.load_final_norm(config.hidden_dim) {
        Ok(norm) => norm,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Load(error));
        }
    };
    let mut scratch = match Scratch::new(&config) {
        Ok(scratch) => scratch,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Config(error));
        }
    };
    let mut kv_cache = match KvCache::new(&config) {
        Ok(cache) => cache,
        Err(error) => {
            let _ = std::fs_close(fd);
            return Err(FileEmbedError::Config(error));
        }
    };
    let mut row = alloc::vec::Vec::new();
    let ran = weights::run_streamed_embedding(
        &mut reader,
        &config,
        tokens,
        &mut scratch,
        &mut kv_cache,
        &final_norm,
        &mut row,
        &mut out[..config.hidden_dim],
    );
    let _ = std::fs_close(fd);

    match ran {
        Ok((run, head_applied)) => Ok(FileEmbedReport {
            hidden_dim: config.hidden_dim,
            positions: run.positions,
            blocks_run: run.blocks_run,
            tensors_read: run.tensors_read,
            bytes_read: run.bytes_read,
            head_applied,
        }),
        Err(error) => Err(FileEmbedError::Load(error)),
    }
}

/// The words and the status that say why the installed file did not answer.
///
/// WHY one function returns both: a status chosen apart from the sentence that
/// explains it is a second mapping for the same refusal, and the two drift. Each
/// arm below names the fact, the number that proves it, and the code a reader
/// branches on, together.
///
/// WHY the codes differ at all: "this file quantises a dtype the engine has no
/// layout for", "the heap refused the cache" and "the copy stopped mid-tensor"
/// are three different facts with three different next moves -- change the file,
/// resize the run, re-install -- and a reader handed one code for all three
/// cannot tell which would help.
///
/// Pure over the refusal: it reads no file and makes no syscall, so the mapping
/// is testable from a `LoadError` value alone.
fn file_embed_notice<'a>(error: &FileEmbedError, buf: &'a mut [u8]) -> (u32, &'a [u8]) {
    let mut notice = Notice::new(buf);
    let status = match error {
        FileEmbedError::Open => {
            notice.word(
                b"Embedding refused: the probed model file could not be opened since the probe ran.",
            );
            STATUS_UNAVAILABLE
        }
        FileEmbedError::Load(weights::LoadError::UnsupportedDtype(name, dtype)) => {
            notice.word(b"Embedding refused: the file's tensor ");
            notice.word(name.as_bytes());
            notice.word(b" declares quantisation ");
            notice.number(*dtype);
            notice.word(b", a dtype this engine has no row layout for.");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::MixedBlockDtype(name, a, b)) => {
            notice.word(b"Embedding refused: the block at ");
            notice.word(name.as_bytes());
            notice.word(b" splits across quantisations ");
            notice.number(*a);
            notice.word(b" and ");
            notice.number(*b);
            notice.word(b", and one block runs with one type.");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::TensorAbsent(name, _)) => {
            notice.word(b"Embedding refused: the file holds no tensor named ");
            notice.word(name.as_bytes());
            notice.word(b", so this is not a graph the loader can build.");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::BadShape(name, stated, wanted)) => {
            notice.word(b"Embedding refused: the tensor ");
            notice.word(name.as_bytes());
            notice.word(b" declares ");
            notice.byte_count(*stated);
            notice.word(b" where the graph needs ");
            notice.byte_count(*wanted);
            notice.word(b".");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::BadAlignment(value)) => {
            notice.word(b"Embedding refused: the file states alignment ");
            notice.byte_count(*value);
            notice.word(b", which no GGUF file can place tensors with.");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::ShortRead(position, asked, got)) => {
            notice.word(b"Embedding refused: a positioned read at byte ");
            notice.byte_count(*position);
            notice.word(b" came back with ");
            notice.number(*got as u32);
            notice.word(b" of the ");
            notice.number(*asked as u32);
            notice.word(b" bytes the descriptor states -- an install that stopped part-way.");
            STATUS_ERROR
        }
        FileEmbedError::Config(InferenceError::ScratchTooLarge(needed, budget)) => {
            notice.word(b"Embedding refused: this model needs ");
            notice.byte_count(*needed as u64);
            notice.word(b" bytes of scratch against a budget of ");
            notice.byte_count(*budget as u64);
            notice.word(b".");
            STATUS_UNAVAILABLE
        }
        FileEmbedError::Config(InferenceError::ScratchUnavailable) => {
            notice.word(
                b"Embedding refused: the service heap refused the scratch this shape prices.",
            );
            STATUS_UNAVAILABLE
        }
        FileEmbedError::Config(InferenceError::ModelDeclaration(key, stated, limit)) => {
            notice.word(b"Embedding refused: the file declares ");
            notice.word(key.as_bytes());
            notice.word(b" as ");
            match stated {
                Some(value) => notice.byte_count(*value as u64),
                None => notice.word(b"nothing"),
            }
            notice.word(b" where the engine is bounded at ");
            notice.byte_count(*limit as u64);
            notice.word(b".");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Config(other) => {
            notice.word(b"Embedding refused: the engine could not be built from this file (");
            notice.word(inference_error_word(other));
            notice.word(b").");
            STATUS_UNSUPPORTED
        }
        FileEmbedError::Load(weights::LoadError::Reader(error)) => {
            notice.word(b"Embedding refused: the file's descriptors did not parse (");
            notice.word(gguf_error_word(error));
            notice.word(b").");
            STATUS_ERROR
        }
        FileEmbedError::Load(weights::LoadError::Graph(error)) => {
            notice.word(b"Embedding refused: the graph stopped (");
            notice.word(inference_error_word(error));
            notice.word(b").");
            STATUS_ERROR
        }
        FileEmbedError::Load(weights::LoadError::Arithmetic) => {
            notice.word(
                b"Embedding refused: a tensor's size did not fit this device's address width.",
            );
            STATUS_ERROR
        }
    };
    (status, notice.finish())
}

/// A short name for an engine failure the loader surfaced whole.
fn inference_error_word(error: &InferenceError) -> &'static [u8] {
    match error {
        InferenceError::BufferTooSmall => b"buffer too small",
        InferenceError::CorruptedWeights => b"corrupted weights",
        InferenceError::ScratchUnavailable => b"scratch unavailable",
        _ => b"unsupported configuration",
    }
}

/// A short name for a reader failure the loader surfaced whole.
fn gguf_error_word(error: &crate::gguf::GgufError) -> &'static [u8] {
    use crate::gguf::GgufError as E;
    match error {
        E::BufferTooShort => b"buffer too short",
        E::InvalidMagic => b"not a GGUF file",
        E::UnsupportedVersion(_) => b"unsupported GGUF version",
        E::FileNotFound => b"file not found",
        E::ReadFailed => b"read failed",
        E::NoModelsFound => b"no model found",
        E::InvalidMetadata => b"invalid metadata",
        E::ArithmeticOverflow => b"arithmetic overflow",
        E::CorruptedTensor => b"corrupted tensor",
        E::InvalidAlignment(_) => b"invalid alignment",
        E::MissingMetadata(..) => b"missing metadata",
    }
}

/// Why the arm ran nothing at all.
fn embedding_gap_notice<'a>(gap: EmbeddingGap, buf: &'a mut [u8]) -> (u32, &'a [u8]) {
    let mut notice = Notice::new(buf);
    let status = match gap {
        EmbeddingGap::NoWeights => {
            notice.word(
                b"Embedding refused: the strict-neural flag is set and the model namespace holds no weights file the probe read.",
            );
            STATUS_UNAVAILABLE
        }
        EmbeddingGap::NoVocabTokenizer => {
            notice.word(
                b"Embedding refused: the strict-neural flag is set and this request carries text, not token ids. No mapping from text to this file's vocabulary is wired -- the vocabulary array is never walked and the built-in id table is a guess checked against nothing -- so the file's tensors cannot be run over this prompt. Send a token-id stream, or drop the flag and take the labelled lexical baseline.",
            );
            STATUS_UNSUPPORTED
        }
    };
    (status, notice.finish())
}

/// Answers an embedding request with the lexical projection, labelled as such.
///
/// WHY a function: three branches can land here (no file, file plus text, and a
/// planner change), and every one of them must carry the same status. The bug
/// this closes was one of them carrying `STATUS_SUCCESS` instead.
fn reply_lexical_baseline(
    sender_task_id: u64,
    query_text: &[u8],
    target_dim: usize,
    latency_micros: u64,
    tx_buf: &mut [u8; 1024],
) {
    let mut q7_out = [0i8; EMBEDDING_DIM];
    embedding::compute_embedding_768(query_text, target_dim, None, Some(&mut q7_out));

    let resp = std::ai::AiInferenceResponse {
        status: STATUS_BASELINE_LEXICAL,
        output_dim: target_dim as u32,
        latency_micros,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };
    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
    };
    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);

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

/// Dispatches incoming IPC queries for `SERVICE_INFERENCE` (#12) with zero heap allocation.
pub fn handle_ipc_inference_query(
    sender_task_id: u64,
    msg_type: u32,
    payload: &[u8],
    probed_models: &gguf::ProbedModels,
    tx_buf: &mut [u8; 1024],
) {
    // WHY the install is answered before anything is read as a prompt: the
    // request is four bytes long, and a reader that fell through to the query
    // paths would answer it with a model notice about an empty prompt.
    if msg_type == MSG_TYPE_INSTALL || (payload.len() >= 4 && &payload[..4] == b"INSL") {
        install_model_file(sender_task_id, tx_buf);
        return;
    }
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
                            // The file's own declaration says what the engine is
                            // sized to, and the answer says what stopped it.
                            let prompt_text = extract_prompt_text(payload);
                            match gguf::read_model_info(path) {
                                Ok(info) => {
                                    let gap = match inference::ModelConfig::from_gguf(&info) {
                                        Ok(_) => ModelGap::Configured,
                                        Err(inference::InferenceError::ModelDeclaration(
                                            key,
                                            stated,
                                            limit,
                                        )) => ModelGap::Declaration(key, stated, limit),
                                        // The one size limit the engine carries now:
                                        // the byte budget, answered with both numbers
                                        // and the overage rather than "too big".
                                        Err(inference::InferenceError::ScratchTooLarge(
                                            needed,
                                            budget,
                                        )) => ModelGap::Scratch(needed, budget),
                                        Err(inference::InferenceError::ScratchUnavailable) => {
                                            ModelGap::ScratchUnavailable
                                        }
                                        Err(_) => {
                                            reply_with_answer(
                                                sender_task_id,
                                                std::ai::STATUS_UNSUPPORTED,
                                                b"Generative model error: unsupported architecture or tensor configuration.",
                                                tx_buf,
                                            );
                                            return;
                                        }
                                    };
                                    // The file's own tokenizer declarations, read by
                                    // the walk that can pass the vocabulary the walk
                                    // above stops at.
                                    let tokenizer = gguf::read_tokenizer_info(path).ok();
                                    // WHY the window is `NOTICE_WINDOW_BYTES` and
                                    // not a literal at this site: this arm writes
                                    // the longest sentence the service can produce
                                    // -- the identity clause, three byte counts and
                                    // everything the tokenizer walk reached -- and
                                    // every other arm writes a shorter one into the
                                    // same window instead of its own narrower
                                    // guess. The measurement is in
                                    // `test_the_longest_notice_still_fits_the_window_it_is_written_in`.
                                    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                                    let notice = model_gap_notice(
                                        &info,
                                        gap,
                                        prompt_text,
                                        tokenizer.as_ref(),
                                        &mut scratch,
                                    );
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        notice,
                                        tx_buf,
                                    );
                                }
                                Err(gguf::GgufError::MissingMetadata(key, info)) => {
                                    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                                    let notice = model_gap_notice(
                                        &info,
                                        ModelGap::Declaration(key, None, 0),
                                        prompt_text,
                                        None,
                                        &mut scratch,
                                    );
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        notice,
                                        tx_buf,
                                    );
                                }
                                Err(_) => {
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        b"Generative model error: the model file's metadata could not be read.",
                                        tx_buf,
                                    );
                                }
                            }
                        }
                        Ok(_) => {
                            // The header parsed and the file names no tensor at all.
                            // WHY that is not "unsupported architecture": the engine
                            // was never offered a tensor to reject, and a person who
                            // just installed a file deserves to be told the file
                            // holds metadata and no weights, which is what a
                            // placeholder leaves behind.
                            let prompt_text = extract_prompt_text(payload);
                            match gguf::read_model_info(path) {
                                Ok(info) => {
                                    let tokenizer = gguf::read_tokenizer_info(path).ok();
                                    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                                    let notice = model_gap_notice(
                                        &info,
                                        ModelGap::NoTensors,
                                        prompt_text,
                                        tokenizer.as_ref(),
                                        &mut scratch,
                                    );
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        notice,
                                        tx_buf,
                                    );
                                }
                                Err(gguf::GgufError::MissingMetadata(key, info)) => {
                                    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                                    let notice = model_gap_notice(
                                        &info,
                                        ModelGap::Declaration(key, None, 0),
                                        prompt_text,
                                        None,
                                        &mut scratch,
                                    );
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        notice,
                                        tx_buf,
                                    );
                                }
                                Err(_) => {
                                    reply_with_answer(
                                        sender_task_id,
                                        std::ai::STATUS_UNSUPPORTED,
                                        b"Generative model error: unsupported architecture or tensor configuration.",
                                        tx_buf,
                                    );
                                }
                            }
                        }
                        Err(gguf::GgufError::BufferTooShort | gguf::GgufError::ReadFailed | gguf::GgufError::FileNotFound) => {
                            reply_with_answer(
                                sender_task_id,
                                std::ai::STATUS_UNAVAILABLE,
                                b"On-device generative stream unavailable: no local model weights loaded.",
                                tx_buf,
                            );
                        }
                        Err(_) => {
                            reply_with_answer(
                                sender_task_id,
                                std::ai::STATUS_UNSUPPORTED,
                                b"Generative model error: unsupported architecture or corrupted weights.",
                                tx_buf,
                            );
                        }
                    }
                }
                Err(_) => {
                    reply_with_answer(
                        sender_task_id,
                        std::ai::STATUS_UNAVAILABLE,
                        b"On-device generative stream unavailable: no local model weights loaded.",
                        tx_buf,
                    );
                }
            }
        } else {
            // Platform Truth Contract: Report unavailable when physical generative weights are absent
            reply_with_answer(
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

        // Platform Truth Contract: `is_neural` answers "did a model file's tensors
        // produce this match", and for every match this arm can produce the honest
        // answer is no. `rag::RagIndex::ingest` embeds each chunk through
        // `embedding::compute_embedding_768` and `RagIndex::search` embeds the
        // query the same way, so both sides of every cosine scored here are the
        // lexical hash projection. Deriving the flag from
        // `probed_models.embedding.is_some()` labelled a lexical match neural the
        // moment anyone installed a file -- a file's presence is not a file's
        // arithmetic. The flag now comes off the index that ran, so a neural
        // retrieval has to change what the index was built from before a hit can
        // claim to be one; until it does, the arm refuses the claim rather than
        // inheriting it from a directory listing.
        let mut index = rag::RagIndex::new(false);
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
                // The index's own flag, which is the flag the scoring ran under,
                // not a second reading of the probe. `STATUS_SUCCESS` on the frame
                // header means this retrieval ran and these matches are real; it
                // has never meant "neural", and a reader that needs that question
                // asks `is_neural`, which is now answerable.
                is_neural: m.is_neural as u8,
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

        // WHY the frame goes over the channel whole and framed: the mailbox cuts
        // a message at sixty-four bytes, and a ranked result is 24 bytes of
        // record followed by a path and a snippet, so every reply that carried
        // text arrived cut -- the reader then walked the hits the header counted
        // and read whatever lay past the bytes that had arrived. The frame keeps
        // its header so the reader parses one layout whether it came by channel
        // or, when no channel was free, by mailbox.
        if !stream_answer(
            sender_task_id,
            std::ai::STATUS_SUCCESS,
            MSG_TYPE_SEARCH_CHUNK,
            &tx_buf[..cursor],
        ) && sender_task_id != 0
        {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_SEARCH, &tx_buf[..cursor]);
        }
        return;
    }

    // 4. Embedding inference query
    let mut target_dim = embedding::EMBEDDING_DIM;
    let mut query_text = payload;
    let mut strict_neural_required = false;
    let mut request_flags: u32 = 0;

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
            request_flags = req.flags;
            if let Some(m_dim) = std::ai::MatryoshkaDim::from_u32(req.target_dim) {
                target_dim = m_dim.as_usize();
            }
            if (req.flags & std::ai::FLAG_STRICT_NEURAL) != 0 {
                strict_neural_required = true;
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

    // The ids this request carries, if it carries any. Read before the branch is
    // chosen, because the branch is chosen from them: `Some(_)` is the only input
    // that lets the installed file run at all.
    let token_ids = if (request_flags & std::ai::FLAG_INPUT_TOKEN_IDS) != 0 {
        read_token_ids(query_text)
    } else {
        None
    };

    let start_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
        .ok()
        .and_then(|_| std::get_ticks().ok())
        .unwrap_or(0);

    // Platform Truth Contract: one decision, made from the inputs, that the reply
    // then reports. `probed_models.embedding.is_some()` is an INPUT to the plan
    // and never the answer -- a file sitting on the device is not a vector that
    // came from it.
    let plan = plan_embedding_run(
        probed_models.embedding.is_some(),
        token_ids.map(|ids| ids.len),
        strict_neural_required,
    );

    let (report, dim, vector) = match plan {
        EmbeddingPlan::Refuse(gap) => {
            let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
            let (status, notice) = embedding_gap_notice(gap, &mut scratch);
            emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf);
            return;
        }
        EmbeddingPlan::LexicalBaseline => {
            let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
                .ok()
                .and_then(|_| std::get_ticks().ok())
                .unwrap_or(start_tick);
            let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);
            reply_lexical_baseline(sender_task_id, query_text, target_dim, latency_micros, tx_buf);
            return;
        }
        EmbeddingPlan::RunInstalledFile => {
            // The plan asked for both of these. A pair that is not here answers
            // from the baseline rather than unwrapping, so no future change to the
            // planner can turn this arm into a panic inside the service loop.
            let (path, ids) = match (probed_models.embedding, token_ids) {
                (Some((_hdr, path)), Some(ids)) => (path, ids),
                _ => {
                    let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
                        .ok()
                        .and_then(|_| std::get_ticks().ok())
                        .unwrap_or(start_tick);
                    let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);
                    reply_lexical_baseline(
                        sender_task_id,
                        query_text,
                        target_dim,
                        latency_micros,
                        tx_buf,
                    );
                    return;
                }
            };

            let mut vector = [0.0f32; embedding::EMBEDDING_DIM];
            match run_installed_embedding(path, ids.as_slice(), &mut vector) {
                Ok(report) => {
                    // The Matryoshka prefix, renormalised: the same convention
                    // `inference::extract_embedding` applies when it pools a
                    // stream into `dim` components, so a truncated neural vector
                    // and a truncated in-memory one are cut by one rule, not two.
                    let dim = target_dim.min(report.hidden_dim);
                    inference::l2_normalise(&mut vector[..dim]);
                    (report, dim, vector)
                }
                Err(error) => {
                    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                    let (status, notice) = file_embed_notice(&error, &mut scratch);
                    emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf);
                    return;
                }
            }
        }
    };

    // Only the file-backed branch reaches here, and it carries the status the
    // branch that actually ran earned: `STATUS_SUCCESS` now means tensors of the
    // installed file were read and the graph was stepped over them position by
    // position -- nothing more, and nothing less.
    let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
        .ok()
        .and_then(|_| std::get_ticks().ok())
        .unwrap_or(start_tick);
    let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);

    let mut q7_out = [0i8; embedding::EMBEDDING_DIM];
    std::ai::quantize_f32_to_q7(&vector[..dim], &mut q7_out[..dim]);

    let resp = std::ai::AiInferenceResponse {
        status: STATUS_SUCCESS,
        output_dim: dim as u32,
        latency_micros,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };

    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
    };
    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);

    // Copy Q7 embedding vector immediately after response header
    let vector_bytes = unsafe { core::slice::from_raw_parts(q7_out.as_ptr() as *const u8, dim) };
    let total_len = resp_hdr_len + dim;
    if total_len <= tx_buf.len() && sender_task_id != 0 {
        tx_buf[resp_hdr_len..total_len].copy_from_slice(vector_bytes);
        let _ = std::ipc_send(sender_task_id, MSG_TYPE_EMBED, &tx_buf[..total_len]);
    }

    // WHAT THE VECTOR IS, said where a reader can check it rather than in a
    // comment: the counts the run measured, and whether the file's pooling head
    // ran. A pre-head vector presented as the model's embedding is the defect
    // `weights.rs` returns that flag to prevent, so the flag travels.
    let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
    let mut notice = Notice::new(&mut scratch);
    notice.word(b"[file-backed embedding: ");
    notice.byte_count(report.bytes_read);
    notice.word(b" bytes, ");
    notice.number(report.positions as u32);
    notice.word(b" positions x ");
    notice.number((report.blocks_run / report.positions.max(1)) as u32);
    notice.word(b" blocks over ");
    notice.number(report.hidden_dim as u32);
    notice.word(b" wide, ");
    if report.head_applied {
        notice.word(b"pooling head applied, L2-normalised]");
    } else {
        notice.word(b"PRE-HEAD: this file declares no pooling head]");
    }
    let notice = notice.finish();
    emit_response_as(sender_task_id, STATUS_SUCCESS, MSG_TYPE_EMBED, notice, tx_buf);
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
        // Platform Truth Contract: this vector is the lexical hash projection, and
        // the status says so. It used to be `0` -- `STATUS_SUCCESS` -- which made a
        // function of the prompt's bytes indistinguishable from a model's weights.
        assert_eq!(resp.status, STATUS_BASELINE_LEXICAL);
        assert_ne!(resp.status, STATUS_SUCCESS, "the baseline claimed weights");
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
        assert_eq!(resp.status, STATUS_BASELINE_LEXICAL);
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
        // A real embedding table, because the engine reads a token's row out of
        // it and refuses when the row is not there. Fabricating the row from the
        // token id would have this test certify an answer computed from nothing.
        let mut embd = [0u8; 290 * 64 * 4];
        for token in 0..290usize {
            for dim in 0..64usize {
                let value = (((token * 7 + dim * 13) % 17) as f32 - 8.0) / 16.0;
                let at = (token * 64 + dim) * 4;
                embd[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        let layer = inference::LayerWeights {
            attn_norm: &ones,
            attn_q_norm: &[],
            attn_k_norm: &[],
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
        // A slice and not a fixed array: the block count belongs to the config,
        // and an array literal could only ever describe the model it was written
        // for. The fixture's config declares one block, so the second slot that
        // `generate` never reaches is dropped rather than carried.
        let layers = [Some(layer)];
        let model = inference::ModelWeights {
            token_embd: inference::TokenEmbedding::Table {
                bytes: &embd,
                rows: 290,
                dtype: 0,
            },
            layers: &layers,
            final_norm: &ones,
            output_weight: Some(&head_w),
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
        // The long raw query is still the lexical baseline -- labelled, where it
        // used to report `0`/`STATUS_SUCCESS` beside the projection.
        assert_eq!(resp.status, STATUS_BASELINE_LEXICAL);
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

    /// A file that states the architecture the notice can quote it; one that does
    /// not must not be answered as if it had.
    fn declared(arch: &[u8], hidden: u32, layers: u32, vocab: u32) -> gguf::GgufModelInfo {
        let mut info = gguf::GgufModelInfo::empty();
        info.architecture[..arch.len()].copy_from_slice(arch);
        info.architecture_reading = gguf::StringReading::Read(arch.len());
        info.hidden_dim = Some(hidden);
        info.num_layers = Some(layers);
        info.vocab_size = Some(vocab);
        info
    }

    /// A file that states every identity field as well as the dimensions.
    fn declaring(
        name: &[u8],
        arch: &[u8],
        tokenizer_model: &[u8],
        file_type: u32,
        tensors: u64,
        hidden: u32,
        layers: u32,
        vocab: u32,
    ) -> gguf::GgufModelInfo {
        let mut info = declared(arch, hidden, layers, vocab);
        info.name[..name.len()].copy_from_slice(name);
        info.name_reading = gguf::StringReading::Read(name.len());
        info.tokenizer_model[..tokenizer_model.len()].copy_from_slice(tokenizer_model);
        info.tokenizer_model_reading = gguf::StringReading::Read(tokenizer_model.len());
        info.file_type = Some(file_type);
        info.quantization_version = Some(1);
        info.tensor_count = Some(tensors);
        info.kv_pair_count = Some(12);
        info.pairs_walked = 12;
        info
    }

    #[test]
    fn test_model_gap_notice_quotes_the_file_and_counts_the_prompt() {
        // A file that names itself: the notice quotes the name its author wrote,
        // the architecture its tensors are laid out for, the quantisation it
        // states by number and the tensors it counted -- none of them supplied by
        // the path the bytes were opened at.
        let info = declaring(
            b"EmbeddingGemma 2",
            b"gemma2",
            b"gpt2",
            7,
            196,
            64,
            2,
            290,
        );
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"Summarize my notifications",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file named EmbeddingGemma 2, declares gemma2, tokenizer model gpt2, quantisation type 7, 196 tensors, embedding length 64, over 2 blocks, vocabulary 290 tokens, none of whose tensors this reply reads; the prompt's 26 bytes were not answered."
        );
    }

    /// WHY the sentence names the reply and not the engine.
    ///
    /// The older wording claimed the engine reads no tensor of the file. That was a
    /// statement about capability, and it stopped being true when `weights` began
    /// reading an installed file's tensors block by block and running `inference`'s
    /// graph over the bytes it read -- measured, against the 333 MB file, by
    /// `weights::tests::test_the_installed_file_is_read_block_by_block_and_run`,
    /// which prints its own byte counts and says NOT VERIFIED when the file is not in
    /// front of it. `test_the_notice_for_the_file_this_host_holds_quotes_that_file`
    /// keeps the two in touch: it hands the same path to both the notice and the
    /// loader. A notice that lost the claim without gaining the reply's own fact would
    /// be the lie deleted so a gate could pass, so both halves are pinned here.
    #[test]
    fn test_the_gap_notice_says_what_the_reply_skipped_not_what_the_engine_cannot() {
        let info = declared(b"gemma2", 64, 2, 290);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(&info, ModelGap::Configured, b"hi", None, &mut out);
        let text = core::str::from_utf8(notice).unwrap_or("<unprintable>");
        assert!(
            text.contains("none of whose tensors this reply reads"),
            "the refusal no longer says what this reply skipped: {text}"
        );
        for capability_claim in [
            "the engine does not read",
            "the engine cannot read",
            "the engine has no way to read",
        ] {
            assert!(
                !text.contains(capability_claim),
                "the notice claims {capability_claim:?}, which the loader's measurement \
                 against the installed file contradicts"
            );
        }
        // The other arm of the same shape still distinguishes a file that holds no
        // tensor at all, which is a different fact and a different next move.
        let mut empty = [0u8; NOTICE_WINDOW_BYTES];
        let text = core::str::from_utf8(model_gap_notice(
            &info,
            ModelGap::NoTensors,
            b"hi",
            None,
            &mut empty,
        ))
        .unwrap_or("<unprintable>");
        assert!(text.contains(", and holds no tensors"), "{text}");
        assert!(
            !text.contains("this reply reads"),
            "the empty-file arm borrowed the token arm's sentence: {text}"
        );
    }

    /// The same sentence for a file whose `general.name` the walk never reached a
    /// value for: the name is reported as not stated, and no stand-in -- not the
    /// architecture, not a vendor's model family, not the path -- takes its place.
    #[test]
    fn test_model_gap_notice_says_a_file_named_nothing_rather_than_naming_it_by_its_path() {
        let info = declared(b"gemma2", 64, 2, 290);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"Summarize my notifications",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 64, over 2 blocks, vocabulary 290 tokens, none of whose tensors this reply reads; the prompt's 26 bytes were not answered."
        );
    }

    /// A file that parses and holds no tensor is not an unsupported architecture;
    /// it is a file with nothing in it yet, and the answer has to say so.
    #[test]
    fn test_model_gap_notice_says_so_when_the_file_holds_no_tensors() {
        let info = declared(b"gemma2", 64, 2, 290);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(&info, ModelGap::NoTensors, b"hello", None, &mut out);
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 64, over 2 blocks, vocabulary 290 tokens, and holds no tensors; the prompt's 5 bytes were not answered."
        );
    }

    #[test]
    fn test_model_gap_notice_reports_a_declaration_beyond_the_engine() {
        let info = declared(b"gemma2", 2304, 26, 256_000);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Declaration("<arch>.embedding_length", Some(2304), 256),
            b"hi",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 2304, and states <arch>.embedding_length as 2304 against the engine's bound of 256; the prompt's 2 bytes were not answered."
        );
    }

    #[test]
    fn test_model_gap_notice_names_the_key_a_file_omitted() {
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &gguf::GgufModelInfo::empty(),
            ModelGap::Declaration("<arch>.block_count", None, 0),
            b"",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, states no architecture, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length undeclared, and states no <arch>.block_count; the prompt's 0 bytes were not answered."
        );
    }

    /// The one size limit the engine carries now is a byte budget, so the refusal
    /// has to carry three byte counts: what the run would have taken, the budget it
    /// was refused against, and how far over it is. "Unsupported architecture"
    /// names none of them, and a person cannot choose a smaller model from that.
    #[test]
    fn test_the_scratch_refusal_names_the_budget_and_the_overage() {
        let info = declared(b"gemma2", 3072, 48, 256_000);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Scratch(437_421_056, 125_829_120),
            b"hi",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 3072, whose run needs 437421056 scratch bytes against the 125829120 byte budget, over by 311591936 bytes; the prompt's 2 bytes were not answered."
        );
    }

    /// The budget and the mapping are two different answers: one says the device
    /// has no room for this model, the other says it could not be handed out right
    /// now. Reporting the second as the first would send someone looking for a
    /// smaller file when the machine is what is full.
    #[test]
    fn test_the_budget_refusal_names_the_memory_the_device_would_not_map() {
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &gguf::GgufModelInfo::empty(),
            ModelGap::ScratchUnavailable,
            b"",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, states no architecture, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length undeclared, whose scratch the byte budget prices as fitting but the device would not map; the prompt's 0 bytes were not answered."
        );
    }

    /// A tokenizer walk whose vocabulary sits past the reader's load cap and whose
    /// template the walk held -- the shape a real Gemma file leaves.
    fn walked_vocab_and_template() -> gguf::GgufTokenizerInfo {
        let mut tok = gguf::GgufTokenizerInfo::empty();
        tok.vocab = gguf::VocabReading::Counted { elements: 256_000 };
        tok.bos_token_id = Some(2);
        tok.eos_token_id = Some(1);
        tok.pad_token_id = Some(0);
        tok.add_bos_token = Some(true);
        tok.pre[..5].copy_from_slice(b"gemma");
        tok.pre_reading = gguf::StringReading::Read(5);
        let template = b"<start_of_turn>user\n{prompt}";
        tok.chat_template[..template.len()].copy_from_slice(template);
        tok.template_reading = gguf::StringReading::Read(template.len());
        tok
    }

    #[test]
    fn test_the_notice_carries_the_counted_vocabulary_and_the_template_it_read() {
        let info = declared(b"gemma2", 64, 2, 256_000);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"hello",
            Some(&walked_vocab_and_template()),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 64, over 2 blocks, vocabulary 256000 tokens, none of whose tensors this reply reads, vocabulary counted not loaded, template 28 bytes from file, bos 2 eos 1 pad 0, add-bos 1, pre gemma; the prompt's 5 bytes were not answered."
        );
    }

    /// WHY this is its own case: the platform has a built-in template, and a screen
    /// that showed a formatted prompt could not tell the person whether the model's
    /// own formatting or the platform's default ran. The answer names the default.
    #[test]
    fn test_the_notice_names_the_built_in_default_when_the_file_declared_no_tokenizer() {
        let mut info = declared(b"gemma2", 64, 2, 290);
        info.vocab_size = None;
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::NoTensors,
            b"hello",
            Some(&gguf::GgufTokenizerInfo::empty()),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 64, over 2 blocks, vocabulary undeclared, and holds no tensors, tokenizer absent, built-in template in use; the prompt's 5 bytes were not answered."
        );
    }

    #[test]
    fn test_the_notice_reports_a_refused_template_by_the_length_the_file_stated() {
        let mut tok = gguf::GgufTokenizerInfo::empty();
        tok.vocab = gguf::VocabReading::Counted { elements: 256_000 };
        tok.template_reading = gguf::StringReading::RefusedTooLong(4_096);
        let mut info = declared(b"gemma2", 64, 2, 256_000);
        info.intermediate_dim = Some(128);
        info.num_heads = Some(4);
        info.head_dim = Some(16);
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"",
            Some(&tok),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file names no model, declares gemma2, tokenizer model undeclared, quantisation type undeclared, tensors uncounted, embedding length 64, over 2 blocks, vocabulary 256000 tokens, none of whose tensors this reply reads, vocabulary counted not loaded, template 4096 bytes refused, no token ids; the prompt's 0 bytes were not answered."
        );
        // A refused template is not an empty one: the engine runs the default, and
        // the walk keeps the length the file gave.
        assert_eq!(
            tok.active_chat_template().1,
            gguf::TemplateSource::BuiltInDefault
        );
    }

    /// The widest model file the reader can hand to a notice: every string at the
    /// length `gguf` caps it to, and every number at the most digits it holds.
    ///
    /// WHY the caps and not the fixture's short names: the identity clause quotes
    /// the bytes the file wrote, so the sentence's worst case is set by how long a
    /// name a file is allowed to state -- not by the six characters `gemma2` takes.
    fn widest_file() -> gguf::GgufModelInfo {
        let mut info = gguf::GgufModelInfo::empty();
        info.name[..].copy_from_slice(&[b'n'; gguf::MAX_MODEL_NAME_LEN]);
        info.name_reading = gguf::StringReading::Read(gguf::MAX_MODEL_NAME_LEN);
        info.architecture[..].copy_from_slice(&[b'a'; gguf::MAX_ARCH_NAME_LEN]);
        info.architecture_reading = gguf::StringReading::Read(gguf::MAX_ARCH_NAME_LEN);
        info.tokenizer_model[..].copy_from_slice(&[b't'; gguf::MAX_MODEL_NAME_LEN]);
        info.tokenizer_model_reading = gguf::StringReading::Read(gguf::MAX_MODEL_NAME_LEN);
        info.file_type = Some(u32::MAX);
        info.quantization_version = Some(u32::MAX);
        info.tensor_count = Some(u64::MAX);
        info.hidden_dim = Some(u32::MAX);
        info.num_layers = Some(u32::MAX);
        info.vocab_size = Some(u32::MAX);
        info.kv_pair_count = Some(12);
        info.pairs_walked = 12;
        info
    }

    /// The widest tokenizer walk: a counted vocabulary over the load cap, a
    /// template refused at the most bytes a length can name, all three ids, the
    /// flag, a pre-name at its cap and a truncation on the longest key the walk
    /// holds.
    fn widest_walk() -> gguf::GgufTokenizerInfo {
        let mut tok = gguf::GgufTokenizerInfo::empty();
        tok.vocab = gguf::VocabReading::Counted { elements: u32::MAX };
        tok.bos_token_id = Some(u32::MAX);
        tok.eos_token_id = Some(u32::MAX);
        tok.pad_token_id = Some(u32::MAX);
        tok.add_bos_token = Some(true);
        tok.pre[..].copy_from_slice(&[b'p'; gguf::MAX_TOKENIZER_PRE_LEN]);
        tok.pre_reading = gguf::StringReading::Read(gguf::MAX_TOKENIZER_PRE_LEN);
        tok.template_reading = gguf::StringReading::RefusedTooLong(u64::MAX);
        tok.truncated_in = Some(b"tokenizer.ggml.padding_token_id");
        tok
    }

    /// WHY the worst case is measured and not assumed: `Notice` stops at the end
    /// of its window without complaining, and a notice cut mid-sentence is a
    /// sentence that no longer says what the file said. Every arm the dispatch
    /// site can take is written here at its widest, into the window that site
    /// hands it, because the arm a fixture happens to exercise is not the arm a
    /// real file lands on.
    #[test]
    fn test_the_longest_notice_still_fits_the_window_it_is_written_in() {
        let info = widest_file();
        let tok = widest_walk();
        let widest_key: &'static str = "<arch>.attention.head_count";

        // The prompt is the one number this measurement leaves short: the fixture
        // hands an empty one, so its count writes a single digit where a `u32`'s
        // longest answer would write ten.
        const PROMPT_DIGIT_SLACK: usize = 10;

        let measure = |label: &str, gap: ModelGap, quoted: Option<&gguf::GgufTokenizerInfo>| {
            let mut out = [0u8; NOTICE_WINDOW_BYTES];
            let notice = model_gap_notice(&info, gap, b"", quoted, &mut out);
            let text = core::str::from_utf8(notice).unwrap_or("<unprintable>");
            assert!(
                notice.len() + PROMPT_DIGIT_SLACK <= NOTICE_WINDOW_BYTES,
                "the {label} would not survive the {NOTICE_WINDOW_BYTES}-byte window it is \
                 written in ({} bytes before the prompt's own digits): {text}",
                notice.len(),
            );
            assert!(
                text.ends_with("bytes were not answered."),
                "the {label} did not reach the end of its sentence: {text}"
            );
            notice.len()
        };

        let scratch = measure(
            "scratch refusal",
            ModelGap::Scratch(usize::MAX, inference::SCRATCH_BYTE_BUDGET),
            Some(&tok),
        );
        let configured = measure("configured tensors", ModelGap::Configured, Some(&tok));
        let empty_file = measure("empty file", ModelGap::NoTensors, Some(&tok));
        let unmappable = measure("unmappable scratch", ModelGap::ScratchUnavailable, Some(&tok));
        let over_bound = measure(
            "declaration over the bound",
            ModelGap::Declaration(widest_key, Some(u32::MAX), usize::MAX),
            Some(&tok),
        );
        let omitted = measure(
            "omitted declaration",
            ModelGap::Declaration(widest_key, None, 0),
            None,
        );

        // WHY the widest sentence is also checked for its fields: a guard that only
        // measures a length goes quiet the day its fixture stops filling them, and
        // the window would then be sized by a sentence no file can produce.
        use alloc::string::ToString;
        let mut widest_out = [0u8; NOTICE_WINDOW_BYTES];
        let widest = model_gap_notice(
            &info,
            ModelGap::Scratch(usize::MAX, inference::SCRATCH_BYTE_BUDGET),
            b"",
            Some(&tok),
            &mut widest_out,
        );
        let widest_text = core::str::from_utf8(widest).unwrap_or("<unprintable>");
        let quoted_name = " named ".to_string() + &"n".repeat(gguf::MAX_MODEL_NAME_LEN);
        let quoted_arch = ", declares ".to_string() + &"a".repeat(gguf::MAX_ARCH_NAME_LEN);
        assert!(
            widest_text.contains(&quoted_name),
            "the name the file stated is not quoted at the cap the reader holds: {widest_text}"
        );
        assert!(
            widest_text.contains(&quoted_arch),
            "the architecture the file stated is not quoted at the cap the reader holds: {widest_text}"
        );

        // WHY the arm is named rather than the largest length taken: the window's
        // doc says the scratch refusal is the sentence it is sized for. If another
        // arm ever grows past it, that sentence is what stops being true, and this
        // is where it gets said.
        assert!(
            scratch >= configured
                && scratch >= empty_file
                && scratch >= unmappable
                && scratch >= over_bound
                && scratch >= omitted,
            "the scratch refusal is no longer the widest arm the window is sized for: \
             scratch {scratch}, configured {configured}, empty {empty_file}, unmappable \
             {unmappable}, over bound {over_bound}, omitted {omitted}"
        );
    }

    /// The sentence this host's own model file is answered with.
    ///
    /// WHY this exists next to the widest-fixture guard: that guard sizes the window
    /// against what the reader *can* report, and nothing in it proves the identity
    /// clause quotes a file that really exists. A real Gemma leaves its vocabulary
    /// partway through the metadata, so the walk stops with some fields read and some
    /// not -- and only the file itself says which sentence the device actually sends.
    ///
    /// WHY it is gated on an environment variable: the file is 333 MB and is not in
    /// the repository, so its host path arrives in `INTEROS_REAL_GGUF_PATH`. Without
    /// that variable the test prints what it did not check, which is the honest
    /// alternative to a green test that opened nothing.
    #[test]
    fn test_the_notice_for_the_file_this_host_holds_quotes_that_file() {
        extern crate std as real_file;
        use real_file::io::Read;

        let Some(path) = real_file::env::var("INTEROS_REAL_GGUF_PATH").ok() else {
            real_file::println!(
                "NOT VERIFIED against a real model file: INTEROS_REAL_GGUF_PATH is unset, so \
                 the identity clause has only ever quoted a hand-written fixture and the \
                 loader has only ever been handed one."
            );
            return;
        };
        // The same window the engine's own real-file test reads: enough for every key
        // before the vocabulary, and the walk stops there by design.
        const WINDOW: usize = 64 * 1024;
        let mut bytes = real_file::vec![0u8; WINDOW];
        let mut file = real_file::fs::File::open(&path)
            .unwrap_or_else(|error| panic!("the real file at {path} could not be opened: {error}"));
        let mut filled = 0usize;
        while filled < WINDOW {
            match file.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(got) => filled += got,
                Err(error) => panic!("the real file at {path} could not be read: {error}"),
            }
        }
        let head = &bytes[..filled];

        let mut stream = gguf::GgufStream::from_slice(head);
        let info = gguf::read_model_info_stream(&mut stream)
            .unwrap_or_else(|error| panic!("the real file at {path} did not parse: {error:?}"));
        let prompt = b"Summarize my notifications";
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let notice = model_gap_notice(&info, ModelGap::Configured, prompt, None, &mut out);
        let text = core::str::from_utf8(notice).unwrap_or("<not utf-8>");
        real_file::println!("REAL FILE {path} answered without the tokenizer walk:");
        real_file::println!("  {} bytes: {text}", notice.len());

        assert!(
            notice.len() <= NOTICE_WINDOW_BYTES,
            "the real file's sentence is {} bytes, over the window it is written into",
            notice.len()
        );
        assert!(
            text.ends_with("bytes were not answered."),
            "the real file's sentence was cut before its ending: {text}"
        );
        assert!(
            text.contains(" declares gemma-embedding"),
            "the real file's own architecture is not quoted: {text}"
        );
        assert!(
            !text.contains("names no model"),
            "the real file states a name, and the notice said it declared none: {text}"
        );
        assert!(
            text.contains("none of whose tensors this reply reads"),
            "the sentence this file triggers no longer says what the reply skipped: \
             {text}"
        );

        // WHY the loader runs on this path too: the clause above was narrowed from a
        // claim about the engine ("whose tensors the engine does not read") to a claim
        // about the reply, and the only thing that licenses that narrowing is this
        // file's own bytes going into the graph. So the file that triggers the sentence
        // is handed to the loader here, and one block of it is read -- the long
        // measurement, over all 24 blocks and two prompts, is
        // `weights::tests::test_the_installed_file_is_read_block_by_block_and_run`.
        let (table, _head, _walked) = weights::tests::walk_the_installed_table(&path)
            .unwrap_or_else(|why| panic!("the descriptor table of {path} never walked: {why}"));
        let mut reader = weights::WeightReader::new(
            table,
            weights::tests::HostFileSource {
                file: real_file::fs::File::open(&path)
                    .unwrap_or_else(|error| panic!("the real file at {path} could not be opened: {error}")),
            },
        );
        let block = reader
            .load_block(0)
            .unwrap_or_else(|error| panic!("block 0 of {path} did not load: {error:?}"));
        real_file::println!(
            "  the same file, loaded: {} tensors and {} bytes of block 0 are in the \
             graph's shapes, so the sentence above may not claim the engine reads none \
             of them (it claims only that this reply does)",
            block.tensors_read,
            block.bytes_read,
        );
        assert_eq!(
            block.tensors_read, 13,
            "the notice's file did not hand the loader a whole block"
        );
        assert!(
            block.bytes_read > 0,
            "the loader read a block of {} bytes from the file the notice names",
            block.bytes_read
        );
    }

    #[test]
    fn test_install_notice_calls_only_a_whole_file_installed() {
        let mut out = [0u8; 256];
        let (status, notice) = install_notice(InstallOutcome::Installed(4096), &mut out);
        assert_eq!(status, std::ai::STATUS_SUCCESS);
        assert_eq!(
            notice,
            b"Model file installed: 4096 bytes at /data/models/local_model.gguf."
        );

        let mut out = [0u8; 256];
        let (status, notice) = install_notice(InstallOutcome::Short(1024, 4096), &mut out);
        assert_ne!(status, std::ai::STATUS_SUCCESS);
        assert_eq!(
            notice,
            b"Install incomplete: the model namespace holds 1024 of the 4096 bytes the file states."
        );
        assert!(!notice.starts_with(b"Model file installed"), "a part copy is not an install");

        let mut out = [0u8; 256];
        let (status, notice) = install_notice(InstallOutcome::Absent(4096), &mut out);
        assert_ne!(status, std::ai::STATUS_SUCCESS);
        assert_eq!(
            notice,
            b"Install did not reach the device: the model namespace holds no file of the 4096 bytes the source states."
        );
    }

    #[test]
    fn test_install_notice_reports_what_was_never_asked_for() {
        let mut out = [0u8; 256];
        let (status, notice) = install_notice(InstallOutcome::NothingToInstall, &mut out);
        assert_eq!(status, std::ai::STATUS_UNAVAILABLE);
        assert_eq!(
            notice,
            b"Nothing to install: the download and incoming folders hold no readable .gguf."
        );

        let mut out = [0u8; 256];
        let (status, notice) = install_notice(InstallOutcome::RequestRefused, &mut out);
        assert_eq!(status, std::ai::STATUS_ERROR);
        assert_eq!(
            notice,
            b"Install refused: the storage service would not take the request."
        );
    }

    /// WHY this is tested on its own: a model file is larger than a `u32` can
    /// count, and an install that reported `4_294_967_296` as `0` would tell the
    /// person the device received nothing when it received everything.
    #[test]
    fn test_install_notice_counts_bytes_past_a_u32() {
        let mut out = [0u8; 256];
        let (_, notice) = install_notice(InstallOutcome::Installed(5_000_000_000), &mut out);
        assert_eq!(
            notice,
            b"Model file installed: 5000000000 bytes at /data/models/local_model.gguf."
        );
    }

    // -----------------------------------------------------------------------
    // The embedding arm's branch selection, proved from its inputs.
    //
    // WHY these are the tests and not a run of the engine: the branch is chosen
    // before a byte of any file is read, so the decision is host-provable, while
    // the run it selects goes through `fs_open`/`fs_read_at` -- guest syscalls the
    // host suite cannot drive, exactly as `weights.rs` says of `ModelFileDescriptor`.
    // Nothing below mocks the loader: each case feeds the arm the same triple it
    // reads from the wire and the probe, and asserts which branch answers.
    // -----------------------------------------------------------------------

    fn probed_embedding_models() -> gguf::ProbedModels {
        gguf::ProbedModels {
            embedding: Some((
                gguf::GgufHeader {
                    magic: gguf::GGUF_MAGIC,
                    version: 3,
                    tensor_count: 316,
                    kv_count: 40,
                },
                gguf::PATH_EMBEDDING_MODEL_DATA,
            )),
            generative: None,
        }
    }

    /// Every input combination, in a table rather than in prose.
    #[test]
    fn test_the_embedding_branch_is_chosen_from_the_inputs() {
        use {EmbeddingGap::*, EmbeddingPlan::*};
        let cases: [(bool, Option<usize>, bool, EmbeddingPlan); 8] = [
            // Weights plus caller-owned ids: the file runs, flag or no flag.
            (true, Some(3), false, RunInstalledFile),
            (true, Some(3), true, RunInstalledFile),
            // Weights, but text and a demand for neural: refuse, and name the
            // vocabulary gap. This is the case the lexical projection used to
            // answer with `STATUS_SUCCESS`.
            (true, None, true, Refuse(NoVocabTokenizer)),
            // Weights and text, nothing demanded: the baseline, labelled.
            (true, None, false, LexicalBaseline),
            // No file. Ids cannot run against nothing.
            (false, Some(3), true, Refuse(NoWeights)),
            (false, None, true, Refuse(NoWeights)),
            (false, Some(3), false, LexicalBaseline),
            (false, None, false, LexicalBaseline),
        ];
        for (weights, ids, strict, expected) in cases {
            assert_eq!(
                plan_embedding_run(weights, ids, strict),
                expected,
                "weights={weights} ids={ids:?} strict={strict}"
            );
        }
    }

    /// The defect this closes, stated as one assertion: a file on the device must
    /// not become a neural answer on its own.
    #[test]
    fn test_a_file_present_is_never_by_itself_a_neural_answer() {
        // The old search arm asked exactly this question and got the wrong answer.
        assert_eq!(
            plan_embedding_run(true, None, false),
            EmbeddingPlan::LexicalBaseline,
            "a probe that found a file now plans a run it has no ids for"
        );
        assert_ne!(
            plan_embedding_run(true, None, false),
            EmbeddingPlan::RunInstalledFile,
            "the plan runs a file from the file's existence"
        );
    }

    /// A token-id stream is read whole or refused whole -- never partly.
    #[test]
    fn test_a_token_id_stream_is_read_or_refused_whole() {
        let two = [1701u32, 4423];
        let bytes: alloc::vec::Vec<u8> = two
            .iter()
            .flat_map(|id| id.to_le_bytes())
            .collect();
        let ids = read_token_ids(&bytes).expect("two whole ids parse");
        assert_eq!(ids.len, 2);
        assert_eq!(ids.as_slice(), &two[..]);

        assert_eq!(read_token_ids(b"").map(|i| i.len), None, "no bytes are no ids");
        assert_eq!(
            read_token_ids(&[0u8, 0, 0]).map(|i| i.len),
            None,
            "a part of an id parsed as an id would run the wrong token"
        );
        let too_many = [0u8; (EMBED_TOKEN_IDS_MAX + 1) * 4];
        assert_eq!(
            read_token_ids(&too_many).map(|i| i.len),
            None,
            "a stream past the bound was accepted, so one request is unbounded work"
        );
        let at_the_bound = [0u8; EMBED_TOKEN_IDS_MAX * 4];
        assert_eq!(read_token_ids(&at_the_bound).map(|i| i.len), Some(EMBED_TOKEN_IDS_MAX));
    }

    /// The arm, end to end, with a file installed and a text query: the baseline
    /// still answers, and says it is the baseline.
    ///
    /// WHY through `handle_ipc_inference_query` and not the planner alone: the
    /// status is written where the vector is emitted. A planner that chooses right
    /// and an arm that still reports `0` is the exact bug that shipped.
    #[test]
    fn test_the_arm_answers_the_baseline_labelled_even_with_a_file_installed() {
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        handle_ipc_inference_query(
            0,
            MSG_TYPE_EMBED,
            b"query: battery consumption profile",
            &models,
            &mut tx_buf,
        );
        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, STATUS_BASELINE_LEXICAL);
        assert_eq!(resp.output_dim, 768, "the baseline vector is still a real answer");
        let vector = &tx_buf[resp_hdr_len..resp_hdr_len + 768];
        assert!(
            vector.iter().any(|&b| b != 0),
            "the labelled baseline stopped producing a vector"
        );
    }

    /// Strict neural, a file on the device, and text: the arm refuses and names
    /// the missing vocabulary mapping instead of quietly answering.
    #[test]
    fn test_strict_neural_with_text_refuses_and_names_the_tokenizer_gap() {
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let payload = embedding_request_with(0x80, b"Summarize the battery report");
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, STATUS_UNSUPPORTED);
        assert_ne!(resp.status, STATUS_BASELINE_LEXICAL, "it answered instead of refusing");
        let notice = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        let notice = core::str::from_utf8(notice).expect("the refusal is text");
        assert!(notice.contains("token ids"), "the refusal lost the gap: {notice}");
        assert!(notice.contains("vocabulary"), "the refusal no longer names what is missing");
    }

    /// Strict neural with no file: refused as unavailable, as it always was --
    /// this pins that the refactor kept the older refusal.
    #[test]
    fn test_strict_neural_with_no_file_is_refused_as_unavailable() {
        let mut tx_buf = [0u8; 1024];
        let empty = gguf::ProbedModels::empty();
        let payload = embedding_request_with(0x80, b"Summarize the battery report");
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &empty, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, STATUS_UNAVAILABLE);
        let notice = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        assert!(core::str::from_utf8(notice).unwrap().contains("no weights file"));
    }

    /// Installing the file changes the branch the arm takes, proved by the reply
    /// not being the baseline any more.
    ///
    /// The run itself reaches `fs_open`, which is a syscall: on the host it is
    /// refused, and the arm answers with the file-backed refusal notice. Either
    /// way it is not the labelled baseline -- which is the branch claim, and the
    /// only claim this host case is allowed to make.
    #[test]
    fn test_a_token_id_stream_takes_the_file_backed_branch_not_the_baseline() {
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let ids: alloc::vec::Vec<u8> = [1701u32, 4423, 88]
            .iter()
            .flat_map(|id| id.to_le_bytes())
            .collect();
        let payload = embedding_request_with(0x10, &ids);
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_ne!(
            resp.status,
            STATUS_BASELINE_LEXICAL,
            "a request carrying ids was answered by the projection"
        );
        assert_ne!(
            resp.output_dim,
            768,
            "the reply still has the baseline's whole-width vector"
        );
    }

    /// Every refusal the loader can surface earns its own code and its own words.
    #[test]
    fn test_a_loader_refusal_maps_to_the_status_that_names_it() {
        let mut out = [0u8; NOTICE_WINDOW_BYTES];

        let (status, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::UnsupportedDtype("blk.0.attn_q.weight", 6)),
            &mut out,
        );
        assert_eq!(status, STATUS_UNSUPPORTED);
        assert_eq!(
            core::str::from_utf8(notice).unwrap(),
            "Embedding refused: the file's tensor blk.0.attn_q.weight declares quantisation 6, a dtype this engine has no row layout for."
        );

        let (status, notice) = file_embed_notice(
            &FileEmbedError::Config(InferenceError::ScratchTooLarge(125_829_120, 12)),
            &mut out,
        );
        assert_eq!(status, STATUS_UNAVAILABLE, "a shape the device cannot hold is not a bad file");
        assert!(core::str::from_utf8(notice).unwrap().contains("125829120"));

        let (status, _) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::ShortRead(4096, 816, 512)),
            &mut out,
        );
        assert_eq!(status, STATUS_ERROR, "a short read is neither the file's shape nor the heap's");

        let (status, _) = file_embed_notice(&FileEmbedError::Open, &mut out);
        assert_eq!(status, STATUS_UNAVAILABLE);

        // A refusal that shares a code with an unrelated refusal cannot be acted on.
        let (dtype_status, _) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::BadAlignment(3)),
            &mut out,
        );
        let (short_status, _) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::ShortRead(0, 8, 1)),
            &mut out,
        );
        assert_ne!(dtype_status, short_status);

        // `Notice::word` appends without a separator, so a fragment that forgot
        // its own space reads as one invented word -- and a refusal a person cannot
        // parse is barely a refusal. The interleavings (name then verb, number then
        // word) are where it hides, so they are pinned as whole sentences.
        let (_, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::BadShape(
                "token_embd.weight",
                3072,
                768,
            )),
            &mut out,
        );
        assert_eq!(
            core::str::from_utf8(notice).unwrap(),
            "Embedding refused: the tensor token_embd.weight declares 3072 where the graph needs 768."
        );
        let (_, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::ShortRead(4096, 816, 512)),
            &mut out,
        );
        assert_eq!(
            core::str::from_utf8(notice).unwrap(),
            "Embedding refused: a positioned read at byte 4096 came back with 512 of the 816 bytes the descriptor states -- an install that stopped part-way."
        );
        for error in [
            FileEmbedError::Open,
            FileEmbedError::Load(weights::LoadError::Arithmetic),
            FileEmbedError::Config(InferenceError::CorruptedWeights),
        ] {
            let (_, notice) = file_embed_notice(&error, &mut out);
            let text = core::str::from_utf8(notice).unwrap();
            assert!(!text.contains("  "), "{text} carries a missing space");
            assert!(text.starts_with("Embedding refused: "), "{text} does not open as a refusal");
        }
    }

    /// Builds an embedding request carrying `input` as its input, under `flags`.
    fn embedding_request_with(flags: u32, input: &[u8]) -> alloc::vec::Vec<u8> {
        let req = std::ai::AiInferenceRequest {
            model_id: 1,
            target_dim: 768,
            input_len: input.len() as u32,
            flags,
            input_offset: 0,
        };
        let hdr_len = core::mem::size_of::<std::ai::AiInferenceRequest>();
        let mut payload = alloc::vec::Vec::with_capacity(hdr_len + input.len());
        payload.extend_from_slice(unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, hdr_len)
        });
        payload.extend_from_slice(input);
        payload
    }
}

#[cfg(not(test))]
#[panic_handler]

fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
