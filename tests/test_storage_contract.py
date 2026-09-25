#!/usr/bin/env python3
"""Static storage architecture contracts for M19-M22.

These checks verify that the block-device abstraction, persistent journal,
VFS snapshot boundary, and crash-safe write ordering remain explicit.
"""
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ATA = ROOT / "kernel/src/drivers/ata.rs"
SDHCI = ROOT / "kernel/src/drivers/sdhci.rs"
STORAGE = ROOT / "kernel/src/storage/mod.rs"
PERSISTENT = ROOT / "kernel/src/storage/persistent.rs"
VFS = ROOT / "kernel/src/fs/vfs.rs"


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def main() -> None:
    ata = ATA.read_text()
    sdhci = SDHCI.read_text()
    storage = STORAGE.read_text()
    persistent = PERSISTENT.read_text()
    vfs = VFS.read_text()

    # M19: hardware implementations share one sector-level abstraction.
    must(ata, "pub trait BlockDevice: Send + Sync", "canonical block-device trait")
    must(ata, "fn read_sector", "block read operation")
    must(ata, "fn write_sector", "block write operation")
    must(ata, "fn sector_count", "block geometry")
    must(ata, "fn model", "device identity")
    must(ata, "impl BlockDevice for AtaDrive", "ATA block-device implementation")
    must(sdhci, "impl BlockDevice for SdhciController", "SDHCI block-device implementation")
    must(storage, "pub mod persistent;", "storage subsystem persistence boundary")

    # M20-M21: persistent storage is layered above the raw block interface.
    must(persistent, "use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};", "journal uses block abstraction")
    must(persistent, "pub fn load_latest_kind", "persistent metadata read API")
    must(persistent, "pub fn commit_kind", "persistent metadata commit API")
    must(persistent, "load_latest_kind(out, 4)", "default persistent read record kind")
    must(persistent, "commit_kind(payload, 4)", "default persistent write record kind")
    assert "load_latest_kind(out, 0)" not in persistent, "default loader must not use unsupported kind 0"
    assert "commit_kind(payload, 0)" not in persistent, "default commit must not use unsupported kind 0"
    must(vfs, "pub fn init()", "VFS initialization")
    must(vfs, "vfs.restore_data_snapshot();", "persistent VFS restore")
    must(vfs, 'crate::storage::persistent::commit_kind(&buf[..pos],4)', "VFS snapshot persistence")
    must(vfs, 'path.starts_with("/data/")', "persistent mutable-data boundary")

    # M22: write payload sectors first and publish the generation header last.
    payload_write = persistent.index("drive.write_sector(base + sector, &buf)")
    header_write = persistent.index("drive.write_sector(base, &header)")
    assert payload_write < header_write, "journal header must be published after payload sectors"
    must(persistent, "Sha256::digest(payload)", "journal payload integrity digest")
    must(persistent, "generation", "journal generation tracking")
    must(persistent, "const SLOT_COUNT: u32 = 16;", "redundant journal slots for eight record kinds")
    must(persistent, "5 => 8usize", "package payload journal kind")
    must(persistent, "SLOT_SECTORS", "fixed journal slot geometry")

    # VFS descriptor close must release the FD-table borrow before persistence I/O.
    must(vfs, "let closed = {", "VFS close mutation scope")
    must(vfs, "if self.persist_data_snapshot().is_err()", "VFS close persistence")
    must(vfs, "End the descriptor borrow before persisting", "VFS borrow/persistence ordering")

    print("storage architecture contract: PASS")


if __name__ == "__main__":
    main()
