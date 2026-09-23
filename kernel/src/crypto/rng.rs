//! Hardware True Random Number Generator (TRNG) & Entropy Pool
//!
//! Gathers hardware entropy via x86 RDRAND instruction, CPU timestamp counters,
//! interrupt jitter, and touch digitizer coordinates to generate cryptographically
//! secure random numbers for Sovereign DID keys, nonces, and session tokens.

use super::Sha256;
use spin::Mutex;

/// Hardware-backed Entropy Generator.
pub struct HardwareRng {
    has_rdrand: bool,
    seed_state: [u8; 32],
    counter: u64,
}

impl HardwareRng {
    pub const fn new() -> Self {
        Self {
            has_rdrand: false,
            seed_state: [0x5A; 32],
            counter: 0,
        }
    }

    /// Probes CPU features to detect if hardware RDRAND is supported.
    pub fn init(&mut self) {
        let ecx_features: u32;
        unsafe {
            core::arch::asm!(
                "mov {tmp}, rbx",
                "cpuid",
                "mov rbx, {tmp}",
                inout("eax") 1u32 => _,
                out("ecx") ecx_features,
                out("edx") _,
                tmp = out(reg) _,
                options(nostack, preserves_flags)
            );
        }

        self.has_rdrand = (ecx_features & (1 << 30)) != 0;

        // Seed internal entropy pool with initial hardware ticks and TSC
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let ticks = crate::interrupts::get_system_ticks();
        self.mix_entropy(tsc ^ ticks);
    }

    /// Injects auxiliary hardware entropy (touch noise, timer jitter) into the pool.
    pub fn mix_entropy(&mut self, entropy: u64) {
        self.counter = self.counter.wrapping_add(1);
        let mut mix_buf = [0u8; 48];
        mix_buf[0..32].copy_from_slice(&self.seed_state);
        mix_buf[32..40].copy_from_slice(&entropy.to_le_bytes());
        mix_buf[40..48].copy_from_slice(&self.counter.to_le_bytes());
        self.seed_state = Sha256::digest(&mix_buf);
    }

    /// Generates a single 64-bit cryptographically secure random integer.
    pub fn next_u64(&mut self) -> u64 {
        if self.has_rdrand {
            // Attempt hardware RDRAND instruction up to 10 retries
            for _ in 0..10 {
                let val: u64;
                let success: u8;
                unsafe {
                    core::arch::asm!(
                        "rdrand {0}",
                        "setc {1}",
                        out(reg) val,
                        out(reg_byte) success,
                        options(nostack)
                    );
                }
                if success != 0 {
                    // Mix into internal pool and return
                    self.mix_entropy(val);
                    return val;
                }
            }
        }

        // Fallback: SHA-256 CSPRNG expansion from mixed pool
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        self.mix_entropy(tsc);
        u64::from_le_bytes([
            self.seed_state[0], self.seed_state[1], self.seed_state[2], self.seed_state[3],
            self.seed_state[4], self.seed_state[5], self.seed_state[6], self.seed_state[7],
        ])
    }

    /// Fills a buffer with cryptographically secure pseudorandom bytes.
    pub fn fill_bytes(&mut self, dest: &mut [u8]) {
        let mut offset = 0;
        while offset < dest.len() {
            let rnd = self.next_u64();
            let rnd_bytes = rnd.to_le_bytes();
            let chunk = (dest.len() - offset).min(8);
            dest[offset..offset + chunk].copy_from_slice(&rnd_bytes[..chunk]);
            offset += chunk;
        }
    }
}

/// Global static instance of the Hardware TRNG.
pub static TRNG: Mutex<HardwareRng> = Mutex::new(HardwareRng::new());

/// Initializes the hardware TRNG subsystem.
pub fn init() {
    TRNG.lock().init();
}

/// Safe helper to fill a slice with random bytes.
pub fn get_random_bytes(buf: &mut [u8]) {
    TRNG.lock().fill_bytes(buf);
}
