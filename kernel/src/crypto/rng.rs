// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Hardware True Random Number Generator (TRNG) & Entropy Pool
//!
//! Gathers hardware entropy via CPU TRNG instructions (x86 RDRAND, ARMv8.5 RNDR, RISC-V Zkr),
//! verified firmware boot seeds, and physical digitizer jitter to generate cryptographically
//! secure random numbers for Sovereign DID keys, nonces, and session tokens.
//!
//! Strict Security Invariant:
//! If verified hardware entropy is unavailable, security-critical generation fails closed
//! rather than deriving predictable material from unverified clocks or fixed constants.

use super::Sha256;
use spin::Mutex;

/// Entropy generation error when trustworthy physical entropy cannot be certified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyError {
    /// Hardware TRNG is not available on this CPU model or firmware.
    HardwareUnavailable,
    /// Entropy pool has not gathered sufficient min-entropy to generate secure bytes.
    InsufficientEntropy,
}

/// Entropy Source Classification & Audit Trail
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntropyState {
    /// Uninitialized.
    Uninitialized,
    /// Verified true hardware entropy source active.
    HardwareSeeded { source: &'static str },
    /// Verified explicit seed injected (bootloader handoff or test fixture).
    ExplicitSeeded,
    /// Degraded pseudo-jitter only (timestamps/ticks). INSUFFICIENT for cryptographic keys/nonces.
    DegradedPseudoOnly,
}

/// Trait for platform-specific hardware entropy providers (e.g. Device Tree rng-seed, SoC PRNG MMIO).
pub trait PlatformEntropyProvider: Send + Sync {
    /// Unique name of the platform entropy source.
    fn name(&self) -> &'static str;
    /// Harvests physical hardware entropy into `out`. Returns the number of bytes written.
    fn get_entropy(&self, out: &mut [u8]) -> Result<usize, EntropyError>;
}

static PLATFORM_ENTROPY_PROVIDER: Mutex<Option<&'static dyn PlatformEntropyProvider>> = Mutex::new(None);

/// Registers an external platform entropy provider (e.g. from early board bootloader or SoC bring-up).
pub fn register_platform_entropy_provider(provider: &'static dyn PlatformEntropyProvider) {
    *PLATFORM_ENTROPY_PROVIDER.lock() = Some(provider);
}

/// Hardware-backed Entropy Generator.
pub struct HardwareRng {
    pub state: EntropyState,
    pub has_rdrand: bool,
    pub has_feat_rng: bool,
    entropy_pool: [u8; 32],
    counter: u64,
}

impl HardwareRng {
    pub const fn new() -> Self {
        Self {
            state: EntropyState::Uninitialized,
            has_rdrand: false,
            has_feat_rng: false,
            entropy_pool: [0u8; 32],
            counter: 0,
        }
    }

    /// Probes CPU architecture features to detect hardware TRNG capability.
    pub fn init(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
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
            if self.has_rdrand {
                let mut test_val: u64 = 0;
                let mut success: u8 = 0;
                for _ in 0..10 {
                    unsafe {
                        core::arch::asm!(
                            "rdrand {0}",
                            "setc {1}",
                            out(reg) test_val,
                            out(reg_byte) success,
                            options(nostack)
                        );
                    }
                    if success != 0 {
                        break;
                    }
                }
                if success != 0 {
                    self.state = EntropyState::HardwareSeeded { source: "x86_64 RDRAND" };
                    self.mix_entropy(test_val);
                    return;
                }
            }
            self.state = EntropyState::DegradedPseudoOnly;
        }

        #[cfg(target_arch = "aarch64")]
        {
            // AArch64: Check ID_AA64ISAR0_EL1 for FEAT_RNG (RNDR)
            let isar0: u64;
            unsafe {
                core::arch::asm!("mrs {}, id_aa64isar0_el1", out(reg) isar0, options(nomem, nostack));
            }
            let rng_field = (isar0 >> 60) & 0x0F;
            self.has_feat_rng = rng_field >= 1;
            if self.has_feat_rng {
                let mut test_val: u64 = 0;
                let mut success: u8 = 0;
                for _ in 0..10 {
                    unsafe {
                        core::arch::asm!(
                            "mrs {val}, s3_3_c2_c4_0",
                            "cset {succ}, ne",
                            val = out(reg) test_val,
                            succ = out(reg_byte) success,
                            options(nostack)
                        );
                    }
                    if success != 0 {
                        break;
                    }
                }
                if success != 0 {
                    self.state = EntropyState::HardwareSeeded { source: "AArch64 FEAT_RNG" };
                    self.mix_entropy(test_val);
                    return;
                }
            }
            self.state = EntropyState::DegradedPseudoOnly;
        }

        #[cfg(target_arch = "riscv64")]
        {
            // RISC-V: Zkr entropy extension (CSR 0x015 'seed').
            // Direct CSR reads trap if Zkr is not implemented in physical silicon.
            // Under microkernel security rules, we fail closed and report DegradedPseudoOnly
            // unless hardware verification confirms Zkr presence.
            self.state = EntropyState::DegradedPseudoOnly;
        }

        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "riscv64")))]
        {
            self.state = EntropyState::DegradedPseudoOnly;
        }

        if self.state == EntropyState::DegradedPseudoOnly {
            if let Some(provider) = *PLATFORM_ENTROPY_PROVIDER.lock() {
                let mut seed = [0u8; 32];
                if let Ok(n) = provider.get_entropy(&mut seed) {
                    if n >= 32 {
                        self.inject_verified_seed(&seed);
                        self.state = EntropyState::HardwareSeeded { source: provider.name() };
                    }
                }
            }
        }

        #[cfg(test)]
        if self.state == EntropyState::DegradedPseudoOnly {
            self.state = EntropyState::ExplicitSeeded;
            self.entropy_pool = [0x5A; 32];
        }
    }

    /// Injects an externally verified true entropy seed (e.g. from bootloader UEFI RNG / DTB or unit test).
    pub fn inject_verified_seed(&mut self, seed: &[u8; 32]) {
        self.counter = self.counter.wrapping_add(1);
        let mut mix_buf = [0u8; 64];
        mix_buf[0..32].copy_from_slice(&self.entropy_pool);
        mix_buf[32..64].copy_from_slice(seed);
        self.entropy_pool = Sha256::digest(&mix_buf);
        crate::crypto::zeroize_slice(&mut mix_buf);
        self.state = EntropyState::ExplicitSeeded;
    }

    /// Injects auxiliary physical entropy (touch noise, timer jitter).
    /// NOTE: Auxiliary jitter alone does NOT upgrade `DegradedPseudoOnly` to `HardwareSeeded`.
    pub fn mix_auxiliary_jitter(&mut self, entropy: u64) {
        self.mix_entropy(entropy);
    }

    /// Mixes 64-bit entropy into internal pool via SHA-256 compression.
    pub fn mix_entropy(&mut self, entropy: u64) {
        self.counter = self.counter.wrapping_add(1);
        let mut mix_buf = [0u8; 48];
        mix_buf[0..32].copy_from_slice(&self.entropy_pool);
        mix_buf[32..40].copy_from_slice(&entropy.to_le_bytes());
        mix_buf[40..48].copy_from_slice(&self.counter.to_le_bytes());
        self.entropy_pool = Sha256::digest(&mix_buf);
        crate::crypto::zeroize_slice(&mut mix_buf);
    }

    /// Generates a single 64-bit cryptographically secure random integer.
    pub fn try_next_u64(&mut self) -> Result<u64, EntropyError> {
        match self.state {
            EntropyState::Uninitialized => Err(EntropyError::InsufficientEntropy),
            EntropyState::DegradedPseudoOnly => Err(EntropyError::HardwareUnavailable),
            EntropyState::HardwareSeeded { .. } => {
                #[cfg(target_arch = "x86_64")]
                if self.has_rdrand {
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
                            self.mix_entropy(val);
                            return Ok(val);
                        }
                    }
                    return Err(EntropyError::HardwareUnavailable);
                }

                #[cfg(target_arch = "aarch64")]
                if self.has_feat_rng && matches!(self.state, EntropyState::HardwareSeeded { source: "AArch64 FEAT_RNG" }) {
                    for _ in 0..10 {
                        let val: u64;
                        let success: u8;
                        unsafe {
                            core::arch::asm!(
                                "mrs {val}, s3_3_c2_c4_0",
                                "cset {succ}, ne",
                                val = out(reg) val,
                                succ = out(reg_byte) success,
                                options(nostack)
                            );
                        }
                        if success != 0 {
                            self.mix_entropy(val);
                            return Ok(val);
                        }
                    }
                    return Err(EntropyError::HardwareUnavailable);
                }

                self.expand_pool()
            }
            EntropyState::ExplicitSeeded => {
                self.expand_pool()
            }
        }
    }

    fn expand_pool(&mut self) -> Result<u64, EntropyError> {
        self.counter = self.counter.wrapping_add(1);
        let mut mix_buf = [0u8; 40];
        mix_buf[0..32].copy_from_slice(&self.entropy_pool);
        mix_buf[32..40].copy_from_slice(&self.counter.to_le_bytes());
        self.entropy_pool = Sha256::digest(&mix_buf);
        crate::crypto::zeroize_slice(&mut mix_buf);
        let val = u64::from_le_bytes([
            self.entropy_pool[0], self.entropy_pool[1], self.entropy_pool[2], self.entropy_pool[3],
            self.entropy_pool[4], self.entropy_pool[5], self.entropy_pool[6], self.entropy_pool[7],
        ]);
        Ok(val)
    }

    /// Fills a buffer with cryptographically secure pseudorandom bytes.
    pub fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), EntropyError> {
        let mut offset = 0;
        while offset < dest.len() {
            let rnd = self.try_next_u64()?;
            let rnd_bytes = rnd.to_le_bytes();
            let chunk = (dest.len() - offset).min(8);
            dest[offset..offset + chunk].copy_from_slice(&rnd_bytes[..chunk]);
            offset += chunk;
        }
        Ok(())
    }
}

/// Global static instance of the Hardware TRNG.
pub static TRNG: Mutex<HardwareRng> = Mutex::new(HardwareRng::new());

/// Initializes the hardware TRNG subsystem.
pub fn init() {
    TRNG.lock().init();
}

/// Safe helper to query the current entropy readiness state.
pub fn entropy_state() -> EntropyState {
    TRNG.lock().state
}

/// Injects a verified true entropy seed into the global pool.
pub fn inject_verified_seed(seed: &[u8; 32]) {
    TRNG.lock().inject_verified_seed(seed);
}

/// Injects auxiliary physical jitter into the pool.
pub fn mix_auxiliary_jitter(jitter: u64) {
    TRNG.lock().mix_auxiliary_jitter(jitter);
}

/// Attempts to fill a slice with cryptographically secure random bytes.
/// Fails closed if hardware entropy is unavailable.
pub fn try_get_random_bytes(buf: &mut [u8]) -> Result<(), EntropyError> {
    let mut trng = TRNG.lock();
    if trng.state == EntropyState::Uninitialized {
        trng.init();
    }
    trng.try_fill_bytes(buf)
}

/// Safe helper to fill a slice with random bytes.
/// Fails closed (panics) rather than populating buffers with predictable material.
pub fn get_random_bytes(buf: &mut [u8]) {
    if let Err(e) = try_get_random_bytes(buf) {
        panic!("[FATAL SECURITY] Hardware entropy unavailable ({:?}): refusing to generate insecure cryptographic material", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninitialized_rng_fails_closed() {
        let mut rng = HardwareRng::new();
        let mut buf = [0u8; 16];
        assert_eq!(rng.try_fill_bytes(&mut buf), Err(EntropyError::InsufficientEntropy));
    }

    #[test]
    fn test_degraded_pseudo_only_fails_closed() {
        let mut rng = HardwareRng::new();
        rng.state = EntropyState::DegradedPseudoOnly;
        let mut buf = [0u8; 16];
        assert_eq!(rng.try_fill_bytes(&mut buf), Err(EntropyError::HardwareUnavailable));
    }

    #[test]
    fn test_verified_seed_injection_and_expansion() {
        let mut rng = HardwareRng::new();
        let seed = [0x42u8; 32];
        rng.inject_verified_seed(&seed);
        assert_eq!(rng.state, EntropyState::ExplicitSeeded);

        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];
        assert_eq!(rng.try_fill_bytes(&mut buf1), Ok(()));
        assert_eq!(rng.try_fill_bytes(&mut buf2), Ok(()));
        assert_ne!(buf1, [0u8; 32], "Output must not be all-zeros");
        assert_ne!(buf1, buf2, "Subsequent outputs must be distinct");
    }
}
