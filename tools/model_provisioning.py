# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Writes a published model file onto the data disk the guest mounts, from the host.

WHY THIS TOOL EXISTS. The guest can already ingest a model that a host tool left on
its data disk: `kernel/src/fs/vfs.rs:2619` `import_provisioned_models()` reads one
manifest off the block filesystem at boot and registers the payloads it names. What
the repository did not have is the other half -- a way to put 333,590,944 bytes and
their manifest line onto an InterFS region without streaming them past the serial
port a hex chunk at a time. This tool is that half. It is host-side only; it does
not launch QEMU and it does not claim anything was verified on a device.

THE FORMAT, AS READ OUT OF THE KERNEL (this is the contract this file implements):

    region placement   `storage/filesystem.rs:14,23,50` -- the InterFS region starts
                       at LBA 4096 (`BLOCK_FS_BASE_LBA`) and stops 2,560 sectors
                       (640 KiB of journal, `BLOCK_FS_TAIL_RESERVED` = `SLOT_SECTORS`
                       128 x `TOTAL_SLOT_COUNT` 20) short of the end of the device.
    geometry (v2)      `storage/interfs.rs:282` `FsLayout::for_region` -- sector 0 of
                       the region is the superblock, then a bitmap of
                       `ceil(region_sectors / 4096)` sectors (one bit per data
                       sector), then a 32-sector inode table (256 inodes x 64 bytes),
                       then the data extents. Every address is stored in the
                       superblock and `Superblock::decode` (`interfs.rs:373`) refuses
                       a disk whose stored geometry is not the one the region size
                       derives, so a hand-written layout cannot drift.
    superblock         `interfs.rs:330` -- magic `IOFS`, u32 version 2, then LE u32s
                       at 8 total_sectors, 12 data_start, 16 data_sectors,
                       20 inode_count, 24 bitmap_lba, 28 bitmap_sectors,
                       32 inode_lba, 36 inode_sectors, 40 sector_size (512).
                       Bytes 64..128 carry the transaction record (`interfs.rs:105`).
    inode record       `interfs.rs:403` -- 64 bytes: [0] used (0 or 1), [1..9]
                       file_size as u64 LE, [9..13] start_sector u32 LE,
                       [13..17] sector_count u32 LE, [17] path_len, [18..64] path.
                       The path field is 46 bytes (`MAX_PATH`, `interfs.rs:101`), and
                       the namespace is flat: a "directory" is only ever a prefix of
                       a name, there is no directory inode and no parent link.
    extent             one contiguous run per file -- `start_sector` plus
                       `sector_count`, addressed from `data_start` (`interfs.rs:851`).
                       There is no multi-extent file: `filesystem.rs:134` says so out
                       loud when it refuses a fragmented disk, and the number that
                       decides whether a payload can be stored at all is the longest
                       consecutive free run the bitmap holds (`SpaceReport`,
                       `interfs.rs:200`). Allocation is first-fit ascending over the
                       bitmap (`find_free_extent`, `interfs.rs:666`); `create_file`
                       then marks the run, writes the sectors zero-padded at the tail,
                       and publishes the inode (`interfs.rs:987-1007`).
    validation         `read_inode` (`interfs.rs:590`) refuses a used record whose
                       path is empty, does not start with `/`, contains `//`, is not
                       UTF-8, or whose extent runs past `data_sectors` or whose
                       `file_size` exceeds `sector_count * 512`. `check_integrity`
                       (`interfs.rs:1241`, reachable from the serial console at
                       `kernel/src/serial.rs:1540`) additionally requires the bitmap
                       to equal the union of the extents exactly -- no leaked bits,
                       no double-referenced sector -- so this tool sets bits only for
                       runs it actually hands to a file.
    boot guard         `boot_guard.rs:425` -- a device whose LBA 0 carries no 0x55AA
                       boot signature is the only kind of device InterFS may be
                       mounted on. A staged data disk therefore keeps LBA 0 zeroed.

THE PROVISIONING CONTRACT THE GUEST READS:

    manifest file      `/provision/models.list` (`filesystem.rs:491`) inside InterFS,
                       read into a 1,024-byte window (`filesystem.rs:501`) by
                       `load_provision_manifest_with_device` (`filesystem.rs:525`),
                       which is a plain `read_at` -- so a manifest of 1,024 bytes or
                       more has its tail cut and the entry spanning the cut fails its
                       own validation. This tool refuses to write one that long
                       rather than let the device silently see fewer models.
    manifest line      `path|bytes\n` (`vfs.rs:2662` `apply_provisioned_manifest`):
                       UTF-8, exactly one `|`, the path equal to its own normalised
                       form (`normalize_path`, `vfs.rs:2949`), inside
                       `/data/models/` (`MODEL_NAMESPACE`, `vfs.rs:75`), at most 256
                       bytes (`MAX_PATH_LEN`, `vfs.rs:24`), and a length of decimal
                       digits only -- no sign, space, unit or hex prefix
                       (`parse_provisioned_size`, `vfs.rs:2933`).
    payload name       the VFS path digested, not stored verbatim: `/data/.vfs.s.`
                       plus the first 8 bytes of the path's SHA-256 in lowercase hex
                       (`spill_path_for`, `vfs.rs:2910`) -- 29 bytes, inside the 46
                       the device allows.
    length authority   the device, not the manifest: `vfs.rs:2742` takes
                       `store.file_len(spill_name)` and imports with that. A manifest
                       that disagrees is *reported* (`vfs.rs:2750`, "the copy looks
                       interrupted") and then **accepted at the device's length**.
                       That is why the disagreement has to be caught here, on the
                       host, before anyone calls a run of this tool a provisioning:
                       the guest will not reject a truncated copy, it will hand a
                       loader a short file and a one-line console notice.

WHAT THIS TOOL DOES NOT DO. It does not re-run the kernel's Rust by shelling out to
it, and the reader below is written from the byte layout above rather than from the
Rust code path, so a mistake in one shows up as a failure in the other. It formats a
fresh image it creates itself; it never opens the repository's persistent data disk
(see `FORBIDDEN_DISK_NAMES`), and the refusal is purely lexical so the forbidden file
is not even stat-ed. And nothing here is device evidence: a green run proves the
bytes on the disk are what the kernel's own format rules describe, which is a
different claim from "the guest booted and read it".

SIZE HONESTY. The image is sized to the payload plus `--headroom-mib` of free data
region by default, not to the emulator's 6 GiB data disk, because on a host whose
filesystem allocates extended files rather than punching holes a 6 GiB image costs
6 GiB of real disk. `--full-data-disk` asks for the production geometry explicitly and
`--total-sectors` for any other one, and `--skip-space-check` is the only way past the
free-space guard, which measures the output directory rather than assuming the host is
roomy.

ONE MORE REFUSAL, BEFORE ANY BYTE IS WRITTEN. A payload is checked for the GGUF magic
before the output image is created, so refusing a file that is not a model costs the
host nothing rather than leaving a formatted disk holding a partial extent and no
manifest. `--allow-any-payload` opts out, for a container this tool should not judge.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import struct
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# --- kernel/storage constants, with the line each one comes from -------------------

SECTOR_SIZE = 512                                    # storage/block.rs:8
FS_MAGIC = b"IOFS"                                   # storage/interfs.rs:76
FS_VERSION = 2                                       # storage/interfs.rs:77
SUPERBLOCK_SECTORS = 1                               # storage/interfs.rs:81
INODE_SIZE = 64                                      # storage/interfs.rs:83
INODES_PER_SECTOR = SECTOR_SIZE // INODE_SIZE        # storage/interfs.rs:85 -> 8
INODE_SECTORS = 32                                   # storage/interfs.rs:87
MAX_INODES = INODE_SECTORS * INODES_PER_SECTOR       # storage/interfs.rs:89 -> 256
MAX_PATH = 46                                        # storage/interfs.rs:101
MIN_REGION_SECTORS = SUPERBLOCK_SECTORS + 1 + INODE_SECTORS + 1  # interfs.rs:104 -> 35
TXN_OFFSET = 64                                      # storage/interfs.rs:105
TXN_SIZE = 64                                        # storage/interfs.rs:106
TXN_MAGIC = b"IFTX"                                  # storage/interfs.rs:107
TXN_VERSION = 2                                      # storage/interfs.rs:108

BLOCK_FS_BASE_LBA = 4096                             # storage/filesystem.rs:14
JOURNAL_SLOT_SECTORS = 128                           # storage/persistent.rs:32
JOURNAL_PRIMARY_SLOTS = 16                           # storage/persistent.rs:36
JOURNAL_EXTENSION_SLOTS = 4                          # storage/persistent.rs:38
BLOCK_FS_TAIL_RESERVED = JOURNAL_SLOT_SECTORS * (
    JOURNAL_PRIMARY_SLOTS + JOURNAL_EXTENSION_SLOTS
)                                                     # storage/filesystem.rs:23 -> 2560

PROVISION_MANIFEST_PATH = "/provision/models.list"   # storage/filesystem.rs:491
PROVISION_MANIFEST_WINDOW = 1024                     # storage/filesystem.rs:501

MODEL_NAMESPACE = "/data/models/"                    # fs/vfs.rs:75
MAX_VFS_PATH_LEN = 256                               # fs/vfs.rs:24
SPILL_PREFIX = "/data/.vfs.s."                       # fs/vfs.rs:67

# The first candidate the inference service probes (system-service/src/gguf.rs:16).
DEFAULT_MODEL_VFS_PATH = "/data/models/embeddinggemma-2.gguf"

# The emulator's data disk: 6 GiB, `emulator/server.py:1341` `ensure_data_disk`.
FULL_DATA_DISK_SECTORS = 6 * 1024 * 1024 * 2          # 12,582,912

CHUNK_BYTES = 1 << 20                                 # 1 MiB of buffered I/O

# Disks this tool must never touch, whatever the caller asks for. The names come
# from `emulator/server.py:2328`: `inter-os-data-master.img` on x86_64 and
# `interos-data-<arch>.img` elsewhere are the *user's* persistent data.
FORBIDDEN_DISK_NAMES = (re.compile(r"^inter-os-data.*\.img$"),
                        re.compile(r"^interos-data-.*\.img$"))

SUPERBLOCK_FIELDS = (
    ("total_sectors", 8),
    ("data_start", 12),
    ("data_sectors", 16),
    ("inode_count", 20),
    ("bitmap_lba", 24),
    ("bitmap_sectors", 28),
    ("inode_lba", 32),
    ("inode_sectors", 36),
    ("sector_size", 40),
)

INODE_USED_OFFSET = 0        # storage/interfs.rs:405
INODE_SIZE_OFFSET = 1        # storage/interfs.rs:406
INODE_START_OFFSET = 9       # storage/interfs.rs:407
INODE_COUNT_OFFSET = 13      # storage/interfs.rs:408
INODE_PATH_LEN_OFFSET = 17   # storage/interfs.rs:409
INODE_PATH_OFFSET = 18       # storage/interfs.rs:410


class ProvisioningError(Exception):
    """A refusal to write, or a disk that does not say what it is supposed to."""


# --- geometry ----------------------------------------------------------------------


def bitmap_sectors_for(region_sectors):
    """Sectors of allocation bitmap needed to address `region_sectors`.

    Mirrors `bitmap_sectors_for` (interfs.rs:307): one bit per sector, and one
    bitmap sector covers 4096 addressed sectors. Never empty.
    """
    payload_bytes = (region_sectors + 7) // 8
    sectors = (payload_bytes + SECTOR_SIZE - 1) // SECTOR_SIZE
    return max(1, sectors)


def layout_for_region(region_sectors):
    """The v2 geometry of a region, or None if the region cannot be formatted.

    Mirrors `FsLayout::for_region` (interfs.rs:282) including its refusal when no
    data sector is left. `bitmap_lba` is the sector right after the superblock.
    """
    if region_sectors == 0:
        return None
    bitmap_sectors = bitmap_sectors_for(region_sectors)
    inode_lba = SUPERBLOCK_SECTORS + bitmap_sectors
    data_start = inode_lba + INODE_SECTORS
    data_sectors = region_sectors - data_start
    if data_sectors <= 0:
        return None
    return {
        "region_sectors": region_sectors,
        "bitmap_lba": SUPERBLOCK_SECTORS,
        "bitmap_sectors": bitmap_sectors,
        "inode_lba": inode_lba,
        "inode_sectors": INODE_SECTORS,
        "data_start": data_start,
        "data_sectors": data_sectors,
        "base_lba": BLOCK_FS_BASE_LBA,
    }


def region_for_device(total_sectors):
    """Sectors of `total_sectors` the filesystem region may use (filesystem.rs:50)."""
    end = total_sectors - BLOCK_FS_TAIL_RESERVED
    if end <= 0:
        raise ProvisioningError(
            "the device is {} sectors, which does not clear the {}-sector journal tail".format(
                total_sectors, BLOCK_FS_TAIL_RESERVED))
    available = end - BLOCK_FS_BASE_LBA
    if available < MIN_REGION_SECTORS:
        raise ProvisioningError(
            "the region would hold {} sectors; a filesystem needs at least {}".format(
                available, MIN_REGION_SECTORS))
    return available


def sectors_for_bytes(nbytes):
    """Sectors a payload of `nbytes` needs (interfs.rs:316)."""
    return (nbytes + SECTOR_SIZE - 1) // SECTOR_SIZE


def bytes_for_sectors(nsectors):
    return nsectors * SECTOR_SIZE


def fit_total_sectors(payload_bytes, headroom_bytes):
    """Device sectors holding `payload_bytes` plus `headroom_bytes` of free region.

    The bitmap grows with the region, so the region's own size feeds back into where
    the data starts. It is solved by iterating to the fixed point instead of by
    guessing: two passes always settle here, and the loop keeps going until the
    answer stops moving and then adds one sector to be sure the payload still fits.
    """
    needed = sectors_for_bytes(payload_bytes)
    headroom = sectors_for_bytes(headroom_bytes)
    total = BLOCK_FS_BASE_LBA + BLOCK_FS_TAIL_RESERVED + needed + headroom + 1
    while True:
        layout = layout_for_region(region_for_device(total))
        if layout is None:
            raise ProvisioningError("no geometry holds this payload")
        proposed = (BLOCK_FS_BASE_LBA + BLOCK_FS_TAIL_RESERVED
                    + layout["data_start"] + needed + headroom)
        if proposed <= total:
            return total
        total = proposed


# --- names -------------------------------------------------------------------------


def sha256_file(path, progress=None):
    """Digest and length of a host file, read a megabyte at a time."""
    digest = hashlib.sha256()
    total = 0
    with open(path, "rb") as handle:
        while True:
            block = handle.read(CHUNK_BYTES)
            if not block:
                break
            total += len(block)
            digest.update(block)
            if progress is not None:
                progress(total)
    return digest.hexdigest(), total


def spill_name_for(vfs_path):
    """The device-side name a VFS path's payload is kept under (vfs.rs:2910).

    The block filesystem names a file in 46 bytes and a VFS path does not fit, so the
    first 8 bytes of the path's SHA-256, lowercase hex, follow the spill prefix. Same
    path, same name, every boot -- which is the only reason a host copy and a guest
    restore agree about where a model's bytes live.
    """
    digest = hashlib.sha256(vfs_path.encode("utf-8")).digest()
    return SPILL_PREFIX + digest[:8].hex()


def normalize_vfs_path(path):
    """`normalize_path` (vfs.rs:2949), restated in Python.

    The importer only accepts a path equal to this form, so a writer that names a
    doubled slash or a `..` writes an entry the guest skips. Reproducing it here is
    what lets the tool refuse such a name before it reaches a disk.
    """
    out = "/"
    first = True
    for component in path.split("/"):
        if component in ("", "."):
            continue
        if component == "..":
            return "/"
        if len(component.encode("utf-8")) > 255:
            return "/"
        if not first:
            out += "/"
        out += component
        first = False
    return out


def manifest_entry(vfs_path, payload_bytes):
    """One manifest line as the guest's decoder expects it (vfs.rs:2662)."""
    return "{}|{}\n".format(vfs_path, payload_bytes)


def parse_manifest(text):
    """Splits a manifest into entries, applying the importer's own acceptance rules.

    Returns (accepted, rejected), each a list of dicts. Every rule here has a line in
    `apply_provisioned_manifest` behind it, and the reason strings are what that call
    would have printed on the console -- so a rejected entry is reported with the
    kernel's own excuse rather than this tool's guess at one.
    """
    accepted = []
    rejected = []
    for line in text.split(b"\n"):
        if line.endswith(b"\r"):
            line = line[:-1]
        if not line:
            continue
        try:
            entry = line.decode("utf-8")
        except UnicodeDecodeError:
            rejected.append({"line": line, "reason": "a manifest entry is not text; it is skipped"})
            continue
        if "|" not in entry:
            rejected.append({"line": entry, "reason": "names no payload length; it is skipped"})
            continue
        # Rust's `split_once` takes the first separator and keeps the rest whole.
        path, _, claimed = entry.partition("|")
        if normalize_vfs_path(path) != path:
            rejected.append({"line": entry,
                             "reason": "{} is not a file path as the tree holds one; it is skipped".format(path)})
            continue
        if not path.startswith(MODEL_NAMESPACE):
            rejected.append({"line": entry,
                             "reason": "{} is outside {}; only model payloads are provisioned".format(path, MODEL_NAMESPACE)})
            continue
        if len(path.encode("utf-8")) > MAX_VFS_PATH_LEN:
            rejected.append({"line": entry,
                             "reason": "{} is longer than the {} bytes a path may be; it is skipped".format(path, MAX_VFS_PATH_LEN)})
            continue
        if not re.fullmatch(r"[0-9]+", claimed):
            rejected.append({"line": entry,
                             "reason": "{}'s length '{}' is not a byte count; it is skipped".format(path, claimed)})
            continue
        accepted.append({"path": path, "claimed_bytes": int(claimed), "line": entry})
    return accepted, rejected


# --- sector device -------------------------------------------------------------------


class SectorDevice:
    """A raw disk image addressed in sectors -- the host twin of `BlockDevice`.

    Deliberately narrow: read, write, sector count, flush. Keeping the addressing in
    one place means the reader and the writer agree on where every structure lives,
    and a disagreement between them would be a bug in this file rather than a silent
    difference between two tools.
    """

    def __init__(self, path, total_sectors=None, create=False, truncate_existing=False):
        self.path = path
        if create:
            parent = os.path.dirname(os.path.abspath(path))
            os.makedirs(parent, exist_ok=True)
            mode = "wb+" if truncate_existing else "xb"
            self.handle = open(path, mode)  # noqa: SIM115 - closed by close()/context
            if total_sectors is None:
                raise ProvisioningError("a new image needs a sector count")
            self.total = total_sectors
            # Extend without writing: untouched sectors read back as zero, which is
            # what the kernel's own staging builder did (`set_len`).
            self.handle.seek(bytes_for_sectors(total_sectors) - 1)
            self.handle.write(b"\x00")
            self.handle.flush()
            self.total = total_sectors
            return
        self.handle = open(path, "rb+")
        size = os.path.getsize(path)
        if size % SECTOR_SIZE:
            self.close()
            raise ProvisioningError("{} is {} bytes, not a whole number of sectors".format(path, size))
        self.total = size // SECTOR_SIZE
        if total_sectors is not None and total_sectors != self.total:
            self.close()
            raise ProvisioningError("{} holds {} sectors, the caller expected {}".format(
                path, self.total, total_sectors))

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        self.close()
        return False

    def close(self):
        """Flushes and releases the handle, and is safe to call twice.

        `provision` closes the device explicitly before re-opening it for the read-back,
        so the context manager's own close has to be a no-op rather than an error.
        """
        if self.handle is None or self.handle.closed:
            return
        try:
            self.handle.flush()
            os.fsync(self.handle.fileno())
        finally:
            self.handle.close()

    def sector_count(self):
        return self.total

    def read_sector(self, lba):
        if not 0 <= lba < self.total:
            raise ProvisioningError("LBA {} is outside a {} sector device".format(lba, self.total))
        self.handle.seek(bytes_for_sectors(lba))
        data = self.handle.read(SECTOR_SIZE)
        if len(data) != SECTOR_SIZE:
            raise ProvisioningError("LBA {} read {} of 512 bytes".format(lba, len(data)))
        return data

    def write_sector(self, lba, data):
        if len(data) != SECTOR_SIZE:
            raise ProvisioningError("a sector write must be exactly 512 bytes")
        if not 0 <= lba < self.total:
            raise ProvisioningError("LBA {} is outside a {} sector device".format(lba, self.total))
        self.handle.seek(bytes_for_sectors(lba))
        self.handle.write(data)

    def read_range(self, byte_offset, length):
        if byte_offset < 0 or byte_offset + length > bytes_for_sectors(self.total):
            raise ProvisioningError("bytes {}..{} run past the device".format(
                byte_offset, byte_offset + length))
        self.handle.seek(byte_offset)
        data = self.handle.read(length)
        if len(data) != length:
            raise ProvisioningError("asked for {} bytes at {} and got {}".format(
                length, byte_offset, len(data)))
        return data

    def write_range(self, byte_offset, data):
        if byte_offset < 0 or byte_offset + len(data) > bytes_for_sectors(self.total):
            raise ProvisioningError("bytes {}..{} run past the device".format(
                byte_offset, byte_offset + len(data)))
        self.handle.seek(byte_offset)
        self.handle.write(data)

    def data_byte_offset(self, layout, sector):
        """Absolute byte offset of data sector `sector` (interfs.rs:851 addressing)."""
        return bytes_for_sectors(layout["base_lba"] + layout["data_start"] + sector)


# --- records -------------------------------------------------------------------------


def encode_superblock(layout):
    sector = bytearray(SECTOR_SIZE)
    sector[0:4] = FS_MAGIC
    sector[4:8] = struct.pack("<I", FS_VERSION)
    sector[8:12] = struct.pack("<I", layout["region_sectors"])
    sector[12:16] = struct.pack("<I", layout["data_start"])
    sector[16:20] = struct.pack("<I", layout["data_sectors"])
    sector[20:24] = struct.pack("<I", MAX_INODES)
    sector[24:28] = struct.pack("<I", layout["bitmap_lba"])
    sector[28:32] = struct.pack("<I", layout["bitmap_sectors"])
    sector[32:36] = struct.pack("<I", layout["inode_lba"])
    sector[36:40] = struct.pack("<I", layout["inode_sectors"])
    sector[40:44] = struct.pack("<I", SECTOR_SIZE)
    # 64..128 is the transaction record and stays zero: an all-zero record fails
    # `FsTransaction::decode`'s magic test, so a mount of this disk recovers nothing
    # and writes nothing (`interfs.rs:552`).
    return bytes(sector)


def decode_superblock(sector):
    """`Superblock::decode` (interfs.rs:345), with the same refusals and the same reasons."""
    if sector[0:4] != FS_MAGIC:
        raise ProvisioningError("no InterFS superblock here (magic is {!r})".format(bytes(sector[0:4])))
    version = struct.unpack_from("<I", sector, 4)[0]
    if version != FS_VERSION:
        raise ProvisioningError(
            "the disk says version {}, this kernel reads only version {} and refuses to "
            "reinterpret the other one".format(version, FS_VERSION))
    fields = {name: struct.unpack_from("<I", sector, offset)[0]
              for name, offset in SUPERBLOCK_FIELDS}
    if fields["sector_size"] != SECTOR_SIZE or fields["total_sectors"] == 0:
        raise ProvisioningError("the superblock's sector size or region size is not readable")
    expected = layout_for_region(fields["total_sectors"])
    if expected is None:
        raise ProvisioningError("no geometry derives from a {} sector region".format(
            fields["total_sectors"]))
    for name in ("data_start", "data_sectors", "bitmap_lba", "bitmap_sectors",
                 "inode_lba", "inode_sectors"):
        if fields[name] != expected[name]:
            raise ProvisioningError(
                "the superblock stores {} = {} but the region size derives {}".format(
                    name, fields[name], expected[name]))
    if fields["inode_count"] != MAX_INODES:
        raise ProvisioningError("the superblock stores inode_count = {}, the format writes {}".format(
            fields["inode_count"], MAX_INODES))
    layout = dict(expected)
    layout["version"] = version
    return layout


def encode_inode(used, file_size, start_sector, sector_count, path):
    raw = path.encode("utf-8")
    if len(raw) > MAX_PATH:
        raise ProvisioningError(
            "'{}' is {} bytes and one inode record names a file in {} at most".format(
                path, len(raw), MAX_PATH))
    record = bytearray(INODE_SIZE)
    record[INODE_USED_OFFSET] = 1 if used else 0
    record[INODE_SIZE_OFFSET:INODE_SIZE_OFFSET + 8] = struct.pack("<Q", file_size)
    record[INODE_START_OFFSET:INODE_START_OFFSET + 4] = struct.pack("<I", start_sector)
    record[INODE_COUNT_OFFSET:INODE_COUNT_OFFSET + 4] = struct.pack("<I", sector_count)
    record[INODE_PATH_LEN_OFFSET] = len(raw)
    record[INODE_PATH_OFFSET:INODE_PATH_OFFSET + len(raw)] = raw
    return bytes(record)


def decode_inode(record, layout, base_lba=BLOCK_FS_BASE_LBA):
    """`DiskInode::decode` plus `read_inode`'s validation (interfs.rs:413,590).

    A record the kernel would refuse raises here rather than returning a dict, because
    the only useful answer to "what does this inode say" about an inode the mount
    refuses is "nothing, and this is why".
    """
    if record[INODE_USED_OFFSET] > 1:
        raise ProvisioningError("the used flag is {}, which is neither 0 nor 1".format(
            record[INODE_USED_OFFSET]))
    path_len = record[INODE_PATH_LEN_OFFSET]
    if path_len > MAX_PATH:
        raise ProvisioningError("the path length byte says {}, the field holds {} at most".format(
            path_len, MAX_PATH))
    used = record[INODE_USED_OFFSET] != 0
    file_size = struct.unpack_from("<Q", record, INODE_SIZE_OFFSET)[0]
    start_sector = struct.unpack_from("<I", record, INODE_START_OFFSET)[0]
    sector_count = struct.unpack_from("<I", record, INODE_COUNT_OFFSET)[0]
    inode = {
        "used": used,
        "file_size": file_size,
        "start_sector": start_sector,
        "sector_count": sector_count,
        "path_len": path_len,
        "capacity_bytes": bytes_for_sectors(sector_count),
        "base_lba": base_lba,
    }
    if not used:
        return inode
    raw = bytes(record[INODE_PATH_OFFSET:INODE_PATH_OFFSET + path_len])
    try:
        inode["path"] = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ProvisioningError("the inode's path is not UTF-8: {}".format(error)) from None
    if (not inode["path"] or not inode["path"].startswith("/")
            or "//" in inode["path"]):
        raise ProvisioningError("'{}' is a path the mount refuses".format(inode["path"]))
    if file_size == 0:
        if start_sector != 0 or sector_count != 0:
            raise ProvisioningError("{} is empty yet claims sectors {}..{}".format(
                inode["path"], start_sector, start_sector + sector_count))
        return inode
    if sector_count == 0:
        raise ProvisioningError("{} holds {} bytes in no sectors".format(
            inode["path"], file_size))
    if start_sector + sector_count > layout["data_sectors"]:
        raise ProvisioningError("{}'s extent runs to sector {}, past the region's {}".format(
            inode["path"], start_sector + sector_count, layout["data_sectors"]))
    if file_size > inode["capacity_bytes"]:
        raise ProvisioningError("{} publishes {} bytes in a {} byte extent".format(
            inode["path"], file_size, inode["capacity_bytes"]))
    return inode


def inode_record_offset(layout, index):
    """Absolute byte offset of inode `index`'s record."""
    sector = layout["inode_lba"] + index // INODES_PER_SECTOR
    within = (index % INODES_PER_SECTOR) * INODE_SIZE
    return bytes_for_sectors(layout["base_lba"] + sector) + within


def bitmap_storage_bytes(layout):
    """Bytes the on-disk bitmap actually occupies."""
    return bytes_for_sectors(layout["bitmap_sectors"])


def pack_allocation_bits(extents, layout):
    """The bitmap bytes a set of `(start, count)` data runs must read as.

    One bit per data sector, least significant bit first inside each byte
    (`interfs.rs:644`, `(sector[byte_index % SECTOR_SIZE] & (1 << (block % 8)))`).
    Whole bytes are filled by slice assignment so a 6 GiB region costs a few thousand
    operations rather than twelve million, which is the difference between a check
    that runs and one that does not.
    """
    packed = bytearray(bitmap_storage_bytes(layout))
    for start, count in extents:
        if count == 0:
            continue
        end = start + count
        first = start // 8
        last = (end - 1) // 8
        if first == last:
            low = start % 8
            high = (end - 1) % 8
            packed[first] |= ((1 << (high - low + 1)) - 1) << low
            continue
        low = start % 8
        packed[first] |= 0xFF if low == 0 else ((1 << (8 - low)) - 1) << low
        for middle in range(first + 1, last):
            packed[middle] = 0xFF
        packed[last] |= (1 << ((end - 1) % 8 + 1)) - 1
    return bytes(packed)


def _compare_bitmap(expected, actual):
    """Problem strings for every place the on-disk bitmap disagrees with the inodes.

    `check_integrity` (interfs.rs:1283-1291) walks the region's bits and refuses the
    filesystem the moment one differs from the union of the extents, so this reports the
    same disagreement rather than a bare "mismatch". The sector is named because the
    sector is what the guest would hand to somebody else.
    """
    problems = []
    if len(expected) != len(actual):
        problems.append("the bitmap covers {} bytes and the extents need {}".format(
            len(actual), len(expected)))
    for position in range(min(len(expected), len(actual))):
        if expected[position] == actual[position]:
            continue
        differing = expected[position] ^ actual[position]
        for bit in range(8):
            if differing & (1 << bit):
                sector = position * 8 + bit
                state = "is marked allocated by no file" if actual[position] & (1 << bit) \
                    else "is handed to a file but not marked"
                problems.append("data sector {} {}; the guest would place its next file there".format(
                    sector, state))
                if len(problems) > 8:
                    problems.append("further bitmap disagreements not listed")
                    return problems
    return problems


# --- reader --------------------------------------------------------------------------


class InterFsImage:
    """Reads an InterFS region off a raw image, from scratch.

    Deliberately does not call the kernel: the point of a second implementation is
    that a byte both tools agree on is evidence, while a byte the writer printed about
    itself is not.
    """

    def __init__(self, device):
        self.device = device
        self.total = device.sector_count()
        self.layout = None
        self.inodes = {}

    def mount(self):
        """Reads and validates the geometry the way `mount_region` does (interfs.rs:502).

        Refuses the same four things the kernel refuses, and for the same reason: a
        superblock whose stored geometry is not the region's own derived one is not a
        filesystem this build can read, and reinterpreting it would return somebody
        else's file.
        """
        boot_sector = self.device.read_sector(0)
        if boot_sector[510:512] == b"\x55\xAA":
            raise ProvisioningError(
                "LBA 0 carries a boot signature; the kernel's boot guard (boot_guard.rs:425) "
                "only lets InterFS mount a device that could not have booted, so this image "
                "would be refused on the guest")
        region = region_for_device(self.total)
        header = self.device.read_sector(BLOCK_FS_BASE_LBA)
        layout = decode_superblock(header)
        if layout["region_sectors"] > region:
            raise ProvisioningError(
                "the filesystem was formatted on {} sectors and this device's region is {}".format(
                    layout["region_sectors"], region))
        txn = self.device.read_sector(BLOCK_FS_BASE_LBA)[TXN_OFFSET:TXN_OFFSET + TXN_SIZE]
        if self._transaction_pending(txn):
            raise ProvisioningError(
                "a transaction record is still pending, so the kernel's mount would rewrite "
                "this disk's metadata instead of leaving it as it was published")
        self.layout = layout
        self._load_inodes()
        return layout

    @staticmethod
    def _transaction_pending(record):
        """`FsTransaction::decode` (interfs.rs:164): magic, version and the digest cover."""
        if record[0:4] != TXN_MAGIC or record[4] != TXN_VERSION:
            return False
        digest = hashlib.sha256(record[:56]).digest()
        if digest[:8] != bytes(record[56:64]):
            return False
        return record[12] <= 1 and record[29] <= 1

    def _load_inodes(self):
        table_bytes = bytes_for_sectors(self.layout["inode_sectors"])
        table = self.device.read_range(
            bytes_for_sectors(self.layout["base_lba"] + self.layout["inode_lba"]), table_bytes)
        for index in range(MAX_INODES):
            record = table[index * INODE_SIZE:(index + 1) * INODE_SIZE]
            inode = decode_inode(record, self.layout, self.layout["base_lba"])
            inode["index"] = index
            if inode["used"]:
                self.inodes[inode["path"]] = inode

    def inode(self, path):
        if self.layout is None:
            self.mount()
        return self.inodes.get(path)

    def read_file(self, path, sink=None, limit=None, chunk=CHUNK_BYTES):
        """Streams a file's published bytes into `sink.update`, returning (bytes, sha256).

        Reads through the extent exactly as `read_extent_bytes` (interfs.rs:868) does:
        sector by sector from `base + data_start + start_sector`. `limit` caps the read
        so a caller can ask for the head of a file it is not going to hash.
        """
        inode = self.inode(path)
        if inode is None:
            raise ProvisioningError("the disk holds no file named '{}'".format(path))
        want = inode["file_size"] if limit is None else min(limit, inode["file_size"])
        digest = hashlib.sha256()
        read = 0
        start = self.layout["base_lba"] + self.layout["data_start"] + inode["start_sector"]
        while read < want:
            length = min(chunk, want - read)
            data = self.device.read_range(bytes_for_sectors(start) + read, length)
            digest.update(data)
            if sink is not None:
                sink.update(data)
            read += length
        return want, digest.hexdigest()

    def read_at(self, path, offset, length):
        inode = self.inode(path)
        if inode is None:
            raise ProvisioningError("the disk holds no file named '{}'".format(path))
        if offset >= inode["file_size"]:
            return b""
        take = min(length, inode["file_size"] - offset)
        start = self.layout["base_lba"] + self.layout["data_start"] + inode["start_sector"]
        return self.device.read_range(bytes_for_sectors(start) + offset, take)

    def manifest(self):
        """The manifest as the guest's bounded read would hand it over.

        `load_provision_manifest_with_device` (filesystem.rs:525) is a `read_at` into a
        1,024-byte window, so the window -- not the file -- is what the importer parses,
        and a truncated last line is a real outcome rather than a hypothetical.
        """
        inode = self.inode(PROVISION_MANIFEST_PATH)
        if inode is None:
            return None, None, False
        data = self.read_at(PROVISION_MANIFEST_PATH, 0, PROVISION_MANIFEST_WINDOW)
        return data, inode["file_size"], inode["file_size"] > PROVISION_MANIFEST_WINDOW

    def bitmap(self):
        """The on-disk allocation bitmap, exactly as stored."""
        return self.device.read_range(
            bytes_for_sectors(self.layout["base_lba"] + self.layout["bitmap_lba"]),
            bitmap_storage_bytes(self.layout))

    def first_allocated_run(self):
        """The lowest and highest marked bits, for a report that names the region used."""
        marked = self.bitmap()
        used = [index * 8 + bit for index, byte in enumerate(marked)
                for bit in range(8) if byte & (1 << bit)]
        return (used[0], used[-1] + 1) if used else (0, 0)

    def integrity_report(self):
        """`check_integrity` (interfs.rs:1241) restated: bitmap == union of extents.

        Returns a list of problem strings; empty means the disk is self-consistent and
        no sector is both handed to a file and offered to the next one. A leaked bit
        matters to a provisioned disk specifically: the guest's next `create_file` would
        take the sector the model is standing on.
        """
        problems = []
        extents = []
        for inode in sorted(self.inodes.values(), key=lambda item: item["index"]):
            if inode["file_size"] == 0:
                continue
            required = sectors_for_bytes(inode["file_size"])
            if inode["sector_count"] < required:
                problems.append("{}'s extent holds {} sectors for a file needing {}".format(
                    inode["path"], inode["sector_count"], required))
                continue
            extents.append((inode["start_sector"], inode["sector_count"]))
        ordered = sorted(extents)
        for earlier, later in zip(ordered, ordered[1:]):
            if earlier[0] + earlier[1] > later[0]:
                problems.append("the extents {}..{} and {}..{} overlap".format(
                    earlier[0], earlier[0] + earlier[1], later[0], later[0] + later[1]))
        expected = pack_allocation_bits(extents, self.layout)
        actual = self.bitmap()
        covered = (self.layout["data_sectors"] + 7) // 8
        problems.extend(_compare_bitmap(expected[:covered], actual[:covered]))
        if any(actual[covered:]):
            problems.append(
                "the bitmap has bits set past data sector {}; the format zeroes them and "
                "a stale one would name a sector that is not in the filesystem".format(
                    self.layout["data_sectors"]))
        return problems


# --- writer --------------------------------------------------------------------------


def guard_output_path(path, allow_any_location=False):
    """Refuses an output the tool must not create, without touching the filesystem.

    Purely lexical on purpose: the user's persistent data disk is never opened, read,
    stat-ed or hashed by this function, so a refusal cannot be the thing that reads it.
    """
    name = os.path.basename(os.path.normpath(path))
    for pattern in FORBIDDEN_DISK_NAMES:
        if pattern.match(name):
            raise ProvisioningError(
                "refusing '{}' as an output: '{}' is a guest data disk this repository "
                "treats as the user's persistent state. Provision a throwaway image under "
                "target/verify/ instead.".format(path, name))
    absolute = os.path.abspath(path)
    verify_root = os.path.join(REPO_ROOT, "target", "verify") + os.sep
    if not allow_any_location and not (absolute + os.sep).startswith(verify_root):
        raise ProvisioningError(
            "refusing '{}' as an output: staged disks live under {} unless the caller names "
            "--allow-any-location, so no run can aim this tool at a disk by accident.".format(
                path, os.path.dirname(verify_root)))
    return absolute


def require_free_space(output_path, total_sectors):
    """Refuses to start a write the host has no room for.

    Measured, not assumed: a host whose filesystem allocates an extended file in full
    would otherwise die halfway through a 333 MB copy and leave a disk nobody mounted.
    The image is that file's whole length here, so the length is what is checked.
    """
    needed = bytes_for_sectors(total_sectors)
    parent = os.path.dirname(os.path.abspath(output_path)) or "."
    os.makedirs(parent, exist_ok=True)
    free = shutil.disk_usage(parent).free
    if free < needed:
        raise ProvisioningError(
            "'{}' would be {} bytes and {} has {} bytes free. Free the disk, or lower "
            "--headroom-mib; a partial image is worse than no image.".format(
                output_path, needed, parent, free))
    return needed, free


def largest_free_run(extents, data_sectors):
    """The longest consecutive free run, from the runs already handed out.

    Answers the question `SpaceReport` (interfs.rs:200) answers by walking every bit of
    the bitmap: one extent per file means the largest model the *next* guest write can
    take is the largest gap, not the free total. Derived from the sorted runs instead of
    the bits so a 6 GiB region costs a handful of comparisons rather than twelve million
    -- the two agree because a bitmap this writer produces has exactly one bit per
    sector of every run in the list.
    """
    largest = 0
    cursor = 0
    for start, count in sorted(extents):
        if start > cursor:
            largest = max(largest, start - cursor)
        cursor = max(cursor, start + count)
    if data_sectors > cursor:
        largest = max(largest, data_sectors - cursor)
    return largest


class ProvisioningWriter:
    """Lays a fresh InterFS region carrying model payloads and their manifest entry.

    The writes follow `format_region` and then `create_file` per file (interfs.rs:464,
    962): superblock, zeroed bitmap, zeroed inode table, then for each file the extent
    marked in the bitmap, its sectors written zero-padded, and its inode published. The
    transaction record is left zero throughout, so every intermediate state of this
    image is one a mount reads as committed rather than as interrupted -- this tool has
    no crash to recover from and should not imitate one.
    """

    def __init__(self, device, layout, log=None):
        self.device = device
        self.layout = layout
        self.log = log or (lambda message: None)
        self.extents = []
        self.next_inode = 0

    def format(self):
        self.device.write_sector(BLOCK_FS_BASE_LBA, encode_superblock(self.layout))
        zero = bytes(SECTOR_SIZE)
        for lba in range(self.layout["bitmap_lba"],
                         self.layout["bitmap_lba"] + self.layout["bitmap_sectors"]):
            self.device.write_sector(BLOCK_FS_BASE_LBA + lba, zero)
        for lba in range(self.layout["inode_lba"],
                         self.layout["inode_lba"] + self.layout["inode_sectors"]):
            self.device.write_sector(BLOCK_FS_BASE_LBA + lba, zero)

    def _alloc_inode_index(self):
        """`alloc_inode_index` (interfs.rs:815): the first record nobody is using."""
        if self.next_inode >= MAX_INODES:
            raise ProvisioningError("the inode table holds {} files and is full".format(MAX_INODES))
        index = self.next_inode
        self.next_inode += 1
        return index

    def write_file(self, path, source_path, gguf=True):
        """Creates one file from a host file, streamed so no payload is ever resident.

        The write mirrors `create_file` (interfs.rs:992-1002): every sector of the
        extent is written, the final one zero-padded out to the sector boundary, and the
        inode publishes the payload's real length rather than the padded one. That
        padding is why the reader must never take `sector_count * 512` as the file size.
        """
        payload_bytes = os.path.getsize(source_path)
        sectors = sectors_for_bytes(payload_bytes)
        if sectors > self.layout["data_sectors"]:
            raise ProvisioningError(
                "{} bytes does not fit one extent: this region's data area is {} sectors".format(
                    payload_bytes, self.layout["data_sectors"]))
        start = self.reserve(sectors)
        byte_base = self.device.data_byte_offset(self.layout, start)
        placed = 0
        with open(source_path, "rb") as handle:
            while placed < payload_bytes:
                want = min(CHUNK_BYTES, payload_bytes - placed)
                block = handle.read(want)
                if len(block) != want:
                    raise ProvisioningError(
                        "{} ended after {} bytes; the manifest was going to claim {}".format(
                            source_path, placed, payload_bytes))
                if placed == 0 and gguf and block[:4] != b"GGUF":
                    raise ProvisioningError(
                        "{} does not open with the GGUF magic; this tool provisions model files "
                        "and will not call anything else one (pass --allow-any-payload for a "
                        "container this tool should not judge)".format(source_path))
                if want % SECTOR_SIZE:
                    block += bytes(SECTOR_SIZE - (want % SECTOR_SIZE))
                self.device.write_range(byte_base + placed, block)
                placed += want
        return self._finish(path, payload_bytes, start, sectors)

    def write_bytes(self, path, data):
        """Creates one file from an in-memory buffer (the manifest is a few dozen bytes)."""
        sectors = sectors_for_bytes(len(data))
        if sectors > self.layout["data_sectors"]:
            raise ProvisioningError("no region space for a {} byte file".format(len(data)))
        start = self.reserve(sectors)
        if sectors:
            padded = data + bytes(bytes_for_sectors(sectors) - len(data))
            self.device.write_range(self.device.data_byte_offset(self.layout, start), padded)
        return self._finish(path, len(data), start, sectors)

    def reserve(self, sectors):
        """Takes the lowest free run of `sectors`, exactly where the kernel would.

        `find_free_extent` (interfs.rs:666) scans the bitmap upward from data sector 0
        and returns the first run long enough, so on a freshly formatted region the model
        lands at sector 0 and the manifest right behind it. Walking the gaps between the
        runs already handed out gives the same answer without holding twelve million
        bits in a Python list.
        """
        if sectors == 0:
            return 0
        cursor = 0
        for start, count in sorted(self.extents):
            if start - cursor >= sectors:
                break
            cursor = max(cursor, start + count)
        if self.layout["data_sectors"] - cursor < sectors:
            raise ProvisioningError(
                "no run of {} free data sectors is left; the region holds {} and {} are handed "
                "out".format(sectors, self.layout["data_sectors"],
                             sum(count for _, count in self.extents)))
        self.extents.append((cursor, sectors))
        self._write_bitmap()
        return cursor

    def _finish(self, path, file_size, start, sectors):
        index = self._alloc_inode_index()
        self.device.write_range(inode_record_offset(self.layout, index),
                                encode_inode(True, file_size, start, sectors, path))
        self.log("laid {} bytes at data sector {} (inode {}) as '{}'".format(
            file_size, start, index, path))
        return {"path": path, "index": index, "start_sector": start,
                "sector_count": sectors, "file_size": file_size}

    def _write_bitmap(self):
        self.device.write_range(
            bytes_for_sectors(self.layout["base_lba"] + self.layout["bitmap_lba"]),
            pack_allocation_bits(self.extents, self.layout))


def provision(model_specs, output, total_sectors=None, headroom_mib=512,
              overwrite=False, gguf=True, allow_any_location=False,
              check_space=True, log=None):
    """Writes a throwaway data disk holding `model_specs` and its manifest entry.

    `model_specs` is a list of (host model path, VFS path). The manifest is written
    last, because the manifest is the promise and the payloads are what it promises:
    an image that stops halfway then carries no entry, and the guest imports nothing
    rather than importing a name with no bytes behind it.
    """
    say = log or (lambda message: None)
    if not model_specs:
        raise ProvisioningError("no model was named to provision")
    output_path = guard_output_path(output, allow_any_location)
    for model_path, vfs_path in model_specs:
        if not os.path.isfile(model_path):
            raise ProvisioningError(
                "the model file '{}' is not on this host; a provisioning that copies "
                "nothing publishes a name with no bytes behind it".format(model_path))
        if os.path.abspath(model_path) == output_path:
            raise ProvisioningError("the model and the output are the same file")
        if spill_name_for(vfs_path) == PROVISION_MANIFEST_PATH:
            raise ProvisioningError("no payload may take the manifest's own device name")
        if gguf:
            # Read the magic here rather than inside the writer's loop. Nothing has been
            # written to `output_path` at this point, so a payload that is not a model
            # costs the host nothing; discovered mid-copy, the same refusal would leave
            # a formatted disk holding a partial extent and no manifest behind.
            with open(model_path, "rb") as handle:
                magic = handle.read(4)
            if magic != b"GGUF":
                raise ProvisioningError(
                    "'{}' does not open with the GGUF magic (it starts with {!r}); this tool "
                    "provisions model files and will not call anything else one. Nothing was "
                    "written (pass --allow-any-payload for a container this tool should not "
                    "judge).".format(model_path, magic))
    sizes = {model_path: os.path.getsize(model_path) for model_path, _ in model_specs}
    if total_sectors is None:
        payload_total = sum(sizes.values())
        total_sectors = fit_total_sectors(payload_total, headroom_mib * 1024 * 1024)
    if total_sectors < BLOCK_FS_BASE_LBA + BLOCK_FS_TAIL_RESERVED + MIN_REGION_SECTORS:
        raise ProvisioningError("{} sectors is below the smallest formatable device".format(
            total_sectors))

    manifest = "".join(manifest_entry(vfs_path, sizes[model_path])
                       for model_path, vfs_path in model_specs)
    named = [vfs_path for _, vfs_path in model_specs]
    if len(set(named)) != len(named):
        raise ProvisioningError(
            "two models name the same VFS path {}; two inodes would answer to one name and "
            "`find_inode` returns the first, so the second payload would be unreachable".format(
                [name for name in set(named) if named.count(name) > 1]))
    if len(manifest.encode("utf-8")) >= PROVISION_MANIFEST_WINDOW:
        raise ProvisioningError(
            "the manifest is {} bytes and the guest reads a {} byte window; it would parse a "
            "cut entry. Provision fewer models.".format(
                len(manifest.encode("utf-8")), PROVISION_MANIFEST_WINDOW))
    accepted, rejected = parse_manifest(manifest.encode("utf-8"))
    if rejected:
        raise ProvisioningError("the manifest this tool built is not one the importer accepts: {}".format(
            [item["reason"] for item in rejected]))
    if len(accepted) != len(model_specs):
        raise ProvisioningError("the manifest has {} entries for {} models".format(
            len(accepted), len(model_specs)))
    for _, vfs_path in model_specs:
        if not vfs_path.lower().endswith(".gguf"):
            say("noting: '{}' is provisioned under a name with no .gguf suffix; the inference "
                "service only ever asks for .gguf files".format(vfs_path))

    needed, free = (require_free_space(output_path, total_sectors) if check_space
                    else (bytes_for_sectors(total_sectors), None))
    say("target: {} sectors = {} bytes ({} bytes free at the destination)".format(
        total_sectors, needed, free))

    exists = os.path.exists(output_path)
    if exists and not overwrite:
        raise ProvisioningError(
            "'{}' already exists; pass --overwrite to replace it (a fresh disk, not an "
            "append: this tool does not edit a filesystem in place)".format(output_path))
    if exists and overwrite:
        # Refuse a file that could be boot media before truncating it: the guard is
        # cheap and the truncation is not undoable.
        with open(output_path, "rb") as handle:
            head = handle.read(512)
        if len(head) == 512 and head[510:512] == b"\x55\xAA":
            raise ProvisioningError(
                "'{}' carries a boot signature at LBA 0, so it is boot media: this tool will "
                "not format over it, and the kernel's boot guard would refuse it anyway".format(
                    output_path))

    layout = layout_for_region(region_for_device(total_sectors))
    if layout is None:
        raise ProvisioningError("no geometry fits a {} sector device".format(total_sectors))

    records = []
    with SectorDevice(output_path, total_sectors=total_sectors, create=True,
                      truncate_existing=overwrite) as device:
        writer = ProvisioningWriter(device, layout, log=say)
        writer.format()
        for model_path, vfs_path in model_specs:
            records.append(writer.write_file(
                spill_name_for(vfs_path), source_path=model_path, gguf=gguf))
        records.append(writer.write_bytes(PROVISION_MANIFEST_PATH, manifest.encode("utf-8")))
        device.close()

    report = verify_image(output_path, model_specs=model_specs, expect_gguf=gguf)
    if not report["ok"]:
        raise ProvisioningError(
            "the disk this tool just wrote does not pass its own read-back: {}".format(
                "; ".join(report["problems"])))
    report["written"] = {"total_sectors": total_sectors,
                         "image_bytes": bytes_for_sectors(total_sectors),
                         "layout": layout, "records": records,
                         "manifest_bytes": len(manifest.encode("utf-8")),
                         "manifest_text": manifest,
                         "output": output_path}
    return report


def verify_image(image_path, model_specs=None, expect_gguf=True):
    """Independently re-parses a provisioned disk and says whether the guest would take it.

    With `model_specs` it also hashes every payload off the disk against the host file
    it came from. Without them it reports what the disk itself says, which is the
    stronger question: the guest takes its lengths from the device, so a disk that
    disagrees with the file it was built from is a disk that will import a truncated
    model and print one line about it.
    """
    problems = []
    observations = []
    files = []
    with SectorDevice(image_path) as device:
        image = InterFsImage(device)
        try:
            layout = image.mount()
        except ProvisioningError as error:
            return {"ok": False, "problems": [str(error)], "layout": None, "files": [],
                     "manifest": None, "image": os.path.abspath(image_path)}

        problems.extend(image.integrity_report())

        # The room the guest has left for its own files, in the one shape InterFS can
        # give it: a single contiguous run. A provisioned disk that leaves only fragments
        # is a disk whose next `store_data_file` fails with the numbers on the console
        # (`filesystem.rs:122`), which is a real cost of laying 333 MB here.
        extents = [(inode["start_sector"], inode["sector_count"])
                   for inode in image.inodes.values() if inode["file_size"]]
        run_sectors = largest_free_run(extents, layout["data_sectors"])
        allocation = {
            "data_sectors": layout["data_sectors"],
            "used_sectors": sum(count for _, count in extents),
            "largest_free_run_sectors": run_sectors,
            "largest_free_run_bytes": bytes_for_sectors(run_sectors),
        }

        raw, device_len, window_cut = image.manifest()
        entries = []
        if raw is None:
            problems.append("the disk holds no '{}' manifest, so the guest imports nothing".format(
                PROVISION_MANIFEST_PATH))
        else:
            accepted, rejected = parse_manifest(raw)
            for entry in rejected:
                problems.append("manifest entry {!r}: {}".format(entry["line"], entry["reason"]))
            if window_cut or (device_len is not None and device_len >= PROVISION_MANIFEST_WINDOW):
                problems.append(
                    "the manifest is {} bytes and the guest reads {} of them; the entry at the "
                    "cut is not imported".format(device_len, PROVISION_MANIFEST_WINDOW))
            entries = accepted

        expected = {}
        if model_specs:
            for model_path, vfs_path in model_specs:
                expected[vfs_path] = model_path

        for entry in entries:
            vfs_path = entry["path"]
            claimed = entry["claimed_bytes"]
            spill = spill_name_for(vfs_path)
            inode = image.inode(spill)
            record = {"vfs_path": vfs_path, "spill_name": spill,
                      "manifest_claims_bytes": claimed,
                      "device_holds_bytes": None if inode is None else inode["file_size"],
                      "extent": None if inode is None else [inode["start_sector"],
                                                             inode["start_sector"] + inode["sector_count"]],
                      "payload_sha256_on_disk": None,
                      "payload_sha256_on_host": None}
            if inode is None:
                problems.append(
                    "'{}' is named by the manifest but the device holds no payload at its spill "
                    "name '{}'; the importer would say the device holds nothing for it".format(
                        vfs_path, spill))
                files.append(record)
                continue
            if inode["file_size"] != claimed:
                problems.append(
                    "'{}' was provisioned as {} bytes but the device holds {}; the kernel imports "
                    "the device's figure and prints one line, so this copy looks interrupted and "
                    "is not a provisioning".format(vfs_path, claimed, inode["file_size"]))
            head = image.read_at(spill, 0, 4)
            record["payload_magic"] = head.decode("latin-1") if head else ""
            if expect_gguf and head[:4] != b"GGUF":
                problems.append("the payload for '{}' starts with {!r}, not the GGUF magic".format(
                    vfs_path, head[:4]))
            length, disk_digest = image.read_file(spill)
            record["read_back_bytes"] = length
            record["payload_sha256_on_disk"] = disk_digest
            host_path = expected.get(vfs_path)
            if host_path is not None:
                host_digest, host_length = sha256_file(host_path)
                record["payload_sha256_on_host"] = host_digest
                record["host_bytes"] = host_length
                if host_digest != disk_digest or host_length != length:
                    problems.append(
                        "the bytes on the disk for '{}' are not the bytes of '{}': {} bytes "
                        "{} on disk versus {} bytes {} on the host".format(
                            vfs_path, host_path, length, disk_digest[:16],
                            host_length, host_digest[:16]))
            elif not model_specs:
                observations.append(
                    "'{}' was hashed off the disk at {} bytes with no host file to compare; "
                    "the digest is reported, not a match".format(vfs_path, length))
            files.append(record)

        if model_specs:
            provisioned = {entry["path"] for entry in entries}
            for _, vfs_path in model_specs:
                if vfs_path not in provisioned:
                    problems.append("the caller asked for '{}' and the manifest does not name it".format(
                        vfs_path))

        manifest_inode = image.inode(PROVISION_MANIFEST_PATH)
        result = {
            "ok": not problems,
            "image": os.path.abspath(image_path),
            "device_sectors": image.total,
            "layout": layout,
            "allocation": allocation,
            "manifest": {"path": PROVISION_MANIFEST_PATH,
                         "device_holds_bytes": None if manifest_inode is None
                         else manifest_inode["file_size"],
                         "window_bytes": PROVISION_MANIFEST_WINDOW,
                         "text": raw.decode("utf-8", "replace") if raw is not None else None,
                         "sha256": hashlib.sha256(raw).hexdigest() if raw is not None else None,
                         "entries": entries},
            "files": files,
            "problems": problems,
            "observations": observations,
        }
        return result


def geometry_summary(total_sectors):
    """The geometry a device of `total_sectors` yields -- pure arithmetic, no I/O."""
    region = region_for_device(total_sectors)
    layout = layout_for_region(region)
    if layout is None:
        raise ProvisioningError("no geometry fits a {} sector device".format(total_sectors))
    return {
        "device_sectors": total_sectors,
        "base_lba": BLOCK_FS_BASE_LBA,
        "journal_tail_sectors": BLOCK_FS_TAIL_RESERVED,
        "region_sectors": layout["region_sectors"],
        "bitmap_sectors": layout["bitmap_sectors"],
        "inode_lba": layout["inode_lba"],
        "data_start": layout["data_start"],
        "data_sectors": layout["data_sectors"],
        "data_region_bytes": bytes_for_sectors(layout["data_sectors"]),
        "one_extent_max_bytes": bytes_for_sectors(layout["data_sectors"]),
    }


# --- CLI ------------------------------------------------------------------------------


def _parse_model_argument(values, vfs_paths):
    """Pairs each --model with a --vfs-path, one VFS name per payload."""
    if len(vfs_paths) == 1 and len(values) > 1:
        raise ProvisioningError(
            "one --vfs-path for {} models would put them all at the same path; name one per "
            "model".format(len(values)))
    if not vfs_paths:
        vfs_paths = [DEFAULT_MODEL_VFS_PATH] + [None] * (len(values) - 1)
    if len(vfs_paths) != len(values):
        raise ProvisioningError(
            "got {} --model values and {} --vfs-path values; they pair by order".format(
                len(values), len(vfs_paths)))
    specs = []
    for model_path, vfs_path in zip(values, vfs_paths):
        if vfs_path is None:
            vfs_path = "{}{}".format(MODEL_NAMESPACE, os.path.basename(model_path))
        specs.append((model_path, vfs_path))
    return specs


def main(argv=None):
    parser = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        epilog="Every refusal this tool makes is a claim the guest would not have accepted.")
    sub = parser.add_subparsers(dest="command", required=True)

    p_prov = sub.add_parser("provision", help="write a fresh data disk holding a model file")
    p_prov.add_argument("--model", action="append", required=True,
                        help="host GGUF path; repeatable, pairs with --vfs-path by order")
    p_prov.add_argument("--vfs-path", action="append", default=[],
                        help="the /data/models/ path the guest should show it at")
    p_prov.add_argument("--output", required=True,
                        help="image to create; must be under target/verify/ unless "
                             "--allow-any-location, and is never a guest data disk")
    p_prov.add_argument("--total-sectors", type=int, default=None,
                        help="device size to format; the default sizes the disk to the payload")
    p_prov.add_argument("--full-data-disk", action="store_true",
                        help="format the emulator's 6 GiB data-disk geometry ({} sectors)".format(
                            FULL_DATA_DISK_SECTORS))
    p_prov.add_argument("--headroom-mib", type=int, default=512,
                        help="free data region to leave the guest (default 512)")
    p_prov.add_argument("--overwrite", action="store_true",
                        help="replace an existing image at --output (fresh, never in place)")
    p_prov.add_argument("--allow-any-payload", action="store_true",
                        help="do not check the GGUF magic before copying")
    p_prov.add_argument("--allow-any-location", action="store_true",
                        help="write outside target/verify/ (a data disk is still refused)")
    p_prov.add_argument("--skip-space-check", action="store_true",
                        help="do not check the host has room for the image")
    p_prov.add_argument("--json", action="store_true", help="print the report as JSON")

    p_verify = sub.add_parser("verify", help="re-parse a provisioned disk independently")
    p_verify.add_argument("--image", required=True)
    p_verify.add_argument("--model", action="append", default=[],
                          help="compare the disk's bytes against this host GGUF path")
    p_verify.add_argument("--vfs-path", action="append", default=[])
    p_verify.add_argument("--allow-any-payload", action="store_true")
    p_verify.add_argument("--json", action="store_true", help="print the report as JSON")

    p_geom = sub.add_parser("geometry", help="print the region a device size yields")
    p_geom.add_argument("--total-sectors", type=int, default=FULL_DATA_DISK_SECTORS)

    args = parser.parse_args(argv)

    if args.command == "geometry":
        print(json.dumps(geometry_summary(args.total_sectors), indent=2))
        return 0

    if args.command == "verify":
        specs = _parse_model_argument(args.model, args.vfs_path) if args.model else None
        report = verify_image(args.image, model_specs=specs,
                              expect_gguf=not args.allow_any_payload)
        if args.json:
            print(json.dumps(report, indent=2, default=list))
        else:
            print_report(report)
        return 0 if report["ok"] else 1

    specs = _parse_model_argument(args.model, args.vfs_path)
    total = args.total_sectors
    if args.full_data_disk:
        if total is not None:
            raise ProvisioningError("--full-data-disk and --total-sectors both name a size")
        total = FULL_DATA_DISK_SECTORS
    report = provision(specs, args.output, total_sectors=total,
                       headroom_mib=args.headroom_mib, overwrite=args.overwrite,
                       gguf=not args.allow_any_payload,
                       allow_any_location=args.allow_any_location,
                       check_space=not args.skip_space_check,
                       log=lambda message: print("[provision] {}".format(message)))
    if args.json:
        print(json.dumps(report, indent=2, default=list))
    else:
        print_report(report)
    return 0 if report["ok"] else 1


def print_report(report):
    layout = report.get("layout")
    print("image    : {}".format(report["image"]))
    if layout:
        print("geometry : region {} sectors, bitmap {} sectors, inode table at {}, "
              "data starts at {} for {} sectors ({} bytes)".format(
                  layout["region_sectors"], layout["bitmap_sectors"], layout["inode_lba"],
                  layout["data_start"], layout["data_sectors"],
                  bytes_for_sectors(layout["data_sectors"])))
    manifest = report.get("manifest") or {}
    allocation = report.get("allocation")
    if allocation:
        print("space    : {:,} of {:,} data sectors used, longest free run {:,} sectors ({:,} bytes)".format(
            allocation["used_sectors"], allocation["data_sectors"],
            allocation["largest_free_run_sectors"], allocation["largest_free_run_bytes"]))
    if manifest.get("text") is not None:
        print("manifest : {} ({} bytes on the device, {} byte window)".format(
            manifest["path"], manifest["device_holds_bytes"], manifest["window_bytes"]))
        for line in manifest["text"].splitlines():
            print("           {}".format(line))
    for record in report.get("files", []):
        print("payload  : {}".format(record.get("vfs_path", "?")))
        print("           device name {}".format(record["spill_name"]))
        print("           manifest claims {} bytes, the device holds {}, read back {} bytes".format(
            record["manifest_claims_bytes"], record["device_holds_bytes"],
            record.get("read_back_bytes")))
        if record.get("payload_sha256_on_disk"):
            print("           sha256 on disk {}".format(record["payload_sha256_on_disk"]))
        if record.get("payload_sha256_on_host"):
            print("           sha256 on host {} ({:,} bytes)".format(
                record["payload_sha256_on_host"], record["host_bytes"]))
            print("           match    {}".format(
                record["payload_sha256_on_host"] == record["payload_sha256_on_disk"]))
    for observation in report.get("observations", []):
        print("note     : {}".format(observation))
    for problem in report.get("problems", []):
        print("PROBLEM  : {}".format(problem))
    print("verdict  : {}".format("accepted" if report["ok"] else "REJECTED"))


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ProvisioningError as error:
        print("refused: {}".format(error), file=sys.stderr)
        sys.exit(2)
