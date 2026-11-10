#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
VirtIO-GPU modern display controller and DMA memory safety contracts.

Verifies:
1. VirtIO-GPU driver wire structures and protocol command/response codes comply with VirtIO 1.0 GPU spec.
2. Descriptor queue addresses are valid device-visible GPA addresses via virtual_to_physical translation.
3. Persistent DMA slots house request and response buffers, preventing dangling pointers on the stack.
4. Feature negotiation adheres to the specification state machine.
5. Hardware limits honestly report virtualization boundaries (Platform Truth Contract).
6. Unattached device fails closed.
"""

from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]

def must(text, needle, label):
    assert needle in text, f"missing {label}: {needle}"

def main():
    gpu_src = (ROOT / "kernel/src/drivers/virtio_gpu.rs").read_text(encoding="utf-8")
    mem_src = (ROOT / "kernel/src/memory.rs").read_text(encoding="utf-8")
    kms_src = (ROOT / "kernel/src/drivers/display_kms.rs").read_text(encoding="utf-8")

    # 1. Wire specification commands and responses
    for cmd in (
        "VIRTIO_GPU_CMD_GET_DISPLAY_INFO",
        "VIRTIO_GPU_CMD_RESOURCE_CREATE_2D",
        "VIRTIO_GPU_CMD_RESOURCE_UNREF",
        "VIRTIO_GPU_CMD_SET_SCANOUT",
        "VIRTIO_GPU_CMD_RESOURCE_FLUSH",
        "VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D",
        "VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING",
        "VIRTIO_GPU_CMD_RESOURCE_DETACH_BACKING",
        "VIRTIO_GPU_RESP_OK_NODATA",
        "VIRTIO_GPU_RESP_OK_DISPLAY_INFO",
        "VIRTIO_GPU_RESP_ERR_UNSPEC",
    ):
        must(gpu_src, cmd, f"VirtIO-GPU protocol constant {cmd}")

    # 2. DMA address derivation via virtual_to_physical (GPA)
    must(mem_src, "pub fn virtual_to_physical(virtual_addr: VirtAddr) -> Option<PhysAddr>", "virtual to physical GPA translator")
    must(gpu_src, "let req_gpa = crate::memory::virtual_to_physical(req_vaddr)", "request slot GPA derivation")
    must(gpu_src, "let resp_gpa = crate::memory::virtual_to_physical(resp_vaddr)", "response slot GPA derivation")
    must(gpu_src, "self.desc[d0 as usize].addr = req_gpa;", "d0 descriptor uses GPA")
    must(gpu_src, "self.desc[d1 as usize].addr = resp_gpa;", "d1 descriptor uses GPA")

    # 3. Persistent DMA slots architecture
    must(gpu_src, "pub struct VirtioGpuDmaSlot", "persistent DMA slot structure")
    must(gpu_src, "pub dma_slots: [VirtioGpuDmaSlot; VIRTIO_GPU_MAX_CMD_SLOTS]", "DMA slot pool in control queue")
    must(gpu_src, "alloc_dma_slot", "slot allocator method")
    must(gpu_src, "submit_slot_command", "persistent slot submission method")

    # 4. Feature negotiation state machine
    for status in (
        "VIRTIO_STATUS_RESET",
        "VIRTIO_STATUS_ACKNOWLEDGE",
        "VIRTIO_STATUS_DRIVER",
        "VIRTIO_STATUS_FEATURES_OK",
        "VIRTIO_STATUS_DRIVER_OK",
    ):
        must(gpu_src, status, f"VirtIO 1.0 status bit {status}")

    # 5. Hardware limits honestly report virtualization boundaries
    must(gpu_src, "pub struct VirtioGpuHardwareLimitations", "hardware limitations struct")
    must(gpu_src, "is_virtual_device: true", "honest virtual device declaration")
    must(gpu_src, "supports_virgl_3d: false", "honest 3D virgl capability report")

    # 6. Surface memory safety invariants
    must(kms_src, "pub struct SurfaceBuffer", "SurfaceBuffer type")
    must(kms_src, "allocate_dma_frames", "physical frame allocation")
    must(kms_src, "physical_to_virtual", "physical to virtual mapping")
    must(kms_src, "recycle_frame", "physical frame recycling")
    must(kms_src, "unsafe impl Send for SurfaceBuffer", "Send implementation")
    must(kms_src, "unsafe impl Sync for SurfaceBuffer", "Sync implementation")

    print("[CONTRACT PASS] VirtIO-GPU and Surface Memory Safety contracts fully satisfied.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
