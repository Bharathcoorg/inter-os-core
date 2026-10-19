#![no_std]

use interos_std as std;

pub fn run(service_id: u8, name: &str) -> ! {
    let _ = std::log(0, name);
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    loop {
        let _ = std::service_heartbeat(service_id);
        #[cfg(feature = "ci-fault-injection")]
        if service_id == 8 {
            if let Some(slot) = timer_slot {
                if let Ok(ticks) = std::get_ticks(slot) {
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
        if let Some(slot) = timer_slot {
            let _ = std::get_ticks(slot);
        }
    }
}

/// Display-service execution path. The only Ring-3 task holding the
/// FrameBufferDirect object-1 capability owns the hardware-facing display ABI.
pub fn run_display() -> ! {
    let _ = std::log(0, "[displayd] userspace display manager online.");
    let surface = match std::surface_create(4, 320, 480, 0) {
        Ok(id) => id,
        Err(_) => {
            let _ = std::log(0, "[displayd] display surface creation rejected.");
            std::exit_with_code(1);
        }
    };
    let pixels = [0xFF202830u32, 0xFF202830, 0xFF202830, 0xFF202830];
    let _ = std::surface_write(4, surface, 0, &pixels);
    let _ = std::surface_flip(4);
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    loop {
        let _ = std::service_heartbeat(1);
        let now = timer_slot.and_then(|s| std::get_ticks(s).ok()).unwrap_or(last_tick);
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
    let _ = std::log(0, "[inputd] userspace input manager online.");
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
            let _ = std::log(0, "[inputd] normalized input event received.");
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
    let _ = std::log(0, "[netd] userspace network manager online.");
    let socket = match std::net_socket(4242) {
        Ok(id) => id,
        Err(_) => {
            let _ = std::log(0, "[netd] socket creation rejected.");
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

        let now = timer_slot.and_then(|s| std::get_ticks(s).ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            let mut drained = 0usize;
            if let Ok(len) = std::net_recvfrom(socket, &mut rx) {
                drained += len;
            }
            if drained > 0 {
                let _ = std::log(0, "[netd] loopback datapath drained; no network device is attached.");
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
    let _ = std::log(0, "[powerd] userspace power manager online.");
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
    let _ = std::log(0, "[modemd] userspace telephony manager online.");
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
    let _ = std::log(0, "[camerad] userspace camera manager online.");
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
    let _ = std::log(0, "[audiod] userspace audio manager online.");
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
    let _ = std::log(0, "[sensord] userspace sensor manager online.");
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
    let _ = std::log(0, "[storaged] userspace storage manager online.");
    let timer_slot = std::cap_lookup(std::cap_type::TIMER_TICK_READ, 0).ok();
    let write_slot = std::cap_lookup(std::cap_type::FILE_SYSTEM_WRITE, 0).ok();
    let has_epoll = std::cap_lookup(std::cap_type::EVENT_POLL, 0).is_ok();
    let mut last_tick = 0u64;
    let mut health = [0u8; 48];

    loop {
        let _ = std::service_heartbeat(3);
        let now = timer_slot.and_then(|s| std::get_ticks(s).ok()).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            let msg = b"storaged: userspace storage service heartbeat\n";
            health[..msg.len()].copy_from_slice(msg);
            if let Some(w_slot) = write_slot {
                if let Ok(fd) = std::fs_open(w_slot, "/data/audit.log", std::FS_WRITE) {
                    let _ = std::fs_write(w_slot, fd, &health[..msg.len()]);
                    let _ = std::fs_close(fd);
                } else {
                    let _ = std::log(0, "[storaged] filesystem request rejected.");
                }
            } else {
                let _ = std::log(0, "[storaged] filesystem write capability missing.");
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
