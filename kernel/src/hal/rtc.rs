//! Real-Time Clock (RTC / CMOS) Hardware Driver & Civil Calendar Subsystem
//!
//! Directly interrogates MC146818-compatible CMOS RTC hardware registers via
//! I/O Ports 0x70 (Address / Command) and 0x71 (Data) with NMI safety masking.
//! Performs BCD-to-binary decoding, UIP status polling, 24-hour UTC normalization,
//! and accurate leap-year-aware Unix epoch timestamp calculations.

use x86_64::instructions::port::Port;
use spin::Mutex;

const CMOS_ADDRESS_PORT: u16 = 0x70;
const CMOS_DATA_PORT: u16 = 0x71;

// CMOS Registers
const RTC_REG_SECONDS: u8 = 0x00;
const RTC_REG_MINUTES: u8 = 0x02;
const RTC_REG_HOURS: u8 = 0x04;
const RTC_REG_DAY_OF_MONTH: u8 = 0x07;
const RTC_REG_MONTH: u8 = 0x08;
const RTC_REG_YEAR: u8 = 0x09;
const RTC_REG_STATUS_A: u8 = 0x0A;
const RTC_REG_STATUS_B: u8 = 0x0B;
const RTC_REG_CENTURY: u8 = 0x32;

/// Days in each Gregorian calendar month (non-leap year).
const DAYS_IN_MONTH: [u16; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// Representation of a parsed wall-clock date and time in UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub unix_epoch_secs: u64,
}

impl DateTime {
    /// Formats the date-time into a human-readable ISO-8601 UTC timestamp.
    pub fn format_iso(&self, out: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct SliceWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for SliceWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let rem = self.buf.len().saturating_sub(self.pos);
                let to_write = bytes.len().min(rem);
                self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                self.pos += to_write;
                Ok(())
            }
        }
        let mut writer = SliceWriter { buf: out, pos: 0 };
        let _ = write!(
            writer,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        writer.pos
    }
}

/// Determines if a given Gregorian year is a leap year.
#[inline]
pub const fn is_leap_year(year: u16) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Computes cumulative days from 1970-01-01 to the beginning of the specified year.
pub fn days_before_year(year: u16) -> u64 {
    if year < 1970 {
        return 0;
    }
    let mut days: u64 = 0;
    let mut y = 1970;
    while y < year {
        days += if is_leap_year(y) { 366 } else { 365 };
        y += 1;
    }
    days
}

/// Computes cumulative days within the year prior to the specified 1-based month.
pub fn days_before_month(year: u16, month: u8) -> u64 {
    if month <= 1 {
        return 0;
    }
    let mut days: u64 = 0;
    let leap = is_leap_year(year);
    let limit = (month - 1).min(12) as usize;
    let mut m = 0;
    while m < limit {
        let mut d = DAYS_IN_MONTH[m] as u64;
        if m == 1 && leap {
            d += 1;
        }
        days += d;
        m += 1;
    }
    days
}

/// Computes Unix epoch seconds (seconds since 1970-01-01 00:00:00 UTC) for a DateTime.
pub fn calculate_unix_epoch(dt: &DateTime) -> u64 {
    if dt.year < 1970 || dt.month == 0 || dt.day == 0 {
        return 0;
    }
    let total_days = days_before_year(dt.year)
        + days_before_month(dt.year, dt.month)
        + ((dt.day.saturating_sub(1)) as u64);

    (total_days * 86400)
        + ((dt.hour as u64) * 3600)
        + ((dt.minute as u64) * 60)
        + (dt.second as u64)
}

/// Decodes a 2-digit Binary Coded Decimal (BCD) byte into binary integer representation.
#[inline]
pub const fn bcd_to_binary(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}

/// Reads a CMOS register byte, setting Bit 7 (0x80) on Address Port 0x70 to suppress NMI.
unsafe fn read_cmos_register(reg: u8) -> u8 {
    let mut addr_port = Port::<u8>::new(CMOS_ADDRESS_PORT);
    let mut data_port = Port::<u8>::new(CMOS_DATA_PORT);
    // Bit 7 disables NMI during register selection
    addr_port.write(reg | 0x80);
    data_port.read()
}

/// Checks if an RTC update is currently in progress (UIP flag in Status Register A).
fn is_update_in_progress() -> bool {
    unsafe { (read_cmos_register(RTC_REG_STATUS_A) & 0x80) != 0 }
}

/// Manager and cache for the real-time hardware clock.
pub struct RtcManager {
    last_time: DateTime,
    boot_epoch: u64,
}

impl RtcManager {
    pub const fn new() -> Self {
        Self {
            last_time: DateTime {
                year: 2026,
                month: 9,
                day: 19,
                hour: 0,
                minute: 0,
                second: 0,
                unix_epoch_secs: 0,
            },
            boot_epoch: 0,
        }
    }

    /// Interrogates hardware CMOS registers and reads the current wall-clock time.
    pub fn read_hardware(&mut self) -> DateTime {
        // Wait until any active RTC clock update completes (max 10000 spins)
        let mut spins = 0;
        while is_update_in_progress() && spins < 10_000 {
            core::hint::spin_loop();
            spins += 1;
        }

        let mut sec = unsafe { read_cmos_register(RTC_REG_SECONDS) };
        let mut min = unsafe { read_cmos_register(RTC_REG_MINUTES) };
        let mut hour = unsafe { read_cmos_register(RTC_REG_HOURS) };
        let mut day = unsafe { read_cmos_register(RTC_REG_DAY_OF_MONTH) };
        let mut month = unsafe { read_cmos_register(RTC_REG_MONTH) };
        let mut year_reg = unsafe { read_cmos_register(RTC_REG_YEAR) };
        let century_reg = unsafe { read_cmos_register(RTC_REG_CENTURY) };
        let status_b = unsafe { read_cmos_register(RTC_REG_STATUS_B) };

        // Bit 2 of Status Register B indicates whether registers are binary (1) or BCD (0).
        let is_binary = (status_b & 0x04) != 0;

        if !is_binary {
            sec = bcd_to_binary(sec);
            min = bcd_to_binary(min);
            hour = ((hour & 0x0F) + (((hour & 0x70) >> 4) * 10)) | (hour & 0x80);
            day = bcd_to_binary(day);
            month = bcd_to_binary(month);
            year_reg = bcd_to_binary(year_reg);
        }

        // Convert 12-hour format to 24-hour format if Status B bit 1 is clear
        let is_24hr = (status_b & 0x02) != 0;
        if !is_24hr && (hour & 0x80) != 0 {
            hour = ((hour & 0x7F) + 12) % 24;
        }

        // Calculate full 4-digit Gregorian year
        let full_year = if century_reg > 0 && century_reg < 100 {
            let century = if !is_binary { bcd_to_binary(century_reg) } else { century_reg };
            (century as u16) * 100 + (year_reg as u16)
        } else if year_reg < 70 {
            2000 + (year_reg as u16)
        } else {
            1900 + (year_reg as u16)
        };

        let mut dt = DateTime {
            year: full_year,
            month: month.clamp(1, 12),
            day: day.clamp(1, 31),
            hour: hour % 24,
            minute: min % 60,
            second: sec % 60,
            unix_epoch_secs: 0,
        };

        dt.unix_epoch_secs = calculate_unix_epoch(&dt);
        self.last_time = dt;
        if self.boot_epoch == 0 {
            self.boot_epoch = dt.unix_epoch_secs;
        }
        dt
    }

    /// Returns the cached or current wall-clock date-time.
    pub fn now(&self) -> DateTime {
        self.last_time
    }

    /// Returns the boot timestamp in Unix epoch seconds.
    pub fn boot_epoch(&self) -> u64 {
        self.boot_epoch
    }
}

pub static RTC: Mutex<RtcManager> = Mutex::new(RtcManager::new());

/// Initializes the Real-Time Clock subsystem and validates calendar calculations.
pub fn init() {
    let mut rtc = RTC.lock();
    let current_dt = rtc.read_hardware();

    // Verify civil calendar calculations with known reference date
    // 2026-09-19 00:00:00 UTC -> 1789776000 seconds
    let ref_dt = DateTime {
        year: 2026,
        month: 9,
        day: 19,
        hour: 0,
        minute: 0,
        second: 0,
        unix_epoch_secs: 0,
    };
    let epoch = calculate_unix_epoch(&ref_dt);
    assert_eq!(epoch, 1789776000, "Unix epoch calculation for reference date must match expected 1789776000");

    let mut buf = [0u8; 32];
    let len = current_dt.format_iso(&mut buf);
    let str_view = core::str::from_utf8(&buf[..len]).unwrap_or("INVALID_DATE");

    crate::serial_println!(
        "[TEST] Hardware RTC / CMOS: Real wall-clock time read: '{}' (Unix Epoch: {} s).",
        str_view,
        current_dt.unix_epoch_secs
    );
}
