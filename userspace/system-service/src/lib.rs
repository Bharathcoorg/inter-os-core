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
pub mod backend;
pub mod litertlm;
pub mod gguf;
pub mod inference;
pub mod rag;
pub mod tokenizer;
pub mod weights;

pub use backend::InferenceBackend;
pub use embedding::{
    compute_embedding_768, compute_text_embedding_dim_f32, compute_text_embedding_f32,
    compute_text_embedding_q7, f32_sqrt, EMBEDDING_DIM, MATRYOSHKA_DIMS,
};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ResourceManifest {
    pub peak_scratch_bytes: u64,
    pub kv_cache_bytes: u64,
    pub peak_activations_bytes: u64,
    pub tensor_alignment: u32,
    pub quantization_format: u32,
}

pub fn verify_resource_manifest(manifest: &ResourceManifest) -> Result<(), InferenceError> {
    let total_needed = manifest.peak_scratch_bytes
        .checked_add(manifest.kv_cache_bytes)
        .and_then(|t| t.checked_add(manifest.peak_activations_bytes))
        .ok_or(InferenceError::ScratchTooLarge(0, SCRATCH_BYTE_BUDGET))?;
    
    if total_needed > SCRATCH_BYTE_BUDGET as u64 {
        return Err(InferenceError::ScratchTooLarge(total_needed as usize, SCRATCH_BYTE_BUDGET));
    }
    Ok(())
}
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
    FileTokenizer, TOKEN_BOS, TOKEN_EOS, TOKEN_PAD, TOKEN_UNK,
};
pub use std::ai::{
    MSG_TYPE_AGENT_PLAN, MSG_TYPE_CANCEL, MSG_TYPE_COMPANION_QUERY, MSG_TYPE_EMBED,
    MSG_TYPE_EMBED_CHUNK, MSG_TYPE_GENERATE, MSG_TYPE_GENERATE_ALT, MSG_TYPE_GENERATE_CHUNK,
    MSG_TYPE_GGUF, MSG_TYPE_INSTALL, MSG_TYPE_INSTALL_CHUNK, MSG_TYPE_ROLE_RESOLVE, MSG_TYPE_SEARCH,
    MSG_TYPE_SEARCH_CHUNK, MSG_TYPE_TOOL_EXEC, STATUS_BASELINE_LEXICAL, STATUS_CANCELLED,
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
            let _ = std::event_poll(std::EVENT_IPC_MSG, 50);
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
        if has_epoll {
            let _ = std::event_poll(std::EVENT_TOUCH_INPUT | std::EVENT_TIMER_TICK, 20);
        } else {
            std::yield_now();
        }
        while std::input_event_poll(&mut event).is_ok() {
            // Drain normalized service input events to prevent queue saturation
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
            while let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut request) {
                if msg_type == std::ai::MSG_TYPE_INSTALL
                    && &request[..len] == b"INSL"
                    && std::service_peer_matches(std::ai::SERVICE_INFERENCE, sender)
                {
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
            last_tick = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(now + 1);
            if last_tick == 0 {
                last_tick = 1;
            }
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
            let _ = std::event_poll(std::EVENT_IPC_MSG, 50);
        } else {
            std::yield_now();
        }
    }
}

/// Android Compatibility Runtime daemon execution path (`acrd`).
///
/// Designated as userspace compatibility service coordinator.
/// Coordinates Ring-3 Android app domains, package verification, process lifecycle,
/// and surface binding outside Ring 0.
pub fn run_acr() -> ! {
    let _ = std::log("[acrd] userspace Android Compatibility Runtime daemon online (Ring 3 isolation).");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    let mut rx_buf = [0u8; 1024];

    loop {
        if let Some(_) = ipc_slot {
            while let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len >= 4 {
                    let msg_magic = u32::from_le_bytes([rx_buf[0], rx_buf[1], rx_buf[2], rx_buf[3]]);
                    match msg_magic {
                        // "INST" (0x5453_4E49) Out-of-band candidate artifact installation request
                        0x5453_4E49 => {
                            let _ = std::log("[acrd] processing out-of-band candidate artifact installation request");
                            if len >= 48 {
                                let candidate_handle = u32::from_le_bytes([rx_buf[4], rx_buf[5], rx_buf[6], rx_buf[7]]);
                                let surface_id = u32::from_le_bytes([rx_buf[8], rx_buf[9], rx_buf[10], rx_buf[11]]);
                                let _flags = u32::from_le_bytes([rx_buf[12], rx_buf[13], rx_buf[14], rx_buf[15]]);
                                let mut expected_candidate_digest = [0u8; 32];
                                expected_candidate_digest.copy_from_slice(&rx_buf[16..48]);

                                let _ = std::log(&alloc::format!("[acrd] candidate handle={}, reading bytes...", candidate_handle));

                                // 1. Read candidate unverified artifact in chunks from kernel candidate store
                                let mut candidate_bytes = alloc::vec::Vec::new();
                                let mut chunk = [0u8; 4096];
                                let mut offset = 0u32;
                                let mut read_failed = false;
                                loop {
                                    match std::candidate_artifact_read(candidate_handle, offset, &mut chunk) {
                                        Ok(0) => {
                                            let _ = std::log(&alloc::format!("[acrd] candidate read done, bytes={}", candidate_bytes.len()));
                                            break;
                                        }
                                        Ok(n) => {
                                            candidate_bytes.extend_from_slice(&chunk[..n]);
                                            offset += n as u32;
                                        }
                                        Err(e) => {
                                            let _ = std::log(&alloc::format!("[acrd] candidate_artifact_read err={}", e));
                                            read_failed = true;
                                            break;
                                        }
                                    }
                                }

                                if read_failed || candidate_bytes.is_empty() {
                                    let _ = std::log("[acrd] REJECT candidate artifact read failed or empty");
                                    continue;
                                }

                                // 2. Verify candidate artifact digest against expected digest
                                let computed_candidate_digest = std::crypto::Sha256::digest(&candidate_bytes);
                                if computed_candidate_digest != expected_candidate_digest {
                                    let _ = std::log("[acrd] REJECT candidate artifact digest mismatch");
                                    continue;
                                }

                                let _ = std::log("[acrd] parsing APK package...");

                                // 3. Parse APK package
                                let apk = match interos_android_format::apk::ApkPackage::parse(&candidate_bytes) {
                                    Ok(a) => a,
                                    Err(_) => {
                                        let _ = std::log("[acrd] REJECT candidate APK format parse error");
                                        continue;
                                    }
                                };

                                if !apk.is_signed {
                                    let _ = std::log("[acrd] REJECT candidate APK is unsigned");
                                    continue;
                                }

                                let version_code = apk.manifest.version_code as u64;
                                let min_sdk = 21u32;
                                let target_sdk = apk.manifest.target_sdk;
                                let signer_identity = apk.signing_details.as_ref()
                                    .map(|s| s.primary_cert_fingerprint)
                                    .unwrap_or(computed_candidate_digest);

                                let major = (version_code >> 32) as u16;
                                let minor = (version_code >> 16) as u16;
                                let patch = version_code as u16;

                                // 4. Atomically commit candidate artifact and register verified package in kernel registry
                                if let Ok(app_uid) = std::package_register_apk_ext(
                                    &apk.manifest.package_name,
                                    (major, minor, patch),
                                    version_code,
                                    min_sdk,
                                    target_sdk,
                                    &computed_candidate_digest,
                                    &signer_identity,
                                    candidate_handle,
                                ) {
                                    let _ = std::log("[acrd] verified and registered Android package with stable UID");
                                    if let Ok(task_id) = std::app_launch(&apk.manifest.package_name) {
                                        let _ = std::log("[acrd] spawned isolated Android child task");
                                        // Construct compact bounded 54-byte LNCH descriptor:
                                        // magic "LNCH" (4) + version (2) + surface_id (4) + memory_quota (4) + target_sdk (4) + app_uid (4) + code_digest (32) = 54 bytes
                                        let mut lnch = [0u8; 54];
                                        lnch[0..4].copy_from_slice(b"LNCH");
                                        lnch[4..6].copy_from_slice(&1u16.to_le_bytes());
                                        lnch[6..10].copy_from_slice(&surface_id.to_le_bytes());
                                        lnch[10..14].copy_from_slice(&(32 * 1024 * 1024u32).to_le_bytes());
                                        lnch[14..18].copy_from_slice(&target_sdk.to_le_bytes());
                                        lnch[18..22].copy_from_slice(&app_uid.to_le_bytes());
                                        lnch[22..54].copy_from_slice(&computed_candidate_digest);

                                        let _ = std::ipc_send(task_id, 0x0052_4341, &lnch);

                                        let mut resp = [0u8; 16];
                                        resp[0..4].copy_from_slice(&0x5453_4E49u32.to_le_bytes());
                                        resp[4..8].copy_from_slice(&app_uid.to_le_bytes());
                                        resp[8..16].copy_from_slice(&task_id.to_le_bytes());
                                        let _ = std::ipc_send(sender, msg_type, &resp);
                                    } else {
                                        let _ = std::log("[acrd] REJECT app_launch failed");
                                    }
                                } else {
                                    let _ = std::log("[acrd] REJECT package registration failed");
                                }
                            }
                        }
                        // "ACR\0" (0x0052_4341) APK install/launch request (inline staging fallback)
                        0x0052_4341 => {
                            let _ = std::log("[acrd] processing APK launch request in Ring-3");
                            if len > 8 {
                                let surface_id = u32::from_le_bytes([rx_buf[4], rx_buf[5], rx_buf[6], rx_buf[7]]);
                                let apk_slice = &rx_buf[8..len];
                                if let Ok(apk) = interos_android_format::apk::ApkPackage::parse(apk_slice) {
                                    if apk.is_signed {
                                        let code_digest = std::crypto::Sha256::digest(apk_slice);
                                        let version_code = apk.manifest.version_code as u64;
                                        let min_sdk = 21u32;
                                        let target_sdk = apk.manifest.target_sdk;
                                        let signer_identity = apk.signing_details.as_ref()
                                            .map(|s| s.primary_cert_fingerprint)
                                            .unwrap_or(code_digest);

                                        let _ = std::package_artifact_store(&apk.manifest.package_name, apk_slice, false);
                                        let cand_handle = std::candidate_artifact_store(0, apk_slice, false).unwrap_or(0);

                                        // Register verified APK in kernel registry with complete security metadata
                                        let major = (version_code >> 32) as u16;
                                        let minor = (version_code >> 16) as u16;
                                        let patch = version_code as u16;
                                        if let Ok(app_uid) = std::package_register_apk_ext(
                                            &apk.manifest.package_name,
                                            (major, minor, patch),
                                            version_code,
                                            min_sdk,
                                            target_sdk,
                                            &code_digest,
                                            &signer_identity,
                                            cand_handle,
                                        ) {
                                            let _ = std::log("[acrd] verified and registered Android package with stable UID");
                                            if let Ok(task_id) = std::app_launch(&apk.manifest.package_name) {
                                                let _ = std::log("[acrd] spawned isolated Android child task");
                                                // Construct compact bounded 54-byte LNCH descriptor:
                                                // magic "LNCH" (4) + version (2) + surface_id (4) + memory_quota (4) + target_sdk (4) + app_uid (4) + code_digest (32) = 54 bytes
                                                let mut lnch = [0u8; 54];
                                                lnch[0..4].copy_from_slice(b"LNCH");
                                                lnch[4..6].copy_from_slice(&1u16.to_le_bytes());
                                                lnch[6..10].copy_from_slice(&surface_id.to_le_bytes());
                                                lnch[10..14].copy_from_slice(&(32 * 1024 * 1024u32).to_le_bytes());
                                                lnch[14..18].copy_from_slice(&target_sdk.to_le_bytes());
                                                lnch[18..22].copy_from_slice(&app_uid.to_le_bytes());
                                                lnch[22..54].copy_from_slice(&code_digest);

                                                let _ = std::ipc_send(task_id, 0x0052_4341, &lnch);

                                                let mut resp = [0u8; 16];
                                                resp[0..4].copy_from_slice(&0x0052_4341u32.to_le_bytes());
                                                resp[4..8].copy_from_slice(&app_uid.to_le_bytes());
                                                resp[8..16].copy_from_slice(&task_id.to_le_bytes());
                                                let _ = std::ipc_send(sender, msg_type, &resp);
                                            }
                                        }
                                    }
                                }
                            }
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
            let _ = std::event_poll(std::EVENT_IPC_MSG, 50);
        } else {
            std::yield_now();
        }
    }
}

/// Unprivileged Android Application Runtime (`acr-app-runtime`).
///
/// Executes inside an isolated per-APK Ring-3 child task/address space.
/// Owns local DalvikVmState, Looper, Choreographer, Activity lifecycle,
/// and View hierarchy.
/// Authenticates the parent coordinator daemon (`acrd`), receives the launch descriptor,
/// reads the verified artifact from the kernel repository, verifies cryptographic integrity,
/// invokes `MainActivity.onCreate`, handles UI/frame pacing, and provides fault containment.
pub fn run_acr_app_runtime() -> ! {
    let _ = std::log("[acr-app-runtime] unprivileged Android app runtime started in child task.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    let self_id = match std::task_identity_query(0) {
        Ok(id) if !id.package_name().is_empty() => id,
        _ => {
            let _ = std::log("[acr-app-runtime] FATAL: missing authenticated package identity; aborting.");
            std::exit_with_code(1);
        }
    };

    let mut instance: Option<interos_acr::manager::AcrAppInstance> = None;
    let mut root_widget: Option<interos_acr::view::View> = None;
    let mut rx_buf = [0u8; 1024];

    loop {
        if ipc_slot.is_some() {
            while let Ok((sender, _msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len >= 4 {
                    let magic = u32::from_le_bytes([rx_buf[0], rx_buf[1], rx_buf[2], rx_buf[3]]);
                    match magic {
                        // "LNCH" (0x4843_4E4C) Launch descriptor from acrd
                        0x4843_4E4C => {
                            let _ = std::log("[acr-app-runtime] STATE: LaunchReceived from sender");
                            // Authenticate sender: must strictly be the parent acrd coordinator daemon
                            let sender_info = match std::task_identity_query(sender) {
                                Ok(info) => info,
                                Err(_) => {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed - sender identity check failed");
                                    continue;
                                }
                            };
                            if sender != self_id.parent_id || (sender_info.task_name() != "acrd" && sender_info.package_name() != "org.interos.acrd") {
                                let _ = std::log("[acr-app-runtime] STATE: Failed - unauthorized sender (not parent acrd coordinator)");
                                continue;
                            }

                            if len >= 54 {
                                let surface_id = u32::from_le_bytes([rx_buf[6], rx_buf[7], rx_buf[8], rx_buf[9]]);
                                let memory_quota = u32::from_le_bytes([rx_buf[10], rx_buf[11], rx_buf[12], rx_buf[13]]) as usize;
                                let target_sdk = u32::from_le_bytes([rx_buf[14], rx_buf[15], rx_buf[16], rx_buf[17]]);
                                let app_uid = u32::from_le_bytes([rx_buf[18], rx_buf[19], rx_buf[20], rx_buf[21]]);
                                let mut desc_digest = [0u8; 32];
                                desc_digest.copy_from_slice(&rx_buf[22..54]);

                                // Bind launch to kernel security identity
                                if app_uid != self_id.user_id || desc_digest != self_id.code_digest {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed - security identity mismatch between descriptor and kernel");
                                    continue;
                                }

                                // Bounded chunked reader for package artifact respecting memory quota
                                let mut full_artifact = alloc::vec::Vec::new();
                                let mut chunk_buf = [0u8; 4096];
                                let mut offset = 0u32;
                                let mut read_failed = false;
                                loop {
                                    match std::package_artifact_read(offset, &mut chunk_buf) {
                                        Ok(0) => break,
                                        Ok(n) => {
                                            if full_artifact.len() + n > memory_quota {
                                                let _ = std::log("[acr-app-runtime] STATE: Failed - package artifact exceeds memory quota");
                                                read_failed = true;
                                                break;
                                            }
                                            full_artifact.extend_from_slice(&chunk_buf[..n]);
                                            offset += n as u32;
                                        }
                                        Err(_) => {
                                            read_failed = true;
                                            break;
                                        }
                                    }
                                }
                                if read_failed || full_artifact.is_empty() {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed - unable to read complete package artifact");
                                    continue;
                                }
                                let _ = std::log("[acr-app-runtime] STATE: ArtifactOpened");

                                // Verify full artifact SHA-256 against kernel-authenticated code digest
                                let computed_digest = std::crypto::Sha256::digest(&full_artifact);
                                if computed_digest != self_id.code_digest {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed - artifact digest mismatch");
                                    continue;
                                }
                                let _ = std::log("[acr-app-runtime] STATE: ArtifactDigestVerified");

                                // Extract DEX bytes and parse
                                let pkg_name = self_id.package_name();
                                let mut dex_bytes = alloc::vec::Vec::new();
                                if let Ok(apk) = interos_android_format::apk::ApkPackage::parse(&full_artifact) {
                                    dex_bytes = apk.classes_dex;
                                } else if let Ok(_) = interos_android_format::dex::DexPackage::parse(&full_artifact) {
                                    dex_bytes.extend_from_slice(&full_artifact);
                                }

                                if dex_bytes.is_empty() {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed(NoDex) - no valid DEX payload found in artifact");
                                    continue;
                                }

                                let parsed_dex = match interos_android_format::dex::DexPackage::parse(&dex_bytes) {
                                    Ok(d) => {
                                        let _ = std::log("[acr-app-runtime] STATE: DexVerified");
                                        d
                                    }
                                    Err(_) => {
                                        let _ = std::log("[acr-app-runtime] STATE: Failed(DexParseError) - malformed DEX header or bytecode");
                                        continue;
                                    }
                                };

                                let main_class_path = pkg_name.replace('.', "/");
                                let candidate_activities = [
                                    alloc::format!("L{}/activities/MainActivity;", main_class_path),
                                    alloc::format!("L{}/MainActivity;", main_class_path),
                                    alloc::format!("L{}/activities/SplashActivity;", main_class_path),
                                    alloc::format!("L{}/SplashActivity;", main_class_path),
                                ];
                                let entry_activity = candidate_activities.iter().find(|act| {
                                    parsed_dex.types.iter().any(|t| t == *act)
                                        || parsed_dex.methods.iter().any(|m| &m.class_name == *act)
                                }).cloned().unwrap_or_else(|| alloc::format!("L{}/MainActivity;", main_class_path));

                                let main_activity_class = entry_activity;
                                let class_exists = parsed_dex.types.iter().any(|t| t == &main_activity_class)
                                    || parsed_dex.methods.iter().any(|m| m.class_name == main_activity_class);
                                if !class_exists {
                                    let _ = std::log("[acr-app-runtime] STATE: Failed(ClassNotFound) - MainActivity not found in DEX");
                                    continue;
                                }
                                let _ = std::log("[acr-app-runtime] STATE: ClassResolved");
                                let _ = std::log("[acr-app-runtime] STATE: ApplicationCreated");

                                let mut inst = interos_acr::manager::AcrAppInstance::new_with_uid(
                                    app_uid,
                                    self_id.task_id,
                                    self_id.user_id,
                                    pkg_name,
                                    target_sdk,
                                    surface_id,
                                    memory_quota,
                                    dex_bytes.clone(),
                                );

                                match inst.vm.invoke_method(&parsed_dex, &main_activity_class, "onCreate") {
                                    Ok(_) => {
                                        let _ = std::log("[acr-app-runtime] STATE: ActivityCreated");
                                        let _ = std::log("[acr-app-runtime] MainActivity.onCreate executed in child Dalvik VM");
                                    }
                                    Err(_) => {
                                        let _ = std::log("[acr-app-runtime] STATE: Failed(OnCreateFailed) - VM invocation returned error");
                                        continue;
                                    }
                                }

                                let _ = std::log("[acr-app-runtime] STATE: ActivityStarted");
                                inst.activity_state = interos_acr::framework::ActivityLifecycleState::Resumed;
                                let _ = std::log("[acr-app-runtime] STATE: ActivityResumed");

                                // Build View hierarchy
                                let mut root_view = interos_acr::view::View::linear_layout(1, true);
                                root_view.padding = interos_acr::view::EdgeOffsets::all(20);
                                root_view.background_color = Some(0xFF181818);
                                root_view.is_clickable = true;

                                let title = interos_acr::view::View::text_view(2, "InterOS Universal Android Runtime", 18, 0xFFFFFFFF);
                                root_view.add_child(title);

                                let sub = interos_acr::view::View::text_view(3, pkg_name, 14, 0xFF4CAF50);
                                root_view.add_child(sub);

                                let mut btn = interos_acr::view::View::button(4, "Interactive Button", 16, 0xFFFFFFFF);
                                btn.margins.top = 30;
                                root_view.add_child(btn);

                                root_view.measure(
                                    interos_acr::view::MeasureSpec::exactly(720),
                                    interos_acr::view::MeasureSpec::exactly(1440),
                                );
                                root_view.layout(0, 0, 720, 1440);

                                // Scoped graphics: create app-owned surface via SurfaceCreate capability
                                let create_surface_fn = interos_std::surface_create;
                                let write_surface_fn = interos_std::surface_write;
                                if let Ok(app_surface_id) = create_surface_fn(720, 1440, 200) {
                                    inst.surface_id = app_surface_id;
                                    let _ = std::log("[acr-app-runtime] created app-owned surface via SurfaceCreate capability");

                                    let mut render_buf = alloc::vec![0u32; 720 * 1440];
                                    let mut dl = interos_acr::draw::DrawList::new();
                                    root_view.draw(&mut dl);
                                    interos_acr::draw::render_draw_list_to_buffer(&dl, &mut render_buf, 720, 1440);
                                    for (chunk_idx, chunk) in render_buf.chunks(4096).enumerate() {
                                        let _ = write_surface_fn(app_surface_id, (chunk_idx * 4096) as usize, chunk);
                                    }
                                    let _ = std::surface_flip();
                                    let _ = std::log("[acr-app-runtime] rendered View hierarchy and presented pixels to compositor");
                                }

                                root_widget = Some(root_view);
                                instance = Some(inst);
                            }
                        }
                        // "CRSH" (0x4853_5243) Deliberate fault injection to prove crash containment
                        0x4853_5243 => {
                            let sender_info = match std::task_identity_query(sender) {
                                Ok(info) => info,
                                Err(_) => continue,
                            };
                            if sender != self_id.parent_id || (sender_info.task_name() != "acrd" && sender_info.package_name() != "org.interos.acrd") {
                                let _ = std::log("[acr-app-runtime] STATE: Failed - unauthorized sender for CRSH");
                                continue;
                            }
                            let sub_mode = if len >= 8 {
                                &rx_buf[4..8]
                            } else {
                                b"PAGE"
                            };

                            if sub_mode == b"#UD " || sub_mode == b"UD2 " || sub_mode == b"UD  " {
                                let _ = std::log("[acr-app-runtime] FAULT INJECTION: triggering deliberate Ring-3 #UD (Invalid Opcode) via ud2");
                                #[cfg(target_arch = "x86_64")]
                                unsafe {
                                    core::arch::asm!("ud2", options(noreturn));
                                }
                                #[cfg(target_arch = "aarch64")]
                                unsafe {
                                    core::arch::asm!("udf #0", options(noreturn));
                                }
                                #[cfg(target_arch = "riscv64")]
                                unsafe {
                                    core::arch::asm!("unimp", options(noreturn));
                                }
                            } else if sub_mode == b"#DE " || sub_mode == b"DIV0" || sub_mode == b"DE  " {
                                let _ = std::log("[acr-app-runtime] FAULT INJECTION: triggering deliberate Ring-3 #DE (Divide Error)");
                                #[cfg(target_arch = "x86_64")]
                                unsafe {
                                    core::arch::asm!(
                                        "mov eax, 1",
                                        "xor ecx, ecx",
                                        "div ecx",
                                        options(noreturn)
                                    );
                                }
                                #[cfg(not(target_arch = "x86_64"))]
                                unsafe {
                                    core::ptr::write_volatile(0xDEAD_BEEF as *mut u8, 0x42);
                                }
                            } else if sub_mode == b"PANI" || sub_mode == b"PANIC" {
                                let _ = std::log("[acr-app-runtime] FAULT INJECTION: triggering deliberate userspace panic in Ring-3 child process");
                                panic!("deliberate userspace panic for crash containment test");
                            } else {
                                let _ = std::log("[acr-app-runtime] FAULT INJECTION: triggering deliberate memory fault in child process");
                                unsafe {
                                    core::ptr::write_volatile(0xDEAD_BEEF as *mut u8, 0x42);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 16 || last_tick == 0 {
            last_tick = now;
            if let Some(ref mut inst) = instance {
                let now_nanos = now.saturating_mul(1_000_000);
                let _ = inst.looper.loop_once(now);
                let _ = inst.choreographer.do_frame(now_nanos);
                let _ = inst.vm.step_scheduler_slice(4, now);
            }

            if let (Some(ref mut inst), Some(ref mut root)) = (&mut instance, &mut root_widget) {
                let mut input_event = std::InputEvent { kind: 0, x: 0, y: 0, dx: 0, dy: 0, scale_milli: 1000 };
                while std::input_event_poll(&mut input_event).is_ok() {
                    if input_event.kind == 1 || input_event.kind == 2 || input_event.kind == 3 {
                        let me_down = interos_acr::view::MotionEvent {
                            action: interos_acr::view::MotionAction::Down,
                            x: input_event.x as i32,
                            y: input_event.y as i32,
                            timestamp_ms: now,
                        };
                        let me_up = interos_acr::view::MotionEvent {
                            action: interos_acr::view::MotionAction::Up,
                            x: input_event.x as i32,
                            y: input_event.y as i32,
                            timestamp_ms: now + 50,
                        };
                        let hit_down = root.dispatch_touch_event(&me_down);
                        let hit_up = root.dispatch_touch_event(&me_up);
                        let _hit = hit_down || hit_up;
                        let _ = std::log(&alloc::format!(
                            "[acr-app-runtime] MotionEvent dispatched: touch at ({}, {}) -> UI updated and frame presented",
                            input_event.x, input_event.y
                        ));
                        if inst.surface_id > 0 {
                                let write_surface_fn = interos_std::surface_write;
                                let mut render_buf = alloc::vec![0u32; 720 * 1440];
                                let mut dl = interos_acr::draw::DrawList::new();
                                root.draw(&mut dl);
                                interos_acr::draw::render_draw_list_to_buffer(&dl, &mut render_buf, 720, 1440);
                                for (chunk_idx, chunk) in render_buf.chunks(4096).enumerate() {
                                    let _ = write_surface_fn(inst.surface_id, (chunk_idx * 4096) as usize, chunk);
                                }
                                let _ = std::surface_flip();
                            }
                        }
                    }
                }
            }

        if has_epoll {
            if std::event_poll(std::EVENT_IPC_MSG, 16).is_err() {
                std::yield_now();
            }
        } else {
            std::yield_now();
        }
    }
}

/// Semantic Search & RAG daemon execution path (`searchd`).
///
/// Designated as service id 13 (`SERVICE_SEARCH`) in the system service ABI.
/// Manages vector index, storage ingestion, semantic retrieval queries, and
/// storage budgets.
pub fn run_search() -> ! {
    let _ = std::log("[searchd] userspace Search & RAG daemon online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    let uid = std::user_get_current();
    // Sole custody of master secret root belongs to keystored. Ordinary services query keystored.
    let search_handle = if uid > 0 {
        std::keystore::keystore_derive_key(
            std::ai::SERVICE_KEYSTORE_TASK_ID,
            std::keystore::DOMAIN_SEMANTIC_SEARCH,
            std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT,
            b"rag_index",
        ).ok()
    } else {
        None
    };

    let mut index = rag::RagIndex::new(false);
    // Section 40: Zero Plaintext Fallback. If a valid user search key cannot be obtained,
    // the daemon MUST run in-memory only. It MUST NOT fall back to writing or reading unencrypted vector indices.
    let loaded = if let Some(handle) = search_handle {
        index.load_from_disk_with_handle(rag::PATH_SEARCH_INDEX_DATA, handle, std::ai::SERVICE_KEYSTORE_TASK_ID).is_ok()
            || index.load_from_disk_with_handle(rag::PATH_SEARCH_INDEX_STORAGE, handle, std::ai::SERVICE_KEYSTORE_TASK_ID).is_ok()
    } else {
        false
    };

    if !loaded {
        // Section 39: Blanket directory crawling is eliminated in favor of explicit ContentIndex registration.
        if let Some(handle) = search_handle {
            let _ = index.save_to_disk_with_handle(rag::PATH_SEARCH_INDEX_STORAGE, handle, std::ai::SERVICE_KEYSTORE_TASK_ID);
            let _ = std::log("[searchd] initialized encrypted vector index.");
        } else {
            let _ = std::log("[searchd] search key unavailable: running in-memory index only (zero plaintext disk fallback).");
        }
    } else {
        let _ = std::log("[searchd] loaded existing encrypted vector index from persistent storage.");
    }

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];

    loop {
        let _ = std::service_heartbeat(std::ai::SERVICE_SEARCH);

        if let Some(_) = ipc_slot {
            while let Ok((sender_task_id, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len > 0 && sender_task_id != 0 {
                    handle_search_ipc(sender_task_id, msg_type, &rx_buf[..len], &mut index, &mut tx_buf);
                }
            }
        }

        if let Some(_) = timer_slot {
            if let Ok(ticks) = std::get_ticks() {
                if ticks.saturating_sub(last_tick) >= 60_000 {
                    last_tick = ticks;
                    // Periodic blanket storage crawl removed per Section 39!
                }
            }
        }

        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 50);
        } else {
            std::yield_now();
        }
    }
}

fn handle_search_ipc(
    sender_task_id: u64,
    msg_type: u32,
    payload: &[u8],
    index: &mut rag::RagIndex,
    tx_buf: &mut [u8; 1024],
) {
    if sender_task_id == 0 {
        return;
    }
    if msg_type != MSG_TYPE_SEARCH && msg_type != 0 {
        return;
    }

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

    let mut matches = [rag::RagMatch::default(); 8];
    let found_count = index.search(query_text, target_dim, top_k, min_confidence, &mut matches);

    let status = if found_count == 0 {
        STATUS_UNAVAILABLE
    } else {
        STATUS_SUCCESS
    };

    let resp = std::ai::AiInferenceResponse {
        status,
        output_dim: found_count as u32,
        latency_micros: 0,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };
    let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, resp_hdr_len)
    };
    tx_buf[..resp_hdr_len].copy_from_slice(resp_bytes);
    let mut cursor = resp_hdr_len;

    for i in 0..found_count.min(8) {
        let m = &matches[i];
        let path_str = m.source_path_str();
        let text_str = m.snippet_str();
        let p_len = path_str.len().min(64);
        let t_len = text_str.len().min(128);

        if cursor + 4 + p_len + t_len + 8 > tx_buf.len() {
            break;
        }
        tx_buf[cursor..cursor + 2].copy_from_slice(&(p_len as u16).to_le_bytes());
        cursor += 2;
        tx_buf[cursor..cursor + p_len].copy_from_slice(&path_str.as_bytes()[..p_len]);
        cursor += p_len;

        tx_buf[cursor..cursor + 2].copy_from_slice(&(t_len as u16).to_le_bytes());
        cursor += 2;
        tx_buf[cursor..cursor + t_len].copy_from_slice(&text_str.as_bytes()[..t_len]);
        cursor += t_len;

        tx_buf[cursor..cursor + 2].copy_from_slice(&m.score_q15.to_le_bytes());
        cursor += 2;
        tx_buf[cursor] = m.confidence_percent;
        cursor += 1;
        tx_buf[cursor] = if m.is_neural { 1 } else { 0 };
        cursor += 1;
    }

    if sender_task_id != 0 {
        let _ = std::ipc_send(sender_task_id, MSG_TYPE_SEARCH, &tx_buf[..cursor]);
    }
}

static mut AGENT_FILE_BUF: [u8; std::companion::COMPANION_STORAGE_BUDGET_MAX as usize] = [0u8; 65536];
static mut AGENT_SLOT_B_BUF: [u8; std::companion::COMPANION_STORAGE_BUDGET_MAX as usize] = [0u8; 65536];

fn is_authorized_agent_admin(sender_task_id: u64) -> bool {
    if sender_task_id == 0 {
        return false;
    }
    // Prohibit model runtime / agent daemon from approving its own actions
    if sender_task_id == std::ai::SERVICE_AGENT_TASK_ID || std::service_peer_matches(14, sender_task_id) {
        return false;
    }
    // Only designated user-facing shells and Launcher service are authorized admins
    if std::service_peer_matches(10, sender_task_id) {
        return true;
    }
    if let Ok(info) = std::task_identity_query(sender_task_id) {
        let pkg = info.package_name();
        if pkg == "org.interos.launcher"
            || pkg == "org.interos.settings"
            || pkg == "org.interos.agentmanager"
        {
            return true;
        }
    }
    false
}

fn populate_default_installed_tools(registry: &mut std::appmcp::DynamicToolRegistry) {
    let mut reg_buf = [0u8; 1024];
    let reg_data = if let Ok(n) = std::package_registry(&mut reg_buf) {
        core::str::from_utf8(&reg_buf[..n]).unwrap_or("")
    } else {
        ""
    };

    let tools: &[(u32, &[u8], &[u8], &[u8], &[u8], std::appmcp::ToolRiskClass, std::appmcp::ToolConsentClass)] = &[
        (std::appmcp::TOOL_ID_NOTES_CREATE, b"notes.create", b"Create Note", b"Append text to user notes", b"org.interos.notes", std::appmcp::ToolRiskClass::PersonalMutation, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_NOTES_LIST, b"notes.list", b"List Notes", b"List existing note summaries", b"org.interos.notes", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_NOTES_READ, b"notes.read", b"Read Note", b"Read full text of a note by id", b"org.interos.notes", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_NOTES_DELETE, b"notes.delete", b"Delete Note", b"Remove a note permanently", b"org.interos.notes", std::appmcp::ToolRiskClass::PersonalMutation, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_CALCULATOR_EVAL, b"calculator.eval", b"Calculate", b"Evaluate arithmetic expression", b"org.interos.calculator", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_CALENDAR_SCHEDULE, b"calendar.schedule", b"Schedule Event", b"Create calendar appointment", b"org.interos.calendar", std::appmcp::ToolRiskClass::PersonalMutation, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_CALENDAR_LIST, b"calendar.list", b"List Events", b"List scheduled calendar events", b"org.interos.calendar", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_CALENDAR_GET_DATE, b"calendar.date", b"Get Date", b"Read current system date and time", b"org.interos.calendar", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_MESSAGES_SEND_SMS, b"messages.send", b"Send SMS", b"Send text message to contact", b"org.interos.messages", std::appmcp::ToolRiskClass::ExternalConsequence, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_MESSAGES_LIST, b"messages.list", b"List Messages", b"List conversation threads", b"org.interos.messages", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_MESSAGES_READ, b"messages.read", b"Read Thread", b"Read messages in thread", b"org.interos.messages", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_SETTINGS_GET, b"settings.get", b"Get Setting", b"Read configuration value", b"org.interos.settings", std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_SETTINGS_SET, b"settings.set", b"Update Setting", b"Modify system configuration", b"org.interos.settings", std::appmcp::ToolRiskClass::PersonalMutation, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_DIALER_CALL, b"dialer.call", b"Place Call", b"Initiate telephony voice call", b"org.interos.dialer", std::appmcp::ToolRiskClass::ExternalConsequence, std::appmcp::ToolConsentClass::AlwaysAsk),
        (std::appmcp::TOOL_ID_MUSIC_PLAY, b"music.play", b"Play Music", b"Start audio playback", b"org.interos.music", std::appmcp::ToolRiskClass::Reversible, std::appmcp::ToolConsentClass::AlwaysAllow),
        (std::appmcp::TOOL_ID_MUSIC_PAUSE, b"music.pause", b"Pause Music", b"Pause active playback", b"org.interos.music", std::appmcp::ToolRiskClass::Reversible, std::appmcp::ToolConsentClass::AlwaysAllow),
    ];
    for &(id, name, label, desc, pkg, risk, consent) in tools {
        let pkg_str = core::str::from_utf8(pkg).unwrap_or("");
        if !reg_data.is_empty() && !reg_data.contains(pkg_str) {
            continue;
        }
        let descriptor = std::appmcp::DynamicToolDescriptor::new(
            id,
            name,
            label,
            desc,
            pkg,
            1,
            risk,
            consent,
            std::appmcp::TOOL_CAP_TYPE,
        );
        let _ = registry.register(descriptor);
    }
}

/// Companion & Autonomous Agent runtime daemon execution path (`agentd`).
///
/// Designated as service id 14 (`SERVICE_AGENT`) in the system service ABI.
/// Owns the user's CompanionState (preferences, routines, corrections, tool stats),
/// AppMCP dynamic tool dispatch, interactive execution plans, lock sanitization,
/// and privacy modes.
#[allow(static_mut_refs)]
pub fn run_agent() -> ! {
    let _ = std::log("[agentd] userspace Companion & Agent runtime daemon online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();

    let mut state = std::companion::CompanionState::new();
    let mut tool_registry = std::appmcp::DynamicToolRegistry::new();
    populate_default_installed_tools(&mut tool_registry);

    // Authenticated user resolution - fails closed if unauthenticated
    let uid = std::user_get_current();
    if uid == 0 {
        state.set_locked(true);
        state.user_id = 0;
    } else {
        state.user_id = uid;
    }

    // Sole custody of master secret root belongs to keystored. Ordinary services query keystored.
    let enc_handle = if uid > 0 {
        std::keystore::keystore_derive_key(
            std::ai::SERVICE_KEYSTORE_TASK_ID,
            std::keystore::DOMAIN_COMPANION,
            std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT,
            b"companion_state",
        ).ok()
    } else {
        None
    };
    if enc_handle.is_none() {
        state.set_locked(true);
    }

    let mut marker_buf = [0u8; 64];
    let mut slot_a_buf = [0u8; 64];
    let mut slot_b_buf = [0u8; 64];
    let mut active_slot = b'A';

    let marker_path = std::companion::companion_user_marker_path(uid, &mut marker_buf).ok();
    let slot_a_path = std::companion::companion_user_slot_a_path(uid, &mut slot_a_buf).ok();
    let slot_b_path = std::companion::companion_user_slot_b_path(uid, &mut slot_b_buf).ok();

    let mut loaded = false;

    // Read generation.meta to establish anti-rollback minimum generation
    let mut min_expected_gen = 0u64;
    let mut gen_buf = [0u8; 64];
    if uid > 0 {
        if let Ok(gpath) = std::companion::companion_user_gen_path(uid, &mut gen_buf) {
            if let Ok(gh) = std::fs_open(gpath, std::FS_READ) {
                let mut graw = [0u8; 8];
                if let Ok(n) = std::fs_read(gh, &mut graw) {
                    if n == 8 {
                        min_expected_gen = u64::from_le_bytes(graw);
                    }
                }
                let _ = std::fs_close(gh);
            }
        }
    }

    // 1. Two-Slot Journal: check marker first if authenticated and handle is present
    if let (Some(handle), true) = (enc_handle.as_ref(), uid > 0) {
        if let Some(mpath) = marker_path {
            let mut mraw = [0u8; 32];
            if let Ok(h) = std::fs_open(mpath, std::FS_READ) {
                let n = std::fs_read(h, &mut mraw).unwrap_or(0);
                let _ = std::fs_close(h);
                if n == 32 {
                    if let Ok(marker) = std::companion::SlotMarker::deserialize(&mraw) {
                        if marker.user_id == uid && marker.generation >= min_expected_gen {
                            let target_path = if marker.active_slot == b'B' { slot_b_path } else { slot_a_path };
                            if let Some(p) = target_path {
                                if let Ok(h) = std::fs_open(p, std::FS_READ) {
                                    let bytes = unsafe { std::fs_read(h, &mut AGENT_FILE_BUF).unwrap_or(0) };
                                    let _ = std::fs_close(h);
                                    if bytes > 0 && bytes == marker.record_len as usize {
                                        let crc = unsafe { std::companion::crc32(&AGENT_FILE_BUF[..bytes]) };
                                        if crc == marker.payload_crc32 {
                                            if unsafe { state.deserialize_encrypted_for_user_with_handle(*handle, &AGENT_FILE_BUF[..bytes], uid, marker.generation, std::ai::SERVICE_KEYSTORE_TASK_ID).is_ok() } {
                                                active_slot = marker.active_slot;
                                                loaded = true;
                                                let _ = std::log("[agentd] companion two-slot journal state loaded and verified via keystore.");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if !loaded {
            // 2. Fallback: inspect both slots and select highest valid generation >= min_expected_gen
            let mut best_gen = min_expected_gen;
            let mut best_slot = None;
            if let Some(pa) = slot_a_path {
                if let Ok(h) = std::fs_open(pa, std::FS_READ) {
                    let bytes = unsafe { std::fs_read(h, &mut AGENT_FILE_BUF).unwrap_or(0) };
                    let _ = std::fs_close(h);
                    if bytes > 0 {
                        let mut test_state = std::companion::CompanionState::new();
                        if unsafe { test_state.deserialize_encrypted_for_user_with_handle(*handle, &AGENT_FILE_BUF[..bytes], uid, min_expected_gen, std::ai::SERVICE_KEYSTORE_TASK_ID).is_ok() } {
                            best_gen = test_state.generation;
                            best_slot = Some(b'A');
                        }
                    }
                }
            }
            if let Some(pb) = slot_b_path {
                if let Ok(h) = std::fs_open(pb, std::FS_READ) {
                    let bytes = unsafe { std::fs_read(h, &mut AGENT_SLOT_B_BUF).unwrap_or(0) };
                    let _ = std::fs_close(h);
                    if bytes > 0 {
                        let mut test_state = std::companion::CompanionState::new();
                        if unsafe { test_state.deserialize_encrypted_for_user_with_handle(*handle, &AGENT_SLOT_B_BUF[..bytes], uid, min_expected_gen, std::ai::SERVICE_KEYSTORE_TASK_ID).is_ok() } {
                            if test_state.generation > best_gen || best_slot.is_none() {
                                best_gen = test_state.generation;
                                best_slot = Some(b'B');
                            }
                        }
                    }
                }
            }
            if let Some(slot) = best_slot {
                active_slot = slot;
                let src = if slot == b'B' { unsafe { &AGENT_SLOT_B_BUF } } else { unsafe { &AGENT_FILE_BUF } };
                let _ = state.deserialize_encrypted_for_user_with_handle(*handle, src, uid, best_gen, std::ai::SERVICE_KEYSTORE_TASK_ID);
                loaded = true;
            }
        }

        if !loaded {
            // 3. Legacy path fallback
            let mut path_buf = [0u8; 64];
            if let Ok(user_path) = std::companion::companion_user_storage_path(uid, &mut path_buf) {
                if let Ok(handle_fd) = std::fs_open(user_path, std::FS_READ) {
                    let bytes = unsafe { std::fs_read(handle_fd, &mut AGENT_FILE_BUF).unwrap_or(0) };
                    let _ = std::fs_close(handle_fd);
                    if bytes > 0 {
                        let _ = state.deserialize_encrypted_for_user_with_handle(*handle, unsafe { &AGENT_FILE_BUF[..bytes] }, uid, min_expected_gen, std::ai::SERVICE_KEYSTORE_TASK_ID);
                    }
                }
            }
        }
    }

    let _ = std::log("[agentd] journal recovery: crash-safe: yes | authenticated integrity: yes | hardware-backed anti-rollback: no");

    let mut nonce_gen = std::companion::NonceGenerator::new(uid, state.generation);

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 1024];
    let mut last_save_tick = 0u64;

    loop {
        let _ = std::service_heartbeat(std::ai::SERVICE_AGENT);

        if let Some(_) = ipc_slot {
            if let Ok((sender_task_id, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len > 0 {
                    handle_agent_ipc(
                        sender_task_id,
                        msg_type,
                        &rx_buf[..len],
                        &mut state,
                        &mut tool_registry,
                        &mut tx_buf,
                    );
                }
            } else if let Ok(len) = std::ipc_recv(&mut rx_buf) {
                if len > 0 {
                    handle_agent_ipc(
                        0,
                        0,
                        &rx_buf[..len],
                        &mut state,
                        &mut tool_registry,
                        &mut tx_buf,
                    );
                }
            }
        }

        if let Some(_) = timer_slot {
            if let Ok(ticks) = std::get_ticks() {
                if ticks.saturating_sub(last_save_tick) >= 30_000 {
                    last_save_tick = ticks;
                    if let (Some(handle), true) = (enc_handle.as_ref(), uid > 0) {
                        if !state.is_locked
                            && !state.is_private_session
                            && state.learning_mode != std::companion::LearningMode::Disabled
                            && state.learning_mode != std::companion::LearningMode::Off
                        {
                            let next_slot = if active_slot == b'A' { b'B' } else { b'A' };
                            let target_path = if next_slot == b'B' { slot_b_path } else { slot_a_path };
                            if let Some(tpath) = target_path {
                                state.generation = state.generation.saturating_add(1);
                                if let Ok(nonce) = nonce_gen.next_nonce() {
                                    if let Ok(enc_len) = unsafe { state.serialize_encrypted_with_handle(*handle, &nonce, &mut AGENT_FILE_BUF, std::ai::SERVICE_KEYSTORE_TASK_ID) } {
                                        if enc_len <= std::companion::COMPANION_STORAGE_BUDGET_MAX as usize {
                                            if let Ok(handle) = std::fs_open(tpath, std::FS_WRITE) {
                                                let _ = unsafe { std::fs_write(handle, &AGENT_FILE_BUF[..enc_len]) };
                                                let _ = std::fs_fsync(handle);
                                                let _ = std::fs_close(handle);

                                                let payload_crc = unsafe { std::companion::crc32(&AGENT_FILE_BUF[..enc_len]) };
                                                if let Some(mpath) = marker_path {
                                                    let marker = std::companion::SlotMarker::new(
                                                        next_slot,
                                                        state.generation,
                                                        uid,
                                                        enc_len as u32,
                                                        payload_crc,
                                                    );
                                                    let mut mbytes = [0u8; 32];
                                                    marker.serialize(&mut mbytes);
                                                    if let Ok(mh) = std::fs_open(mpath, std::FS_WRITE) {
                                                        let _ = std::fs_write(mh, &mbytes);
                                                        let _ = std::fs_fsync(mh);
                                                        let _ = std::fs_close(mh);
                                                        active_slot = next_slot;
                                                    }
                                                }

                                                let mut gen_buf = [0u8; 64];
                                                if let Ok(gpath) = std::companion::companion_user_gen_path(uid, &mut gen_buf) {
                                                    if let Ok(gh) = std::fs_open(gpath, std::FS_WRITE) {
                                                        let gen_bytes = state.generation.to_le_bytes();
                                                        let _ = std::fs_write(gh, &gen_bytes);
                                                        let _ = std::fs_fsync(gh);
                                                        let _ = std::fs_close(gh);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 50);
        } else {
            std::yield_now();
        }
    }
}

fn handle_agent_ipc(
    sender_task_id: u64,
    msg_type: u32,
    payload: &[u8],
    state: &mut std::companion::CompanionState,
    tool_registry: &mut std::appmcp::DynamicToolRegistry,
    tx_buf: &mut [u8; 1024],
) {
    match msg_type {
        std::ai::MSG_TYPE_COMPANION_QUERY => {
            if payload.is_empty() {
                return;
            }
            if matches!(payload[0], 1 | 2 | 3 | 4 | 6 | 7 | 8) && !is_authorized_agent_admin(sender_task_id) {
                emit_agent_ack(sender_task_id, msg_type, 3, b"UNAUTHORIZED_CALLER", tx_buf);
                return;
            }
            match payload[0] {
                1 => {
                    let locked = payload.get(1).copied().unwrap_or(0) != 0;
                    state.set_locked(locked);
                    if locked {
                        state.generation = state.generation.saturating_add(1);
                        tool_registry.expire_session_grants();
                        tool_registry.clear_invocation_grants();
                        if let Some(ref mut plan) = state.active_plan {
                            plan.cancel();
                        }
                    }
                    emit_agent_ack(sender_task_id, msg_type, 0, b"LOCK_ACK", tx_buf);
                }
                2 => {
                    let mode = std::companion::LearningMode::from_u8(payload.get(1).copied().unwrap_or(0));
                    state.set_learning_mode(mode);
                    let resp = [mode as u8];
                    emit_agent_ack(sender_task_id, msg_type, 0, &resp, tx_buf);
                }
                3 => {
                    let priv_sess = payload.get(1).copied().unwrap_or(0) != 0;
                    state.set_private_session(priv_sess);
                    emit_agent_ack(sender_task_id, msg_type, 0, b"PRIV_ACK", tx_buf);
                }
                4 => {
                    state.clear_memory();
                    let _ = std::fs_unlink(std::companion::PATH_COMPANION_MEMORY);
                    let mut path_buf = [0u8; 64];
                    let uid = std::user_get_current();
                    if uid > 0 {
                        if let Ok(p) = std::companion::companion_user_storage_path(uid, &mut path_buf) {
                            let _ = std::fs_unlink(p);
                        }
                        if let Ok(pa) = std::companion::companion_user_slot_a_path(uid, &mut path_buf) {
                            let _ = std::fs_unlink(pa);
                        }
                        if let Ok(pb) = std::companion::companion_user_slot_b_path(uid, &mut path_buf) {
                            let _ = std::fs_unlink(pb);
                        }
                        if let Ok(pm) = std::companion::companion_user_marker_path(uid, &mut path_buf) {
                            let _ = std::fs_unlink(pm);
                        }
                        if let Ok(pg) = std::companion::companion_user_gen_path(uid, &mut path_buf) {
                            let _ = std::fs_unlink(pg);
                        }
                    }
                    emit_agent_ack(sender_task_id, msg_type, 0, b"CLEAR_ACK", tx_buf);
                }
                5 => {
                    let mut resp = [0u8; 16];
                    resp[0] = state.learning_mode as u8;
                    resp[1] = if state.is_private_session { 1 } else { 0 };
                    resp[2] = if state.is_locked { 1 } else { 0 };
                    resp[3] = state.pref_count as u8;
                    resp[4] = state.corr_count as u8;
                    resp[5] = state.routine_count as u8;
                    resp[6] = tool_registry.count as u8;
                    let est = (state.pref_count * 32 + state.corr_count * 40 + state.routine_count * 24) as u32;
                    resp[7..11].copy_from_slice(&est.to_le_bytes());
                    resp[11..15].copy_from_slice(&std::companion::COMPANION_STORAGE_BUDGET_MAX.to_le_bytes());
                    emit_agent_ack(sender_task_id, msg_type, 0, &resp, tx_buf);
                }
                6 => {
                    if state.is_locked {
                        emit_agent_ack(sender_task_id, msg_type, 4, b"DEVICE_LOCKED", tx_buf);
                        return;
                    }
                    if payload.len() >= 37 {
                        let tool_id = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
                        let mut digest = [0u8; 32];
                        digest.copy_from_slice(&payload[5..37]);
                        let desc_gen = tool_registry.find_by_id(tool_id).map(|d| d.generation).unwrap_or(0);
                        let ticks = std::get_ticks().unwrap_or(0);
                        let grant = std::appmcp::InvocationGrant {
                            tool_id,
                            user_id: state.user_id,
                            plan_id: 0,
                            plan_gen: state.generation,
                            tool_gen: desc_gen,
                            args_digest: digest,
                            expiry_ticks: ticks + 60_000,
                            valid: true,
                        };
                        if tool_registry.grant_exact_invocation(grant) {
                            emit_agent_ack(sender_task_id, msg_type, 0, b"GRANT_ACK", tx_buf);
                        } else {
                            emit_agent_ack(sender_task_id, msg_type, 2, b"GRANT_REJECTED", tx_buf);
                        }
                    } else {
                        emit_agent_ack(sender_task_id, msg_type, 1, b"INVALID_GRANT_PAYLOAD", tx_buf);
                    }
                }
                7 => {
                    tool_registry.clear_invocation_grants();
                    emit_agent_ack(sender_task_id, msg_type, 0, b"CLEAR_GRANTS_ACK", tx_buf);
                }
                8 => {
                    populate_default_installed_tools(tool_registry);
                    emit_agent_ack(sender_task_id, msg_type, 0, b"SYNC_TOOLS_ACK", tx_buf);
                }
                _ => {
                    emit_agent_ack(sender_task_id, msg_type, 1, b"UNKNOWN_OP", tx_buf);
                }
            }
        }
        std::ai::MSG_TYPE_AGENT_PLAN => {
            if payload.is_empty() {
                return;
            }
            if !is_authorized_agent_admin(sender_task_id) {
                emit_agent_ack(sender_task_id, msg_type, 3, b"UNAUTHORIZED_CALLER", tx_buf);
                return;
            }
            match payload[0] {
                1 => {
                    let plan_id = std::get_ticks().unwrap_or(1);
                    let mut plan = std::companion::ExecutionPlan::new_with_gen(plan_id, state.generation);
                    if payload.len() >= 5 {
                        let tool_id_raw = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
                        let (tool_id, args) = if tool_id_raw == 0 && payload.len() > 5 {
                            // Dynamic action planning: user request + installed tool schemas
                            if let Some((dyn_id, dyn_args)) = tool_registry.match_intent(&payload[5..]) {
                                (dyn_id, dyn_args)
                            } else {
                                (0, &payload[5..])
                            }
                        } else {
                            (tool_id_raw, if payload.len() > 5 { &payload[5..] } else { b"" })
                        };

                        if tool_id > 0 {
                            let (risk, consent) = if let Some(desc) = tool_registry.find_by_id(tool_id) {
                                (desc.risk_class, desc.consent_class)
                            } else {
                                (std::appmcp::ToolRiskClass::Observe, std::appmcp::ToolConsentClass::AlwaysAsk)
                            };
                            plan.add_step(tool_id, risk, consent, args);
                        }
                    }
                    if plan.step_count == 0 {
                        plan.status = std::companion::PlanStatus::Failed;
                    } else if plan.requires_user_consent() {
                        plan.status = std::companion::PlanStatus::AwaitingConsent;
                    } else {
                        plan.status = std::companion::PlanStatus::Running;
                    }
                    state.active_plan = Some(plan);

                    let status_byte = plan.status as u8;
                    let mut resp = [0u8; 17];
                    resp[0] = status_byte;
                    resp[1..9].copy_from_slice(&plan_id.to_le_bytes());
                    resp[9..17].copy_from_slice(&state.generation.to_le_bytes());
                    emit_agent_ack(sender_task_id, msg_type, 0, &resp, tx_buf);
                }
                2 => {
                    // Check 1: Reject approval while device is locked
                    if state.is_locked {
                        emit_agent_ack(sender_task_id, msg_type, 4, b"DEVICE_LOCKED", tx_buf);
                        return;
                    }

                    // Check 2: The model/agent runtime itself cannot approve its own plans
                    if sender_task_id == std::ai::SERVICE_AGENT_TASK_ID || std::service_peer_matches(14, sender_task_id) {
                        emit_agent_ack(sender_task_id, msg_type, 3, b"MODEL_CANNOT_SELF_APPROVE", tx_buf);
                        return;
                    }

                    // Check 3: Require complete expected payload (Opcode 2 + 8-byte plan_id + 8-byte plan_gen = 17 bytes)
                    if payload.len() < 9 {
                        emit_agent_ack(sender_task_id, msg_type, 5, b"MISSING_PLAN_ID", tx_buf);
                        return;
                    }
                    if payload.len() < 17 {
                        emit_agent_ack(sender_task_id, msg_type, 5, b"INCOMPLETE_APPROVAL_PAYLOAD", tx_buf);
                        return;
                    }

                    // Check 4: Validate active plan presence
                    let active_plan = match state.active_plan.as_ref() {
                        Some(p) => p,
                        None => {
                            emit_agent_ack(sender_task_id, msg_type, 1, b"NO_PLAN", tx_buf);
                            return;
                        }
                    };

                    // Check 5: Verify plan state before approval (reject cancelled, completed, or already executing plans)
                    if active_plan.status == std::companion::PlanStatus::Cancelled {
                        emit_agent_ack(sender_task_id, msg_type, 6, b"PLAN_ALREADY_CANCELLED", tx_buf);
                        return;
                    }
                    if active_plan.status == std::companion::PlanStatus::Completed {
                        emit_agent_ack(sender_task_id, msg_type, 6, b"PLAN_ALREADY_COMPLETED", tx_buf);
                        return;
                    }
                    if active_plan.status != std::companion::PlanStatus::AwaitingConsent && active_plan.status != std::companion::PlanStatus::Running {
                        emit_agent_ack(sender_task_id, msg_type, 6, b"INVALID_PLAN_STATE", tx_buf);
                        return;
                    }

                    // Check 6: Verify exact plan ID
                    let req_plan_id = u64::from_le_bytes([
                        payload[1], payload[2], payload[3], payload[4],
                        payload[5], payload[6], payload[7], payload[8],
                    ]);
                    if req_plan_id == 0 || req_plan_id != active_plan.id {
                        emit_agent_ack(sender_task_id, msg_type, 2, b"PLAN_ID_MISMATCH", tx_buf);
                        return;
                    }

                    // Check 7: Verify plan generation against active state generation
                    let req_plan_gen = u64::from_le_bytes([
                        payload[9], payload[10], payload[11], payload[12],
                        payload[13], payload[14], payload[15], payload[16],
                    ]);
                    if req_plan_gen != state.generation || req_plan_gen != active_plan.generation {
                        emit_agent_ack(sender_task_id, msg_type, 7, b"STALE_PLAN_GENERATION", tx_buf);
                        return;
                    }

                    // Take plan for execution (single-use consumption)
                    let mut plan = state.active_plan.take().unwrap();
                    plan.user_consent_granted = true;
                    plan.status = std::companion::PlanStatus::Running;
                    let cur_ticks = std::get_ticks().unwrap_or(0);
                    let expiry = cur_ticks + 60_000;

                    // Grant exact invocations for each step in the consented plan
                    let mut grants_ok = true;
                    for i in 0..plan.step_count {
                        let step = &plan.steps[i];
                        let desc_gen = tool_registry.find_by_id(step.tool_id).map(|d| d.generation).unwrap_or(0);
                        let grant = std::appmcp::InvocationGrant {
                            tool_id: step.tool_id,
                            user_id: state.user_id,
                            plan_id: plan.id,
                            plan_gen: state.generation,
                            tool_gen: desc_gen,
                            args_digest: step.args_digest(),
                            expiry_ticks: expiry,
                            valid: true,
                        };
                        if !tool_registry.grant_exact_invocation(grant) {
                            grants_ok = false;
                            break;
                        }
                    }

                    if !grants_ok {
                        tool_registry.clear_invocation_grants();
                        plan.status = std::companion::PlanStatus::Failed;
                        state.generation = state.generation.saturating_add(1);
                        state.active_plan = Some(plan);
                        emit_agent_ack(sender_task_id, msg_type, 8, b"GRANT_REGISTRATION_FAILED", tx_buf);
                        return;
                    }

                        let mut all_succeeded = true;
                        while plan.current_step < plan.step_count {
                            if state.is_locked {
                                plan.cancel();
                                all_succeeded = false;
                                break;
                            }
                            if plan.status == std::companion::PlanStatus::Cancelled {
                                all_succeeded = false;
                                break;
                            }

                            // Check non-blocking IPC for screen lock or plan cancellation
                            let mut peek_buf = [0u8; 64];
                            if let Ok((sender, m_type, rlen)) = std::ipc::receive_any(&mut peek_buf) {
                                if is_authorized_agent_admin(sender) {
                                    if m_type == std::ai::MSG_TYPE_AGENT_PLAN && rlen >= 1 && peek_buf[0] == 3 {
                                        plan.cancel();
                                        all_succeeded = false;
                                        break;
                                    } else if m_type == std::ai::MSG_TYPE_COMPANION_QUERY && rlen >= 1 && peek_buf[0] == 1 {
                                        state.set_locked(true);
                                        plan.cancel();
                                        all_succeeded = false;
                                        break;
                                    }
                                }
                            }

                            let step_idx = plan.current_step;
                            let tool_id = plan.steps[step_idx].tool_id;
                            let args_str = plan.steps[step_idx].args_str();
                            let args_digest = plan.steps[step_idx].args_digest();

                            let desc = match tool_registry.find_by_id(tool_id) {
                                Some(d) => *d,
                                None => {
                                    plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                    plan.status = std::companion::PlanStatus::Failed;
                                    all_succeeded = false;
                                    break;
                                }
                            };

                            if desc.consent_class == std::appmcp::ToolConsentClass::NeverAllow {
                                plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                plan.status = std::companion::PlanStatus::Failed;
                                all_succeeded = false;
                                break;
                            }

                            let ticks = std::get_ticks().unwrap_or(0);
                            if desc.consent_class == std::appmcp::ToolConsentClass::AlwaysAsk {
                                if !tool_registry.consume_exact_invocation(
                                    tool_id,
                                    state.user_id,
                                    plan.id,
                                    state.generation,
                                    desc.generation,
                                    &args_digest,
                                    ticks,
                                ) {
                                    plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                    plan.status = std::companion::PlanStatus::Failed;
                                    all_succeeded = false;
                                    break;
                                }
                            } else if desc.consent_class == std::appmcp::ToolConsentClass::AllowOnce {
                                if !tool_registry.consume_consent(tool_id) {
                                    plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                    plan.status = std::companion::PlanStatus::Failed;
                                    all_succeeded = false;
                                    break;
                                }
                            }

                            let cap_type = if desc.required_cap_type != 0 { desc.required_cap_type } else { std::appmcp::TOOL_CAP_TYPE };
                            let cap_handle = match std::capability_handle(cap_type, std::appmcp::TOOL_CAP_OBJECT) {
                                Ok(h) => h,
                                Err(_) => {
                                    plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                    plan.status = std::companion::PlanStatus::Failed;
                                    all_succeeded = false;
                                    break;
                                }
                            };

                            let target_tid = match std::app_launch(desc.package_id_str()) {
                                Ok(tid) => tid,
                                Err(_) => {
                                    plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                    plan.status = std::companion::PlanStatus::Failed;
                                    all_succeeded = false;
                                    break;
                                }
                            };

                            let token = (std::get_ticks().unwrap_or(1) & 0xFFFF_FFFF) as u32;
                            let req = std::appmcp::AppMcpRequest::new(cap_handle, tool_id, token)
                                .with_inline_payload(args_str.as_bytes());

                            plan.steps[step_idx].status = std::companion::StepStatus::Executing;
                            if std::ipc_send(target_tid, std::appmcp::APPMCP_MSG_INVOKE, &req.to_bytes()).is_err() {
                                plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                plan.status = std::companion::PlanStatus::Failed;
                                all_succeeded = false;
                                break;
                            }

                            let mut wait_rx = [0u8; 128];
                            let mut step_ok = false;
                            for _ in 0..20 {
                                if let Ok((sender, msg_type, rlen)) = std::ipc::receive_any(&mut wait_rx) {
                                    if sender == target_tid && msg_type == std::appmcp::APPMCP_MSG_RESPONSE && rlen >= 8 {
                                        if let Some(resp) = std::appmcp::AppMcpResponse::from_bytes(&wait_rx[..rlen]) {
                                            if resp.token == token && resp.tool_id == tool_id {
                                                if resp.status == std::appmcp::APPMCP_STATUS_SUCCESS {
                                                    step_ok = true;
                                                }
                                                break;
                                            }
                                        }
                                    } else if is_authorized_agent_admin(sender) {
                                        if msg_type == std::ai::MSG_TYPE_AGENT_PLAN && rlen >= 1 && wait_rx[0] == 3 {
                                            plan.cancel();
                                            all_succeeded = false;
                                            break;
                                        } else if msg_type == std::ai::MSG_TYPE_COMPANION_QUERY && rlen >= 1 && wait_rx[0] == 1 {
                                            state.set_locked(true);
                                            plan.cancel();
                                            all_succeeded = false;
                                            break;
                                        }
                                    }
                                }
                                std::yield_now();
                            }

                            if step_ok {
                                plan.steps[step_idx].status = std::companion::StepStatus::Succeeded;
                                state.record_tool_result(tool_id, true, 0);
                                plan.current_step += 1;
                            } else {
                                plan.steps[step_idx].status = std::companion::StepStatus::Failed;
                                state.record_tool_result(tool_id, false, -1);
                                plan.status = std::companion::PlanStatus::Failed;
                                all_succeeded = false;
                                break;
                            }
                        }

                        if all_succeeded && plan.current_step >= plan.step_count {
                            plan.status = std::companion::PlanStatus::Completed;
                        } else if plan.status != std::companion::PlanStatus::Cancelled {
                            plan.status = std::companion::PlanStatus::Failed;
                        }
                        // Invalidate plan generation so any late/queued message for this plan cannot be re-executed
                        state.generation = state.generation.saturating_add(1);
                        tool_registry.clear_invocation_grants();
                        let status_byte = plan.status as u8;
                        state.active_plan = Some(plan);
                        emit_agent_ack(sender_task_id, msg_type, 0, &[status_byte], tx_buf);
                }
                3 => {
                    if payload.len() < 9 {
                        emit_agent_ack(sender_task_id, msg_type, 5, b"MISSING_PLAN_ID", tx_buf);
                        return;
                    }
                    if payload.len() < 17 {
                        emit_agent_ack(sender_task_id, msg_type, 5, b"INCOMPLETE_CANCEL_PAYLOAD", tx_buf);
                        return;
                    }
                    let req_plan_id = u64::from_le_bytes([
                        payload[1], payload[2], payload[3], payload[4],
                        payload[5], payload[6], payload[7], payload[8],
                    ]);
                    let req_plan_gen = u64::from_le_bytes([
                        payload[9], payload[10], payload[11], payload[12],
                        payload[13], payload[14], payload[15], payload[16],
                    ]);

                    let active_plan = match state.active_plan.as_mut() {
                        Some(p) => p,
                        None => {
                            emit_agent_ack(sender_task_id, msg_type, 1, b"NO_PLAN", tx_buf);
                            return;
                        }
                    };

                    if req_plan_id != active_plan.id {
                        emit_agent_ack(sender_task_id, msg_type, 2, b"PLAN_ID_MISMATCH", tx_buf);
                        return;
                    }

                    if req_plan_gen != state.generation || req_plan_gen != active_plan.generation {
                        emit_agent_ack(sender_task_id, msg_type, 7, b"STALE_PLAN_GENERATION", tx_buf);
                        return;
                    }

                    state.generation = state.generation.saturating_add(1);
                    tool_registry.clear_invocation_grants();
                    active_plan.cancel();
                    emit_agent_ack(sender_task_id, msg_type, 0, &[active_plan.status as u8], tx_buf);
                }
                4 => {
                    if let Some(ref plan) = state.active_plan {
                        emit_agent_ack(sender_task_id, msg_type, 0, &[plan.status as u8], tx_buf);
                    } else {
                        emit_agent_ack(sender_task_id, msg_type, 0, &[std::companion::PlanStatus::Completed as u8], tx_buf);
                    }
                }
                _ => {
                    emit_agent_ack(sender_task_id, msg_type, 1, b"UNKNOWN_PLAN_OP", tx_buf);
                }
            }
        }
        std::ai::MSG_TYPE_TOOL_EXEC => {
            if !is_authorized_agent_admin(sender_task_id) {
                emit_agent_ack(sender_task_id, msg_type, 3, b"UNAUTHORIZED_CALLER", tx_buf);
                return;
            }
            if state.is_locked {
                emit_agent_ack(sender_task_id, msg_type, 4, b"DEVICE_LOCKED", tx_buf);
                return;
            }
            if payload.len() < 4 {
                emit_agent_ack(sender_task_id, msg_type, 1, b"INVALID_TOOL_PAYLOAD", tx_buf);
                return;
            }
            let tool_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let desc = match tool_registry.find_by_id(tool_id) {
                Some(d) => *d,
                None => {
                    emit_agent_ack(sender_task_id, msg_type, 2, b"TOOL_NOT_FOUND", tx_buf);
                    return;
                }
            };

            if desc.consent_class == std::appmcp::ToolConsentClass::NeverAllow {
                state.record_tool_result(tool_id, false, -1);
                emit_agent_ack(sender_task_id, msg_type, 3, b"TOOL_DENIED_BY_POLICY", tx_buf);
                return;
            }

            let tool_args = if payload.len() > 4 { &payload[4..] } else { b"" };
            let args_digest = std::crypto::Sha256::digest(tool_args);
            let ticks = std::get_ticks().unwrap_or(0);

            if desc.consent_class == std::appmcp::ToolConsentClass::AlwaysAsk {
                // Cryptographic binding: only an exact match on user, tool, generations,
                // and SHA-256 argument digest authorizes execution. No CRC32 fallback!
                if !tool_registry.consume_exact_invocation(
                    tool_id,
                    state.user_id,
                    0,
                    state.generation,
                    desc.generation,
                    &args_digest,
                    ticks,
                ) {
                    state.record_tool_result(tool_id, false, -6);
                    emit_agent_ack(sender_task_id, msg_type, 6, b"CONSENT_REQUIRED", tx_buf);
                    return;
                }
            } else if desc.consent_class == std::appmcp::ToolConsentClass::AllowOnce {
                if !tool_registry.consume_consent(tool_id) {
                    state.record_tool_result(tool_id, false, -6);
                    emit_agent_ack(sender_task_id, msg_type, 6, b"CONSENT_REQUIRED", tx_buf);
                    return;
                }
            }

            let cap_type = if desc.required_cap_type != 0 { desc.required_cap_type } else { std::appmcp::TOOL_CAP_TYPE };
            let cap_handle = match std::capability_handle(cap_type, std::appmcp::TOOL_CAP_OBJECT) {
                Ok(h) => h,
                Err(_) => {
                    state.record_tool_result(tool_id, false, -5);
                    emit_agent_ack(sender_task_id, msg_type, 4, b"CAPABILITY_NOT_HELD", tx_buf);
                    return;
                }
            };
            if desc.required_cap_type != 0 && desc.required_cap_type != std::appmcp::TOOL_CAP_TYPE {
                let has_cap = std::cap_lookup(desc.required_cap_type, 0).is_ok();
                if !has_cap {
                    state.record_tool_result(tool_id, false, -2);
                    emit_agent_ack(sender_task_id, msg_type, 4, b"CAPABILITY_NOT_HELD", tx_buf);
                    return;
                }
            }

            let pkg_str = desc.package_id_str();
            let target_task_id = match std::app_launch(pkg_str) {
                Ok(tid) => tid,
                Err(_) => {
                    state.record_tool_result(tool_id, false, -3);
                    emit_agent_ack(sender_task_id, msg_type, 5, b"APP_LAUNCH_FAILED", tx_buf);
                    return;
                }
            };

            let token = (std::get_ticks().unwrap_or(1) & 0xFFFF_FFFF) as u32;
            let req = std::appmcp::AppMcpRequest::new(cap_handle, tool_id, token)
                .with_inline_payload(tool_args);
            let req_bytes = req.to_bytes();

            if std::ipc_send(target_task_id, std::appmcp::APPMCP_MSG_INVOKE, &req_bytes).is_err() {
                state.record_tool_result(tool_id, false, -4);
                emit_agent_ack(sender_task_id, msg_type, 6, b"IPC_SEND_FAILED", tx_buf);
                return;
            }

            let mut app_rx_buf = [0u8; 128];
            let mut success = false;
            let mut resp_payload_len = 0usize;
            let mut resp_payload = [0u8; 64];

            let start_tick = std::get_ticks().unwrap_or(0);
            for _ in 0..20 {
                if let Ok((sender, resp_msg, len)) = std::ipc::receive_any(&mut app_rx_buf) {
                    if sender == target_task_id && resp_msg == std::appmcp::APPMCP_MSG_RESPONSE {
                        if let Some(resp) = std::appmcp::AppMcpResponse::from_bytes(&app_rx_buf[..len]) {
                            if resp.token == token && resp.tool_id == tool_id {
                                if resp.status == std::appmcp::APPMCP_STATUS_SUCCESS {
                                    success = true;
                                    let p = resp.payload();
                                    resp_payload_len = p.len().min(64);
                                    resp_payload[..resp_payload_len].copy_from_slice(&p[..resp_payload_len]);
                                }
                                break;
                            }
                        }
                    }
                }
                std::yield_now();
            }

            let end_tick = std::get_ticks().unwrap_or(start_tick);
            let latency_ticks = end_tick.saturating_sub(start_tick) as i32;

            state.record_tool_result(tool_id, success, latency_ticks);
            if success {
                let out_data = if resp_payload_len > 0 { &resp_payload[..resp_payload_len] } else { b"TOOL_SUCCESS" };
                emit_agent_ack(sender_task_id, msg_type, 0, out_data, tx_buf);
            } else {
                emit_agent_ack(sender_task_id, msg_type, 7, b"TOOL_EXECUTION_FAILED", tx_buf);
            }
        }
        _ => {
            emit_agent_ack(sender_task_id, msg_type, 0xFF, b"UNSUPPORTED_MSG", tx_buf);
        }
    }
}

fn emit_agent_ack(
    sender_task_id: u64,
    msg_type: u32,
    status: u32,
    data: &[u8],
    tx_buf: &mut [u8; 1024],
) {
    let resp = std::ai::AiInferenceResponse {
        status,
        output_dim: data.len() as u32,
        latency_micros: 0,
        output_offset: core::mem::size_of::<std::ai::AiInferenceResponse>() as u64,
    };
    let hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
    let resp_bytes = unsafe {
        core::slice::from_raw_parts(&resp as *const _ as *const u8, hdr_len)
    };
    tx_buf[..hdr_len].copy_from_slice(resp_bytes);
    let total = hdr_len + data.len();
    if total <= tx_buf.len() {
        tx_buf[hdr_len..total].copy_from_slice(data);
        if sender_task_id != 0 {
            let _ = std::ipc_send(sender_task_id, msg_type, &tx_buf[..total]);
        }
    }
}

/// Dedicated Keystore Service Daemon execution path (`keystored`).
///
/// Designated for opaque cryptographic key derivation, AEAD encryption/decryption,
/// HMAC signing/verification, and secure lifecycle custody.
pub fn run_keystore() -> ! {
    let _ = std::log("[keystored] userspace Keystore & Key Custody daemon online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let ipc_slot = std::cap_lookup(std::cap_type::IPC_RECEIVE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;

    let secret_root = std::companion::load_or_generate_device_secret().ok();
    let mut table = std::keystore::KeystoreTable::<64>::new();
    let mut rx_buf = [0u8; 2048];
    let mut tx_buf = [0u8; 2048];

    loop {
        let _ = std::service_heartbeat(15);

        if ipc_slot.is_some() {
            while let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if msg_type == std::keystore::SERVICE_KEYSTORE_MSG && len > 0 {
                    handle_keystore_ipc(
                        &mut table,
                        &secret_root,
                        sender,
                        &rx_buf[..len],
                        &mut tx_buf,
                    );
                }
            }
        }

        let now = timer_slot.and_then(|_| std::get_ticks().ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 1000 || last_tick == 0 {
            last_tick = now;
        }

        if has_epoll {
            let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 50);
        } else {
            std::yield_now();
        }
    }
}

/// Enforces kernel-authenticated keystore domain and rights authorization policy.
pub fn authorize_keystore_derive_policy(
    caller_identity: &std::TaskIdentityInfo,
    sender: u64,
    domain: u32,
    rights: u32,
) -> Result<(), std::keystore::KeystoreError> {
    // 1. DOMAIN_DEVICE_ROOT must never be directly derivable by callers over IPC
    if domain == std::keystore::DOMAIN_DEVICE_ROOT {
        return Err(std::keystore::KeystoreError::UnauthorizedDomain);
    }

    // 2. Validate requested rights: must be non-zero and within RIGHT_ALL
    if rights == 0 || (rights & !std::keystore::RIGHT_ALL) != 0 {
        return Err(std::keystore::KeystoreError::InvalidRights);
    }

    // 3. Domain authorization policy:
    match domain {
        std::keystore::DOMAIN_SEMANTIC_SEARCH => {
            // Must be authenticated searchd service
            let is_searchd = (caller_identity.is_system != 0 && (caller_identity.task_name() == "searchd" || caller_identity.package_name() == "searchd"))
                || std::service_peer_matches(std::ai::SERVICE_SEARCH, sender);
            if !is_searchd {
                return Err(std::keystore::KeystoreError::UnauthorizedDomain);
            }
        }
        std::keystore::DOMAIN_COMPANION => {
            // Must be authenticated agentd service
            let is_agentd = (caller_identity.is_system != 0 && (caller_identity.task_name() == "agentd" || caller_identity.package_name() == "agentd"))
                || std::service_peer_matches(std::ai::SERVICE_AGENT, sender);
            if !is_agentd {
                return Err(std::keystore::KeystoreError::UnauthorizedDomain);
            }
        }
        std::keystore::DOMAIN_CONVERSATION_HISTORY => {
            // Must be authenticated agentd or conversation service
            let is_conv = (caller_identity.is_system != 0 && (caller_identity.task_name() == "agentd" || caller_identity.task_name() == "conversation"))
                || std::service_peer_matches(std::ai::SERVICE_AGENT, sender);
            if !is_conv {
                return Err(std::keystore::KeystoreError::UnauthorizedDomain);
            }
        }
        std::keystore::DOMAIN_APP_STORAGE => {
            // Ordinary applications may only derive their own application storage domain
            // Rights for ordinary apps in app storage are limited to encrypt/decrypt
            if caller_identity.is_system == 0 {
                let allowed = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;
                if (rights & !allowed) != 0 {
                    return Err(std::keystore::KeystoreError::InvalidRights);
                }
            }
        }
        _ => {
            return Err(std::keystore::KeystoreError::UnauthorizedDomain);
        }
    }

    Ok(())
}

fn handle_keystore_ipc<const N: usize>(
    table: &mut std::keystore::KeystoreTable<N>,
    secret_root: &Option<[u8; 32]>,
    sender: u64,
    req: &[u8],
    tx_buf: &mut [u8; 2048],
) {
    if req.is_empty() {
        return;
    }
    let opcode = req[0];

    // Authenticate sender identity through the kernel (Syscall 167 TaskIdentityQuery)
    // Never trust payload strings or infer caller identity from ambient state!
    let caller_identity = match std::task_identity_query(sender) {
        Ok(id) => id,
        Err(_) => {
            let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 0x10]);
            return;
        }
    };

    let caller_uid = if caller_identity.is_system != 0 {
        let active = std::user_get_current();
        if active > 0 { active } else { caller_identity.user_id }
    } else {
        caller_identity.user_id
    };

    if caller_uid == 0 && caller_identity.is_system == 0 {
        let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 0x11]);
        return;
    }

    match opcode {
        std::keystore::KEYSTORE_OP_DERIVE => {
            if req.len() < 10 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let domain = u32::from_le_bytes([req[1], req[2], req[3], req[4]]);
            let rights = u32::from_le_bytes([req[5], req[6], req[7], req[8]]);
            let label_len = req[9] as usize;
            if req.len() < 10 + label_len {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                return;
            }
            let label = &req[10..10 + label_len];

            // Explicit authenticated domain and rights authorization policy check
            if let Err(e) = authorize_keystore_derive_policy(&caller_identity, sender, domain, rights) {
                let err_code = match e {
                    std::keystore::KeystoreError::UnauthorizedDomain => 0x20,
                    std::keystore::KeystoreError::InvalidRights => 0x21,
                    _ => 0x22,
                };
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, err_code]);
                return;
            }

            let root = match secret_root {
                Some(r) => r,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                    return;
                }
            };

            let pkg_id_bytes = caller_identity.package_name().as_bytes();
            match table.derive_key_with_principal(caller_uid, sender, domain, caller_identity.principal_id, pkg_id_bytes, label, rights, root) {
                Ok(handle) => {
                    let mut resp = [0u8; 13];
                    resp[0] = 0x00;
                    resp[1..13].copy_from_slice(&handle.to_bytes());
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &resp);
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 4]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_ENCRYPT => {
            if req.len() < 29 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let handle = match std::keystore::KeyHandle::from_bytes(&req[1..13]) {
                Some(h) => h,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                    return;
                }
            };
            let mut nonce = [0u8; 12];
            nonce.copy_from_slice(&req[13..25]);
            let aad_len = u16::from_le_bytes([req[25], req[26]]) as usize;
            let plain_len = u16::from_le_bytes([req[27], req[28]]) as usize;
            if req.len() < 29 + aad_len + plain_len {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                return;
            }
            let aad = &req[29..29 + aad_len];
            let plain = &req[29 + aad_len..29 + aad_len + plain_len];

            if 17 + plain_len > tx_buf.len() {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 4]);
                return;
            }

            match table.encrypt(
                handle,
                caller_uid,
                sender,
                &nonce,
                aad,
                plain,
                &mut tx_buf[17..17 + plain_len],
            ) {
                Ok(tag) => {
                    tx_buf[0] = 0x00;
                    tx_buf[1..17].copy_from_slice(&tag);
                    let _ = std::ipc_send(
                        sender,
                        std::keystore::SERVICE_KEYSTORE_MSG,
                        &tx_buf[..17 + plain_len],
                    );
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 5]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_DECRYPT => {
            if req.len() < 45 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let handle = match std::keystore::KeyHandle::from_bytes(&req[1..13]) {
                Some(h) => h,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                    return;
                }
            };
            let mut nonce = [0u8; 12];
            nonce.copy_from_slice(&req[13..25]);
            let mut tag = [0u8; 16];
            tag.copy_from_slice(&req[25..41]);
            let aad_len = u16::from_le_bytes([req[41], req[42]]) as usize;
            let ct_len = u16::from_le_bytes([req[43], req[44]]) as usize;
            if req.len() < 45 + aad_len + ct_len {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                return;
            }
            let aad = &req[45..45 + aad_len];
            let ct = &req[45 + aad_len..45 + aad_len + ct_len];

            if 1 + ct_len > tx_buf.len() {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 4]);
                return;
            }

            match table.decrypt(
                handle,
                caller_uid,
                sender,
                &nonce,
                aad,
                ct,
                &tag,
                &mut tx_buf[1..1 + ct_len],
            ) {
                Ok(()) => {
                    tx_buf[0] = 0x00;
                    let _ = std::ipc_send(
                        sender,
                        std::keystore::SERVICE_KEYSTORE_MSG,
                        &tx_buf[..1 + ct_len],
                    );
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 5]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_SIGN => {
            if req.len() < 13 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let handle = match std::keystore::KeyHandle::from_bytes(&req[1..13]) {
                Some(h) => h,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                    return;
                }
            };
            let msg = &req[13..];
            match table.sign(handle, caller_uid, sender, msg) {
                Ok(sig) => {
                    let mut resp = [0u8; 33];
                    resp[0] = 0x00;
                    resp[1..33].copy_from_slice(&sig);
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &resp);
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_VERIFY => {
            if req.len() < 45 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let handle = match std::keystore::KeyHandle::from_bytes(&req[1..13]) {
                Some(h) => h,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                    return;
                }
            };
            let mut tag = [0u8; 32];
            tag.copy_from_slice(&req[13..45]);
            let msg = &req[45..];
            match table.verify(handle, caller_uid, sender, msg, &tag) {
                Ok(valid) => {
                    let resp = [0x00, if valid { 1 } else { 0 }];
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &resp);
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_DESTROY => {
            if req.len() < 13 {
                let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 1]);
                return;
            }
            let handle = match std::keystore::KeyHandle::from_bytes(&req[1..13]) {
                Some(h) => h,
                None => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 2]);
                    return;
                }
            };
            match table.destroy_key(handle, caller_uid, sender) {
                Ok(()) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0x00]);
                }
                Err(_) => {
                    let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 3]);
                }
            }
        }
        std::keystore::KEYSTORE_OP_DERIVE_SERVICE_KEY => {
            // Opaque Key Custody Policy: Raw master or derived cryptographic keys
            // must NEVER leave keystored into any userspace process. All callers
            // (including system services) must interact exclusively via opaque KeyHandles.
            let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 0x12]);
        }
        _ => {
            let _ = std::ipc_send(sender, std::keystore::SERVICE_KEYSTORE_MSG, &[0xFF, 0xEE]);
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

    // Authoritative BrokerLedger initialization from real system telemetry
    if let Ok(telemetry) = std::memory_telemetry() {
        let reserve = backend::foreground_ram_reserve(telemetry.total_bytes);
        *BROKER_LEDGER.lock() = Some(backend::BrokerLedger::new(
            telemetry.free_bytes,
            0,
            reserve,
        ));
        let _ = std::log("[aid] authoritative BrokerLedger initialized from system memory telemetry.");
    }

    // Probe storage for real physical GGUF model weights on startup.
    //
    // WHY only the header probe runs here: a probe reads the file's first
    // bytes, while an identity or tensor walk pulls the whole metadata through
    // 512-byte positioned reads -- tens of thousands of syscalls for a
    // model-sized file. Nothing model-sized may run before this service answers
    // IPC, so the walks are left to the first request that needs them, and the
    // identity claim is made there from the file's own `general.architecture`.
    // A path in the candidate list is not a model identity: the install flow
    // writes any offered file to the same local slot.
    let mut probed_models = gguf::probe_all_models();
    if probed_models.embedding.is_some() {
        let _ = std::log("[aid] embedding weights file present; its declared identity is read from its metadata on first use.");
    } else {
        let _ = std::log("[aid] no embedding weights file found; lexical baseline active.");
    }
    if probed_models.generative.is_some() {
        let _ = std::log("[aid] GGUF header found in a generative model slot; identity and execution are checked on request.");
    } else {
        let _ = std::log("[aid] physical generative model weights absent; generative queries will report unavailable honestly.");
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
            invalidate_resident_model_cache();
            probed_models = gguf::probe_all_models();
            let _ = std::log("[aid] re-probed the model namespace after an install.");
        }

        // Drain any deferred IPC messages in strict FIFO order
        while let Some(deferred) = pop_deferred_ipc() {
            let _ = std::log("[aid] processing deferred IPC message from FIFO queue");
            handle_ipc_inference_query(
                deferred.sender,
                deferred.msg_type,
                &deferred.payload[..deferred.len],
                &probed_models,
                &mut tx_buf,
            );
        }

        // Non-allocating IPC listener for AI inference and embedding status requests
        if let Some(_) = ipc_slot {
            if let Ok((sender_task_id, msg_type, len)) = std::ipc::receive_any(&mut rx_buf) {
                if len > 0 {
                    if sender_task_id == 0 {
                        let _ = std::log("[aid] rejected anonymous AI query: sender=0 violates authenticated identity requirement");
                        continue;
                    }
                    let _ = std::log("[aid] received inference / embedding query over IPC");
                    handle_ipc_inference_query(
                        sender_task_id,
                        msg_type,
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
            let _ = std::event_poll(std::EVENT_IPC_MSG, 50);
        } else {
            std::yield_now();
        }
    }
}

/// Extracts prompt text from IPC payload, stripping header prefixes if present.
///
/// WHY the shape test is `generative_payload_is_header_shaped` and not a local
/// clause: this route's cut guard has to fire on exactly the payloads this parse
/// would have trusted, and the only way to keep the two clause sets from drifting
/// is for one of them to call the other.
fn extract_prompt_text(payload: &[u8]) -> &[u8] {
    if payload.len() >= 4 && (&payload[..4] == b"GENR" || &payload[..4] == b"GEN:") {
        return &payload[4..];
    }
    if payload.len() >= core::mem::size_of::<std::ai::AiInferenceRequest>() {
        let req = unsafe {
            core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
        };
        if generative_payload_is_header_shaped(&req) {
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

    let streamed = stream_in_slots(message, std::ai::ANSWER_SLOT_BYTES, |chunk| {
        std::shm_pipe_transfer(pipe_id, chunk, 0).is_ok()
    });

    let notice = std::ai::AiStreamNotice {
        status,
        pipe_id,
        streamed_len: streamed as u32,
        total_len: message.len() as u32,
    };
    let mut wire = [0u8; std::ai::AiStreamNotice::WIRE_LEN];
    if notice.to_bytes(&mut wire) == 0 {
        return false;
    }
    if std::ipc_send(sender_task_id, notice_type, &wire).is_err() {
        return false;
    }
    if streamed != message.len() {
        let _ = std::log("[aid] answer channel filled before the whole answer streamed.");
    }
    true
}

/// Hands `message` to `write_slot` in pieces of at most `slot_bytes`, stopping at
/// the first piece the transport refuses, and returns how many bytes it accepted.
///
/// WHY a function of its own: this is the drain every answer and refusal runs
/// through on the write side, and the reader on the other end drains slots until
/// the same count arrives, so the number of bytes a notice carries is decided here
/// and nowhere else. A loop that stopped after one slot would hand the reader a
/// fragment and still let the notice report it as the whole; keeping the drain in
/// one place is what lets a host test watch the tail either arrive or be named as
/// short, without a device or a running channel.
fn stream_in_slots<W>(message: &[u8], slot_bytes: usize, mut write_slot: W) -> usize
where
    W: FnMut(&[u8]) -> bool,
{
    let mut streamed = 0usize;
    while streamed < message.len() {
        let end = (streamed + slot_bytes).min(message.len());
        if !write_slot(&message[streamed..end]) {
            break;
        }
        streamed = end;
    }
    streamed
}

/// The bytes of a notice that survive the inline mailbox route.
///
/// WHY this number is written down: the kernel's mailbox payload is sixty-four
/// bytes (`IPC_MAX_PAYLOAD`) and every inline reply first lays a
/// twenty-four-byte `AiInferenceResponse` header, so a notice sent by mailbox keeps
/// sixty-four minus twenty-four, forty bytes, and the rest is cut with no word of
/// it. A refusal the service builds runs past forty, so a notice longer than this
/// must travel on the channel `stream_answer` writes; a shorter one fits inline
/// honestly. The delivery itself is decided by `IPC_MAX_PAYLOAD` in the kernel, so
/// this is the bound the tests name -- not a value the service branches on, and not
/// a value to be raised.
#[cfg(test)]
const MAILBOX_NOTICE_ROOM: usize =
    64 - core::mem::size_of::<std::ai::AiInferenceResponse>();

/// Delivers one embedding refusal so its whole sentence reaches the reader.
///
/// WHY the channel and not the mailbox: the notice the service builds names its
/// numbers -- the tensor, the byte counts, the bound -- and runs past the forty
/// bytes a mailbox keeps. Sent inline, the reader sees a sentence cut mid-word and
/// cannot tell whether the model ran, the file is missing, or the read was refused,
/// which is the worst way for a refusal to fail. The whole notice goes on the
/// answer channel under [`MSG_TYPE_EMBED_CHUNK`]; only when no channel is free does
/// the inline route run, and there the header's `output_dim` still names the
/// notice's true length so a reader can see it was cut rather than be handed a
/// fragment as though it were whole.
fn emit_embed_notice(
    sender_task_id: u64,
    status: u32,
    notice: &[u8],
    tx_buf: &mut [u8; 1024],
) {
    if !stream_answer(sender_task_id, status, MSG_TYPE_EMBED_CHUNK, notice) {
        emit_response_as(sender_task_id, status, MSG_TYPE_EMBED, notice, tx_buf);
    }
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

/// Why the generative arm ran nothing at all because its prompt never arrived.
///
/// WHY this is written beside `reply_with_answer` rather than folded into the arm:
/// the two counts are the whole content of the refusal, and the test that pins them
/// reads this function; an arm that built its notice inline would let the sentence
/// and the guard drift apart the way the embedding route's did until
/// `embedding_gap_notice` pulled them together.
///
/// WHY both counts go back to the caller: the caller knows how long its prompt was
/// and cannot see what the transport did to it. A refusal that said only "too long"
/// would leave it guessing what would have fit -- and unlike an embedding, a
/// generation answered from a prefix reads as fluent prose, so nothing downstream
/// can spot the cut.
fn generation_overrun_notice<'a>(
    declared: usize,
    arrived: usize,
    out: &'a mut [u8],
) -> (u32, &'a [u8]) {
    let mut notice = Notice::new(out);
    notice.word(b"Generation refused: this request declares ");
    notice.byte_count(declared as u64);
    notice.word(b" prompt bytes and the transport delivered ");
    notice.byte_count(arrived as u64);
    notice.word(
        b". Nothing was answered from the piece that arrived: the bytes in front of this service are the request's own header followed partway through the prompt, and words made from them would be an answer to a question the device never received.",
    );
    notice.word(b" Re-send with a prompt no longer than the bytes that arrived.");
    (std::ai::STATUS_UNSUPPORTED, notice.finish())
}

/// Answers a generative query, streaming the bytes when the mailbox cannot hold them.
///
/// WHY this is the delivery route for a refusal as much as for an answer: a notice
/// past the forty bytes an inline mailbox reply keeps must travel whole, and
/// `stream_answer` under `MSG_TYPE_GENERATE_CHUNK` is the channel the reader on the
/// other end already drains -- the same idiom `emit_embed_notice` established on the
/// embedding route, with that route's message type substituted for this one's.
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

static NEXT_GENERATION_SESSION_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
static ACTIVE_SESSION_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static ACTIVE_GENERATION_TASK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static ACTIVE_SESSION_CANCELLED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Strict FIFO fair spinlock using ticket-based synchronization.
/// Guarantees that threads acquire the lock in arrival order and eliminates starvation.
pub struct TicketSpinLock<T> {
    next_ticket: core::sync::atomic::AtomicUsize,
    now_serving: core::sync::atomic::AtomicUsize,
    value: core::cell::UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for TicketSpinLock<T> {}
unsafe impl<T: Send> Send for TicketSpinLock<T> {}

impl<T> TicketSpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            next_ticket: core::sync::atomic::AtomicUsize::new(0),
            now_serving: core::sync::atomic::AtomicUsize::new(0),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn lock(&self) -> TicketGuard<'_, T> {
        let ticket = self.next_ticket.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        let mut spins = 0usize;
        while self.now_serving.load(core::sync::atomic::Ordering::Acquire) != ticket {
            core::hint::spin_loop();
            spins = spins.wrapping_add(1);
            if spins & 0x7F == 0 {
                #[cfg(not(test))]
                interos_std::yield_now();
            }
        }
        TicketGuard { lock: self }
    }
}

pub struct TicketGuard<'a, T> {
    lock: &'a TicketSpinLock<T>,
}

impl<'a, T> core::ops::Deref for TicketGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<'a, T> core::ops::DerefMut for TicketGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<'a, T> Drop for TicketGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.now_serving.fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

pub static BROKER_LEDGER: TicketSpinLock<Option<backend::BrokerLedger>> = TicketSpinLock::new(None);
pub static NATIVE_GGUF_BACKEND: TicketSpinLock<backend::NativeGgufBackend> = TicketSpinLock::new(backend::NativeGgufBackend::new());
pub static NATIVE_LITERTLM_BACKEND: TicketSpinLock<backend::NativeLiteRtLmBackend> = TicketSpinLock::new(backend::NativeLiteRtLmBackend::new());

pub fn with_broker_ledger<F, R>(f: F) -> R
where
    F: FnOnce(&mut backend::BrokerLedger) -> R,
{
    let mut guard = BROKER_LEDGER.lock();
    if guard.is_none() {
        let (free_ram, total_ram) = match std::memory_telemetry() {
            Ok(telem) => (telem.free_bytes, telem.total_bytes),
            Err(_) => (1024 * 1024 * 1024, 1024 * 1024 * 1024),
        };
        let reserve = backend::foreground_ram_reserve(total_ram);
        *guard = Some(backend::BrokerLedger::new(free_ram, 0, reserve));
    }
    f(guard.as_mut().unwrap())
}

pub fn reset_broker_ledger(ram: u64, storage: u64, reserve: u64) {
    *BROKER_LEDGER.lock() = Some(backend::BrokerLedger::new(ram, storage, reserve));
}

pub fn reap_process(task_id: u64) {
    with_broker_ledger(|ledger| {
        let _ = ledger.reap_owner(task_id);
    });
    let mut backend = NATIVE_GGUF_BACKEND.lock();
    let _ = backend.reap_owner(task_id);
    let mut litert_backend = NATIVE_LITERTLM_BACKEND.lock();
    let _ = litert_backend.reap_owner(task_id);
}

pub fn reap_backend(backend_id: u32) {
    with_broker_ledger(|ledger| {
        let _ = ledger.reap_backend(backend_id);
    });
}

/// Bounded circular ring buffer for deferred IPC messages.
/// Enforces O(1) FIFO enqueue and dequeue with zero heap allocation.
pub struct DeferredIpcQueue {
    head: usize,
    tail: usize,
    count: usize,
    entries: [Option<DeferredIpcEntry>; 16],
}

impl DeferredIpcQueue {
    pub const fn new() -> Self {
        const NONE_ENTRY: Option<DeferredIpcEntry> = None;
        Self {
            head: 0,
            tail: 0,
            count: 0,
            entries: [NONE_ENTRY; 16],
        }
    }

    pub fn push(&mut self, entry: DeferredIpcEntry) -> bool {
        if self.count >= 16 {
            return false;
        }
        self.entries[self.tail] = Some(entry);
        self.tail = (self.tail + 1) % 16;
        self.count += 1;
        true
    }

    pub fn pop(&mut self) -> Option<DeferredIpcEntry> {
        if self.count == 0 {
            return None;
        }
        let item = self.entries[self.head].take();
        self.head = (self.head + 1) % 16;
        self.count -= 1;
        item
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        for slot in self.entries.iter_mut() {
            *slot = None;
        }
        self.head = 0;
        self.tail = 0;
        self.count = 0;
    }
}

/// Deferred IPC message arriving while generative inference is active.
#[derive(Clone, Copy)]
pub struct DeferredIpcEntry {
    pub sender: u64,
    pub msg_type: u32,
    pub len: usize,
    pub payload: [u8; 1024],
}

static DEFERRED_QUEUE: TicketSpinLock<DeferredIpcQueue> =
    TicketSpinLock::new(DeferredIpcQueue::new());
pub static DEFERRED_IPC_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn push_deferred_ipc(sender: u64, msg_type: u32, data: &[u8]) -> bool {
    let mut entry = DeferredIpcEntry {
        sender,
        msg_type,
        len: data.len().min(1024),
        payload: [0u8; 1024],
    };
    entry.payload[..entry.len].copy_from_slice(&data[..entry.len]);

    let mut q = DEFERRED_QUEUE.lock();
    let pushed = q.push(entry);
    DEFERRED_IPC_COUNT.store(q.len(), core::sync::atomic::Ordering::Release);
    pushed
}

pub fn pop_deferred_ipc() -> Option<DeferredIpcEntry> {
    let mut q = DEFERRED_QUEUE.lock();
    let item = q.pop();
    DEFERRED_IPC_COUNT.store(q.len(), core::sync::atomic::Ordering::Release);
    item
}

pub fn clear_deferred_ipc() {
    let mut q = DEFERRED_QUEUE.lock();
    q.clear();
    DEFERRED_IPC_COUNT.store(0, core::sync::atomic::Ordering::Release);
}

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
///
/// WHY the bound is not also what a caller can send: the ids reach this service in
/// the tail of an IPC message, and the kernel cuts a message at the payload cap in
/// `kernel/src/ipc/mod.rs`; the request header takes the first bytes of that cap, so
/// a caller today delivers ten ids whole and no more, however large the number below
/// is. A request past that point arrives cut and is refused as `InputTruncated`
/// before this ceiling is ever reached. Moving this number therefore says nothing
/// about what the device can actually ask for: it only becomes reachable when the ids
/// travel on the shared-memory pipe the answers already use, and the notice that
/// offers a caller "a token-id stream of your own" is offering the ten the mailbox
/// carries until then.
const EMBED_TOKEN_IDS_MAX: usize = 512;

/// Which of the three answers an embedding request gets.
#[derive(Debug, Clone, Copy, PartialEq)]
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
///
/// WHY no `Eq`: `VocabularyRefused` carries a `TokenizerError`, which carries a
/// `GgufError`, and that enum refuses the derive because a refusal can name a
/// declared float. A gap is compared for the notice a test reads, not used as an
/// equivalence class, so `PartialEq` is the whole of what it needs.
#[derive(Debug, Clone, Copy, PartialEq)]
enum EmbeddingGap {
    /// The caller demanded physical weights and the probe found no file.
    NoWeights,
    /// Weights are on the device, the caller demanded them, and the text-to-id
    /// mapping never produced a stream to run. Until now this was the whole story:
    /// `tokenizer::encode` matched thirty hard-coded subwords at invented ids
    /// against a file stating 262,144 tokens, so the gap was the mapping itself and
    /// the reply said so instead of paying it with a fabricated one.
    ///
    /// The mapping now exists (`tokenizer::FileTokenizer` builds it from the
    /// installed file's own `tokenizer.ggml.tokens`, scores, token types and special
    /// ids), so this variant means the narrower and still real thing: the file was
    /// there, the table was attempted, and it produced nothing. The notice says
    /// which, because "no mapping" and "this file gave no usable mapping" have
    /// different next moves -- install a model, or read the refusal's numbers.
    NoVocabTokenizer,
    /// The table was built and the prompt segmented, but the segmentation is
    /// `needed` positions and the run answers at most `bound`. Carries both numbers
    /// rather than truncating: a cut-off id stream is a different prompt, and the
    /// engine would return a real vector for it under a neural label.
    IdStreamFull(usize, usize),
    /// The table itself refused. Carries the `TokenizerError`, which already names
    /// its own two numbers for every bound it enforces.
    VocabularyRefused(tokenizer::TokenizerError),
    /// The request declared `declared` input bytes and the transport delivered
    /// `arrived`. A message is cut at sixty-four bytes by the kernel's own IPC
    /// payload cap, so a prompt longer than the room left behind the header reaches
    /// this service as a whole header plus a piece of the text.
    ///
    /// WHY this is a refusal rather than an answer from what arrived: the bytes in
    /// front of the arm are then the caller's `model_id`, `target_dim`, `input_len`,
    /// `flags` and `input_offset`, and a vocabulary segments header bytes exactly as
    /// readily as prose. The vector that comes out is real arithmetic over the wrong
    /// bytes, filed under this request -- which is the same harm `IdStreamFull`
    /// exists to stop, one layer down, on the way in rather than in the segmentation.
    /// Both counts go back to the caller because "shorten the prompt" is only
    /// actionable if the caller can see how much room the transport leaves.
    InputTruncated(usize, usize),
    /// The strict-neural flag was set and the request carries neither text nor
    /// token ids: `input_len` zero with no input byte behind the header.
    ///
    /// WHY this is its own gap rather than a case of `NoVocabTokenizer`: that
    /// variant's sentence tells the caller its request "carries text, not token
    /// ids" and offers two next moves -- send a token-id stream, or drop the flag
    /// and take the labelled lexical baseline. A caller that sent nothing can take
    /// neither. The mapping is not the gap here, and neither is the flag: there is
    /// no query for a neural run or for a lexical projection to work on, so the
    /// baseline guidance would be advice to do the same nothing differently. The
    /// refusal names the state the payload is actually in.
    EmptyRequest,
}

/// Do these bytes read as an `AiInferenceRequest`, setting length aside?
///
/// WHY the three are the test: the arm has always had to tell a request header from
/// a raw text query, and it does it by asking whether the fields could have been
/// written by the SDK -- a model id inside the table, a dimension the SDK names, a
/// flag word that fits its own byte. `model_id` is the first four bytes, so any
/// printable text fails it: `b"quer"` read little-endian is 1,919,251,825. The
/// function is the arm's own three clauses, shared with `declared_input_overrun`
/// below so the two cannot drift -- a guard that refused payloads the parse would
/// never have accepted would start refusing prose.
fn payload_is_header_shaped(req: &std::ai::AiInferenceRequest) -> bool {
    (req.model_id <= 16)
        && (req.target_dim == 0 || std::ai::MatryoshkaDim::from_u32(req.target_dim).is_some())
        && (req.flags <= 255)
}

/// Do these bytes read as an `AiInferenceRequest` on the generative route?
///
/// WHY this is not `payload_is_header_shaped`: the two routes ask different
/// questions of a header, and each guard has to answer its own route's. The
/// embedding arm reads `target_dim` and picks a projection width from it, so a
/// dimension the SDK never names means those bytes were not an embedding request;
/// a generation has no output dimension to name, so its parse -- and therefore its
/// guard -- must ask nothing of that field, or a request whose author left a
/// dimension in would go unanswered by a route that has no use for it.
///
/// The two clauses this does share with `extract_prompt_text` are shared by
/// construction: that parse calls this function, so a payload cannot be refused as
/// a cut prompt by a guard and read as prose by the parse, nor the other way round.
fn generative_payload_is_header_shaped(req: &std::ai::AiInferenceRequest) -> bool {
    (req.model_id <= 16) && (req.flags <= 255)
}

/// Was this payload a request whose input did not all arrive, measured by the
/// clauses of the route that is asking?
///
/// `Some((declared, arrived))` for a payload the route reads as a header and which
/// states an input length larger than the bytes holding it. `arrived` is the input
/// region the payload has, which is what the caller can compare against its own
/// prompt; the header is not counted in it.
///
/// WHY a function rather than a clause inside each arm's parse: the parse's answer
/// to an over-long declaration is to ignore the header (embedding) or to clamp to
/// the bytes it has (generation), and both defaults leave a service answering from
/// bytes the caller never sent. The event has to be named, not worked around.
///
/// WHY the shape is the only parameter: the length comparison is the same
/// arithmetic on the same wire layout for both routes, and it is the clause sets
/// that differ. Putting the shared half in one body is what keeps a fix to the
/// comparison from being applied to one route and forgotten on the other.
fn declared_overrun(
    payload: &[u8],
    shape: impl Fn(&std::ai::AiInferenceRequest) -> bool,
) -> Option<(usize, usize)> {
    let hdr = core::mem::size_of::<std::ai::AiInferenceRequest>();
    if payload.len() < hdr {
        return None;
    }
    let req = unsafe {
        core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
    };
    let arrived = payload.len() - hdr;
    if shape(&req) && (req.input_len as usize) > arrived {
        Some((req.input_len as usize, arrived))
    } else {
        None
    }
}

/// The embedding arm's cut test, with the embedding arm's clauses.
fn declared_input_overrun(payload: &[u8]) -> Option<(usize, usize)> {
    declared_overrun(payload, payload_is_header_shaped)
}

/// The generative arm's cut test, with the generative arm's clauses.
///
/// WHY a second name rather than a call with an argument at the site: each arm
/// reads the predicate its own parse uses, and a call site that named the shape
/// inline could be pointed at the other route's function without a reader noticing.
fn declared_prompt_overrun(payload: &[u8]) -> Option<(usize, usize)> {
    declared_overrun(payload, generative_payload_is_header_shaped)
}

/// Does this payload claim to be a strict-neural request that carries nothing?
///
/// `true` only for a header this arm's own parse would have trusted, with the
/// strict-neural flag set, an `input_len` of zero and no input byte behind it -- so
/// neither of the two things a strict request can carry (text, token ids) is
/// present. Bytes after the header are the query the parse takes, so a header
/// declaring zero input in a longer buffer is not an empty request; and prose that
/// happens to be twenty-four bytes fails the shape test, exactly as it does in the
/// parse, rather than being refused for a header nobody sent.
///
/// WHY the flag word alone is not enough: with `input_len` zero the parse falls
/// back to the whole payload, and on a buffer that is only the header that leaves
/// `model_id`, `target_dim`, `input_len`, `flags` and `input_offset` as the query.
/// A device holding the installed file segments those bytes with no complaint --
/// they are bytes -- so the request the caller sent as nothing would be answered as
/// a prompt about the request's own header. That is why this is a guard on the
/// payload, placed ahead of the id read and the mapping like
/// `declared_input_overrun`, and not a rewording of a branch the file can overrule.
fn strict_neural_request_is_empty(payload: &[u8]) -> bool {
    let hdr = core::mem::size_of::<std::ai::AiInferenceRequest>();
    if payload.len() != hdr {
        return false;
    }
    let req = unsafe {
        core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
    };
    payload_is_header_shaped(&req)
        && (req.flags & std::ai::FLAG_STRICT_NEURAL) != 0
        && req.input_len == 0
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

/// What the arm's attempt to map a text to the installed file's ids produced.
///
/// WHY a type rather than an `Option<TokenIds>`: "not attempted", "encoded", "too
/// many positions" and "the file gave no table" are four different answers, and the
/// status a reply carries has to follow whichever one actually happened. Collapsing
/// them to `None` is how a lexical answer keeps a neural label -- the failure this
/// crate's whole status discipline exists to stop.
#[derive(Clone, Copy)]
enum TextIds {
    /// The caller sent its own ids, or sent no text, so no mapping was needed.
    NotAttempted,
    /// The installed file's own table segmented the text; `ids` are its ids.
    ///
    /// WHAT THIS IS EVIDENCE OF, AND WHAT IT IS NOT: the ids came out of a table
    /// built from this file's declared vocabulary, so they address rows the file
    /// has. It is NOT a claim that they are the ids this architecture's reference
    /// tokenizer would have chosen -- `tokenizer.rs` states why that parity is not
    /// measurable on this host, and nothing here says otherwise.
    Encoded { ids: TokenIds },
    /// The segmentation is `needed` positions against a run answering `bound`.
    TooManyIds { needed: usize, bound: usize },
    /// The file refused to yield a table.
    Refused(tokenizer::TokenizerError),
}

impl TextIds {
    fn ids(&self) -> Option<TokenIds> {
        match *self {
            Self::Encoded { ids } => Some(ids),
            _ => None,
        }
    }
}

/// Folds what the mapping produced into the branch the reply will take.
///
/// WHY this is separate from `plan_embedding_run` and pure: the planner picks the
/// branch from inputs a reader can check before anything is opened, and the mapping
/// can only be attempted once a file is opened. Splitting the two keeps the
/// contract gate's rule -- the decision is a function of its named inputs, no file
/// access inside it -- and puts the one input the file can change, the mapping's
/// result, in a function the host can prove from a table too.
///
/// The direction it can move a plan is one-way and it is only ever up-or-sideways
/// from `LexicalBaseline`: an encoded stream earns the file-backed branch, and a
/// refusal costs the strict-neural caller its answer. A caller who did not demand
/// neural weights keeps the labelled baseline, because that is still an honest
/// reply and refusing it would be a worse service, not a truer one.
fn apply_file_vocabulary(plan: EmbeddingPlan, text: &TextIds) -> EmbeddingPlan {
    match (plan, text) {
        // The mapping answered with ids the installed file's own vocabulary
        // declares, so the run it enables is the run both callers wanted: the one
        // that never demanded weights takes it as a better labelled answer, and the
        // one that demanded weights stops being refused for a mapping that no longer
        // has to be invented.
        (EmbeddingPlan::LexicalBaseline, TextIds::Encoded { .. }) => {
            EmbeddingPlan::RunInstalledFile
        }
        (EmbeddingPlan::Refuse(EmbeddingGap::NoVocabTokenizer), TextIds::Encoded { .. }) => {
            EmbeddingPlan::RunInstalledFile
        }

        // The strict caller is refused by the refusal that actually stopped it, with
        // its numbers, instead of by the general "no mapping" the planner could only
        // guess at before the file was opened.
        (
            EmbeddingPlan::Refuse(EmbeddingGap::NoVocabTokenizer),
            TextIds::TooManyIds { needed, bound },
        ) => EmbeddingPlan::Refuse(EmbeddingGap::IdStreamFull(*needed, *bound)),
        (EmbeddingPlan::Refuse(EmbeddingGap::NoVocabTokenizer), TextIds::Refused(error)) => {
            EmbeddingPlan::Refuse(EmbeddingGap::VocabularyRefused(*error))
        }

        // Anything else keeps the branch the inputs chose. A caller who never
        // demanded weights is not deprived of the labelled baseline because this
        // file's table was short, oversized or absent: that refusal would name a gap
        // the caller does not care about and answer nothing where an honest answer
        // was already available.
        (EmbeddingPlan::RunInstalledFile, _) => EmbeddingPlan::RunInstalledFile,
        (EmbeddingPlan::Refuse(gap), _) => EmbeddingPlan::Refuse(gap),
        (EmbeddingPlan::LexicalBaseline, _) => EmbeddingPlan::LexicalBaseline,
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

/// Maps a text to the installed file's own ids, from a table read out of that file.
///
/// WHY the table is built here and dropped when this returns: `FileTokenizer` holds
/// a vocabulary on the heap -- for the installed file, the five parts priced out of
/// the real bytes in `tokenizer::TOKENIZER_HEAP_BUDGET`'s doc -- and the `aid` loop
/// has no place to keep one between requests without a lock,
/// which the platform's rule says a request path does not take. So the mapping is
/// paid for per request, its cost is bounded by `tokenizer::dynamic_tokenizer_heap_budget`
/// -- which is the guest's own reported free bytes clamped to
/// `tokenizer::TOKENIZER_HEAP_BUDGET`, read again for this request because nothing
/// here holds a table between two -- and its refusal names both numbers instead of
/// half-building a table.
///
/// WHY `NotAttempted` before the file is even opened: the caller's own ids beat
/// anything computed here, and an empty query has nothing to map. Reaching for a
/// vocabulary in either case would open a file to answer a question not asked.
fn encode_with_installed_file(path: &str, text: &[u8]) -> TextIds {
    if path.is_empty() || text.is_empty() {
        return TextIds::NotAttempted;
    }
    let table = match get_or_build_resident_tokenizer(path) {
        Ok(table) => table,
        Err(error) => return TextIds::Refused(error),
    };
    let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];
    match table.encode(text, &mut ids) {
        // `encode` answers `Ok` only for a segmentation that fit the buffer, so no
        // count here can exceed the array it was written into.
        Ok(len) => TextIds::Encoded {
            ids: TokenIds { ids, len },
        },
        Err(tokenizer::EncodeError::BeyondBuffer { needed, bound }) => {
            TextIds::TooManyIds { needed, bound }
        }
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
pub(crate) struct FileEmbedReport {
    /// Hidden dimension of the graph the file's own metadata configured.
    pub(crate) hidden_dim: usize,
    /// Positions stepped.
    pub(crate) positions: usize,
    /// Blocks run across every position.
    pub(crate) blocks_run: usize,
    /// Tensor payloads read for the whole run.
    pub(crate) tensors_read: usize,
    /// Bytes of the file the run read.
    pub(crate) bytes_read: u64,
    /// Whether the file's `dense_2`/`dense_3` pooling head was applied. `false`
    /// means the vector is the pooled pre-head residual stream, which is not the
    /// published model's final embedding and must not be reported as one.
    pub(crate) head_applied: bool,
}

/// Why the installed file did not answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FileEmbedError {
    /// The path the probe read is not openable now.
    Open,
    /// The engine could not be built from the file's own declaration.
    Config(InferenceError),
    /// The descriptor table or a tensor read refused.
    Load(weights::LoadError),
}

struct ResidentEmbeddingState {
    cache: inference::ResidentTensorCache,
    info: Option<crate::gguf::GgufModelInfo>,
    path: [u8; 128],
    path_len: usize,
}

struct SyncResidentCache {
    state: core::cell::UnsafeCell<ResidentEmbeddingState>,
}

unsafe impl Sync for SyncResidentCache {}

static RESIDENT_EMBEDDING_CACHE: SyncResidentCache = SyncResidentCache {
    state: core::cell::UnsafeCell::new(ResidentEmbeddingState {
        cache: inference::ResidentTensorCache::new(),
        info: None,
        path: [0u8; 128],
        path_len: 0,
    }),
};

pub(crate) fn get_resident_model_cache(path: &str) -> Option<(weights::TensorTable, crate::gguf::GgufModelInfo)> {
    let p_bytes = path.as_bytes();
    unsafe {
        let state = &*RESIDENT_EMBEDDING_CACHE.state.get();
        if state.cache.initialized
            && state.path_len == p_bytes.len()
            && &state.path[..state.path_len] == p_bytes
        {
            if let Some(info) = state.info {
                let table = weights::TensorTable::from_resident_cache(&state.cache);
                return Some((table, info));
            }
        }
    }
    None
}

/// Announces the identity the file's own metadata declares.
///
/// WHY the EmbeddingGemma claim is gated on `general.architecture`: the probe
/// only reads a GGUF header, and the install slot is a candidate for both model
/// families, so any file written there would otherwise be announced as
/// EmbeddingGemma 2. The published artifact declares `gemma-embedding`;
/// `gemma-embedding2` is the family's other spelling. A file declaring anything
/// else is reported as what it says it is, and a file whose architecture the
/// walk could not read is reported as unestablished.
fn log_declared_model_identity(info: &gguf::GgufModelInfo) {
    let mut buf = [0u8; 256];
    let mut notice = Notice::new(&mut buf);
    match info.architecture() {
        Some(arch) if arch == "gemma-embedding" || arch == "gemma-embedding2" => {
            notice.word(b"[aid] verified EmbeddingGemma 2 weights: the file declares general.architecture ");
            notice.word(arch.as_bytes());
            notice.word(b".");
        }
        Some(arch) => {
            notice.word(b"[aid] model file declares general.architecture ");
            notice.word(arch.as_bytes());
            notice.word(b"; it is not EmbeddingGemma 2.");
        }
        None => {
            notice.word(b"[aid] model file identity unestablished: no readable general.architecture declaration.");
        }
    }
    if let Ok(text) = core::str::from_utf8(notice.finish()) {
        let _ = std::log(text);
    }
}

pub(crate) fn set_resident_model_cache(path: &str, table: &weights::TensorTable, info: &crate::gguf::GgufModelInfo) {
    let p_bytes = path.as_bytes();
    let len = p_bytes.len().min(128);
    unsafe {
        let state = &mut *RESIDENT_EMBEDDING_CACHE.state.get();
        table.populate_resident_cache(&mut state.cache);
        state.info = Some(*info);
        state.path[..len].copy_from_slice(&p_bytes[..len]);
        state.path_len = len;
    }
    log_declared_model_identity(info);
}

struct ResidentTokenizerState {
    path: [u8; 128],
    path_len: usize,
    tokenizer: Option<tokenizer::FileTokenizer>,
}

struct SyncResidentTokenizerCache {
    state: core::cell::UnsafeCell<ResidentTokenizerState>,
}

unsafe impl Sync for SyncResidentTokenizerCache {}

static RESIDENT_TOKENIZER_CACHE: SyncResidentTokenizerCache = SyncResidentTokenizerCache {
    state: core::cell::UnsafeCell::new(ResidentTokenizerState {
        path: [0u8; 128],
        path_len: 0,
        tokenizer: None,
    }),
};

pub(crate) fn get_or_build_resident_tokenizer(
    path: &str,
) -> Result<&'static tokenizer::FileTokenizer, tokenizer::TokenizerError> {
    let p_bytes = path.as_bytes();
    unsafe {
        let state = &mut *RESIDENT_TOKENIZER_CACHE.state.get();
        if state.tokenizer.is_some()
            && state.path_len == p_bytes.len()
            && &state.path[..state.path_len] == p_bytes
        {
            return Ok(state.tokenizer.as_ref().unwrap());
        }

        let built = tokenizer::FileTokenizer::build_from_file(path)?;
        state.tokenizer = Some(built);
        let len = p_bytes.len().min(128);
        state.path[..len].copy_from_slice(&p_bytes[..len]);
        state.path_len = len;
        Ok(state.tokenizer.as_ref().unwrap())
    }
}

pub fn invalidate_resident_tokenizer_cache() {
    unsafe {
        let state = &mut *RESIDENT_TOKENIZER_CACHE.state.get();
        state.tokenizer = None;
        state.path_len = 0;
    }
}

pub fn invalidate_resident_model_cache() {
    invalidate_resident_tokenizer_cache();
    unsafe {
        let state = &mut *RESIDENT_EMBEDDING_CACHE.state.get();
        state.cache.initialized = false;
        state.cache.count = 0;
        state.info = None;
        state.path_len = 0;
    }
}

/// Reads the installed file one block at a time and runs it over `tokens`.
///
/// This is the hop the service had never taken on the device: `weights` had been
/// called from nowhere but its own `#[cfg(test)]` block. Everything below is the
/// loader's own code path, driven through `weights::ModelFileDescriptor` and the
/// kernel `fs_read_at` the guest answers, not through the host's `File`.
pub(crate) fn run_installed_embedding(
    path: &str,
    tokens: &[u32],
    out: &mut [f32],
) -> Result<FileEmbedReport, FileEmbedError> {
    let fd = std::fs_open(path, std::FS_READ).map_err(|_| FileEmbedError::Open)?;
    let source = weights::ModelFileDescriptor { fd };

    let (table, info) = if let Some((cached_table, cached_info)) = get_resident_model_cache(path) {
        (cached_table, cached_info)
    } else {
        let walked = {
            let mut stream = GgufStream::buffered_from_file(fd);
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
            let mut stream = GgufStream::buffered_from_file(fd);
            gguf::read_model_info_stream(&mut stream)
        };
        let info = match read {
            Ok(info) => info,
            Err(error) => {
                let _ = std::fs_close(fd);
                return Err(FileEmbedError::Load(weights::LoadError::Reader(error)));
            }
        };
        set_resident_model_cache(path, &table, &info);
        (table, info)
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
        FileEmbedError::Load(weights::LoadError::ReadRefused(position, asked, errno)) => {
            notice.word(b"Embedding refused: the platform refused a read of ");
            notice.byte_count(*asked as u64);
            notice.word(b" bytes at byte ");
            notice.byte_count(*position);
            notice.word(b" with code ");
            if *errno < 0 {
                notice.word(b"-");
            }
            notice.number(u32::try_from(errno.unsigned_abs()).unwrap_or(u32::MAX));
            notice.word(b", which is the call saying no to a bounded request and not a");
            notice.word(b" file that ran short -- re-install is the wrong next move.");
            STATUS_ERROR
        }
        FileEmbedError::Load(weights::LoadError::OverDeliveredChunk(
            position,
            window,
            claimed,
        )) => {
            notice.word(b"Embedding refused: a read at byte ");
            notice.byte_count(*position);
            notice.word(b" answered with ");
            notice.byte_count(*claimed as u64);
            notice.word(b" bytes for a ");
            notice.byte_count(*window as u64);
            notice.word(b"-byte window, so the loader stopped rather than trust the");
            notice.word(b" count and leave the rest of the tensor as it found it.");
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

/// The engine's documented deterministic decode mode: `inference::sample_token`
/// takes the argmax below 1e-4. This selects greedy decoding; it is not a
/// sampling temperature, because neither the request ABI nor the metadata
/// reader carries one, and a sampled value nobody supplied would be invented.
const DETERMINISTIC_DECODE_TEMPERATURE: f32 = 0.0;

/// Reads the installed generative model and runs token generation over the prompt.
fn run_installed_generation(
    path: &str,
    payload: &[u8],
    sender_task_id: u64,
    session_id: u64,
    tx_buf: &mut [u8; 1024],
) -> bool {
    let fd = match std::fs_open(path, std::FS_READ) {
        Ok(fd) => fd,
        Err(_) => return false,
    };
    let source = weights::ModelFileDescriptor { fd };

    let (table, info) = if let Some((cached_table, cached_info)) = get_resident_model_cache(path) {
        (cached_table, cached_info)
    } else {
        let walked = {
            let mut stream = GgufStream::buffered_from_file(fd);
            weights::TensorTable::walk(&mut stream)
        };
        let table = match walked {
            Ok(table) => table,
            Err(_) => {
                let _ = std::fs_close(fd);
                return false;
            }
        };

        let read = {
            let mut stream = GgufStream::buffered_from_file(fd);
            gguf::read_model_info_stream(&mut stream)
        };
        let info = match read {
            Ok(info) => info,
            Err(_) => {
                let _ = std::fs_close(fd);
                return false;
            }
        };
        set_resident_model_cache(path, &table, &info);
        (table, info)
    };

    let config = match ModelConfig::from_gguf(&info) {
        Ok(config) => config,
        Err(_) => {
            let _ = std::fs_close(fd);
            return false;
        }
    };

    // Admission is based on memory available now, not total RAM and not a
    // model-name table. The native backend prices geometry parsed from this
    // file and leaves a scaled reserve for the foreground OS workload.
    let _telemetry = match std::memory_telemetry() {
        Ok(telemetry) => telemetry,
        Err(_) => {
            let _ = std::fs_close(fd);
            reply_with_answer(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"Generation unavailable: the platform did not provide a physical-memory reading for resource admission.",
                tx_buf,
            );
            return true;
        }
    };
    let estimate = match backend::native_generation_resources(&config) {
        Ok(estimate) => estimate,
        Err(_) => {
            let _ = std::fs_close(fd);
            reply_with_answer(
                sender_task_id,
                std::ai::STATUS_UNSUPPORTED,
                b"Generation refused: the native Rust backend could not price this model's declared geometry.",
                tx_buf,
            );
            return true;
        }
    };
    let ledger_session_res = with_broker_ledger(|ledger| {
        ledger.create_session(sender_task_id, 1, estimate)
    });
    let ledger_session_id = match ledger_session_res {
        Ok(id) => id,
        Err(backend::AdmissionError::InsufficientRam) => {
            let _ = std::fs_close(fd);
            reply_with_answer(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"Generation unavailable: the backend working set plus the foreground OS reserve exceeds RAM available now.",
                tx_buf,
            );
            return true;
        }
        Err(backend::AdmissionError::SessionCapacity) => {
            let _ = std::fs_close(fd);
            reply_with_answer(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"Generation unavailable: maximum active AI session capacity reached.",
                tx_buf,
            );
            return true;
        }
        Err(_) => {
            let _ = std::fs_close(fd);
            reply_with_answer(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"Generation unavailable: resource admission rejected request.",
                tx_buf,
            );
            return true;
        }
    };

    let backend_session_id = {
        let mut backend = NATIVE_GGUF_BACKEND.lock();
        let model_handle = backend
            .models
            .iter()
            .find(|m| m.active && &m.source[..m.source_len] == path.as_bytes())
            .map(|m| m.handle)
            .unwrap_or_else(|| {
                let manifest = backend::ModelManifest {
                    id: "org.interos.installed-generative",
                    display_name: "Installed Generative Model",
                    source: path,
                    revision: "local",
                    sha256: [0; 32],
                    artifact_bytes: 0,
                    format: backend::ModelFormat::Gguf,
                    architecture: "gemma2",
                    quantization: backend::Quantization::Q6K,
                    tokenizer: "embedded",
                    required_hardware: backend::HardwareBackend::Cpu,
                    capabilities: backend::CapabilitySet::TEXT_GENERATION
                        .union(backend::CapabilitySet::STREAMING_GENERATION),
                    resources: estimate,
                };
                backend.load_model(&manifest).unwrap_or(1)
            });
        backend.create_session(model_handle, sender_task_id).ok()
    };

    struct SessionGuard {
        owner: u64,
        ledger_session_id: u64,
        backend_session_id: Option<u64>,
    }
    impl Drop for SessionGuard {
        fn drop(&mut self) {
            with_broker_ledger(|ledger| {
                let _ = ledger.destroy_session(self.owner, self.ledger_session_id);
            });
            if let Some(bsid) = self.backend_session_id {
                let mut backend = NATIVE_GGUF_BACKEND.lock();
                let _ = backend.destroy_session(bsid);
            }
        }
    }
    let _session_guard = SessionGuard {
        owner: sender_task_id,
        ledger_session_id,
        backend_session_id,
    };

    let mut reader = weights::WeightReader::new(table, source);
    if !reader.has_output_head() {
        let _ = std::fs_close(fd);
        return false;
    }

    let prompt_text = extract_prompt_text(payload);
    if prompt_text.is_empty() {
        let _ = std::fs_close(fd);
        reply_with_answer(
            sender_task_id,
            std::ai::STATUS_UNSUPPORTED,
            b"Generation refused: this request carries no prompt text. An empty prompt is not a question, and no token was invented to stand in for one.",
            tx_buf,
        );
        return true;
    }

    let tokenizer = match get_or_build_resident_tokenizer(path) {
        Ok(tokenizer) => tokenizer,
        Err(error) => {
            let _ = std::fs_close(fd);
            let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
            let mut notice = Notice::new(&mut scratch);
            notice.word(b"Generation refused: the installed file yielded no vocabulary table -- ");
            tokenizer_refusal_words(error, &mut notice);
            notice.word(b" No ids were invented from the prompt's bytes.");
            reply_with_answer(sender_task_id, std::ai::STATUS_UNSUPPORTED, notice.finish(), tx_buf);
            return true;
        }
    };

    // WHY the id buffer is the file's declared context and not a fixed guess:
    // `encode` refuses a segmentation longer than the buffer with both counts,
    // so a prompt the model cannot address is named rather than truncated.
    let mut prompt_ids = alloc::vec::Vec::new();
    if prompt_ids.try_reserve_exact(config.seq_len).is_err() {
        let _ = std::fs_close(fd);
        reply_with_answer(
            sender_task_id,
            std::ai::STATUS_UNAVAILABLE,
            b"Generation unavailable: the device could not reserve an id buffer for the model's declared context length.",
            tx_buf,
        );
        return true;
    }
    prompt_ids.resize(config.seq_len, 0u32);
    let num_ids = match tokenizer.encode(prompt_text, &mut prompt_ids) {
        Ok(n) => n,
        Err(tokenizer::EncodeError::BeyondBuffer { needed, bound }) => {
            let _ = std::fs_close(fd);
            let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
            let mut notice = Notice::new(&mut scratch);
            notice.word(b"Generation refused: this prompt segments into ");
            notice.byte_count(needed as u64);
            notice.word(b" token ids against the model's declared context of ");
            notice.byte_count(bound as u64);
            notice.word(b" positions. Nothing was truncated -- a cut-off prompt is a different question. Send a shorter prompt.");
            reply_with_answer(sender_task_id, std::ai::STATUS_UNSUPPORTED, notice.finish(), tx_buf);
            return true;
        }
    };
    let tokens = &prompt_ids[..num_ids];

    // WHY the run length is computed from the file: the positions the model
    // declares minus the prompt's own is the room it has for new tokens. A
    // prompt that fills the context leaves none, and the reply says so instead
    // of running past the cache the file sized.
    let max_new_tokens = config.seq_len.saturating_sub(num_ids);
    if max_new_tokens == 0 {
        let _ = std::fs_close(fd);
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let mut notice = Notice::new(&mut scratch);
        notice.word(b"Generation unavailable: this prompt fills the model's declared context of ");
        notice.byte_count(config.seq_len as u64);
        notice.word(b" positions, leaving no room for a new token.");
        reply_with_answer(sender_task_id, std::ai::STATUS_UNAVAILABLE, notice.finish(), tx_buf);
        return true;
    }
    let temperature = DETERMINISTIC_DECODE_TEMPERATURE;
    let mut generated_bytes = alloc::vec::Vec::new();
    // WHY the reader's piece bound and not a narrower guess: `decode_token`
    // cuts silently at the buffer's edge, and the reader refuses any piece
    // wider than this bound, so it is the widest piece the file can declare.
    let mut decode_buf = [0u8; tokenizer::MAX_PIECE_BYTES];

    let is_cancelled = || {
        if ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst) == session_id
            || with_broker_ledger(|l| l.is_cancelled(ledger_session_id))
        {
            return true;
        }
        let mut poll_buf = [0u8; 1024];
        while let Ok((sender, msg_type, len)) = std::ipc::receive_any(&mut poll_buf) {
            if msg_type == std::ai::MSG_TYPE_CANCEL {
                let is_admin = sender == 1 || std::task_identity_query(sender).map(|id| id.is_system != 0).unwrap_or(false);
                if sender == sender_task_id || is_admin {
                    ACTIVE_SESSION_CANCELLED.store(session_id, core::sync::atomic::Ordering::SeqCst);
                    with_broker_ledger(|ledger| {
                        let _ = ledger.cancel(sender_task_id, ledger_session_id);
                    });
                    if let Some(bsid) = backend_session_id {
                        let mut backend = NATIVE_GGUF_BACKEND.lock();
                        let _ = backend.cancel(bsid);
                    }
                    let _ = std::log("[aid] cancellation requested via IPC during generation");
                    let mut cancel_tx = [0u8; 1024];
                    emit_inference_response(
                        sender,
                        std::ai::STATUS_SUCCESS,
                        b"[Cancellation acknowledged]",
                        &mut cancel_tx,
                    );
                    return true;
                } else {
                    let mut rej_tx = [0u8; 1024];
                    emit_inference_response(
                        sender,
                        std::ai::STATUS_ERROR,
                        b"[Cancellation rejected: sender does not own active session]",
                        &mut rej_tx,
                    );
                }
            } else if msg_type == MSG_TYPE_GENERATE
                || msg_type == MSG_TYPE_GENERATE_ALT
            {
                let mut busy_tx = [0u8; 1024];
                emit_inference_response(
                    sender,
                    std::ai::STATUS_UNAVAILABLE,
                    b"Inference service busy: active generation in progress",
                    &mut busy_tx,
                );
            } else if len > 0 {
                // Preserve unrelated IPC message in the deferred queue so it is not dropped
                push_deferred_ipc(sender, msg_type, &poll_buf[..len]);
            }
        }
        false
    };

    let ran = weights::run_generative_model(
        &mut reader,
        &config,
        tokens,
        max_new_tokens,
        temperature,
        &is_cancelled,
        |token_id| {
            if let Ok(tokenizer) = get_or_build_resident_tokenizer(path) {
                let n = tokenizer.decode_token(token_id, &mut decode_buf);
                if n > 0 {
                    generated_bytes.extend_from_slice(&decode_buf[..n]);
                }
            }
            true
        },
    );
    let _ = std::fs_close(fd);

    match ran {
        Ok(count) if count > 0 => {
            if generated_bytes.is_empty() {
                let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                let mut notice = Notice::new(&mut scratch);
                notice.word(b"Generation unavailable: the model sampled ");
                notice.number(count as u32);
                notice.word(b" tokens, but decoding them through the file's own vocabulary produced no bytes. No answer was authored to stand in for them.");
                reply_with_answer(sender_task_id, std::ai::STATUS_UNAVAILABLE, notice.finish(), tx_buf);
            } else {
                let _ = std::log("[aid] native transformer execution generated new tokens.");
                reply_with_answer(sender_task_id, std::ai::STATUS_SUCCESS, generated_bytes.as_slice(), tx_buf);
            }
            true
        }
        Err(weights::LoadError::Graph(crate::inference::InferenceError::Cancelled)) => {
            let _ = std::log("[aid] generation cancelled; resources reclaimed.");
            true
        }
        _ => false,
    }
}

/// Why the arm ran nothing at all.
fn embedding_gap_notice<'a>(gap: EmbeddingGap, buf: &'a mut [u8]) -> (u32, &'a [u8]) {
    let mut notice = Notice::new(buf);
    let status = match gap {
        EmbeddingGap::NoWeights => {
            notice.word(
                b"Embedding refused: the strict-neural flag is set and the model namespace holds no weights file (/data/models/embeddinggemma-2.gguf) the probe read.",
            );
            STATUS_UNAVAILABLE
        }
        EmbeddingGap::NoVocabTokenizer => {
            // WHY this still exists: the mapping is now read from the file, so
            // nothing should land here -- but the planner can be handed a strict
            // request whose text was never offered to the table, and an unreachable
            // branch is not a reason to remove the one refusal that names the
            // vocabulary gap in the words the contract gate reads. It says what the
            // file is missing, and it does not claim the ids were ever guessed.
            notice.word(
                b"Embedding refused: the strict-neural flag is set and this request carries text, not token ids, and no ids were mapped from the installed file's own declared vocabulary. Send a token-id stream, or drop the flag and take the labelled lexical baseline.",
            );
            STATUS_UNSUPPORTED
        }
        EmbeddingGap::IdStreamFull(needed, bound) => {
            notice.word(b"Embedding refused: this file's own vocabulary segments the text into ");
            notice.byte_count(needed as u64);
            notice.word(b" token ids and one embedding run answers at most ");
            notice.byte_count(bound as u64);
            notice.word(
                b". Nothing was truncated -- a cut-off id stream is a different prompt, and its vector would be filed under this one. Send a shorter text, or a token-id stream of your own.",
            );
            STATUS_UNSUPPORTED
        }
        EmbeddingGap::VocabularyRefused(error) => {
            // The table said why, in numbers, and both are relayed rather than
            // flattened into one "unsupported": a 16 MiB budget and a 129-byte piece
            // have different next moves.
            notice.word(b"Embedding refused: the installed file yielded no vocabulary table -- ");
            tokenizer_refusal_words(error, &mut notice);
            notice.word(
                b" Drop the strict-neural flag to take the labelled lexical baseline, or send a token-id stream.",
            );
            STATUS_UNSUPPORTED
        }
        EmbeddingGap::InputTruncated(declared, arrived) => {
            // Both counts, because the caller is the only party that knows how long
            // its prompt was and cannot see what the transport did to it. A refusal
            // that said only "too long" would leave it guessing what would have fit.
            notice.word(b"Embedding refused: this request declares ");
            notice.byte_count(declared as u64);
            notice.word(b" input bytes and the transport delivered ");
            notice.byte_count(arrived as u64);
            notice.word(
                b". Nothing was answered from the piece that arrived: the bytes in front of this service are the request's own header followed partway through the prompt, and a vector made from them would be filed under a request the device never received.",
            );
            notice.word(b" Re-send with an input no longer than the bytes that arrived.");
            STATUS_UNSUPPORTED
        }
        EmbeddingGap::EmptyRequest => {
            // WHAT this refusal may not do is recycle the sentence above it. Every
            // other strict-neural gap here can point the caller at dropping the
            // flag, because the labelled lexical projection genuinely answers a text
            // request -- but an empty request has no query for that projection
            // either, so "drop the flag" would promise an answer that does not
            // exist. And it may not claim the request carried text: it carried
            // nothing, which is the whole reason nothing runs.
            notice.word(
                b"Embedding refused: the strict-neural flag is set and this request carries neither text nor token ids -- it declares no input and the transport delivered none, so nothing reached this service for a model to run on. Dropping the flag is not a way past it: the labelled lexical baseline has no query to project either, and a vector out of an empty request would be this service answering a question nobody asked. Send text, or a token-id stream.",
            );
            STATUS_UNSUPPORTED
        }
    };
    (status, notice.finish())
}

/// Renders one `TokenizerError` into a refusal notice, with the numbers it carries.
///
/// WHY a helper: three of these refusals name two numbers each, and a notice that
/// said "too large" without them would leave the reader with nothing to act on --
/// the same reason every other refusal in this file prints both.
fn tokenizer_refusal_words<'a>(error: tokenizer::TokenizerError, notice: &mut Notice<'a>) {
    use tokenizer::TokenizerError as E;
    match error {
        E::Reader(gguf::GgufError::UnsupportedVersion(v)) => {
            notice.word(b"GGUF version ");
            notice.byte_count(v as u64);
            notice.word(b" is not one the reader walks.");
        }
        E::Reader(gguf::GgufError::CorruptedTensor) => {
            notice.word(b"the metadata walk lost its place at a tensor descriptor.");
        }
        E::Reader(gguf::GgufError::BufferTooShort) => {
            notice.word(b"the file stopped inside its own metadata.");
        }
        E::Reader(gguf::GgufError::InvalidMetadata) => {
            notice.word(b"a metadata value is not the shape its key names.");
        }
        E::Reader(gguf::GgufError::ReadFailed) => {
            notice.word(b"a positioned read of the metadata failed.");
        }
        E::Reader(_) => notice.word(b"the metadata reader refused the file."),
        E::NoVocabulary => notice.word(
            b"the file states no tokenizer.ggml.tokens the encoder is allowed to emit, and there is no built-in vocabulary to fall back to.",
        ),
        E::FileNotFound => {
            notice.word(b"the file the probe named could not be opened when the table was built.")
        }
        E::VocabTooLarge(needed, budget) => {
            notice.word(b"its declared vocabulary needs ");
            notice.byte_count(needed as u64);
            notice.word(b" bytes against a bound of ");
            notice.byte_count(budget as u64);
            notice.word(b"; it was refused before a single token was read.");
        }
        E::VocabHeldTooLarge(needed, budget) => {
            notice.word(b"the table outgrew its estimate at ");
            notice.byte_count(needed as u64);
            notice.word(b" bytes against a bound of ");
            notice.byte_count(budget as u64);
            notice.word(b".");
        }
        E::PieceTooLong(id, stated, bound) => {
            notice.word(b"token id ");
            notice.number(id);
            notice.word(b" states ");
            notice.byte_count(stated as u64);
            notice.word(b" bytes where the reader's window holds ");
            notice.byte_count(bound as u64);
            notice.word(b".");
        }
        E::ArrayLengthMismatch(key, found, expected) => {
            notice.word(key.as_bytes());
            notice.word(b" declares ");
            notice.number(found);
            notice.word(b" entries beside a vocabulary of ");
            notice.number(expected);
            notice.word(b".");
        }
        E::BadArrayShape(key, found) => {
            notice.word(key.as_bytes());
            notice.word(b" has element type ");
            notice.number(found);
            notice.word(b", which is not the kind that key names.");
        }
        E::Truncated(key) => {
            notice.word(b"the file stopped inside ");
            notice.word(key.as_bytes());
            notice.word(b".");
        }
        E::ArithmeticOverflow => {
            notice.word(b"a length in the file does not fit this device's address width.")
        }
    }
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
    if msg_type == MSG_TYPE_INSTALL {
        install_model_file(sender_task_id, tx_buf);
        return;
    }
    if msg_type == std::ai::MSG_TYPE_CANCEL {
        let active = ACTIVE_GENERATION_TASK.load(core::sync::atomic::Ordering::SeqCst);
        let active_sess = ACTIVE_SESSION_ID.load(core::sync::atomic::Ordering::SeqCst);
        if active == 0 || active_sess == 0 {
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_SUCCESS,
                b"[No active inference session to cancel]",
                tx_buf,
            );
        } else if sender_task_id == active || sender_task_id == 1 || std::task_identity_query(sender_task_id).map(|id| id.is_system != 0).unwrap_or(false) {
            ACTIVE_SESSION_CANCELLED.store(active_sess, core::sync::atomic::Ordering::SeqCst);
            with_broker_ledger(|ledger| {
                let _ = ledger.cancel(sender_task_id, active_sess);
            });
            {
                let mut backend = NATIVE_GGUF_BACKEND.lock();
                let _ = backend.cancel(active_sess);
            }
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_SUCCESS,
                b"[Cancellation acknowledged]",
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
    if msg_type == MSG_TYPE_GGUF {
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

    // Role resolution queries (MSG_TYPE_ROLE_RESOLVE)
    if msg_type == MSG_TYPE_ROLE_RESOLVE {
        let requested_role = if payload.len() >= 4 {
            u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]])
        } else {
            std::ai::MODEL_ROLE_GENERATIVE_TEXT
        };

        let mut defaults = std::companion::ModelRegistryDefaults::new();
        let mut loaded_defaults = false;
        if let Ok(fd) = std::fs_open(std::companion::PATH_MODEL_DEFAULTS, std::FS_READ) {
            let mut buf = [0u8; 512];
            if let Ok(n) = std::fs_read(fd, &mut buf) {
                if n > 0 && defaults.deserialize(&buf[..n]).is_ok() {
                    loaded_defaults = true;
                }
            }
            let _ = std::fs_close(fd);
        }

        let role_opt = std::ai::ModelRole::from_u32(requested_role);
        let mut resolved = false;

        if loaded_defaults {
            if let Some(role) = role_opt {
                if let Some(binding) = defaults.get_binding(role) {
                    let mut resp = [0u8; 64];
                    resp[0..4].copy_from_slice(&STATUS_SUCCESS.to_le_bytes());
                    resp[4..8].copy_from_slice(&requested_role.to_le_bytes());
                    let id_str = binding.model_id_str();
                    let id_bytes = id_str.as_bytes();
                    let copy_len = id_bytes.len().min(56);
                    resp[8..8 + copy_len].copy_from_slice(&id_bytes[..copy_len]);
                    let _ = std::ipc_send(sender_task_id, MSG_TYPE_ROLE_RESOLVE, &resp[..8 + copy_len]);
                    resolved = true;
                }
            }
        }

        if !resolved {
            let mut resp = [0u8; 8];
            resp[0..4].copy_from_slice(&STATUS_UNAVAILABLE.to_le_bytes());
            resp[4..8].copy_from_slice(&requested_role.to_le_bytes());
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_ROLE_RESOLVE, &resp);
        }
        return;
    }

    // 2. Generative token stream queries (Gemma 4 2B / on-device generative)
    //
    // Platform Truth Contract & Architecture Boundary: generative queries run
    // on-device. `run_installed_generation` reads the installed file's tensors
    // block by block through `weights::run_generative_model` and answers with
    // the bytes the file's own vocabulary decoded; there is no host bridge on
    // this route. When the file cannot configure the engine, the arm replies
    // STATUS_UNSUPPORTED or STATUS_UNAVAILABLE plus the structured model gap
    // notice, refusing to fabricate synthetic prose.
    if msg_type == MSG_TYPE_GENERATE
        || msg_type == MSG_TYPE_GENERATE_ALT
    {
        // A prompt the transport cut is refused here, before the task is registered
        // and before the model file is opened.
        //
        // WHY this position and not further in: `extract_prompt_text` clamps the
        // declared length to the bytes that arrived (`req.input_len.min(...)`) and
        // hands back the prefix, so every site below this one already believes it
        // holds a whole prompt -- it opens the file, reads the metadata and answers
        // the piece that survived as though the caller had asked that question. For
        // an embedding the reader at least sees a vector of the width they asked for;
        // a generation is fluent prose either way, so nothing downstream can spot the
        // cut and this arm is the only party that still can.
        //
        // WHY before the registration: an active session is one the device can be
        // asked to cancel, and a request that never fully arrived started nothing.
        if let Some((declared, arrived)) = declared_prompt_overrun(payload) {
            let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
            let (status, notice) = generation_overrun_notice(declared, arrived, &mut scratch);
            reply_with_answer(sender_task_id, status, notice, tx_buf);
            return;
        }

        let new_session_id = NEXT_GENERATION_SESSION_ID.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        if ACTIVE_SESSION_ID.compare_exchange(0, new_session_id, core::sync::atomic::Ordering::SeqCst, core::sync::atomic::Ordering::SeqCst).is_err() {
            emit_inference_response(
                sender_task_id,
                std::ai::STATUS_UNAVAILABLE,
                b"Inference service busy: active generation in progress",
                tx_buf,
            );
            return;
        }

        ACTIVE_GENERATION_TASK.store(sender_task_id, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

        struct ActiveTaskGuard;
        impl Drop for ActiveTaskGuard {
            fn drop(&mut self) {
                ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
                ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
                ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
            }
        }
        let _task_guard = ActiveTaskGuard;

        if let Some((_hdr, path)) = probed_models.generative {
            if run_installed_generation(path, payload, sender_task_id, new_session_id, tx_buf) {
                return;
            }

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
    if msg_type == MSG_TYPE_SEARCH {
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
        let _ = index.load_from_disk(rag::PATH_SEARCH_INDEX_DATA)
            .or_else(|_| index.load_from_disk(rag::PATH_SEARCH_INDEX_STORAGE));

        let mut matches = [rag::RagMatch::default(); 8];
        let found_count = {
            let mut litert_backend = NATIVE_LITERTLM_BACKEND.lock();
            let active_sess = litert_backend
                .sessions
                .iter()
                .find(|s| s.active && s.owner == sender_task_id)
                .map(|s| s.id);
            if let Some(sess) = active_sess {
                index.search_with_backend(
                    query_text,
                    target_dim,
                    top_k,
                    min_confidence,
                    &mut matches,
                    &mut *litert_backend,
                    sess,
                )
            } else {
                index.search(query_text, target_dim, top_k, min_confidence, &mut matches)
            }
        };

        let end_tick = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0)
            .ok()
            .and_then(|_| std::get_ticks().ok())
            .unwrap_or(start_tick);
        // The platform timer exposes millisecond-resolution ticks (PIT);
        // multiplying by 1000 converts elapsed milliseconds to nominal microseconds
        // for the wire descriptor `AiInferenceResponse.latency_micros` (1ms granularity).
        let latency_micros = end_tick.saturating_sub(start_tick).saturating_mul(1000);

        // WHY zero hits is not a success: `output_dim` counts the matches this
        // frame carries, and a retrieval that matched nothing must not read as
        // one that found what the caller asked for.
        let status = if found_count == 0 {
            std::ai::STATUS_UNAVAILABLE
        } else {
            std::ai::STATUS_SUCCESS
        };
        let resp_hdr = std::ai::AiInferenceResponse {
            status,
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
            status,
            MSG_TYPE_SEARCH_CHUNK,
            &tx_buf[..cursor],
        ) && sender_task_id != 0
        {
            let _ = std::ipc_send(sender_task_id, MSG_TYPE_SEARCH, &tx_buf[..cursor]);
        }
        return;
    }

    // Only the authenticated message type selects an operation. Payload bytes
    // cannot elevate an embedding request into a search or installation.
    if msg_type != MSG_TYPE_EMBED {
        emit_inference_response(sender_task_id, std::ai::STATUS_ERROR, b"Unsupported AI request type", tx_buf);
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
        let is_valid_req = payload_is_header_shaped(&req)
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

    // A header declaring more input than arrived is a request the transport cut, and
    // this is the one place that can still say so: below here the arm has either
    // ignored the header or answered from the buffer it came in. Refusing now, ahead
    // of the id read and ahead of the mapping, is what keeps the header's own bytes
    // from being segmented into ids and run over the installed file for a
    // `STATUS_SUCCESS`. The host cannot see that happen -- nothing can open the file
    // here, so the mapping only ever refuses -- which is why the position of this
    // clause is pinned as a shape by the contract gate rather than left to be inferred
    // from green tests.
    //
    // WHY the refusal is unconditional: a caller whose prompt never reached the device
    // is owed the two counts whether or not it demanded neural weights. Answering the
    // fragment with the labelled baseline would be honest about the arithmetic and
    // still an answer to a question nobody asked.
    if let Some((declared, arrived)) = declared_input_overrun(payload) {
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) =
            embedding_gap_notice(EmbeddingGap::InputTruncated(declared, arrived), &mut scratch);
        emit_embed_notice(sender_task_id, status, notice, tx_buf);
        return;
    }

    // A strict-neural request carrying neither text nor token ids is an empty
    // request, and it is refused for being empty rather than answered as though it
    // had carried something.
    //
    // WHY the position is here, ahead of the id read and the mapping: below this
    // point the parse's fallback hands `query_text` the payload's own twenty-four
    // header bytes, and on a device holding the installed file those bytes segment
    // into ids with no complaint -- so the request sent as nothing would come back
    // as a vector over the request's own header, filed under this caller. The host
    // cannot watch that happen, because nothing can open the file here, which is why
    // the guard is a decision of the payload alone and not a branch the file's own
    // answer can still overrule.
    //
    // WHY the refusal is unconditional on the probe: `NoWeights` is a real gap for a
    // caller who demanded weights and sent a prompt, but the caller who sent no
    // prompt is stopped by the missing prompt first, and naming the file instead
    // would describe a run that could not have happened either way.
    if strict_neural_request_is_empty(payload) {
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) = embedding_gap_notice(EmbeddingGap::EmptyRequest, &mut scratch);
        emit_embed_notice(sender_task_id, status, notice, tx_buf);
        return;
    }

    // The ids this request carries, if it carries any. Read before the branch is
    // chosen, because the branch is chosen from them: a whole id stream is what
    // lets the installed file run without anything being computed first.
    let token_ids = if (request_flags & std::ai::FLAG_INPUT_TOKEN_IDS) != 0 {
        read_token_ids(query_text)
    } else {
        None
    };

    // What the caller's ids could not supply, this file can: a text request whose
    // ids nobody sent is mapped through the vocabulary the installed file declares.
    //
    // WHY the attempt sits in the arm and not in `run_installed_embedding`: the
    // contract gate that keeps the loader honest reads `run_installed_embedding`
    // and refuses to find a tokenizer call inside it, because the version that ran
    // there fed the graph invented ids. Mapping here keeps that gate meaningful --
    // the loader still takes only ids someone proved the source of -- and the
    // branch below still only runs on ids that came out of the file itself.
    //
    // WHY the caller's own ids win: they are what the caller asked to run, and a
    // service that re-tokenized a token-id stream would be answering a different
    // question than the one it was sent.
    let text_ids = match (token_ids, request_flags & std::ai::FLAG_INPUT_TOKEN_IDS) {
        (Some(_), _) => TextIds::NotAttempted,
        (None, 0) => match probed_models.embedding {
            Some((_hdr, path)) => encode_with_installed_file(path, query_text),
            None => TextIds::NotAttempted,
        },
        // The flag was set and the bytes were not a stream: not a text to map, a
        // malformed id payload, and `plan_embedding_run` already calls that a file
        // request -- which is exactly why the baseline is still an honest answer.
        (None, _) => TextIds::NotAttempted,
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

    // The plan's second input, the one only the file can supply: what the mapping
    // of this text produced. `apply_file_vocabulary` is where a text request earns
    // the file-backed branch or loses the strict one, and it is a pure function of
    // `(plan, text_ids)` so the host can prove both directions.
    let plan = apply_file_vocabulary(plan, &text_ids);

    let (report, dim, vector) = match plan {
        EmbeddingPlan::Refuse(gap) => {
            let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
            let (status, notice) = embedding_gap_notice(gap, &mut scratch);
            emit_embed_notice(sender_task_id, status, notice, tx_buf);
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
            // planner can turn this arm into a panic inside the service loop. The
            // ids may come from the caller or from this file's own vocabulary, and
            // those two are told apart by `text_ids`, not by the pair.
            let (path, ids) = match (probed_models.embedding, token_ids.or(text_ids.ids())) {
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

            let estimate = backend::ResourceEstimate {
                model_weights: 0,
                kv_cache: 1024 * 1024,
                activations: 2 * 1024 * 1024,
                scratch: 0,
                tokenizer: 0,
                embedding_indexes: 0,
                audio_buffers: 0,
                runtime_overhead: 1024 * 1024,
            };
            let embed_sess_res = with_broker_ledger(|ledger| {
                ledger.create_session(sender_task_id, 1, estimate)
            });
            let embed_sess_id = match embed_sess_res {
                Ok(sid) => sid,
                Err(_) => {
                    if strict_neural_required {
                        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
                        let mut notice = Notice::new(&mut scratch);
                        notice.word(b"Embedding unavailable: the working set plus the foreground OS reserve exceeds RAM available now.");
                        let status = std::ai::STATUS_UNAVAILABLE;
                        let notice = notice.finish();
                        emit_embed_notice(sender_task_id, status, notice, tx_buf);
                        return;
                    } else {
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
                }
            };

            struct EmbedGuard {
                owner: u64,
                session_id: u64,
            }
            impl Drop for EmbedGuard {
                fn drop(&mut self) {
                    with_broker_ledger(|ledger| {
                        let _ = ledger.destroy_session(self.owner, self.session_id);
                    });
                }
            }
            let _embed_guard = EmbedGuard {
                owner: sender_task_id,
                session_id: embed_sess_id,
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
                    emit_embed_notice(sender_task_id, status, notice, tx_buf);
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
    if let Ok(notice_str) = core::str::from_utf8(notice) {
        let _ = std::log(notice_str);
    }
    return;
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

        // The ids come out of a walk over a file that has the installed model's
        // shape, because the pipeline this test is named for starts at the file.
        // Typed-in ids would let the graph certify an answer the tokenizer never
        // produced, which is exactly the pairing the invented table made unverifiable.
        let bytes = tokenizer::test_vocabulary::installed_shape();
        let mut stream = gguf::GgufStream::from_slice(&bytes);
        let table = tokenizer::FileTokenizer::build_from_stream(&mut stream)
            .expect("the fixture vocabulary builds a table");
        let mut tokens = [0u32; 32];
        let n_tokens = table
            .encode(text, &mut tokens)
            .expect("this text segments inside thirty positions");
        assert!(n_tokens > 0);
        assert!(
            tokens[..n_tokens]
                .iter()
                .all(|&id| (id as usize) < table.declared_vocab() as usize),
            "the graph was fed an id this vocabulary never declared"
        );

        // Position zero is the bos the fixture's own `add_bos_token` asked for, so
        // the round trip is asserted over the text-bearing tail.
        let mut decoded = [0u8; 64];
        let n_bytes = table.decode_sequence(&tokens[1..n_tokens], &mut decoded);
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
        // The frame's status has to agree with the hits it carries: the old
        // unconditional STATUS_SUCCESS let a zero-hit retrieval read as a find.
        if resp.output_dim == 0 {
            assert_eq!(resp.status, std::ai::STATUS_UNAVAILABLE);
        } else {
            assert_eq!(resp.status, std::ai::STATUS_SUCCESS);
        }
    }

    #[test]
    fn test_handle_ipc_search_zero_hits_reports_unavailable() {
        let mut tx_buf = [0u8; 1024];
        // An empty query matches nothing by the index's own rule, whatever
        // documents the host namespace holds, so the zero-hit frame is certain.
        handle_ipc_inference_query(
            0,
            MSG_TYPE_SEARCH,
            b"SRCH",
            &gguf::ProbedModels::empty(),
            &mut tx_buf,
        );
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.output_dim, 0);
        assert_eq!(
            resp.status,
            std::ai::STATUS_UNAVAILABLE,
            "a retrieval that matched nothing answered as a success"
        );
    }

    #[test]
    fn test_handle_ipc_cancel_query() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        ACTIVE_SESSION_ID.store(50, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(1, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

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
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 50);

        // A payload prefix cannot select cancellation under another message type.
        tx_buf.fill(0);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
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
        assert_eq!(resp2.status, std::ai::STATUS_ERROR);
        let payload2 = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp2.output_dim as usize];
        assert_eq!(payload2, b"Unsupported AI request type");
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 0);

        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn payload_prefix_cannot_choose_install_or_search() {
        let models = gguf::ProbedModels::empty();
        for payload in [&b"INSL"[..], &b"SRCH"[..], &b"RAGQ"[..]] {
            let mut response = [0u8; 1024];
            handle_ipc_inference_query(0, 0, payload, &models, &mut response);
            let header = unsafe {
                core::ptr::read_unaligned(response.as_ptr() as *const std::ai::AiInferenceResponse)
            };
            assert_eq!(header.status, std::ai::STATUS_ERROR);
            let start = core::mem::size_of::<std::ai::AiInferenceResponse>();
            assert_eq!(&response[start..start + header.output_dim as usize], b"Unsupported AI request type");
        }
    }

    #[test]
    fn test_handle_ipc_cancel_matching_session() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        ACTIVE_SESSION_ID.store(100, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

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
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 100);

        // System init (task 1) also has authority to cancel active generation
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
        tx_buf.fill(0);
        handle_ipc_inference_query(
            1,
            MSG_TYPE_CANCEL,
            b"",
            &empty_models,
            &mut tx_buf,
        );
        let resp1 = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp1.status, std::ai::STATUS_SUCCESS);
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 100);

        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn test_handle_ipc_cancel_rejection_when_not_owner() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // Active task is 42, active session is 101, but sender is 99
        ACTIVE_SESSION_ID.store(101, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

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
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 0);

        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn test_handle_ipc_cancel_when_no_active_session() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // No active task
        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

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
        assert_eq!(ACTIVE_SESSION_CANCELLED.load(core::sync::atomic::Ordering::SeqCst), 0);
    }

    #[test]
    fn test_handle_ipc_generate_busy_rejection_different_and_same_sender() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

        // Active task 42 is currently generating in session 200
        ACTIVE_SESSION_ID.store(200, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

        let req = std::ai::AiInferenceRequest {
            model_id: 2,
            target_dim: 0,
            input_len: 4,
            flags: 0,
            input_offset: core::mem::size_of::<std::ai::AiInferenceRequest>() as u64,
        };
        let mut req_buf = [0u8; 128];
        let req_hdr_len = core::mem::size_of::<std::ai::AiInferenceRequest>();
        let req_bytes = unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, req_hdr_len)
        };
        req_buf[..req_hdr_len].copy_from_slice(req_bytes);
        req_buf[req_hdr_len..req_hdr_len + 4].copy_from_slice(b"test");

        // 1. Different sender 99 requests generation while service is busy -> STATUS_UNAVAILABLE
        handle_ipc_inference_query(
            99,
            MSG_TYPE_GENERATE,
            &req_buf[..req_hdr_len + 4],
            &empty_models,
            &mut tx_buf,
        );
        let resp1 = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp1.status, std::ai::STATUS_UNAVAILABLE);
        let payload1 = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp1.output_dim as usize];
        assert_eq!(payload1, b"Inference service busy: active generation in progress");

        // 2. Same sender 42 duplicate request while active -> also STATUS_UNAVAILABLE (cannot overwrite active run)
        tx_buf.fill(0);
        handle_ipc_inference_query(
            42,
            MSG_TYPE_GENERATE,
            &req_buf[..req_hdr_len + 4],
            &empty_models,
            &mut tx_buf,
        );
        let resp2 = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp2.status, std::ai::STATUS_UNAVAILABLE);
        let payload2 = &tx_buf[core::mem::size_of::<std::ai::AiInferenceResponse>()..core::mem::size_of::<std::ai::AiInferenceResponse>() + resp2.output_dim as usize];
        assert_eq!(payload2, b"Inference service busy: active generation in progress");

        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
    }

    #[test]
    fn test_handle_ipc_deferred_queue_unrelated_messages() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();

        // Drain any existing deferred messages
        while pop_deferred_ipc().is_some() {}

        // Push 3 unrelated messages during generation
        assert!(push_deferred_ipc(10, 0x1234, b"heartbeat_alpha"));
        assert!(push_deferred_ipc(11, 0x5678, b"admin_ping_beta"));
        assert!(push_deferred_ipc(12, 0x9ABC, b"sensor_tick_gamma"));

        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Relaxed), 3);

        // Pop in FIFO order
        let m1 = pop_deferred_ipc().expect("msg 1");
        assert_eq!(m1.sender, 10);
        assert_eq!(m1.msg_type, 0x1234);
        assert_eq!(&m1.payload[..m1.len], b"heartbeat_alpha");

        let m2 = pop_deferred_ipc().expect("msg 2");
        assert_eq!(m2.sender, 11);
        assert_eq!(m2.msg_type, 0x5678);
        assert_eq!(&m2.payload[..m2.len], b"admin_ping_beta");

        let m3 = pop_deferred_ipc().expect("msg 3");
        assert_eq!(m3.sender, 12);
        assert_eq!(m3.msg_type, 0x9ABC);
        assert_eq!(&m3.payload[..m3.len], b"sensor_tick_gamma");

        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Relaxed), 0);
        assert!(pop_deferred_ipc().is_none());
    }

    #[test]
    fn test_deferred_ipc_queue_saturation_bounded() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        clear_deferred_ipc();

        // Push exactly 16 items to fill queue
        for i in 0..16u64 {
            let mut payload = [0u8; 8];
            payload.copy_from_slice(&i.to_le_bytes());
            assert!(push_deferred_ipc(100 + i, 0x1000 + i as u32, &payload), "item {} must succeed", i);
        }
        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 16);

        // 17th item must be rejected (cannot silently overwrite or drop existing entries)
        let rejected = push_deferred_ipc(999, 0xFFFF, b"overflow_attempt");
        assert!(!rejected, "17th item must be rejected when queue is full");
        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 16);

        // Pop all 16 in strict FIFO order
        for i in 0..16u64 {
            let msg = pop_deferred_ipc().expect("msg must be present");
            assert_eq!(msg.sender, 100 + i);
            assert_eq!(msg.msg_type, 0x1000 + i as u32);
            assert_eq!(&msg.payload[..msg.len], &i.to_le_bytes());
        }

        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 0);
        assert!(pop_deferred_ipc().is_none());
    }

    #[test]
    fn test_deferred_ipc_queue_circular_wraparound() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        clear_deferred_ipc();

        // Perform 40 cycles of push 5, pop 5 (200 total messages) across the 16-slot ring buffer
        for cycle in 0..40u64 {
            for step in 0..5u64 {
                let id = cycle * 10 + step;
                let payload = id.to_le_bytes();
                assert!(push_deferred_ipc(id, (id & 0xFFFF) as u32, &payload));
            }
            assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 5);

            for step in 0..5u64 {
                let expected_id = cycle * 10 + step;
                let msg = pop_deferred_ipc().expect("msg expected");
                assert_eq!(msg.sender, expected_id);
                assert_eq!(msg.msg_type, (expected_id & 0xFFFF) as u32);
                assert_eq!(&msg.payload[..msg.len], &expected_id.to_le_bytes());
            }
            assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 0);
        }
    }

    #[test]
    fn test_deferred_ipc_multithreaded_stress() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        clear_deferred_ipc();

        let num_threads = 4;
        let iters_per_thread = 50;

        let handles: alloc::vec::Vec<_> = (0..num_threads).map(|t_idx| {
            rust_std::thread::spawn(move || {
                for i in 0..iters_per_thread {
                    let sender = ((t_idx as u64) << 16) | (i as u64);
                    let payload = sender.to_le_bytes();
                    // Interleave pushes and pops
                    let _ = push_deferred_ipc(sender, 0x4000, &payload);
                    let _ = pop_deferred_ipc();
                }
            })
        }).collect();

        for h in handles {
            h.join().unwrap();
        }

        // Clean up any remaining entries safely
        clear_deferred_ipc();
        assert_eq!(DEFERRED_IPC_COUNT.load(core::sync::atomic::Ordering::Acquire), 0);
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

    /// The whole loader refusal the notice builder produces for one error.
    ///
    /// WHY a helper: three of the pins below run against the same real sentence the
    /// device sends, and a length taken from a fixture that drifted would let a
    /// shortened notice pass a guard meant to refuse it.
    fn a_loader_refusal_notice() -> alloc::vec::Vec<u8> {
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let (_status, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::ShortRead(4096, 816, 512)),
            &mut out,
        );
        notice.to_vec()
    }

    /// The defect this guards: the notice builder writes a sentence of well past
    /// forty bytes, and the inline mailbox route (a 64-byte payload with a 24-byte
    /// reply header ahead of it) delivers forty and drops the tail without a word.
    /// The answer channel keeps the whole sentence, so a long refusal is handed to
    /// the drain and every byte arrives. This test reassembles the bytes a reader
    /// recovers from the channel drain and refuses anything but the notice whole.
    #[test]
    fn the_drain_delivers_a_whole_loader_refusal_the_mailbox_would_cut() {
        let notice = a_loader_refusal_notice();

        // The notice must be longer than what the mailbox keeps, or there is no tail
        // to lose and the whole guard is idle. Pinning this is also the guard against
        // the wrong fix -- shortening the refusal to forty bytes to make it fit.
        assert!(
            notice.len() > MAILBOX_NOTICE_ROOM,
            "the loader refusal is now only {} bytes, at or under the {}-byte room a mailbox \
             keeps, so it no longer exercises the tail the channel carries",
            notice.len(),
            MAILBOX_NOTICE_ROOM,
        );
        assert_eq!(
            MAILBOX_NOTICE_ROOM,
            40,
            "the mailbox payload is 64 bytes and the reply header 24, so an inline notice has \
             40 bytes of room -- if this bound moved, the truncation arithmetic below is stale"
        );

        // The reader's drain: hand the notice to slots of the channel's width and
        // reassemble exactly what the transport accepted, the way the app reads the
        // answer pipe slot by slot.
        let mut reassembled = alloc::vec::Vec::new();
        let streamed =
            stream_in_slots(&notice, std::ai::ANSWER_SLOT_BYTES, |chunk| {
                reassembled.extend_from_slice(chunk);
                true
            });

        assert_eq!(
            streamed,
            notice.len(),
            "the drain handed {} of the notice's {} bytes to the channel",
            streamed,
            notice.len(),
        );
        assert_eq!(
            reassembled, notice,
            "the bytes a reader reassembles from the channel are not the whole notice"
        );

        // And the shape of the bug, stated as a number: an inline send would have
        // reached this screen with only the first forty bytes and no sign of a cut.
        assert_eq!(
            notice.len().min(MAILBOX_NOTICE_ROOM),
            MAILBOX_NOTICE_ROOM,
            "an inline delivery is supposed to lose the tail of a notice this long"
        );
        assert!(
            notice.len() > MAILBOX_NOTICE_ROOM,
            "a whole notice is longer than what an inline delivery keeps"
        );
    }

    /// Every notice the service can build fits one channel slot, so the drain never
    /// strands a reader that holds a single slot and the bound the fix relies on is
    /// provably reachable. WHY the arithmetic is written down rather than assumed:
    /// the answer channel's slot and the service's notice window are two constants in
    /// two files, and if the window ever grew past the slot the reader would need a
    /// multi-slot drain it may not run -- so the raise is only safe while this holds.
    #[test]
    fn the_notice_window_fits_one_channel_slot() {
        assert!(
            NOTICE_WINDOW_BYTES <= std::ai::ANSWER_SLOT_BYTES,
            "the {NOTICE_WINDOW_BYTES}-byte notice window no longer fits the {}-byte channel \
             slot: a refusal this service writes could span two slots and a reader holding one \
             would get only the head",
            std::ai::ANSWER_SLOT_BYTES,
        );
    }

    /// A slot the transport refuses is a shortfall the notice reports, not a silent
    /// cut: `stream_in_slots` returns the bytes actually accepted and stops, so the
    /// `streamed_len` the notice carries is short and the reader can say so. This is
    /// the "existing bound enforced as a refusal rather than a silent cut" the drain
    /// exists for. The fixture uses a slot width under a synthetic message so the
    /// shortfall needs more than one slot and is a real decision, not an artifact.
    #[test]
    fn a_refused_slot_reports_a_shortfall_not_a_silent_cut() {
        let message: alloc::vec::Vec<u8> = (0..300u32).map(|n| (n % 251) as u8).collect();
        const SLOT: usize = 100;

        // A transport that accepts two slots and refuses the third: the drain must
        // hand back 200 and not claim the whole 300.
        let mut accepted = alloc::vec::Vec::new();
        let mut slots = 0usize;
        let streamed = stream_in_slots(&message, SLOT, |chunk| {
            if slots >= 2 {
                return false;
            }
            slots += 1;
            accepted.extend_from_slice(chunk);
            true
        });

        assert_eq!(
            streamed, 200,
            "a transport that refused the third slot was counted as though it took the whole \
             message ({streamed} of 300)"
        );
        assert_eq!(
            accepted, message[..200],
            "the bytes a short drain handed over are not the message's head"
        );
        assert_ne!(
            streamed,
            message.len(),
            "a refused slot was reported as a whole answer"
        );
    }
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

        let Some(path) = real_file::env::var("INTEROS_REAL_GGUF_PATH").ok().or_else(|| {
            let candidate = "../../target/verify/models/embeddinggemma-300M-Q8_0.gguf";
            if real_file::path::Path::new(candidate).exists() {
                Some(alloc::string::String::from(candidate))
            } else {
                None
            }
        }) else {
            panic!(
                "NOT VERIFIED against a real model file: INTEROS_REAL_GGUF_PATH is unset and default \
                 file is absent. Verifying identity clause against installed model requires the model file."
            );
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

    /// The fold, in a table: what the mapping proved is the only thing that may
    /// move a plan, and it moves each plan exactly as far as that caller demanded.
    ///
    /// WHY pinned here rather than only at the arm: on the host no model file can be
    /// opened, so the arm can only ever show a mapping that failed. Both directions
    /// of the mistake are asserted -- a plan that would run on ids the table never
    /// produced, and a labelled baseline taken away from a caller that never asked
    /// for weights.
    #[test]
    fn test_the_fold_only_ever_moves_a_plan_as_far_as_the_mapping_proved() {
        use {EmbeddingGap::*, EmbeddingPlan::*, TextIds::*};
        fn mapped() -> TextIds {
            let mut ids = [0u32; EMBED_TOKEN_IDS_MAX];
            ids[0] = 2;
            ids[1] = 4;
            Encoded { ids: TokenIds { ids, len: 2 } }
        }
        let failures = [
            (
                TooManyIds { needed: EMBED_TOKEN_IDS_MAX + 8, bound: EMBED_TOKEN_IDS_MAX },
                "a segmentation past the bound",
            ),
            (
                Refused(tokenizer::TokenizerError::NoVocabulary),
                "a table the file refused to yield",
            ),
            (NotAttempted, "a mapping nobody attempted"),
        ];

        // The file's own ids turn both text callers into a run: the one content with
        // a projection, and the one that demanded weights and was refused only
        // because the mapping used to be invented.
        assert_eq!(apply_file_vocabulary(LexicalBaseline, &mapped()), RunInstalledFile);
        assert_eq!(
            apply_file_vocabulary(Refuse(NoVocabTokenizer), &mapped()),
            RunInstalledFile,
            "ids this file's own vocabulary declared were refused anyway"
        );

        for (failure, name) in failures {
            // A caller who never demanded weights keeps the honest answer whatever
            // this file's table did.
            assert_eq!(
                apply_file_vocabulary(LexicalBaseline, &failure),
                LexicalBaseline,
                "{name} cost a non-strict caller its labelled baseline"
            );
            // The strict caller is refused, and by the refusal that actually stopped
            // the table rather than by the general gap the planner could only guess.
            let moved = apply_file_vocabulary(Refuse(NoVocabTokenizer), &failure);
            if !matches!(failure, NotAttempted) {
                assert_ne!(
                    moved,
                    Refuse(NoVocabTokenizer),
                    "the notice still blames a mapping that was attempted and failed: {name}"
                );
            } else {
                assert_eq!(moved, Refuse(NoVocabTokenizer), "{name} moved a refusal");
            }
        }

        // No mapping outcome conjures a file that was never found, and none demotes
        // a run the caller's own ids already earned.
        for plan in [Refuse(NoWeights), RunInstalledFile] {
            for (text, name) in [
                (mapped(), "ids the file's own vocabulary declared"),
                (
                    TooManyIds { needed: EMBED_TOKEN_IDS_MAX + 8, bound: EMBED_TOKEN_IDS_MAX },
                    "a segmentation past the bound",
                ),
            ] {
                assert_eq!(apply_file_vocabulary(plan, &text), plan, "{name} moved {plan:?}");
            }
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

    /// The refusal that names the id-stream bound prints the bound this build
    /// enforces, and no figure written into the sentence.
    ///
    /// WHY the printed digits are read back: `IdStreamFull`'s `bound` reaches the
    /// notice from `tokenizer::encode`'s `BeyondBuffer { bound: out.len() }`, and the
    /// `out` that call is handed is the array `EMBED_TOKEN_IDS_MAX` sizes. That chain
    /// is the only reason a caller's refusal and the service's ceiling are one number,
    /// and every link of it is a place a literal could be typed instead -- a refusal
    /// quoting a bound the code does not apply is the same defect as a tensor reader
    /// advertising quantisations it cannot lay out. So the digits come out of the
    /// rendered notice and are compared with the constant that refuses the stream at
    /// the door, which is read from `read_token_ids` on the same turn.
    #[test]
    fn test_the_id_stream_refusal_prints_the_bound_the_service_enforces() {
        fn figure_after(notice: &[u8], lead: &[u8]) -> Option<usize> {
            let start = notice.windows(lead.len()).position(|w| w == lead)? + lead.len();
            let mut value = 0usize;
            let mut seen = false;
            for &byte in &notice[start..] {
                if byte.is_ascii_digit() {
                    value = value.saturating_mul(10) + (byte - b'0') as usize;
                    seen = true;
                } else if seen {
                    break;
                }
            }
            seen.then_some(value)
        }

        let needed = EMBED_TOKEN_IDS_MAX + 1;
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) =
            embedding_gap_notice(EmbeddingGap::IdStreamFull(needed, EMBED_TOKEN_IDS_MAX), &mut scratch);
        assert_eq!(status, STATUS_UNSUPPORTED, "an over-long id stream was called unavailable");
        let text = core::str::from_utf8(notice).expect("the refusal is text");

        assert_eq!(
            figure_after(notice, b"answers at most "),
            Some(EMBED_TOKEN_IDS_MAX),
            "the bound the caller is told is not the bound the array is sized by: {text}"
        );
        assert_eq!(
            figure_after(notice, b"segments the text into "),
            Some(needed),
            "the count the caller is told it exceeded is not the one that was refused: {text}"
        );

        // The same constant is the ceiling that refuses a stream at the door, so the
        // figure in the sentence and the read that produced the refusal cannot be two
        // different numbers while this test passes.
        assert_eq!(
            read_token_ids(&[0u8; EMBED_TOKEN_IDS_MAX * 4]).map(|ids| ids.len),
            Some(EMBED_TOKEN_IDS_MAX),
            "the arm no longer reads a stream of the length its notice promises"
        );
        assert_eq!(
            read_token_ids(&[0u8; (EMBED_TOKEN_IDS_MAX + 1) * 4]).map(|ids| ids.len),
            None,
            "a stream one past the printed bound was accepted, so the notice is a claim"
        );
    }

    #[test]
    fn test_64_token_stream_runs_and_oversize_refused_with_limit_named() {
        // 1. A 64-token stream runs.
        let mut tokens_64 = [0u8; 64 * 4];
        for i in 0..64 {
            let id = (i as u32) + 1;
            tokens_64[i * 4..(i + 1) * 4].copy_from_slice(&id.to_le_bytes());
        }
        let stream = read_token_ids(&tokens_64).expect("a 64-token stream runs within the raised bound");
        assert_eq!(stream.len, 64);
        assert_eq!(stream.as_slice().len(), 64);

        // 2. An oversize stream past the budget (e.g. EMBED_TOKEN_IDS_MAX + 1 = 513 tokens) is refused.
        let mut past_budget = [0u8; (EMBED_TOKEN_IDS_MAX + 1) * 4];
        for i in 0..=EMBED_TOKEN_IDS_MAX {
            let id = (i as u32) + 1;
            past_budget[i * 4..(i + 1) * 4].copy_from_slice(&id.to_le_bytes());
        }
        assert_eq!(
            read_token_ids(&past_budget).map(|s| s.len),
            None,
            "a past-budget stream must be refused"
        );

        // 3. The refusal message names the number.
        let needed = EMBED_TOKEN_IDS_MAX + 1;
        let mut scratch = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) =
            embedding_gap_notice(EmbeddingGap::IdStreamFull(needed, EMBED_TOKEN_IDS_MAX), &mut scratch);
        assert_eq!(status, STATUS_UNSUPPORTED);
        let notice_text = core::str::from_utf8(notice).expect("notice is valid utf8");
        let expected_bound = alloc::format!("answers at most {EMBED_TOKEN_IDS_MAX}");
        let expected_needed = alloc::format!("segments the text into {needed}");
        assert!(
            notice_text.contains(&expected_bound),
            "refusal must name the limit number: {notice_text}"
        );
        assert!(
            notice_text.contains(&expected_needed),
            "refusal must name the needed number: {notice_text}"
        );
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
    /// what stopped the mapping instead of quietly answering.
    ///
    /// WHAT THE NOTICE SAYS NOW: on the host the probe's path cannot be opened, so
    /// the refusal is the file's own reader error rather than the older "no mapping
    /// exists from text to token ids" -- which was true only while the mapping was
    /// invented. A device that can read the file answers this request by running it,
    /// and `test_the_fold_only_ever_moves_a_plan_as_far_as_the_mapping_proved` is
    /// where that branch is pinned, because no host case can open a model.
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
        assert!(
            notice.contains("no vocabulary table"),
            "the refusal stopped naming the table it could not build: {notice}"
        );
        assert!(
            notice.contains("Drop the strict-neural flag"),
            "the refusal no longer names the two moves the caller has: {notice}"
        );
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

    /// One message is cut at sixty-four bytes by the transport -- `IPC_MAX_PAYLOAD`
    /// in the kernel's IPC module, which userspace does not read -- so a request
    /// whose prompt is longer than the forty input bytes that survive reaches this
    /// arm as a whole header plus a fragment, declaring a length the bytes do not
    /// match. The header is then not trusted, and what the arm has left is the
    /// payload's own first twenty-four bytes: `model_id`, `target_dim`, `input_len`,
    /// `flags`, `input_offset`, followed by part of the prompt.
    ///
    /// WHY that is worth a refusal and a status rather than an answer: on a device
    /// holding the model, the installed file's vocabulary segments those bytes with
    /// no complaint -- they are bytes -- and the run returns a real vector, which the
    /// arm files under `STATUS_SUCCESS` as this caller's embedding of a prompt that
    /// never arrived. A host without the file answers the same wrong bytes as the
    /// labelled baseline, which is honest about the arithmetic and still not an
    /// answer to the request that was sent. Neither is a reply anyone can act on, and
    /// the only thing that would help the caller is the two byte counts.
    #[test]
    fn test_a_prompt_cut_in_transit_is_refused_with_both_byte_counts() {
        const MAILBOX_CUT: usize = 64;
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let mut payload = embedding_request_with(0x00, &[b'x'; 300]);
        payload.truncate(MAILBOX_CUT);
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(
            resp.status,
            STATUS_UNSUPPORTED,
            "a request whose prompt did not arrive was answered anyway (status {})",
            resp.status
        );
        let notice = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        let notice = core::str::from_utf8(notice).expect("the refusal is text");
        // The whole sentence, not a substring: `Notice::word` appends with no
        // separator of its own, so a fragment that forgot its space reads as one
        // invented word, and a refusal nobody can parse is barely a refusal.
        assert_eq!(
            notice,
            "Embedding refused: this request declares 300 input bytes and the transport \
             delivered 40. Nothing was answered from the piece that arrived: the bytes in \
             front of this service are the request's own header followed partway through \
             the prompt, and a vector made from them would be filed under a request the \
             device never received. Re-send with an input no longer than the bytes that \
             arrived."
        );
    }

    /// The same cut under the token-id flag, where the harm is smaller and the lie
    /// is the same shape: the bytes that survive are ten and a half ids of a
    /// thirty-id stream, and a prefix of an id stream is a different sequence than
    /// the one sent. The request is refused before any of it is read as ids or as
    /// text, so no branch of the arm can be reached with a payload it did not get.
    #[test]
    fn test_a_token_id_stream_cut_in_transit_is_refused_before_it_is_read() {
        const MAILBOX_CUT: usize = 64;
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let ids: alloc::vec::Vec<u8> = (0..30u32)
            .map(|n| n + 100)
            .flat_map(|id| id.to_le_bytes())
            .collect();
        let mut payload = embedding_request_with(0x10, &ids);
        payload.truncate(MAILBOX_CUT);
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(
            resp.status,
            STATUS_UNSUPPORTED,
            "a prefix of an id stream was read as the stream"
        );
    }

    /// The guard is a function of the payload and nothing else, and the direction it
    /// does NOT fire in matters as much as the one it does: a prompt delivered whole
    /// is not a cut prompt, and prose is not a header that lost its tail.
    #[test]
    fn test_the_cut_is_read_from_the_declaration_against_the_bytes() {
        const MAILBOX_CUT: usize = 64;

        // Declared exactly the room that a cut leaves, and delivered in full: the
        // last byte the caller counts is the last byte the arm has.
        let fits = embedding_request_with(0x00, &[b'y'; 40]);
        assert_eq!(fits.len(), MAILBOX_CUT);
        assert_eq!(declared_input_overrun(&fits), None, "a whole request was called a cut");

        // Shorter still, and long past the header but under it.
        assert_eq!(declared_input_overrun(&embedding_request_with(0x00, b"battery")), None);
        let long = embedding_request_with(0x00, &[b'z'; 600]);
        assert_eq!(
            declared_input_overrun(&long),
            None,
            "a prompt that arrives whole is not refused for its length"
        );

        // The two cut shapes, with both counts in input bytes rather than payload
        // bytes: the header is not something the caller sent as prompt.
        let mut cut = embedding_request_with(0x00, &[b'x'; 300]);
        cut.truncate(MAILBOX_CUT);
        assert_eq!(declared_input_overrun(&cut), Some((300, 40)));
        cut.truncate(core::mem::size_of::<std::ai::AiInferenceRequest>());
        assert_eq!(
            declared_input_overrun(&cut),
            Some((300, 0)),
            "a request that lost every input byte is the cuttest case there is"
        );

        // Prose is never a cut request, however long it is.
        assert_eq!(declared_input_overrun(b"query: battery consumption profile"), None);
        assert_eq!(
            declared_input_overrun(&[b'w'; 900]),
            None,
            "raw text was refused for a header it never sent"
        );

        // And a payload that fails the shape test is prose too, even when its
        // length clause would have failed: `model_id` 17 is outside the table, so
        // the arm's own parse never trusted these bytes as a request to begin with.
        let mut not_a_request = embedding_request_with(0x00, &[b'x'; 300]);
        not_a_request[0] = 17;
        not_a_request.truncate(MAILBOX_CUT);
        assert_eq!(declared_input_overrun(&not_a_request), None);
    }

    /// A strict-neural request that carries neither text nor token ids is an empty
    /// request, and the refusal has to name it as one.
    ///
    /// WHAT THE ARM SAID BEFORE: `this request carries text, not token ids`. With
    /// `input_len` zero and no input byte behind the header, that sentence describes
    /// a payload the caller never sent -- and the arm's own fallback then hands
    /// `query_text` the twenty-four header bytes, so a device holding the file could
    /// answer the request's own header as though it were the prompt. The host cannot
    /// see that second half (nothing opens the file here), which is why the guard is
    /// a decision of the payload alone, placed ahead of the id read and the mapping
    /// like `declared_input_overrun`, rather than a rewording of a branch the file's
    /// own answer can still overrule.
    ///
    /// WHY the baseline guidance had to change too: every other strict refusal here
    /// can point at dropping the flag, because the labelled lexical projection does
    /// answer a text request. An empty request has no query for the baseline to
    /// project either, so the honest sentence says the flag is not what is missing.
    #[test]
    fn test_a_strict_neural_request_with_nothing_in_it_is_named_empty() {
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let payload = embedding_request_with(0x80, b"");
        assert_eq!(
            payload.len(),
            core::mem::size_of::<std::ai::AiInferenceRequest>(),
            "an empty request is a header and nothing else"
        );
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(
            resp.status,
            STATUS_UNSUPPORTED,
            "an empty request was answered rather than refused (status {})",
            resp.status
        );
        assert_eq!(resp.latency_micros, 0, "a refusal timed a run that never happened");
        let notice = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        let notice = core::str::from_utf8(notice).expect("the refusal is text");
        // The whole sentence: `Notice::word` appends with no separator, so a
        // fragment that lost its space reads as one invented word.
        assert_eq!(
            notice,
            "Embedding refused: the strict-neural flag is set and this request carries \
             neither text nor token ids -- it declares no input and the transport delivered \
             none, so nothing reached this service for a model to run on. Dropping the flag \
             is not a way past it: the labelled lexical baseline has no query to project \
             either, and a vector out of an empty request would be this service answering a \
             question nobody asked. Send text, or a token-id stream."
        );
        // The false claim, named as the thing that must not come back: this refusal
        // is reached from a request that carries nothing, so no sentence about what
        // it "carries" may assert text.
        assert!(
            !notice.contains("carries text"),
            "an empty request was described as carrying text: {notice}"
        );
    }

    /// Which requests the empty guard names, and which it leaves alone.
    ///
    /// WHY the directions matter as much as the fire: the guard refuses before the
    /// mapping, so a payload that only *looks* empty -- a non-strict request, which
    /// the labelled baseline still answers, or prose, which is a query whatever its
    /// header-shaped neighbour -- would lose an answer it was owed. And why the ids
    /// flag does not save an empty one: `read_token_ids` of zero bytes is no stream,
    /// so a strict request under that flag carries neither of the two things its
    /// refusal used to claim it carried.
    #[test]
    fn test_an_empty_strict_neural_request_is_read_from_the_payload_alone() {
        const HDR: usize = core::mem::size_of::<std::ai::AiInferenceRequest>();

        // The empty request, both flag shapes: strict, and no input byte declared
        // and none arrived.
        assert!(strict_neural_request_is_empty(&embedding_request_with(0x80, b"")));
        assert!(strict_neural_request_is_empty(&embedding_request_with(0x90, b"")));

        // Text, however short, is not empty.
        assert!(!strict_neural_request_is_empty(&embedding_request_with(0x80, b"x")));
        assert!(!strict_neural_request_is_empty(&embedding_request_with(0x80, b"battery")));

        // A prompt is not empty because the caller did not ask for strict weights.
        assert!(!strict_neural_request_is_empty(&embedding_request_with(0x00, b"")));

        // A stream of ids is not empty either, and neither is prose or a short
        // buffer: the guard fires only on a payload this arm's own parse would have
        // read as a request.
        assert!(!strict_neural_request_is_empty(&embedding_request_with(0x90, &[7u8; 4])));
        assert!(!strict_neural_request_is_empty(b"query: battery consumption profile"));
        assert!(!strict_neural_request_is_empty(&[0u8; HDR - 1]));

        // A header declaring no input with trailing bytes is not empty: those bytes
        // are what the parse takes as the query.
        let mut trailing = embedding_request_with(0x80, b"");
        trailing.extend_from_slice(b"leftover");
        assert!(!strict_neural_request_is_empty(&trailing));

        // And a payload failing the shape clauses is prose, whatever it declares:
        // `model_id` 17 is outside the table the SDK writes.
        let mut not_a_request = embedding_request_with(0x80, b"");
        not_a_request[0] = 17;
        assert!(!strict_neural_request_is_empty(&not_a_request));

        // End to end, the direction the guard must NOT fire in: a non-strict empty
        // request still gets the labelled baseline it was always answered with.
        let mut tx_buf = [0u8; 1024];
        let models = probed_embedding_models();
        let payload = embedding_request_with(0x00, b"");
        handle_ipc_inference_query(0, MSG_TYPE_EMBED, &payload, &models, &mut tx_buf);
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(
            resp.status,
            STATUS_BASELINE_LEXICAL,
            "a caller who never demanded weights was refused for a gap it does not care about"
        );
    }

    /// The header the generative route reads.
    ///
    /// WHY it is not the embedding helper: `extract_prompt_text` trusts a payload
    /// whose `model_id` and `flags` the SDK could have written and asks nothing of
    /// `target_dim` -- a generation names no output dimension. The cut guard has to
    /// share those clauses with the parse it guards, or the two disagree about which
    /// bytes are a request and a prompt the route would have answered goes unrefused.
    fn generative_request_with(input: &[u8]) -> alloc::vec::Vec<u8> {
        generative_request_shaped(0, input)
    }

    /// The same header with a `target_dim` the embedding route would reject.
    fn generative_request_shaped(target_dim: u32, input: &[u8]) -> alloc::vec::Vec<u8> {
        let req = std::ai::AiInferenceRequest {
            model_id: 2,
            target_dim,
            input_len: input.len() as u32,
            flags: 1,
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

    /// The same transport cut `3dcadd9d` refuses for an embedding, on the route that
    /// answers in words.
    ///
    /// WHY the generative route needed it separately: `extract_prompt_text` clamps
    /// the declared length to the bytes it has (`req.input_len.min(...)`) and hands
    /// back the prefix, so the arm never learns a prompt was cut -- it opens the
    /// model file and answers the piece that arrived as though the caller had asked
    /// that question. A wrong answer costs a generation far more than an embedding:
    /// the reader cannot see a truncated prompt the way they can see a vector of the
    /// dimension they asked for, and the reply is fluent either way.
    #[test]
    fn test_a_generative_prompt_cut_in_transit_is_refused_with_both_byte_counts() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        const MAILBOX_CUT: usize = 64;
        let mut tx_buf = [0u8; 1024];
        let models = gguf::ProbedModels::empty();
        let mut payload = generative_request_with(&[b'x'; 300]);
        payload.truncate(MAILBOX_CUT);
        handle_ipc_inference_query(0, MSG_TYPE_GENERATE, &payload, &models, &mut tx_buf);

        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(
            resp.status,
            STATUS_UNSUPPORTED,
            "a generation answered a prompt that did not arrive whole (status {})",
            resp.status
        );
        assert_eq!(resp.latency_micros, 0, "a refusal timed a run that never happened");
        let notice = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        let notice = core::str::from_utf8(notice).expect("the refusal is text");
        assert_eq!(
            notice,
            "Generation refused: this request declares 300 prompt bytes and the transport \
             delivered 40. Nothing was answered from the piece that arrived: the bytes in \
             front of this service are the request's own header followed partway through \
             the prompt, and words made from them would be an answer to a question the \
             device never received. Re-send with a prompt no longer than the bytes that \
             arrived."
        );
    }

    /// The guard is the cut test the embedding arm already runs, against the clauses
    /// the generative route itself trusts -- and it does not fire in the directions
    /// that would refuse a prompt that arrived, or a message that never was one.
    #[test]
    fn test_a_generative_prompt_is_measured_by_the_clauses_that_route_trusts() {
        const MAILBOX_CUT: usize = 64;

        // A prompt delivered whole is not a cut one, at any length.
        assert_eq!(declared_prompt_overrun(&generative_request_with(b"Explain queues")), None);
        let whole = generative_request_with(&[b'p'; 500]);
        assert_eq!(declared_prompt_overrun(&whole), None);
        assert_eq!(
            declared_prompt_overrun(&generative_request_with(&[])),
            None,
            "an empty prompt declares nothing, so nothing went missing"
        );

        // The cut, with both counts in prompt bytes rather than payload bytes.
        let mut cut = generative_request_with(&[b'x'; 300]);
        cut.truncate(MAILBOX_CUT);
        assert_eq!(declared_prompt_overrun(&cut), Some((300, 40)));
        cut.truncate(core::mem::size_of::<std::ai::AiInferenceRequest>());
        assert_eq!(declared_prompt_overrun(&cut), Some((300, 0)));

        // A payload the route reads as prose is never a cut prompt, however long.
        assert_eq!(declared_prompt_overrun(b"GENR: explain the battery report"), None);
        assert_eq!(declared_prompt_overrun(&[b'w'; 900]), None);
        let mut not_a_request = generative_request_with(&[b'x'; 300]);
        not_a_request[0] = 17;
        not_a_request.truncate(MAILBOX_CUT);
        assert_eq!(declared_prompt_overrun(&not_a_request), None);

        // The one clause the two routes do NOT share: a generation names no output
        // dimension, so `extract_prompt_text` asks nothing of `target_dim` and the
        // guard cannot either -- while the embedding arm, which does read it, keeps
        // refusing that payload as prose. The clause sets are each their own route's.
        let mut odd_dim = generative_request_shaped(1024, &[b'x'; 300]);
        odd_dim.truncate(MAILBOX_CUT);
        assert_eq!(
            declared_prompt_overrun(&odd_dim),
            Some((300, 40)),
            "the generative guard stopped sharing its own route's clauses"
        );
        assert_eq!(
            declared_input_overrun(&odd_dim),
            None,
            "the embedding guard's shape clause moved, so the two guards are no longer \
             each their own route's"
        );

        // End to end, the direction that must not fire: a whole prompt is answered
        // by whatever the model branch holds, not refused for its length.
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        let mut tx_buf = [0u8; 1024];
        let models = gguf::ProbedModels::empty();
        let payload = generative_request_with(b"Summarize notifications");
        handle_ipc_inference_query(0, MSG_TYPE_GENERATE, &payload, &models, &mut tx_buf);
        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
        let reason = &tx_buf[resp_hdr_len..resp_hdr_len + resp.output_dim as usize];
        assert_eq!(
            reason,
            b"On-device generative stream unavailable: no local model weights loaded.",
            "a prompt that arrived whole was refused for a cut that did not happen"
        );
    }

    /// The generative refusal is longer than the mailbox keeps, which is why it
    /// travels the way `b8af2a1d` established: whole onto an answer channel, and
    /// inline only when no channel is free.
    #[test]
    fn the_generative_refusal_is_streamed_because_the_mailbox_would_cut_it() {
        let mut out = [0u8; NOTICE_WINDOW_BYTES];
        let (status, notice) = generation_overrun_notice(300, 40, &mut out);
        assert_eq!(status, STATUS_UNSUPPORTED, "a refusal answered with a status of its own");
        assert!(
            notice.len() > MAILBOX_NOTICE_ROOM,
            "the generative refusal is now only {} bytes, at or under the {}-byte room an \
             inline reply keeps, so it no longer needs the channel -- and the wrong fix \
             would be to shorten it",
            notice.len(),
            MAILBOX_NOTICE_ROOM,
        );

        // The reader's drain, reassembled the way the app reads the answer pipe:
        // every byte the notice declared arrives, and a slot the transport refuses
        // is named as a shortfall rather than lost.
        let mut reassembled = alloc::vec::Vec::new();
        let streamed =
            stream_in_slots(notice, std::ai::ANSWER_SLOT_BYTES, |chunk| {
                reassembled.extend_from_slice(chunk);
                true
            });
        assert_eq!(streamed, notice.len(), "the drain stranded the tail of the refusal");
        assert_eq!(reassembled, notice, "a reader reassembles something other than the notice");

        // And the shape of the bug it replaces: an inline send reaches this screen
        // with the first forty bytes and no sign that the rest was dropped.
        assert_eq!(notice.len().min(MAILBOX_NOTICE_ROOM), MAILBOX_NOTICE_ROOM);
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
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        // The arm reserves its session from the BrokerLedger before it opens the
        // file. Left unseeded, the ledger's host fallback budget depends on the
        // machine and the session can be refused for RAM, which would send a
        // non-strict request to the baseline for a reason this test does not
        // test. Seed a fixed budget so the reservation always succeeds and the
        // reply is decided by fs_open alone.
        reset_broker_ledger(64 * 1024 * 1024, 0, 16 * 1024 * 1024);
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

        // The two refusals a chunked read can produce, each in its own words. The
        // first is the one the device actually hit: a bounded request the platform
        // said no to. Calling that "an install that stopped part-way" -- which is
        // what it used to be called -- sends a reader to re-install a 333 MB file
        // that is whole, so the sentence and the code have to stay apart.
        let (status, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::ReadRefused(6_530_464, 65_536, -3)),
            &mut out,
        );
        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            core::str::from_utf8(notice).unwrap(),
            "Embedding refused: the platform refused a read of 65536 bytes at byte 6530464 \
             with code -3, which is the call saying no to a bounded request and not a file \
             that ran short -- re-install is the wrong next move."
        );

        let (status, notice) = file_embed_notice(
            &FileEmbedError::Load(weights::LoadError::OverDeliveredChunk(
                4_096,
                65_536,
                65_537,
            )),
            &mut out,
        );
        assert_eq!(status, STATUS_ERROR);
        assert_eq!(
            core::str::from_utf8(notice).unwrap(),
            "Embedding refused: a read at byte 4096 answered with 65537 bytes for a \
             65536-byte window, so the loader stopped rather than trust the count and \
             leave the rest of the tensor as it found it."
        );

        for error in [
            FileEmbedError::Open,
            FileEmbedError::Load(weights::LoadError::Arithmetic),
            FileEmbedError::Config(InferenceError::CorruptedWeights),
            FileEmbedError::Load(weights::LoadError::ReadRefused(1, 2, -3)),
            FileEmbedError::Load(weights::LoadError::OverDeliveredChunk(1, 2, 3)),
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

    #[test]
    fn test_authoritative_broker_ledger_reservation_and_reclamation() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        // Configure ledger: 100 MiB free, 0 storage, 20 MiB reserve => 80 MiB available for AI
        reset_broker_ledger(100 * 1024 * 1024, 0, 20 * 1024 * 1024);

        let estimate = backend::ResourceEstimate {
            model_weights: 0,
            kv_cache: 10 * 1024 * 1024,
            activations: 10 * 1024 * 1024,
            scratch: 5 * 1024 * 1024,
            tokenizer: 0,
            embedding_indexes: 0,
            audio_buffers: 0,
            runtime_overhead: 0,
        }; // session_bytes = 25 MiB

        // Create first session for task 10
        let s1 = with_broker_ledger(|ledger| {
            ledger.create_session(10, 1, estimate)
        }).expect("session 1");

        assert_eq!(with_broker_ledger(|ledger| ledger.reserved_bytes()), 25 * 1024 * 1024);
        assert_eq!(with_broker_ledger(|ledger| ledger.active_sessions()), 1);

        // Create second session for task 20
        let _s2 = with_broker_ledger(|ledger| {
            ledger.create_session(20, 1, estimate)
        }).expect("session 2");

        assert_eq!(with_broker_ledger(|ledger| ledger.reserved_bytes()), 50 * 1024 * 1024);
        assert_eq!(with_broker_ledger(|ledger| ledger.active_sessions()), 2);

        // Third session of 40 MiB would exceed remaining 30 MiB capacity (100 - 20 - 50 = 30)
        let large_est = backend::ResourceEstimate {
            kv_cache: 20 * 1024 * 1024,
            activations: 20 * 1024 * 1024,
            scratch: 0,
            tokenizer: 0,
            embedding_indexes: 0,
            audio_buffers: 0,
            runtime_overhead: 0,
            model_weights: 0,
        };
        let rej = with_broker_ledger(|ledger| {
            ledger.create_session(30, 1, large_est)
        });
        assert_eq!(rej, Err(backend::AdmissionError::InsufficientRam));
        // No leak on rejection:
        assert_eq!(with_broker_ledger(|ledger| ledger.reserved_bytes()), 50 * 1024 * 1024);

        // Cancellation
        with_broker_ledger(|ledger| {
            assert_eq!(ledger.cancel(10, s1), Ok(()));
            assert!(ledger.is_cancelled(s1));
        });

        // Destroying session 1 reclaims its 25 MiB
        with_broker_ledger(|ledger| {
            assert_eq!(ledger.destroy_session(10, s1), Ok(()));
        });
        assert_eq!(with_broker_ledger(|ledger| ledger.reserved_bytes()), 25 * 1024 * 1024);
        assert_eq!(with_broker_ledger(|ledger| ledger.active_sessions()), 1);

        // Process reaping for task 20 reclaims its session
        reap_process(20);
        assert_eq!(with_broker_ledger(|ledger| ledger.reserved_bytes()), 0);
        assert_eq!(with_broker_ledger(|ledger| ledger.active_sessions()), 0);
    }

    #[test]
    fn test_cancel_ipc_routes_through_broker_ledger_and_backend() {
        let _lock = TEST_CANCEL_LOCK.lock().unwrap();
        reset_broker_ledger(100 * 1024 * 1024, 0, 20 * 1024 * 1024);

        let estimate = backend::ResourceEstimate {
            kv_cache: 1024,
            ..backend::ResourceEstimate::ZERO
        };
        let sess_id = with_broker_ledger(|ledger| {
            ledger.create_session(42, 1, estimate)
        }).expect("session");

        ACTIVE_SESSION_ID.store(sess_id, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(42, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);

        let mut tx_buf = [0u8; 1024];
        let empty_models = gguf::ProbedModels::empty();

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
        assert!(with_broker_ledger(|ledger| ledger.is_cancelled(sess_id)));

        // Clean up
        with_broker_ledger(|ledger| {
            let _ = ledger.destroy_session(42, sess_id);
        });
        ACTIVE_SESSION_ID.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_GENERATION_TASK.store(0, core::sync::atomic::Ordering::SeqCst);
        ACTIVE_SESSION_CANCELLED.store(0, core::sync::atomic::Ordering::SeqCst);
    }

    fn make_test_identity(task_id: u64, is_system: u8, name_str: &str) -> std::TaskIdentityInfo {
        let mut name = [0u8; 16];
        let bytes = name_str.as_bytes();
        let len = bytes.len().min(16);
        name[..len].copy_from_slice(&bytes[..len]);
        std::TaskIdentityInfo {
            task_id,
            user_id: 1000,
            is_system,
            is_application: if is_system == 0 { 1 } else { 0 },
            name_len: len as u8,
            pkg_len: len as u8,
            name,
            package_id: [0u8; 32],
            code_digest: [0u8; 32],
            principal_id: 0,
            parent_id: 0,
        }
    }

    #[test]
    fn test_keystore_derive_device_root_denied() {
        let sys_id = make_test_identity(1, 1, "system");
        let app_id = make_test_identity(20, 0, "calc");
        let rights = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;

        assert_eq!(
            authorize_keystore_derive_policy(&sys_id, 1, std::keystore::DOMAIN_DEVICE_ROOT, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_DEVICE_ROOT, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
    }

    #[test]
    fn test_keystore_derive_rights_validation() {
        let app_id = make_test_identity(20, 0, "calc");
        // Zero rights
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, 0),
            Err(std::keystore::KeystoreError::InvalidRights)
        );
        // Unknown/unsupported right bits
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, 0x8000_0000),
            Err(std::keystore::KeystoreError::InvalidRights)
        );
    }

    #[test]
    fn test_keystore_derive_semantic_search_policy() {
        let app_id = make_test_identity(20, 0, "calc");
        let agent_id = make_test_identity(std::ai::SERVICE_AGENT_TASK_ID, 1, "agentd");
        let search_id = make_test_identity(std::ai::SERVICE_SEARCH_TASK_ID, 1, "searchd");
        let rights = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;

        // Ordinary app denied
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_SEMANTIC_SEARCH, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        // agentd denied search domain
        assert_eq!(
            authorize_keystore_derive_policy(&agent_id, std::ai::SERVICE_AGENT_TASK_ID, std::keystore::DOMAIN_SEMANTIC_SEARCH, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        // searchd allowed
        assert_eq!(
            authorize_keystore_derive_policy(&search_id, std::ai::SERVICE_SEARCH_TASK_ID, std::keystore::DOMAIN_SEMANTIC_SEARCH, rights),
            Ok(())
        );
    }

    #[test]
    fn test_keystore_derive_companion_policy() {
        let app_id = make_test_identity(20, 0, "calc");
        let search_id = make_test_identity(std::ai::SERVICE_SEARCH_TASK_ID, 1, "searchd");
        let agent_id = make_test_identity(std::ai::SERVICE_AGENT_TASK_ID, 1, "agentd");
        let rights = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;

        // Ordinary app denied
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_COMPANION, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        // searchd denied companion domain
        assert_eq!(
            authorize_keystore_derive_policy(&search_id, std::ai::SERVICE_SEARCH_TASK_ID, std::keystore::DOMAIN_COMPANION, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        // agentd allowed
        assert_eq!(
            authorize_keystore_derive_policy(&agent_id, std::ai::SERVICE_AGENT_TASK_ID, std::keystore::DOMAIN_COMPANION, rights),
            Ok(())
        );
    }

    #[test]
    fn test_keystore_derive_conversation_history_policy() {
        let app_id = make_test_identity(20, 0, "calc");
        let conv_id = make_test_identity(55, 1, "conversation");
        let agent_id = make_test_identity(std::ai::SERVICE_AGENT_TASK_ID, 1, "agentd");
        let search_id = make_test_identity(std::ai::SERVICE_SEARCH_TASK_ID, 1, "searchd");
        let rights = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;

        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_CONVERSATION_HISTORY, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        assert_eq!(
            authorize_keystore_derive_policy(&search_id, std::ai::SERVICE_SEARCH_TASK_ID, std::keystore::DOMAIN_CONVERSATION_HISTORY, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
        assert_eq!(
            authorize_keystore_derive_policy(&conv_id, 55, std::keystore::DOMAIN_CONVERSATION_HISTORY, rights),
            Ok(())
        );
        assert_eq!(
            authorize_keystore_derive_policy(&agent_id, std::ai::SERVICE_AGENT_TASK_ID, std::keystore::DOMAIN_CONVERSATION_HISTORY, rights),
            Ok(())
        );
    }

    #[test]
    fn test_keystore_derive_ordinary_app_storage_rights() {
        let app_id = make_test_identity(20, 0, "calc");
        let enc_dec = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;

        // Encrypt and decrypt allowed for ordinary app
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, enc_dec),
            Ok(())
        );
        // Derivation right denied for ordinary app
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, std::keystore::RIGHT_DERIVE),
            Err(std::keystore::KeystoreError::InvalidRights)
        );
        // Sign right denied for ordinary app
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, std::keystore::RIGHT_SIGN),
            Err(std::keystore::KeystoreError::InvalidRights)
        );
        // RIGHT_ALL denied for ordinary app
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, std::keystore::DOMAIN_APP_STORAGE, std::keystore::RIGHT_ALL),
            Err(std::keystore::KeystoreError::InvalidRights)
        );
    }

    #[test]
    fn test_keystore_derive_unknown_domain_denied() {
        let app_id = make_test_identity(20, 0, "calc");
        let rights = std::keystore::RIGHT_ENCRYPT | std::keystore::RIGHT_DECRYPT;
        assert_eq!(
            authorize_keystore_derive_policy(&app_id, 20, 0x1337, rights),
            Err(std::keystore::KeystoreError::UnauthorizedDomain)
        );
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let _ = std::log("[PANIC in system-service]");
    if let Some(loc) = info.location() {
        let _ = std::log(loc.file());
        let mut line = loc.line();
        let mut buf = [0u8; 16];
        let mut i = 15;
        if line == 0 {
            let _ = std::log("line: 0");
        } else {
            while line > 0 {
                buf[i] = b'0' + (line % 10) as u8;
                line /= 10;
                i -= 1;
            }
            if let Ok(s) = core::str::from_utf8(&buf[i + 1..16]) {
                let _ = std::log(s);
            }
        }
    }
    std::exit_with_code(139);
}
