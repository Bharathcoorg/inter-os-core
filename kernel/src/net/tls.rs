//! Bounded TLS record framing.
//! This is a transport boundary only: cryptographic handshake/authentication is
//! deliberately not implemented here. Callers must not treat parsed records as
//! authenticated until a real TLS implementation is integrated.

pub const TLS_RECORD_HEADER: usize = 5;
pub const TLS_MAX_PLAINTEXT: usize = 16 * 1024;
pub const TLS_MAX_RECORD: usize = TLS_MAX_PLAINTEXT + 256;
pub const TLS_LEGACY_RECORD_VERSION: u16 = 0x0303;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    ChangeCipherSpec = 20,
    Alert = 21,
    Handshake = 22,
    ApplicationData = 23,
}

impl TryFrom<u8> for ContentType {
    type Error = &'static str;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v { 20 => Ok(Self::ChangeCipherSpec), 21 => Ok(Self::Alert), 22 => Ok(Self::Handshake), 23 => Ok(Self::ApplicationData), _ => Err("unknown TLS content type") }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TlsRecord<'a> {
    pub content_type: ContentType,
    pub version: u16,
    pub payload: &'a [u8],
}

impl<'a> TlsRecord<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<Self, &'static str> {
        if buf.len() < TLS_RECORD_HEADER { return Err("TLS record header too small"); }
        let content_type = ContentType::try_from(buf[0])?;
        let version = u16::from_be_bytes([buf[1], buf[2]]);
        if version != TLS_LEGACY_RECORD_VERSION { return Err("unsupported TLS record version"); }
        let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
        if len > TLS_MAX_RECORD || buf.len() < TLS_RECORD_HEADER + len { return Err("invalid TLS record length"); }
        Ok(Self { content_type, version, payload: &buf[5..5 + len] })
    }

    pub fn serialize(&self, out: &mut [u8]) -> Result<usize, &'static str> {
        if self.payload.len() > TLS_MAX_RECORD || out.len() < TLS_RECORD_HEADER + self.payload.len() { return Err("TLS output buffer too small"); }
        if self.version != TLS_LEGACY_RECORD_VERSION { return Err("unsupported TLS record version"); }
        out[0] = self.content_type as u8;
        out[1..3].copy_from_slice(&self.version.to_be_bytes());
        out[3..5].copy_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out[5..5 + self.payload.len()].copy_from_slice(self.payload);
        Ok(5 + self.payload.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn record_round_trip_and_bounds() {
        let r = TlsRecord { content_type: ContentType::Handshake, version: TLS_LEGACY_RECORD_VERSION, payload: b"hello" };
        let mut raw = [0u8; 32];
        let n = r.serialize(&mut raw).unwrap();
        let p = TlsRecord::parse(&raw[..n]).unwrap();
        assert_eq!(p, r);
        raw[3] = 0xff; raw[4] = 0xff;
        assert!(TlsRecord::parse(&raw[..n]).is_err());
    }
}
