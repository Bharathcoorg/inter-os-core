//! InterOS userspace-style system service registry and lifecycle bootstrap.
//!
//! Phase 25 establishes explicit service ownership and capability boundaries.
//! The service functions are intentionally small kernel-hosted workers until
//! the userspace service ABI is fully available; all privileged work remains
//! behind the existing syscall/capability boundary.

use crate::capability::{Capability, CapabilityId, CapabilityTable, CapabilityType, Rights};
use crate::task::{SCHEDULER, TaskId};
use crate::syscall;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ServiceId { Init=0, Display=1, Input=2, Storage=3, Network=4, Power=5, Modem=6, Audio=7, Camera=8 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState { Declared, Running }

#[derive(Debug, Clone, Copy)]
pub struct ServiceRecord { pub id: ServiceId, pub task: TaskId, pub state: ServiceState }

fn caps(id: ServiceId) -> CapabilityTable {
    let mut t = CapabilityTable::new(8);
    let base = 0x5000 + id as u64 * 0x10;
    let _ = t.insert(Capability { id: CapabilityId(base), cap_type: CapabilityType::IpcReceive, rights: Rights::READ, object_id: 0 });
    let _ = t.insert(Capability { id: CapabilityId(base + 1), cap_type: CapabilityType::IpcSend, rights: Rights::WRITE, object_id: 0 });
    let _ = t.insert(Capability { id: CapabilityId(base + 2), cap_type: CapabilityType::TimerTickRead, rights: Rights::READ, object_id: 0 });
    let (kind, rights) = match id {
        ServiceId::Display => (CapabilityType::FrameBufferDirect, Rights::WRITE),
        ServiceId::Input => (CapabilityType::EventPoll, Rights::READ),
        ServiceId::Storage => (CapabilityType::FileSystemRead, Rights::READ | Rights::WRITE),
        ServiceId::Network => (CapabilityType::NetworkSocket, Rights::READ | Rights::WRITE),
        ServiceId::Power | ServiceId::Modem | ServiceId::Audio | ServiceId::Camera => (CapabilityType::HardwarePortIo, Rights::READ | Rights::WRITE),
        ServiceId::Init => (CapabilityType::ProcessSpawn, Rights::GRANT),
    };
    let _ = t.insert(Capability { id: CapabilityId(base + 3), cap_type: kind, rights, object_id: id as u64 });
    t
}

extern "C" fn service_loop(name: &'static str) -> ! {
    syscall::sys_log(0, name);
    loop { syscall::sys_yield(); }
}

extern "C" fn displayd() -> ! { service_loop("[displayd] Display service online: compositor/DRM boundary active.") }
extern "C" fn inputd() -> ! { service_loop("[inputd] Input service online: touch/gesture boundary active.") }
extern "C" fn storaged() -> ! { service_loop("[storaged] Storage service online: VFS/block boundary active.") }
extern "C" fn netd() -> ! { service_loop("[netd] Network service online: socket/network HAL boundary active.") }
extern "C" fn powerd() -> ! { service_loop("[powerd] Power service online: PMIC/DVFS boundary active.") }
extern "C" fn modemd() -> ! { service_loop("[modemd] Modem service online: cellular/RIL boundary active.") }
extern "C" fn audiod() -> ! { service_loop("[audiod] Audio service online: codec/audio HAL boundary active.") }
extern "C" fn camerad() -> ! { service_loop("[camerad] Camera service online: sensor/CSI boundary active.") }

pub static mut SERVICE_RECORDS: [Option<ServiceRecord>; 9] = [None; 9];

/// Starts the Phase 25 service set once, with isolated capability tables.
pub fn init() {
    unsafe { SERVICE_RECORDS[ServiceId::Init as usize] = Some(ServiceRecord { id: ServiceId::Init, task: TaskId(0), state: ServiceState::Running }); }
    let defs: [(ServiceId, &'static str, extern "C" fn() -> !, u8); 8] = [
        (ServiceId::Display, "displayd", displayd, 6),
        (ServiceId::Input, "inputd", inputd, 6),
        (ServiceId::Storage, "storaged", storaged, 5),
        (ServiceId::Network, "netd", netd, 5),
        (ServiceId::Power, "powerd", powerd, 7),
        (ServiceId::Modem, "modemd", modemd, 6),
        (ServiceId::Audio, "audiod", audiod, 5),
        (ServiceId::Camera, "camerad", camerad, 5),
    ];
    for (id, name, entry, priority) in defs {
        let task = SCHEDULER.lock().spawn(name, entry, caps(id), priority);
        crate::ipc::IPC_ROUTER.lock().register_task(task);
        unsafe { SERVICE_RECORDS[id as usize] = Some(ServiceRecord { id, task, state: ServiceState::Running }); }
        crate::serial_println!("[SERVICE] {} registered as Task {} with dedicated capability domain.", name, task.0);
    }
    crate::serial_println!("[SERVICE] Phase 25 service architecture online: displayd inputd storaged netd powerd modemd audiod camerad.");
}

/// Returns a task ID for a named service when registered.
pub fn task_id(id: ServiceId) -> Option<TaskId> {
    unsafe { SERVICE_RECORDS[id as usize].map(|r| r.task) }
}
