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
    fn destroy_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str>;
    fn composite_and_flip(&mut self) -> Result<u64, &'static str>;
    fn snapshot(&self, out: &mut [SurfaceSnapshot]) -> usize;
    fn dimensions(&self) -> (u32, u32);
}

/// DRM/KMS adapter implementing the neutral display-driver boundary.
pub struct KmsDisplayDriver<'a> {
    controller: &'a mut DrmKmsController,
}

impl<'a> KmsDisplayDriver<'a> {
    pub fn new(controller: &'a mut DrmKmsController) -> Self {
        Self { controller }
    }
}

impl DisplayDriver for KmsDisplayDriver<'_> {
    fn create_surface(&mut self, width: u32, height: u32, z_order: u32, owner_task: u64) -> Result<u32, &'static str> {
        if width == 0 || height == 0 ||
            width as usize > 1280 || height as usize > 720 ||
            (width as usize).checked_mul(height as usize).map_or(true, |pixels| pixels > crate::drivers::display_kms::MAX_SURFACE_PIXELS) {
            return Err("invalid display surface dimensions");
        }
        let id = self.controller.create_surface(width, height, z_order, owner_task);
        if id == 0 { return Err("display surface capacity exhausted"); }
        Ok(id)
    }

    fn write_surface_pixels(&mut self, id: u32, owner_task: u64, offset: usize, pixels: &[u32]) -> Result<usize, &'static str> {
        self.controller.write_surface_pixels(id, owner_task, offset, pixels)
    }

    fn destroy_surface(&mut self, id: u32, owner_task: u64) -> Result<(), &'static str> {
        if self.controller.destroy_surface_owned(id, Some(owner_task)) {
            Ok(())
        } else {
            Err("display surface ownership denied")
        }
    }

    fn composite_and_flip(&mut self) -> Result<u64, &'static str> {
        self.controller.composite();
        let flip = self.controller.page_flip();
        crate::framebuffer::present_argb8888(
            &self.controller.front_buffer,
            self.controller.scanout_width,
            self.controller.scanout_height,
        );
        Ok(flip)
    }

    fn snapshot(&self, out: &mut [SurfaceSnapshot]) -> usize {
        self.controller.surface_snapshot(out)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.controller.scanout_width, self.controller.scanout_height)
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
}
