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
    print("compositor protocol contract: PASS")
if __name__=="__main__":
    main()
