//! Stable compositor IPC protocol.
//!
//! M27 keeps rendering policy out of the kernel syscall ABI. Clients submit
//! bounded commands to the display service; the service is responsible for
//! translating them to the M25 display-driver boundary.

pub const COMPOSITOR_MAGIC: [u8; 4] = *b"ICMP";
pub const COMPOSITOR_VERSION: u8 = 1;
pub const COMPOSITOR_MAX_MESSAGE: usize = 256;
pub const COMPOSITOR_HEADER_SIZE: usize = 40;
pub const COMPOSITOR_MSG_TYPE: u32 = 0x434F_4D50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompositorOp {
    CreateSurface = 1,
    WritePixels = 2,
    SetGeometry = 3,
    DestroySurface = 4,
    Commit = 5,
    Focus = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompositorRequest {
    pub op: CompositorOp,
    pub surface_id: u32,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub z_order: u32,
    pub visible: bool,
    pub pixel_offset: u32,
}

impl CompositorRequest {
    pub const HEADER_SIZE: usize = COMPOSITOR_HEADER_SIZE;

    pub fn encode(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if out.len() < Self::HEADER_SIZE { return Err("compositor buffer too small"); }
        out[..Self::HEADER_SIZE].fill(0);
        out[0..4].copy_from_slice(&COMPOSITOR_MAGIC);
        out[4] = COMPOSITOR_VERSION;
        out[5] = self.op as u8;
        out[8..12].copy_from_slice(&self.surface_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.width.to_le_bytes());
        out[16..20].copy_from_slice(&self.height.to_le_bytes());
        out[20..24].copy_from_slice(&self.pixel_offset.to_le_bytes());
        out[24..28].copy_from_slice(&self.x.to_le_bytes());
        out[28..32].copy_from_slice(&self.y.to_le_bytes());
        out[32..36].copy_from_slice(&self.z_order.to_le_bytes());
        out[36] = if self.visible { 1 } else { 0 };
        Ok(Self::HEADER_SIZE)
    }

    pub fn decode(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < Self::HEADER_SIZE { return Err("compositor message too short"); }
        if data[0..4] != COMPOSITOR_MAGIC || data[4] != COMPOSITOR_VERSION {
            return Err("invalid compositor protocol header");
        }
        let op = match data[5] {
            1 => CompositorOp::CreateSurface,
            2 => CompositorOp::WritePixels,
            3 => CompositorOp::SetGeometry,
            4 => CompositorOp::DestroySurface,
            5 => CompositorOp::Commit,
            6 => CompositorOp::Focus,
            _ => return Err("unknown compositor operation"),
        };
        Ok(Self {
            op,
            surface_id: u32::from_le_bytes(data[8..12].try_into().unwrap()),
            width: u32::from_le_bytes(data[12..16].try_into().unwrap()),
            height: u32::from_le_bytes(data[16..20].try_into().unwrap()),
            x: i32::from_le_bytes(data[24..28].try_into().unwrap()),
            y: i32::from_le_bytes(data[28..32].try_into().unwrap()),
            z_order: u32::from_le_bytes(data[32..36].try_into().unwrap()),
            visible: data[36] != 0,
            pixel_offset: u32::from_le_bytes(data[20..24].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CompositorStatus {
    Ok = 0,
    Invalid = 1,
    PermissionDenied = 2,
    NotFound = 3,
    DeviceError = 4,
}

pub const COMPOSITOR_RESPONSE_SIZE: usize = 16;

pub fn encode_response(status: CompositorStatus, value: u32, out: &mut [u8]) -> Result<usize, &'static str> {
    if out.len() < COMPOSITOR_RESPONSE_SIZE { return Err("response buffer too small"); }
    out[..COMPOSITOR_RESPONSE_SIZE].fill(0);
    out[0..4].copy_from_slice(&COMPOSITOR_MAGIC);
    out[4] = COMPOSITOR_VERSION;
    out[5] = status as u8;
    out[8..12].copy_from_slice(&value.to_le_bytes());
    Ok(COMPOSITOR_RESPONSE_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip_and_rejects_bad_header() {
        let req = CompositorRequest {
            op: CompositorOp::CreateSurface, surface_id: 7, width: 320, height: 480,
            x: 0, y: 0, z_order: 1, visible: true, pixel_offset: 0,
        };
        let mut buf = [0u8; COMPOSITOR_HEADER_SIZE];
        let n = req.encode(&mut buf).unwrap();
        assert_eq!(CompositorRequest::decode(&buf[..n]).unwrap(), req);
        buf[0] = b'X';
        assert!(CompositorRequest::decode(&buf[..n]).is_err());
    }

    #[test]
    fn response_is_bounded() {
        let mut buf = [0u8; 16];
        assert_eq!(encode_response(CompositorStatus::Ok, 9, &mut buf), Ok(16));
        assert!(encode_response(CompositorStatus::Ok, 9, &mut [0u8; 8]).is_err());
    }
}
