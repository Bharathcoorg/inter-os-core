#!/usr/bin/env python3
"""Static storage architecture contracts.

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
INTERFS = ROOT / "kernel/src/storage/interfs.rs"
FILESYSTEM = ROOT / "kernel/src/storage/filesystem.rs"


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def must_not(text: str, needle: str, label: str) -> None:
    assert needle not in text, f"forbidden {label}: {needle}"


def main() -> None:
    ata = ATA.read_text(encoding="utf-8")
    sdhci = SDHCI.read_text(encoding="utf-8")
    storage = STORAGE.read_text(encoding="utf-8")
    persistent = PERSISTENT.read_text(encoding="utf-8")
    vfs = VFS.read_text(encoding="utf-8")
    interfs = INTERFS.read_text(encoding="utf-8")
    filesystem = FILESYSTEM.read_text(encoding="utf-8")

    block = (ROOT / "kernel/src/storage/block.rs").read_text(encoding="utf-8")

    # Hardware implementations share one sector-level abstraction.
    must(block, "pub trait BlockDevice: Send + Sync", "canonical block-device trait")
    must(block, "fn read_sector", "block read operation")
    must(block, "fn write_sector", "block write operation")
    must(block, "fn sector_count", "block geometry")
    must(block, "fn model", "device identity")
    must(ata, "impl BlockDevice for AtaDrive", "ATA block-device implementation")
    must(sdhci, "impl BlockDevice for SdhciController", "SDHCI block-device implementation")
    must(storage, "pub mod persistent;", "storage subsystem persistence boundary")

    # Persistent storage is layered above the raw block interface.
    must(persistent, "use crate::drivers::{BlockDevice, ATA_PRIMARY, SECTOR_SIZE};", "journal uses block abstraction")
    must(persistent, "pub fn load_latest_kind", "persistent metadata read API")
    must(persistent, "pub fn commit_kind", "persistent metadata commit API")
    must(persistent, "load_latest_kind(out, 4)", "default persistent read record kind")
    must(persistent, "commit_kind(payload, 4)", "default persistent write record kind")
    # Journal record kinds are globally unique: publisher trust roots use kind 3,
    # while settings use the newly allocated kind 10. A collision here can make
    # one subsystem restore another subsystem's bytes as valid state.
    settings = (ROOT / "kernel/src/settings.rs").read_text(encoding="utf-8")
    revocation = (ROOT / "kernel/src/package/revocation.rs").read_text(encoding="utf-8")
    must(settings, "const KIND: u8 = 10;", "isolated settings journal kind")
    must(revocation, "commit_kind(&buf[..pos], 3)", "publisher trust-root journal kind")
    must(persistent, "pub const JOURNAL_MAX_KIND: u8 = 10;", "journal kind capacity")
    must(persistent, "EXTENSION_SLOT_COUNT: u32 = 4", "reserved extension pair for new metadata")
    must(persistent, "18 | 19", "stable legacy kind-9 extension slots")
    assert "load_latest_kind(out, 0)" not in persistent, "default loader must not use unsupported kind 0"
    assert "commit_kind(payload, 0)" not in persistent, "default commit must not use unsupported kind 0"
    must(vfs, "pub fn init()", "VFS initialization")
    must(vfs, "vfs.restore_data_snapshot();", "persistent VFS restore")
    must(vfs, 'crate::storage::persistent::commit_kind(&sealed,4)', "VFS snapshot persistence")
    must(vfs, 'path.starts_with("/data/")', "persistent mutable-data boundary")
    # VFS persists and reconstructs the mutable inode tree, not only payload bytes.
    must(vfs, 'buf[..4].copy_from_slice(b"IVM2")', "persistent inode metadata format")
    must(vfs, 'persist_metadata_snapshot', "inode metadata persistence routine")
    must(vfs, 'restore_metadata_snapshot', "inode metadata restore routine")
    must(vfs, 'vfs.restore_metadata_snapshot();', "inode metadata restoration before payload restoration")
    must(vfs, 'vfs.rebuild_children();', "persistent parent-child inode reconstruction")
    must(vfs, 'fn parent_path(path: &str)', "hierarchical parent path helper")
    must(vfs, 'inode.id=InodeId(id);', "stable persisted inode identity")
    must(vfs, 'inode.sha256_digest=digest;', "persisted inode integrity metadata")

    # A real on-disk filesystem backend exists above BlockDevice.
    must(interfs, "pub struct InterFs", "block-backed filesystem controller")
    must(interfs, "pub fn format", "filesystem format operation")
    must(interfs, "pub fn format_at", "explicit filesystem region formatting")
    must(interfs, "pub fn format_region", "bounded filesystem formatting")
    must(interfs, "pub fn mount_region", "bounded filesystem mounting")
    must(interfs, "pub fn mount_at", "explicit filesystem region mounting")
    must(interfs, "base_lba", "filesystem region boundary state")
    must(interfs, "pub fn mount", "filesystem mount operation")
    must(interfs, "FS_MAGIC", "filesystem superblock magic")
    must(interfs, "struct DiskInode", "persistent inode record")
    # WHY the bitmap is named as a field and not as a constant: the layout is
    # derived from the size of the region now, so the bitmap starts where the
    # region says it starts and is as long as the region needs it to be. The
    # constant it used to be is the thing the contract was written against.
    must(interfs, "pub bitmap_lba: u32", "persistent allocation bitmap")
    must(interfs, "pub bitmap_sectors: u32", "filesystem bitmap geometry bound")
    must(interfs, "pub inode_lba: u32", "persistent inode table")
    must(interfs, "pub data_start: u32", "filesystem data region boundary")
    must(interfs, "pub data_sectors: u32", "filesystem data region size")
    must(interfs, "pub fn create_file", "on-disk file creation")
    must(interfs, "pub fn read_file", "on-disk file read")
    must(interfs, "pub fn write_file", "on-disk file replacement")
    must(interfs, "self.device.write_sector", "filesystem uses BlockDevice writes")
    must(interfs, "self.device.read_sector", "filesystem uses BlockDevice reads")
    # Region-relative bitmap metadata must never escape the filesystem base.
    must(interfs, "self.device.read_sector(self.base_lba + lba", "bitmap reads stay inside filesystem region")
    must(interfs, "self.device.write_sector(self.base_lba + lba", "bitmap writes stay inside filesystem region")
    assert "self.device.read_sector(lba,&mut sector)" not in interfs, "bitmap read must not bypass filesystem base"
    assert "self.device.write_sector(lba,&sector)" not in interfs, "bitmap write must not bypass filesystem base"
    must(interfs, "inode.start_sector=if sectors == 0 { 0 } else { start };", "zero-length inode extent normalization")
    must(interfs, 'if path.is_empty() || !path.starts_with(\'/\') || path.contains("//")', "strict on-disk path validation")
    must(interfs, "if !inode.used", "unused inode metadata boundary")
    must(interfs, "inode.file_size == 0", "zero-length inode consistency validation")
    must(interfs, "inode.sector_count == 0 || inode.start_sector.checked_add(inode.sector_count)", "inode extent bounds validation")
    must(interfs, "inode.file_size > capacity", "inode payload capacity validation")

    # Metadata mutations use an on-disk transaction record with recovery on mount.
    must(interfs, 'const TXN_MAGIC: [u8; 4] = *b"IFTX";', "filesystem transaction record magic")
    must(interfs, "fn write_transaction", "transaction intent publication")
    must(interfs, "fn recover_transaction", "transaction recovery")
    must(interfs, "fs.recover_transaction()?;", "mount-time transaction recovery")
    must(interfs, "self.write_transaction(&txn)", "mutation transaction publication")
    must(interfs, "self.clear_transaction()?", "transaction completion/cleanup")
    create_pos = interfs.index("pub fn create_file")
    create_body = interfs[create_pos:]
    # WHY the ordering is asserted on the operations and not on the
    # expressions that implement them: the implementation was rewritten to
    # copy the extent rather than to write it sector by sector, and the
    # guarantee the contract is about is the order -- the transaction's
    # intent, then the extent reservation, then the payload, then the
    # inode, and only then the transaction's end.
    def _at(body: str, needle: str) -> int:
        index = body.find(needle)
        assert index >= 0, f"the write path no longer has {needle!r}"
        return index

    transaction = _at(create_body, "write_transaction(&txn)")
    extent = _at(create_body, "mark_extent(start,sectors)")
    inode = _at(create_body, "write_inode(")
    # WHY the LAST clearing is the one that matters: the error paths clear the
    # transaction as they unwind, and the guarantee is that the transaction
    # survives the inode publication, so it is the final clear that has to
    # come after it.
    cleared = create_body.rindex("clear_transaction()")
    assert inode < cleared, "transaction must remain until inode publication completes"
    payload = min(
        (index for index in (
            create_body.find("write_sector"),
            create_body.find("copy_extent"),
        ) if index >= 0),
        default=-1,
    )
    assert payload >= 0, "the write path no longer publishes a payload"
    assert transaction < extent, "transaction must precede extent reservation"
    assert extent < payload, "extent reservation must precede payload publication"
    assert payload < inode, "payload must precede inode publication"
    assert inode < cleared, "transaction must remain until inode publication completes"

    # Persistent mutable VFS state is authenticated and encrypted at rest.
    must(storage, "pub mod encrypted;", "encrypted storage module")
    must(storage, "pub use encrypted::{seal, open, EncryptionError", "encrypted storage API")
    constenc = "ENCRYPTED_MAGIC: [u8; 4] = *b\"ENC1\"";
    must((ROOT / "kernel/src/storage/encrypted.rs").read_text(encoding="utf-8"), constenc, "encrypted envelope magic")
    must((ROOT / "kernel/src/storage/encrypted.rs").read_text(encoding="utf-8"), "HmacSha256::compute", "authenticated encryption primitive")
    must((ROOT / "kernel/src/storage/encrypted.rs").read_text(encoding="utf-8"), "constant_time_eq_32", "constant-time tag verification")
    must((ROOT / "kernel/src/storage/encrypted.rs").read_text(encoding="utf-8"), "storage-at-rest-v1", "storage key domain separation")
    keystore = (ROOT / "kernel/src/crypto/keystore.rs").read_text(encoding="utf-8")
    must(keystore, 'load_latest_kind(&mut seed_record, 6)', "persistent storage key loading")
    must(keystore, 'commit_kind(&record, 6)', "persistent storage key provisioning")
    assert "0x7A, 0x4B, 0x19" not in keystore, "storage root must not be a compiled-in static secret"
    must(vfs, "crate::storage::encrypted::seal", "VFS encrypted persistence")
    must(vfs, "crate::storage::encrypted::open", "VFS encrypted restoration")
    must(vfs, "32768 + crate::storage::encrypted::ENCRYPTED_OVERHEAD", "encrypted restore buffer capacity")
    must(vfs, "legacy unencrypted plaintext snapshots", "migration fallback for legacy snapshots")
    must((ROOT / "kernel/src/main.rs").read_text(encoding="utf-8"), "crypto::keystore::init();", "keystore initialized before VFS")
    assert (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8").index("crypto::keystore::init();") < (ROOT / "kernel/src/main.rs").read_text(encoding="utf-8").index("fs::init();"), "storage key must exist before VFS reconstruction"

    # Write payload sectors first and publish the generation header last.
    payload_write = persistent.index("drive.write_sector(base + sector, &buf)")
    header_write = persistent.index("drive.write_sector(base, &header)")
    assert payload_write < header_write, "journal header must be published after payload sectors"
    # Records are authenticated with a keyed MAC that also covers the header
    # fields. An unkeyed digest only proves the bytes are unchanged, so anything
    # that could write the data disk could replace the stored trust root,
    # revocation list or storage seed and still validate.
    must(persistent, "fn record_tag", "journal record authentication tag")
    must(persistent, "record_tag(expected_kind, generation", "read path verifies the tag")
    must_not(persistent, "Sha256::digest(payload)", "unkeyed journal digest")
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
