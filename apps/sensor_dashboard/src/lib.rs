#![no_std]

pub use interos_std as std;

/// Result of one real probe. Nothing here is ever defaulted to a made-up value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe<T: Copy> {
    Ok(T),
    /// The syscall returned a negative kernel status code.
    Refused(i64),
}

/// One `sensor_read` call, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Telemetry {
    /// The payload declared hardware and carried the fields we need.
    Present {
        lux: i64,
        accel: i64,
        near: bool,
        orientation: [u8; 24],
        orientation_len: usize,
    },
    /// The payload declared that no sensor is bound.
    Absent {
        status: [u8; 24],
        status_len: usize,
        error: [u8; 96],
        error_len: usize,
    },
    /// The call was refused by the capability check.
    Denied(i64),
    /// The call succeeded but the payload had no `present` field or missed required fields.
    Malformed,
}

/// One `thermal_query` call, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thermal {
    /// Zones were serialised. `present` is the kernel's own flag.
    Zones {
        present: bool,
        cpu: i64,
        gpu: i64,
        battery: i64,
        modem: i64,
        charging: i64,
        state: [u8; 16],
        state_len: usize,
        throttling: bool,
    },
    /// `sys_thermal_query` returns `DeviceError` (-12) when no sensor is bound.
    Refused(i64),
    Malformed,
}

/// One `battery_query` call, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Battery {
    Reading {
        voltage_mv: i64,
        soc: i64,
        capacity: i64,
        charging: [u8; 20],
        charging_len: usize,
        temp: i64,
    },
    Absent {
        governor: [u8; 20],
        governor_len: usize,
        mhz: i64,
    },
    Denied(i64),
    Malformed,
}

// -----------------------------------------------------------------------------
// Minimal flat-JSON reader
// -----------------------------------------------------------------------------

pub fn json_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    while i + key.len() + 2 < buf.len() {
        if buf[i] == b'"'
            && &buf[i + 1..i + 1 + key.len()] == key
            && buf[i + 1 + key.len()] == b'"'
            && (i == 0 || buf[i - 1] == b'{' || buf[i - 1] == b',')
        {
            let after = i + key.len() + 2;
            if after < buf.len() && buf[after] == b':' {
                return Some(after + 1);
            }
        }
        i += 1;
    }
    None
}

pub fn json_bool(buf: &[u8], key: &[u8]) -> Option<bool> {
    let i = json_value(buf, key)?;
    if i >= buf.len() {
        return None;
    }
    match buf[i] {
        b't' if buf[i..].starts_with(b"true") => Some(true),
        b'f' if buf[i..].starts_with(b"false") => Some(false),
        _ => None,
    }
}

pub fn json_i64(buf: &[u8], key: &[u8]) -> Option<i64> {
    let mut i = json_value(buf, key)?;
    let negative = i < buf.len() && buf[i] == b'-';
    if negative {
        i += 1;
    }
    let mut v: i64 = 0;
    let mut digits = 0;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((buf[i] - b'0') as i64);
        digits += 1;
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    Some(if negative { -v } else { v })
}

pub fn json_str(buf: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut i = json_value(buf, key)?;
    if i >= buf.len() || buf[i] != b'"' {
        return None;
    }
    i += 1;
    let mut n = 0usize;
    while i < buf.len() && buf[i] != b'"' {
        if n < out.len() {
            out[n] = buf[i];
        }
        n += 1;
        i += 1;
    }
    Some(n.min(out.len()))
}

pub fn parse_telemetry_payload(payload: &[u8]) -> Telemetry {
    match json_bool(payload, b"present") {
        Some(true) => {
            let lux = match json_i64(payload, b"lux") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let accel = match json_i64(payload, b"accel") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let near = match json_bool(payload, b"near") {
                Some(v) => v,
                None => return Telemetry::Malformed,
            };
            let mut orientation = [0u8; 24];
            let orientation_len = match json_str(payload, b"orientation", &mut orientation) {
                Some(k) => k,
                None => return Telemetry::Malformed,
            };
            Telemetry::Present {
                lux,
                accel,
                near,
                orientation,
                orientation_len,
            }
        }
        Some(false) => {
            let mut status = [0u8; 24];
            let mut error = [0u8; 96];
            let status_len = json_str(payload, b"status", &mut status).unwrap_or(0);
            let error_len = json_str(payload, b"error", &mut error).unwrap_or(0);
            Telemetry::Absent {
                status,
                status_len,
                error,
                error_len,
            }
        }
        None => Telemetry::Malformed,
    }
}

pub fn parse_thermal_payload(payload: &[u8]) -> Thermal {
    let present = match json_bool(payload, b"present") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let cpu = match json_i64(payload, b"cpu") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let gpu = match json_i64(payload, b"gpu") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let battery = match json_i64(payload, b"battery") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let modem = match json_i64(payload, b"modem") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let charging = match json_i64(payload, b"charging") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    let mut state = [0u8; 16];
    let state_len = match json_str(payload, b"state", &mut state) {
        Some(k) => k,
        None => return Thermal::Malformed,
    };
    let throttling = match json_bool(payload, b"throttling") {
        Some(v) => v,
        None => return Thermal::Malformed,
    };
    Thermal::Zones {
        present,
        cpu,
        gpu,
        battery,
        modem,
        charging,
        state,
        state_len,
        throttling,
    }
}

pub fn parse_battery_payload(payload: &[u8]) -> Battery {
    match json_bool(payload, b"present") {
        Some(true) => {
            let voltage_mv = match json_i64(payload, b"voltage_mv") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let soc = match json_i64(payload, b"soc") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let capacity = match json_i64(payload, b"capacity") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            let mut charging = [0u8; 20];
            let charging_len = match json_str(payload, b"charging", &mut charging) {
                Some(k) => k,
                None => return Battery::Malformed,
            };
            let temp = match json_i64(payload, b"temp") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            Battery::Reading {
                voltage_mv,
                soc,
                capacity,
                charging,
                charging_len,
                temp,
            }
        }
        Some(false) => {
            let mut governor = [0u8; 20];
            let governor_len = match json_str(payload, b"governor", &mut governor) {
                Some(k) => k,
                None => return Battery::Malformed,
            };
            let mhz = match json_i64(payload, b"mhz") {
                Some(v) => v,
                None => return Battery::Malformed,
            };
            Battery::Absent {
                governor,
                governor_len,
                mhz,
            }
        }
        None => Battery::Malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_value_and_types() {
        let json = b"{\"present\":true,\"lux\":450,\"temp\":-15,\"status\":\"active\"}";
        assert_eq!(json_bool(json, b"present"), Some(true));
        assert_eq!(json_i64(json, b"lux"), Some(450));
        assert_eq!(json_i64(json, b"temp"), Some(-15));

        let mut status = [0u8; 16];
        let n = json_str(json, b"status", &mut status).expect("status");
        assert_eq!(&status[..n], b"active");
    }

    #[test]
    fn test_parse_telemetry_absent() {
        let payload = b"{\"present\":false,\"status\":\"unbound\",\"error\":\"hardware absent\"}";
        let t = parse_telemetry_payload(payload);
        match t {
            Telemetry::Absent { status, status_len, error, error_len } => {
                assert_eq!(&status[..status_len], b"unbound");
                assert_eq!(&error[..error_len], b"hardware absent");
            }
            _ => panic!("expected Telemetry::Absent"),
        }
    }

    #[test]
    fn test_parse_telemetry_present() {
        let payload = b"{\"present\":true,\"lux\":300,\"accel\":981,\"near\":false,\"orientation\":\"portrait\"}";
        let t = parse_telemetry_payload(payload);
        match t {
            Telemetry::Present { lux, accel, near, orientation, orientation_len } => {
                assert_eq!(lux, 300);
                assert_eq!(accel, 981);
                assert_eq!(near, false);
                assert_eq!(&orientation[..orientation_len], b"portrait");
            }
            _ => panic!("expected Telemetry::Present"),
        }
    }

    #[test]
    fn test_parse_thermal_zones() {
        let payload = b"{\"present\":true,\"cpu\":42,\"gpu\":39,\"battery\":31,\"modem\":28,\"charging\":30,\"state\":\"normal\",\"throttling\":false}";
        let th = parse_thermal_payload(payload);
        match th {
            Thermal::Zones { present, cpu, gpu, throttling, state, state_len, .. } => {
                assert!(present);
                assert_eq!(cpu, 42);
                assert_eq!(gpu, 39);
                assert!(!throttling);
                assert_eq!(&state[..state_len], b"normal");
            }
            _ => panic!("expected Thermal::Zones"),
        }
    }

    #[test]
    fn test_parse_battery_absent() {
        let payload = b"{\"present\":false,\"governor\":\"ondemand\",\"mhz\":1200}";
        let b = parse_battery_payload(payload);
        match b {
            Battery::Absent { governor, governor_len, mhz } => {
                assert_eq!(&governor[..governor_len], b"ondemand");
                assert_eq!(mhz, 1200);
            }
            _ => panic!("expected Battery::Absent"),
        }
    }
}
