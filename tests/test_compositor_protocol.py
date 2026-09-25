#!/usr/bin/env python3
"""M27 compositor protocol contracts."""
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]

def must(s,n,label): assert n in s, f"missing {label}: {n}"

def main():
    k=(ROOT/"kernel/src/compositor.rs").read_text()
    m=(ROOT/"kernel/src/main.rs").read_text()
    sdk=(ROOT/"sdk/rust/interos-std/src/lib.rs").read_text()
    for n in ("COMPOSITOR_MAGIC","COMPOSITOR_VERSION","CompositorOp","CompositorRequest",
              "CompositorRequest::decode","encode_response","COMPOSITOR_MSG_TYPE"):
        must(k,n,n)
    must(m,"pub mod compositor;","kernel compositor module")
    must(sdk,"pub mod compositor","userspace compositor module")
    for n in ("MAGIC","VERSION","MSG_TYPE","enum Op","struct Request","fn encode"):
        must(sdk,n,"SDK "+n)
    assert "ICMP" in k and "ICMP" in sdk
    sys=(ROOT/"kernel/src/syscall/mod.rs").read_text()
    user=(ROOT/"kernel/src/task/user.rs").read_text()
    must(sys,"pub fn sys_window_set_geometry(cap_slot: usize","window geometry capability boundary")
    must(sys,"pub fn sys_window_focus(cap_slot: usize","window focus capability boundary")
    must(sys,"require_display_service(cap_slot)","window operations require display service")
    must(user,"FrameBufferDirect, Rights::WRITE, service_id as u64","display service framebuffer capability")
    print("compositor/window protocol contract: PASS")
if __name__=="__main__":
    main()
