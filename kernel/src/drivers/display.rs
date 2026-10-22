//! Display hardware/service boundary.
//!
//! Separates the userspace display service ABI from the concrete DRM/KMS
//! implementation. Future physical framebuffer/DRM backends implement this
//! trait without changing the userspace-facing service contract.

use crate::drivers::display_kms::{DrmKmsController, SurfaceSnapshot};

/// Stable display-driver boundary consumed by the display service.
pub trait DisplayDriver {
    fn create_surface(&mut self, width: u32, height: u32, z_order: u32, owner_task: u64) -> Result<u32, &'static str>;
    fn write_surface_pixels(&mut self, id: u32, owner_task: u64, offset: usize, pixels: &[u32]) -> Result<usize, &'static str>;
    fn commit_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str>;
    fn destroy_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str>;
    fn composite_and_flip(&mut self) -> Result<u64, &'static str>;
    fn snapshot(&self, out: &mut [SurfaceSnapshot]) -> usize;
    fn dimensions(&self) -> (u32, u32);
    fn is_hardware_accelerated(&self) -> bool { false }
    fn refresh_rate(&self) -> u16 { 60 }
}

/// DRM/KMS adapter implementing the neutral display-driver boundary.
pub struct KmsDisplayDriver<'a> {
    controller: &'a mut DrmKmsController,
}

impl<'a> KmsDisplayDriver<'a> {
    pub fn new(controller: &'a mut DrmKmsController) -> Self {
        Self { controller }
    }

    pub fn is_hardware_accelerated(&self) -> bool {
        if let Some(gpu) = crate::drivers::virtio_gpu::VIRTIO_GPU.try_lock() {
            gpu.present && gpu.is_primary
        } else {
            false
        }
    }

    pub fn refresh_rate(&self) -> u16 {
        if let Some(gpu) = crate::drivers::virtio_gpu::VIRTIO_GPU.try_lock() {
            if gpu.present && gpu.is_primary {
                return gpu.target_fps;
            }
        }
        self.controller.refresh_hz
    }
}

impl DisplayDriver for KmsDisplayDriver<'_> {
    fn create_surface(&mut self, width: u32, height: u32, z_order: u32, owner_task: u64) -> Result<u32, &'static str> {
        if width == 0 || height == 0 ||
            width as usize > 3840 || height as usize > 3840 ||
            (width as usize).checked_mul(height as usize).map_or(true, |pixels| pixels > crate::drivers::display_kms::MAX_SCANOUT_PIXELS) {
            return Err("invalid display surface dimensions");
        }
        let id = self.controller.create_surface(width, height, z_order, owner_task);
        if id == 0 { return Err("display surface capacity exhausted"); }
        Ok(id)
    }

    fn write_surface_pixels(&mut self, id: u32, owner_task: u64, offset: usize, pixels: &[u32]) -> Result<usize, &'static str> {
        self.controller.write_surface_pixels(id, owner_task, offset, pixels)
    }

    fn commit_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str> {
        if self.controller.commit_surface(id, Some(owner_task)) {
            Ok(())
        } else {
            Err("display surface ownership denied")
        }
    }

    fn destroy_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str> {
        if self.controller.destroy_surface_owned(id, Some(owner_task)) {
            Ok(())
        } else {
            Err("display surface ownership denied")
        }
    }

    fn composite_and_flip(&mut self) -> Result<u64, &'static str> {
        #[cfg(target_arch = "x86_64")]
        let t0 = unsafe { core::arch::x86_64::_rdtsc() };

        self.controller.composite();

        #[cfg(target_arch = "x86_64")]
        let t1 = unsafe { core::arch::x86_64::_rdtsc() };

        let flip = self.controller.page_flip();
        // WHY the band is taken here and not reused: `composite` has already
        // run, so the controller now holds every row the compositor changed
        // since the previous flip. Taking it clears it, and the next flip
        // re-accumulates from zero (or to the full scanout, if something
        // untrackable changes). The full-scanout default therefore still covers
        // the first flip after boot and any flip after a geometry change.
        let dirty_rows = self.controller.take_present_dirty_rows();

        // Hardware presentation path:
        // Accelerated 2D dirty-band scanout flush when modern VirtIO-GPU is discovered and attached as primary.
        if let Some(mut gpu) = crate::drivers::virtio_gpu::VIRTIO_GPU.try_lock() {
            if gpu.present && gpu.is_primary {
                let sw = self.controller.scanout_width;
                let sh = self.controller.scanout_height;
                if gpu.scanout_width != sw || gpu.scanout_height != sh {
                    let res_id = gpu.active_resource_id;
                    let _ = gpu.set_scanout(0, res_id, sw, sh);
                }
                if dirty_rows.1 > dirty_rows.0 {
                    let dy0 = dirty_rows.0 as u32;
                    let dy1 = (dirty_rows.1 as u32).min(sh);
                    if dy1 > dy0 {
                        let _ = gpu.submit_dirty_band(0, dy0, sw, dy1 - dy0, (dy0 as u64) * (sw as u64) * 4);
                    }
                }
            }
        }

        // Software scanout blit:
        // Maintains the linear framebuffer scanout. Falls back gracefully to this software path
        // when VirtIO-GPU is unattached or absent.
        crate::framebuffer::present_argb8888(
            &self.controller.front_buffer,
            self.controller.scanout_width,
            self.controller.scanout_height,
            dirty_rows,
        );

        #[cfg(target_arch = "x86_64")]
        let t2 = unsafe { core::arch::x86_64::_rdtsc() };

        // WHY computed outside the sampling branch below: the compose and
        // present costs have to exist for every flip, not only the ones that
        // get printed. `[KMS PERF]` samples the first 15 flips and every 30th
        // after that, so the flip carrying an arbitrary input is almost never
        // logged -- and it is exactly that flip a latency benchmark needs.
        #[cfg(target_arch = "x86_64")]
        let (comp_cyc, pres_cyc) = (t1.saturating_sub(t0), t2.saturating_sub(t1));
        #[cfg(not(target_arch = "x86_64"))]
        let (comp_cyc, pres_cyc) = (0u64, 0u64);

        // Published through atomics rather than the PERFORMANCE mutex: the KMS
        // lock is held here, and taking a second lock on the display path to
        // record telemetry would add contention and an inversion risk for no
        // measurement benefit.
        crate::performance::record_frame(comp_cyc, pres_cyc, flip);

        if flip <= 15 || flip % 30 == 0 {
            crate::serial_println!(
                "[KMS PERF] flip={} compose_cycles={} present_cycles={} total_cycles={} scanout={}x{}",
                flip,
                comp_cyc,
                pres_cyc,
                comp_cyc + pres_cyc,
                self.controller.scanout_width,
                self.controller.scanout_height,
            );
        }
        Ok(flip)
    }

    fn snapshot(&self, out: &mut [SurfaceSnapshot]) -> usize {
        self.controller.surface_snapshot(out)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.controller.scanout_width, self.controller.scanout_height)
    }

    fn is_hardware_accelerated(&self) -> bool {
        KmsDisplayDriver::is_hardware_accelerated(self)
    }

    fn refresh_rate(&self) -> u16 {
        KmsDisplayDriver::refresh_rate(self)
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayDriver, KmsDisplayDriver};
    use crate::drivers::display_kms::DrmKmsController;

    #[test]
    fn kms_adapter_enforces_surface_owner() {
        let mut kms = DrmKmsController::new(64, 64);
        let mut display = KmsDisplayDriver::new(&mut kms);
        let id = display.create_surface(2, 2, 0, 10).unwrap();
        assert!(display.write_surface_pixels(id, 11, 0, &[0xFFFF0000]).is_err());
        assert_eq!(display.write_surface_pixels(id, 10, 0, &[0xFFFF0000]), Ok(1));
        assert_eq!(display.composite_and_flip(), Ok(1));
    }

    #[test]
    fn kms_adapter_commits_surface_frame() {
        let mut kms = DrmKmsController::new(64, 64);
        let mut display = KmsDisplayDriver::new(&mut kms);
        let id = display.create_surface(2, 2, 0, 10).unwrap();
        assert!(display.commit_surface(id, 11).is_err());
        assert!(display.commit_surface(id, 10).is_ok());
    }

    #[test]
    fn kms_adapter_falls_back_gracefully_when_virtio_gpu_absent() {
        let mut kms = DrmKmsController::new(64, 64);
        let display = KmsDisplayDriver::new(&mut kms);
        assert!(!display.is_hardware_accelerated());
        assert_eq!(display.refresh_rate(), 60);
    }
}
