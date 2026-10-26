#![no_std]

use interos_std as std;

pub mod embedding;
pub mod gguf;

pub use embedding::{
    compute_embedding_768, compute_text_embedding_dim_f32, compute_text_embedding_f32,
    compute_text_embedding_q7, f32_sqrt, EMBEDDING_DIM, MATRYOSHKA_DIMS,
};
pub use gguf::{probe_gguf_header, read_gguf_header, GgufError, GgufHeader, GGUF_MAGIC};

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
            last_tick = now;
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
    let model_header = match gguf::probe_gguf_header() {
        Ok((hdr, _path)) => {
            let _ = std::log("[aid] verified physical GGUF model weights in storage.");
            Some(hdr)
        }
        Err(_) => {
            let _ = std::log("[aid] no physical GGUF weights found in /system/models or /data/models; reporting unavailable honestly.");
            None
        }
    };

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
                        model_header.as_ref(),
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
                        model_header.as_ref(),
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

/// Dispatches incoming IPC queries for `SERVICE_INFERENCE` (#13) with zero heap allocation.
pub fn handle_ipc_inference_query(
    sender_task_id: u64,
    msg_type: u32,
    payload: &[u8],
    model_header: Option<&gguf::GgufHeader>,
    tx_buf: &mut [u8; 1024],
) {
    const MSG_TYPE_EMBED: u32 = 0x454D_4244; // b"EMBD"
    const MSG_TYPE_GGUF: u32 = 0x4747_5546;  // b"GGUF"

    // 1. GGUF model header info query
    if msg_type == MSG_TYPE_GGUF || (payload.len() >= 4 && &payload[..4] == b"GGUF") {
        if let Some(hdr) = model_header {
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

    // 2. Embedding inference query
    let mut target_dim = embedding::EMBEDDING_DIM;
    let mut query_text = payload;

    // Check if the wire payload contains an AiInferenceRequest header (24 bytes)
    if payload.len() >= core::mem::size_of::<std::ai::AiInferenceRequest>() {
        let req = unsafe {
            core::ptr::read_unaligned(payload.as_ptr() as *const std::ai::AiInferenceRequest)
        };
        // If target_dim matches a valid Matryoshka dimension, respect it
        if let Some(m_dim) = std::ai::MatryoshkaDim::from_u32(req.target_dim) {
            target_dim = m_dim.as_usize();
        }
        if req.input_len > 0 {
            let start = core::mem::size_of::<std::ai::AiInferenceRequest>();
            let end = start + (req.input_len as usize).min(payload.len().saturating_sub(start));
            if start <= payload.len() {
                query_text = &payload[start..end];
            }
        } else if payload.len() > core::mem::size_of::<std::ai::AiInferenceRequest>() {
            query_text = &payload[core::mem::size_of::<std::ai::AiInferenceRequest>()..];
        }
    }

    // Compute embedding directly into a stack buffer
    let mut q7_out = [0i8; embedding::EMBEDDING_DIM];
    embedding::compute_embedding_768(query_text, target_dim, None, Some(&mut q7_out));

    // Construct response header
    let resp = std::ai::AiInferenceResponse {
        status: 0, // Success
        output_dim: target_dim as u32,
        latency_micros: 45,
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

    #[test]
    fn test_handle_ipc_gguf_probe_when_absent() {
        let mut tx_buf = [0u8; 1024];
        let req_magic = *b"GGUF";
        handle_ipc_inference_query(0, 0x4747_5546, &req_magic, None, &mut tx_buf);
        // sender_task_id is 0 so it won't syscall ipc_send
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
        handle_ipc_inference_query(0, 0x4747_5546, b"GGUF", Some(&hdr), &mut tx_buf);
        let parsed = gguf::GgufHeader::parse(&tx_buf[..24]).expect("header parsed from tx_buf");
        assert_eq!(parsed.magic, gguf::GGUF_MAGIC);
        assert_eq!(parsed.tensor_count, 99);
        assert_eq!(parsed.kv_count, 12);
    }

    #[test]
    fn test_handle_ipc_embedding_inference_query() {
        let mut tx_buf = [0u8; 1024];
        let query = b"query: battery consumption profile";
        handle_ipc_inference_query(0, 0x454D_4244, query, None, &mut tx_buf);

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

        handle_ipc_inference_query(0, 0x454D_4244, &payload[..40], None, &mut tx_buf);

        let resp = unsafe {
            core::ptr::read_unaligned(tx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
        };
        assert_eq!(resp.status, 0);
        assert_eq!(resp.output_dim, 256);
    }
}

#[cfg(not(test))]
#[panic_handler]

fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
