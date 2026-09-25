#![no_std]

use interos_std as std;

pub fn run(service_id: u8, name: &str) -> ! {
    let _ = std::log(0, name);
    loop {
        let _ = std::service_heartbeat(service_id);
        #[cfg(feature = "ci-fault-injection")]
        if service_id == 8 {
            if let Ok(ticks) = std::get_ticks(1) {
                if (150..=250).contains(&ticks) {
                    std::exit_with_code(42);
                }
            }
        }
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
        let _ = std::get_ticks(1);
    }
}

/// Storage-service execution path. The filesystem is no longer only a kernel
/// bootstrap facility: storaged owns the userspace storage heartbeat and uses
/// its dedicated FileSystem capability to perform bounded persistent I/O.
pub fn run_storage() -> ! {
    let _ = std::log(0, "[storaged] userspace storage manager online.");
    let mut last_tick = 0u64;
    let mut health = [0u8; 48];

    loop {
        let _ = std::service_heartbeat(3);
        let now = std::get_ticks(1).unwrap_or(last_tick);
        if now.saturating_sub(last_tick) >= 100 || last_tick == 0 {
            let msg = b"storaged: userspace storage service heartbeat\n";
            health[..msg.len()].copy_from_slice(msg);
            if let Ok(fd) = std::fs_open(4, "/data/audit.log", std::FS_WRITE) {
                let _ = std::fs_write(4, fd, &health[..msg.len()]);
                let _ = std::fs_close(fd);
            } else {
                let _ = std::log(0, "[storaged] filesystem request rejected.");
            }
            last_tick = now;
        }
        let _ = std::event_poll(std::EVENT_IPC_MSG | std::EVENT_TIMER_TICK, 20);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop { core::hint::spin_loop(); }
}
