#!/usr/bin/env python3
"""System-service boundary contracts."""
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"
def main():
    services=(ROOT/"kernel/src/services.rs").read_text()
    syscall=(ROOT/"kernel/src/syscall/mod.rs").read_text()
    user=(ROOT/"kernel/src/task/user.rs").read_text()
    process=(ROOT/"kernel/src/task/process.rs").read_text()
    vfs=(ROOT/"kernel/src/fs/vfs.rs").read_text()
    abi=(ROOT/"kernel/src/service_abi.rs").read_text()
    images=(ROOT/"kernel/src/task/images.rs").read_text()
    manifest=(ROOT/"platform/x86_64/userspace-manifest.json").read_text()
    pid1=(ROOT/"userspace/init/src/main.rs").read_text()
    service_lib=(ROOT/"userspace/system-service/src/lib.rs").read_text()
    cargo=(ROOT/"userspace/system-service/Cargo.toml").read_text()
    must(services,"Sensor = 11","sensor service identity")
    must(services,"SERVICE_SENSOR_CONTROL_OBJECT","sensor scoped authority")
    must(services,"ResourceLimits","service resource budgets")
    must(services,"set_limits(process, resource_limits(id))","resource-limit enforcement")
    must(services,"service has running dependents","dependency-aware shutdown")
    must(services,"SERVICE_FAILURES","service failure diagnostics")
    must(services,"let mut states = [None; 12]","full service supervision state table")
    must(services,"11 => ServiceId::Sensor","sensor service lookup")
    must(process,"open_file_count","process file accounting")
    must(process,"reserve_open_file","file budget reservation")
    must(process,"release_open_file","file budget release")
    must(vfs,"reserve_open_file(process)","VFS enforces process file budget")
    must(vfs,"release_open_file(process)","VFS releases process file budget")
    must(services,"heartbeat_for_task","task-bound heartbeat")
    for token in ("SERVICE_POWER_CONTROL_OBJECT","SERVICE_TELEPHONY_CONTROL_OBJECT","SERVICE_AUDIO_CONTROL_OBJECT","SERVICE_CAMERA_CONTROL_OBJECT","SERVICE_SENSOR_CONTROL_OBJECT"):
        must(syscall,token,f"scoped authority {token}")
    must(syscall,"pub fn require_system_control_object","object-scoped system control")
    must(user,"11 => crate::services::ServiceId::Sensor","sensor userspace mapping")
    for cap in ("19 => (CapabilityType::SystemControl","20 => (CapabilityType::SystemControl","21 => (CapabilityType::SystemControl","22 => (CapabilityType::SystemControl","23 => (CapabilityType::SystemControl"):
        must(user,cap,f"RAPP scoped capability {cap}")
    must(abi,"SERVICE_ID_MAX: u8 = 11","ABI service range")
    must(abi,'"org.interos.service.sensord" => Some(11)',"sensor package identity")
    must(images,"debug/sensord","sensor image embedding")
    must(manifest,'"name": "sensord"',"QEMU sensor service manifest")
    must(cargo,'name = "sensord"',"sensor service binary")
    must(pid1,"11","PID1 starts sensor service")
    must(pid1,"seen == 0x07ff","PID1 validates services 1-11")
    must(service_lib,"pub fn run_sensor()","sensor userspace service loop")
    print("system service boundary contract: PASS")
if __name__=="__main__":
    main()
