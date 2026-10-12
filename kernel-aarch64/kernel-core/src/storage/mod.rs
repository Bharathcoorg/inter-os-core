//! Crash-consistent persistent metadata storage, extracted from x86.
//!
//! Module-for-module this is `kernel/src/storage/`, and the three files here are
//! copies of `block.rs`, `boot_guard.rs` and `persistent.rs` that differ only in
//! the seams each file documents on itself — measured: `block.rs` is unchanged
//! (97 lines, 0 differences), `boot_guard.rs` changes the ELF machine type from a
//! x86 constant to a per-target one and the console from a macro to the core's
//! seam, and `persistent.rs` adds one device-binding slot. That is deliberate:
//! the point of extracting rather than reimplementing is that the extracted file
//! can still be diffed against the x86 original, so a fix to one can be reviewed
//! against the other.
//!
//! What the journal needs from the machine is exactly one thing, and it is not
//! the CPU: a block device to commit records to. On x86 that was
//! `crate::drivers::ATA_PRIMARY` (kernel/src/storage/persistent.rs:25, used at
//! :225 and :363), which is an ATA PIO driver at fixed port addresses from
//! kernel/src/drivers/ata.rs:12. The shared core declares
//! `persistent::bind_block_device` instead and each architecture binds its own
//! driver exactly once at boot.
//!
//! What still lives in the x86 tree and is not here, because it is not part of
//! this subsystem: `interfs.rs` (the on-disk filesystem, 670 lines that talk to
//! the same block device), `vault.rs`, `encrypted.rs`, `verified.rs`,
//! `crashdump.rs` and `filesystem.rs`. Those are the next candidates, and they
//! will move through this same seam — the InterFS backend takes the same
//! `BlockDevice` trait, so `interfs.rs` already inherits the abstraction without
//! being rewritten.

pub mod block;
pub mod boot_guard;
pub mod persistent;

pub use block::{BlockDevice, BlockDeviceError, BlockDeviceInfo, BlockDeviceKind, SECTOR_SIZE};
pub use boot_guard::{
    announce_refusal, evaluate_device_writes, evaluate_writes, parse_boot_area, refuse_notice,
    BootArea, ImageExtent, Refusal, RefusalNotice, SectorRange, WriteVerdict,
};
pub use persistent::{
    commit, commit_kind, load_latest, load_latest_kind, set_journal_auth_key, PersistentError,
    JOURNAL_MAGIC, JOURNAL_MAX_KIND, JOURNAL_MIN_KIND, JOURNAL_VERSION, SLOT_SECTORS,
    TOTAL_SLOT_COUNT,
};
