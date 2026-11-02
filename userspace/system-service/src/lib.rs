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
    MSG_TYPE_SEARCH, MSG_TYPE_SEARCH_CHUNK, STATUS_CANCELLED, STATUS_ERROR, STATUS_SUCCESS,
    STATUS_UNAVAILABLE, STATUS_UNSUPPORTED,
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
    /// The file configured the engine; its tensors are not what the engine reads.
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

/// Builds the notice that answers a generative query when the model file is on
/// the disk and nothing was run from it.
///
/// WHY it quotes the file: "no local model weights loaded" was the reply a real
/// Gemma got too, because the code that loads weights was never reached. Naming
/// the architecture and the dimensions the file declares says which of the cases
/// the user is in, and writes down the number the next stage has to satisfy.
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
    match info.architecture() {
        Some(arch) => {
            notice.word(b" declares ");
            notice.word(arch.as_bytes());
        }
        None => notice.word(b" states no architecture"),
    }
    match gap {
        ModelGap::Configured | ModelGap::NoTensors => {
            notice.word(b" at ");
            notice.number(info.hidden_dim.unwrap_or(0));
            notice.word(b" wide over ");
            notice.number(info.num_layers.unwrap_or(0));
            // WHY the vocabulary is named as undeclared when the file never stated
            // it: "vocabulary 0 tokens" is a number the file did not give, and a
            // stripped embedding model is the case that reads it.
            match info.vocab_size {
                Some(tokens) => {
                    notice.word(b" blocks, vocabulary ");
                    notice.number(tokens);
                    notice.word(b" tokens");
                }
                None => notice.word(b" blocks, vocabulary undeclared"),
            }
            // WHY two endings for one shape: a file whose tensors the engine cannot
            // read and a file that holds no tensors at all call for different next
            // moves, and only one of them is a placeholder install.
            if matches!(gap, ModelGap::NoTensors) {
                notice.word(b", and holds no tensors");
            } else {
                notice.word(b", whose tensors the engine does not read");
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
                                    // WHY this window is wider than the one the
                                    // branch below uses: this notice carries the
                                    // vocabulary's count and where the template came
                                    // from as well, and the shorter one would cut the
                                    // sentence off mid-word. The scratch refusal adds
                                    // three full byte counts to that sentence, so the
                                    // window is sized for them too -- see
                                    // `test_the_longest_notice_still_fits_the_window_it_is_written_in`.
                                    let mut scratch = [0u8; 448];
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
                                    let mut scratch = [0u8; 240];
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
                                    let mut scratch = [0u8; 320];
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
                                    let mut scratch = [0u8; 240];
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
            token_embd: &embd,
            token_embd_type: 0,
            layers: &layers,
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

    /// A file that states the architecture the notice can quote it; one that does
    /// not must not be answered as if it had.
    fn declared(arch: &[u8], hidden: u32, layers: u32, vocab: u32) -> gguf::GgufModelInfo {
        let mut info = gguf::GgufModelInfo::empty();
        info.architecture[..arch.len()].copy_from_slice(arch);
        info.architecture_len = arch.len();
        info.hidden_dim = Some(hidden);
        info.num_layers = Some(layers);
        info.vocab_size = Some(vocab);
        info
    }

    #[test]
    fn test_model_gap_notice_quotes_the_file_and_counts_the_prompt() {
        let info = declared(b"gemma2", 64, 2, 290);
        let mut out = [0u8; 240];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"Summarize my notifications",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2 at 64 wide over 2 blocks, vocabulary 290 tokens, whose tensors the engine does not read; the prompt's 26 bytes were not answered."
        );
    }

    /// A file that parses and holds no tensor is not an unsupported architecture;
    /// it is a file with nothing in it yet, and the answer has to say so.
    #[test]
    fn test_model_gap_notice_says_so_when_the_file_holds_no_tensors() {
        let info = declared(b"gemma2", 64, 2, 290);
        let mut out = [0u8; 240];
        let notice = model_gap_notice(&info, ModelGap::NoTensors, b"hello", None, &mut out);
        assert_eq!(
            notice,
            b"Model file declares gemma2 at 64 wide over 2 blocks, vocabulary 290 tokens, and holds no tensors; the prompt's 5 bytes were not answered."
        );
    }

    #[test]
    fn test_model_gap_notice_reports_a_declaration_beyond_the_engine() {
        let info = declared(b"gemma2", 2304, 26, 256_000);
        let mut out = [0u8; 240];
        let notice = model_gap_notice(
            &info,
            ModelGap::Declaration("<arch>.embedding_length", Some(2304), 256),
            b"hi",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2, and states <arch>.embedding_length as 2304 against the engine's bound of 256; the prompt's 2 bytes were not answered."
        );
    }

    #[test]
    fn test_model_gap_notice_names_the_key_a_file_omitted() {
        let mut out = [0u8; 240];
        let notice = model_gap_notice(
            &gguf::GgufModelInfo::empty(),
            ModelGap::Declaration("<arch>.block_count", None, 0),
            b"",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file states no architecture, and states no <arch>.block_count; the prompt's 0 bytes were not answered."
        );
    }

    /// The one size limit the engine carries now is a byte budget, so the refusal
    /// has to carry three byte counts: what the run would have taken, the budget it
    /// was refused against, and how far over it is. "Unsupported architecture"
    /// names none of them, and a person cannot choose a smaller model from that.
    #[test]
    fn test_the_scratch_refusal_names_the_budget_and_the_overage() {
        let info = declared(b"gemma2", 3072, 48, 256_000);
        let mut out = [0u8; 320];
        let notice = model_gap_notice(
            &info,
            ModelGap::Scratch(437_421_056, 125_829_120),
            b"hi",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2, whose run needs 437421056 scratch bytes against the 125829120 byte budget, over by 311591936 bytes; the prompt's 2 bytes were not answered."
        );
    }

    /// The budget and the mapping are two different answers: one says the device
    /// has no room for this model, the other says it could not be handed out right
    /// now. Reporting the second as the first would send someone looking for a
    /// smaller file when the machine is what is full.
    #[test]
    fn test_the_budget_refusal_names_the_memory_the_device_would_not_map() {
        let mut out = [0u8; 240];
        let notice = model_gap_notice(
            &gguf::GgufModelInfo::empty(),
            ModelGap::ScratchUnavailable,
            b"",
            None,
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file states no architecture, whose scratch the byte budget prices as fitting but the device would not map; the prompt's 0 bytes were not answered."
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
        let mut out = [0u8; 320];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"hello",
            Some(&walked_vocab_and_template()),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2 at 64 wide over 2 blocks, vocabulary 256000 tokens, whose tensors the engine does not read, vocabulary counted not loaded, template 28 bytes from file, bos 2 eos 1 pad 0, add-bos 1, pre gemma; the prompt's 5 bytes were not answered."
        );
    }

    /// WHY this is its own case: the platform has a built-in template, and a screen
    /// that showed a formatted prompt could not tell the person whether the model's
    /// own formatting or the platform's default ran. The answer names the default.
    #[test]
    fn test_the_notice_names_the_built_in_default_when_the_file_declared_no_tokenizer() {
        let mut info = declared(b"gemma2", 64, 2, 290);
        info.vocab_size = None;
        let mut out = [0u8; 320];
        let notice = model_gap_notice(
            &info,
            ModelGap::NoTensors,
            b"hello",
            Some(&gguf::GgufTokenizerInfo::empty()),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2 at 64 wide over 2 blocks, vocabulary undeclared, and holds no tensors, tokenizer absent, built-in template in use; the prompt's 5 bytes were not answered."
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
        let mut out = [0u8; 320];
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"",
            Some(&tok),
            &mut out,
        );
        assert_eq!(
            notice,
            b"Model file declares gemma2 at 64 wide over 2 blocks, vocabulary 256000 tokens, whose tensors the engine does not read, vocabulary counted not loaded, template 4096 bytes refused, no token ids; the prompt's 0 bytes were not answered."
        );
        // A refused template is not an empty one: the engine runs the default, and
        // the walk keeps the length the file gave.
        assert_eq!(
            tok.active_chat_template().1,
            gguf::TemplateSource::BuiltInDefault
        );
    }

    /// WHY the worst case is measured and not assumed: `Notice` stops at the end of
    /// its window without complaining, and a notice cut mid-sentence is a sentence
    /// that no longer says what the file said. Every clause the walk can write is
    /// set at once here -- the widest numbers, a counted vocabulary, a read
    /// template, all three token ids, the flag, the pre-name and a truncation --
    /// because that is the only shape that reaches the window's end.
    #[test]
    fn test_the_longest_notice_still_fits_the_window_it_is_written_in() {
        let mut tok = walked_vocab_and_template();
        tok.truncated_in = Some(b"tokenizer.ggml.tokens");
        let info = declared(b"gemma2", 2_048, 26, 256_000);
        let mut out = [0u8; 320];
        let window = out.len();
        let notice = model_gap_notice(
            &info,
            ModelGap::Configured,
            b"a prompt of thirty bytes or thereabouts",
            Some(&tok),
            &mut out,
        );
        let text = core::str::from_utf8(notice).unwrap_or("<unprintable>");
        assert!(
            notice.len() < window,
            "the {window}-byte window is exactly full, so Notice was cutting words off: {text}"
        );
        assert!(
            text.ends_with("bytes were not answered."),
            "the whole sentence did not survive the walk's clauses: {text}"
        );

        // The scratch refusal is the longest sentence this window carries: three
        // byte counts at their widest on top of everything the walk adds, so it is
        // the case the dispatch site's window is sized for.
        let mut scratch_out = [0u8; 448];
        let scratch_window = scratch_out.len();
        let scratch_text = model_gap_notice(
            &info,
            ModelGap::Scratch(usize::MAX, inference::SCRATCH_BYTE_BUDGET),
            b"a prompt of thirty bytes or thereabouts",
            Some(&tok),
            &mut scratch_out,
        );
        let scratch_text = core::str::from_utf8(scratch_text).unwrap_or("<unprintable>");
        assert!(
            scratch_text.len() < scratch_window,
            "the {scratch_window}-byte window is exactly full, so Notice was cutting words off: {scratch_text}"
        );
        assert!(
            scratch_text.ends_with("bytes were not answered."),
            "the scratch refusal did not survive the walk's clauses: {scratch_text}"
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
}

#[cfg(not(test))]
#[panic_handler]

fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
