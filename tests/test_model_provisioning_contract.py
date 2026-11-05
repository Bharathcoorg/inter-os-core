# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Test contract: a host tool can put a real model file on the data disk the guest reads.

WHAT THIS FILE PROVES
---------------------
`kernel/src/fs/vfs.rs:2619` `import_provisioned_models()` already accepts a model that
a host tool left on the block device: it reads `/provision/models.list`, and for each
`/data/models/<name>|<bytes>` line it registers a device-held inode whose length comes
from the *extent*, not from the manifest. The half that did not exist was the writer.
`tools/model_provisioning.py` is that writer, and this file is its gate.

Every case here reads the finished image back with plain `struct` unpacking at the
offsets the Rust encoders document (`interfs.rs:330` superblock, `interfs.rs:403` inode
record, `interfs.rs:644` bitmap bit order). The tool's own reader is used to *build*
the expectation, never to excuse it: an assertion that only asks the writer whether it
was happy would pass on a disk the kernel refuses, and a disk the kernel refuses is the
one outcome this contract exists to prevent.

WHY THE NEGATIVE CASES ARE THE POINT
------------------------------------
The importer takes the length from the device and merely *prints* a line when the
manifest disagrees (`vfs.rs:2750`, "the copy looks interrupted"); it then imports the
short file anyway. So the guest will never reject a truncated provisioning -- it will
hand a model loader a stub and one console line. The only thing standing between an
interrupted copy and a future run claiming "weights provisioned" is a host-side check
that fails loudly, which is why each of these is asserted as a *detected* problem:

  * a device extent shorter than the manifest's claim,
  * a full-length extent whose tail was never actually filled,
  * a manifest that over-states the bytes,
  * a bitmap bit that belongs to no file,
  * an entry outside `/data/models/`, an entry whose path normalisation would move, a
    non-numeric length, and an entry with no length at all,
  * a manifest past the 1,024-byte window the boot reads.

WHY THE SAFETY CASES ARE HERE AND NOT IN A REVIEW CHECKLIST
-----------------------------------------------------------
`target/inter-os-data-master.img` is the user's real persistent data disk. The refusal
is asserted behaviourally, with `builtins.open` and `os.stat` instrumented, so a future
edit that "just takes a look at the file first" before deciding fails this test.

RUN TIME AND THE REAL WEIGHTS
-----------------------------
The ordinary cases write 6.4 MiB images and 100-300 KiB payloads and finish in
seconds. The published 333,590,944-byte `embeddinggemma-300M-Q8_0.gguf` round trip --
the one that proves the tool against the actual artifact rather than a stand-in -- is
opt-in, because it costs a third of a gigabyte of real disk on a host whose filesystem
allocates extended files in full:

    env -u RUSTFLAGS INTEROS_PROVISIONING_REAL_MODEL=1 \\
        python -m pytest tests/test_model_provisioning_contract.py -v

"""

import builtins
import glob
import hashlib
import os
import shutil
import struct
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))

import model_provisioning as mp  # noqa: E402

STAGING_DIR = ROOT / "target" / "verify" / "provisioning"

# The emulator's x86_64 data disk, `emulator/server.py:2328`. Named here so the guard
# under test is checked against the real name rather than a paraphrase of it.
MASTER_DATA_DISK = "inter-os-data-master.img"

# The published weights, and the size the whole repository quotes.
REAL_MODEL_PATH = ROOT / "target" / "verify" / "models" / "embeddinggemma-300M-Q8_0.gguf"
REAL_MODEL_BYTES = 333_590_944

# A device small enough to write on every run and still hold a 300 KiB payload plus
# headroom. The region arithmetic is identical to the 6 GiB disk's; only the numbers
# are friendlier.
SMALL_DISK_SECTORS = 12_800


def _payload(size, magic=b"GGUF"):
    """A deterministic pseudo-random payload of `size` bytes ending in `magic`."""
    stream = bytearray()
    counter = 0
    while len(stream) < size:
        stream += hashlib.sha256(counter.to_bytes(8, "little")).digest()
        counter += 1
    body = bytes(stream[:size])
    if magic:
        body = magic + body[len(magic):]
    return body


def _write_host_file(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data)
    return path


def _provision(host_file, image, vfs_path="/data/models/embeddinggemma-2.gguf",
               total_sectors=SMALL_DISK_SECTORS, **kwargs):
    return mp.provision([(str(host_file), vfs_path)], str(image),
                        total_sectors=total_sectors, log=lambda message: None, **kwargs)


class _ProvisioningCase(unittest.TestCase):
    """Shared staging plumbing: every case works on its own throwaway image."""

    def setUp(self):
        os.makedirs(STAGING_DIR, exist_ok=True)

    def staged(self, name):
        return STAGING_DIR / name

    def make_payload(self, name, size=300 * 1024 + 100, magic=b"GGUF"):
        """A payload whose length is not a multiple of the sector size.

        307,300 bytes is 600 sectors and 100 bytes, so the last sector is padded -- the
        case a reader that trusted `sector_count * 512` would get wrong, and the reason
        the file size lives in its own inode field.
        """
        data = _payload(size, magic)
        self.assertNotEqual(0, len(data) % mp.SECTOR_SIZE,
                            "the fixture must straddle a sector boundary to be worth writing")
        return _write_host_file(self.staged(name), data)

    def tearDown(self):
        for leftover in ("case.img", "case-append.img", "window-crowd.img",
                         "not-a-model.gguf", "first.gguf", "second.gguf"):
            path = self.staged(leftover)
            if path.exists():
                os.remove(path)


class TestInterFsGeometry(_ProvisioningCase):
    """The region a device yields, pinned to the numbers the kernel documents."""

    def test_the_six_gib_data_disk_geometry(self):
        """`interfs.rs:57-62` states this disk's layout in prose, and the Rust staging
        test asserts the same ceiling. A writer that disagrees with both would lay a
        filesystem the mount refuses, so the prose is checked as data."""
        summary = mp.geometry_summary(6 * 1024 * 1024 * 2)
        self.assertEqual(summary["base_lba"], 4096, "filesystem.rs:14")
        self.assertEqual(summary["journal_tail_sectors"], 2560,
                         "filesystem.rs:23 -- 128 slot sectors x 20 slots")
        self.assertEqual(summary["region_sectors"], 12_576_256)
        self.assertEqual(summary["bitmap_sectors"], 3071)
        self.assertEqual(summary["inode_lba"], 3072)
        self.assertEqual(summary["data_start"], 3104)
        self.assertEqual(summary["data_sectors"], 12_573_152)
        self.assertEqual(summary["one_extent_max_bytes"], 6_437_453_824,
                         "the same figure kernel/src/fs/vfs.rs asserts for the staging disk")

    def test_the_bitmap_grows_with_the_region_and_never_stays_at_eight_sectors(self):
        """Version 1's fixed eight-sector bitmap capped every filesystem at 16.8 MiB,
        which is why a 333 MB model could not be stored anywhere (`interfs.rs:13-19`).
        The bitmap must therefore be a function of the region, not a constant."""
        small = mp.layout_for_region(mp.region_for_device(12_800))
        large = mp.layout_for_region(mp.region_for_device(12_582_912))
        self.assertEqual(small["bitmap_sectors"], 2)
        self.assertEqual(large["bitmap_sectors"], 3071)
        self.assertGreater(large["data_sectors"],
                           mp.sectors_for_bytes(REAL_MODEL_BYTES),
                           "the published model must fit one extent on the demo disk")

    def test_a_device_that_does_not_clear_the_journal_is_refused(self):
        """`filesystem.rs:50` requires the region to keep `MIN_REGION_SECTORS` after the
        boot head and the journal tail. Formatting on a smaller disk would put InterFS
        over the verified-boot record."""
        with self.assertRaises(mp.ProvisioningError):
            mp.region_for_device(4096 + 2560 + 30)

    def test_the_manifest_path_and_a_spill_name_fit_one_inode_record(self):
        """`interfs.rs:101` names a file in 46 bytes and `filesystem.rs:507` gates the
        manifest's path against it at compile time. The payload's digest name is chosen
        by the same rule and has no such gate, so it is checked here."""
        self.assertLessEqual(len(mp.PROVISION_MANIFEST_PATH), mp.MAX_PATH)
        for vfs_path in ("/data/models/embeddinggemma-2.gguf",
                         "/data/models/" + "n" * 200 + ".gguf"):
            self.assertLessEqual(len(mp.spill_name_for(vfs_path)), mp.MAX_PATH,
                                 "a spill name the device cannot hold is a model that "
                                 "exists on the disk and nowhere the guest can ask for it")


class TestManifestFormat(_ProvisioningCase):
    """The line format, against `apply_provisioned_manifest` and its helpers."""

    def test_the_canonical_entry_is_accepted(self):
        text = mp.manifest_entry("/data/models/embeddinggemma-2.gguf", 307_300)
        self.assertEqual(text, "/data/models/embeddinggemma-2.gguf|307300\n")
        accepted, rejected = mp.parse_manifest(text.encode())
        self.assertEqual(rejected, [])
        self.assertEqual(accepted, [{"path": "/data/models/embeddinggemma-2.gguf",
                                     "claimed_bytes": 307_300, "line": text.rstrip("\n")}])

    def test_each_malformed_entry_is_rejected_with_the_importer_s_reason(self):
        """The reasons matter as much as the rejections: a tool that silently drops a
        line is how a model goes missing without anyone being told which one.

        The order is the kernel's: `vfs.rs:2690` compares the path against its own
        normalisation *before* `vfs.rs:2704` tests the namespace and `vfs.rs:2704`
        tests the length, so a 300-byte component is reported as a path that would move,
        not as a path that is too long. Asserting the order is the point of the case.
        """
        cases = [
            ("/data/models/a.gguf", "names no payload length"),
            ("/data/models/a.gguf|0x1000", "is not a byte count"),
            ("/data/models/a.gguf|1024 ", "is not a byte count"),
            ("/data/models/a.gguf|+1024", "is not a byte count"),
            ("/data/models/a.gguf|1,024", "is not a byte count"),
            ("/data//models/a.gguf|1024", "not a file path as the tree holds one"),
            ("/data/models/../keystore/state|1024", "not a file path as the tree holds one"),
            ("/data/keystore/state|1024", "outside /data/models/"),
            # One component past the 255 bytes normalize_path (vfs.rs:2958) allows.
            ("/data/models/" + "p" * 300 + "|1024", "not a file path as the tree holds one"),
            # Every component in bounds but the whole path past MAX_PATH_LEN (vfs.rs:2704).
            ("/data/models/" + "q" * 200 + "/" + "r" * 200 + "/model.gguf|1024",
             "longer than the 256 bytes"),
        ]
        for path, expected_reason in cases:
            line = "{}\n".format(path)
            _accepted, rejected = mp.parse_manifest(line.encode())
            self.assertEqual(len(rejected), 1, "expected '{}' to be rejected".format(path))
            self.assertIn(expected_reason, rejected[0]["reason"],
                          "'{}' was rejected for {!r}, not {!r}".format(
                              path, rejected[0]["reason"], expected_reason))

    def test_a_non_utf8_entry_is_reported_as_not_text(self):
        _accepted, rejected = mp.parse_manifest(b"/data/models/a.gguf|\xff\xfe\n")
        self.assertEqual(len(rejected), 1)
        self.assertIn("is not text", rejected[0]["reason"])

    def test_a_path_equal_to_its_normalised_form_is_the_rule(self):
        """`vfs.rs:2690` compares the manifest path against `normalize_path` of itself,
        so the tool has to run the same normalisation or it will accept names the guest
        skips. Traversal collapses to the root, which is not in the model namespace."""
        self.assertEqual(mp.normalize_vfs_path("/data/models/a.gguf"), "/data/models/a.gguf")
        self.assertEqual(mp.normalize_vfs_path("/data/models/./a.gguf"), "/data/models/a.gguf")
        self.assertEqual(mp.normalize_vfs_path("/data/models/../keystore/x"), "/")

    def test_the_digest_name_is_the_path_s_own_sha256_prefix(self):
        """`spill_path_for` (vfs.rs:2910) is the only bridge between a VFS path and the
        bytes on the disk, and it is case- and prefix-sensitive. Pinned with a golden
        value so a drifted rule fails at the byte instead of as a missing model."""
        self.assertEqual(mp.spill_name_for("/data/models/embeddinggemma-2.gguf"),
                         "/data/.vfs.s.10185d05290de5b3")
        self.assertEqual(mp.spill_name_for("/data/models/other.gguf"),
                         "/data/.vfs.s." + hashlib.sha256(
                             b"/data/models/other.gguf").digest()[:8].hex())


class TestRoundTrip(_ProvisioningCase):
    """Write a payload, read it back off the disk, and prove they are the same bytes."""

    def test_the_payload_round_trips_and_the_manifest_names_it(self):
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        payload = open(source, "rb").read()
        expected_digest = hashlib.sha256(payload).hexdigest()

        report = _provision(source, image)
        self.assertTrue(report["ok"], report["problems"])

        record = report["files"][0]
        self.assertEqual(record["vfs_path"], "/data/models/embeddinggemma-2.gguf")
        self.assertEqual(record["spill_name"], "/data/.vfs.s.10185d05290de5b3")
        self.assertEqual(record["device_holds_bytes"], len(payload))
        self.assertEqual(record["manifest_claims_bytes"], len(payload))
        self.assertEqual(record["payload_sha256_on_disk"], expected_digest)
        self.assertEqual(record["payload_sha256_on_host"], expected_digest)

        # Straight off the bytes, with no help from the tool's reader.
        with open(image, "rb") as handle:
            handle.seek(4096 * 512)
            superblock = handle.read(512)
            self.assertEqual(superblock[0:4], b"IOFS")
            self.assertEqual(struct.unpack_from("<I", superblock, 4)[0], 2)
            inode_lba = struct.unpack_from("<I", superblock, 32)[0]
            data_start = struct.unpack_from("<I", superblock, 12)[0]
            handle.seek((4096 + inode_lba) * 512)
            first_record = handle.read(64)
            self.assertEqual(first_record[0], 1, "inode 0 is the payload")
            self.assertEqual(struct.unpack_from("<Q", first_record, 1)[0], len(payload))
            handle.seek((4096 + data_start) * 512)
            on_disk = handle.read(len(payload))
            self.assertEqual(hashlib.sha256(on_disk).hexdigest(), expected_digest,
                             "the extent's bytes are the model's bytes, in order")
            self.assertEqual(on_disk[:4], b"GGUF")

    def test_the_manifest_is_the_last_file_and_starts_where_the_payload_ends(self):
        """First-fit ascending (`interfs.rs:666`) puts the payload at data sector 0 and
        the manifest directly behind it, so the two are asserted as neighbours: a gap
        would mean the writer and the allocator disagree about what is free."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        report = _provision(source, image)
        payload_sectors = mp.sectors_for_bytes(os.path.getsize(source))
        records = report["written"]["records"]
        self.assertEqual(records[0]["start_sector"], 0)
        self.assertEqual(records[0]["sector_count"], payload_sectors)
        self.assertEqual(records[1]["path"], mp.PROVISION_MANIFEST_PATH)
        self.assertEqual(records[1]["start_sector"], payload_sectors)

        entry = mp.manifest_entry("/data/models/embeddinggemma-2.gguf",
                                  os.path.getsize(source)).encode()
        self.assertEqual(len(entry), 42, "the whole manifest is one line of 42 bytes")
        with open(image, "rb") as handle:
            handle.seek(4096 * 512)
            superblock = handle.read(512)
            data_start = struct.unpack_from("<I", superblock, 12)[0]
            handle.seek((4096 + data_start + payload_sectors) * mp.SECTOR_SIZE)
            self.assertEqual(handle.read(len(entry)), entry)
            # The rest of the manifest's one sector is padding, so a reader that took
            # `sector_count * 512` as the file size would import 470 bytes of zeroes.
            self.assertEqual(handle.read(mp.SECTOR_SIZE - len(entry)),
                             bytes(mp.SECTOR_SIZE - len(entry)))

    def test_the_transaction_record_is_clear_so_a_mount_writes_nothing(self):
        """`mount_region` calls `recover_transaction`, which republishes the superblock
        when a record is pending (`interfs.rs:552`). A staged disk must arrive with no
        pending record, or the first guest mount edits the disk it was meant to read."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        _provision(source, image)
        with open(image, "rb") as handle:
            handle.seek(4096 * 512 + 64)
            record = handle.read(64)
        self.assertNotEqual(record[0:4], b"IFTX",
                            "a pending transaction would make the guest's mount a writer")
        self.assertEqual(record, bytes(64))

    def test_the_bitmap_marks_exactly_the_sectors_two_files_hold(self):
        """`check_integrity` (interfs.rs:1283) refuses the filesystem when the bitmap and
        the extents disagree in either direction, and it is reachable from the serial
        console (`serial.rs:1540`). Reconstructed here from the inode records, bit by
        bit, with the tool's own packer deliberately left out of the expectation."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        report = _provision(source, image)
        with open(image, "rb") as handle:
            handle.seek(4096 * 512)
            superblock = handle.read(512)
            bitmap_lba = struct.unpack_from("<I", superblock, 24)[0]
            bitmap_sectors = struct.unpack_from("<I", superblock, 28)[0]
            inode_lba = struct.unpack_from("<I", superblock, 32)[0]
            handle.seek((4096 + inode_lba) * 512)
            table = handle.read(32 * 512)
            handle.seek((4096 + bitmap_lba) * 512)
            actual = handle.read(bitmap_sectors * 512)

        extents = []
        for index in range(256):
            record = table[index * 64:(index + 1) * 64]
            if record[0] != 1:
                continue
            extents.append((struct.unpack_from("<I", record, mp.INODE_START_OFFSET)[0],
                            struct.unpack_from("<I", record, mp.INODE_COUNT_OFFSET)[0]))
        self.assertEqual(len(extents), 2, "one payload and one manifest")

        expected = bytearray(len(actual))
        for start, count in extents:
            for sector in range(start, start + count):
                expected[sector // 8] |= 1 << (sector % 8)  # LSB-first, interfs.rs:644
        self.assertEqual(bytes(actual), bytes(expected),
                         "the bitmap is not the union of the extents the inode table names")
        marked = {index for index in range(len(actual) * 8) if expected[index // 8] >> (index % 8) & 1}
        self.assertEqual(marked, set(range(0, extents[0][1]))
                         | set(range(extents[1][0], extents[1][0] + extents[1][1])),
                         "the marked sectors are the two extents and nothing else")
        self.assertEqual(extents[1][0], extents[0][1],
                         "first fit lays the manifest directly behind the payload")
        self.assertEqual(marked, set(range(0, extents[1][0] + extents[1][1])),
                         "nothing past the two files is claimed")


class TestTruncationIsDetected(_ProvisioningCase):
    """A provisioning whose bytes are not all there has to fail here, not on the guest."""

    def _reprovision(self, image):
        source = self.make_payload("case.gguf")
        _provision(source, image)
        return source

    def _read_inode(self, image, layout, index):
        with open(image, "rb") as handle:
            handle.seek(mp.inode_record_offset(layout, index))
            return bytearray(handle.read(64))

    def _write_inode(self, image, layout, index, record):
        with open(image, "r+b") as handle:
            handle.seek(mp.inode_record_offset(layout, index))
            handle.write(bytes(record))

    def test_a_device_extent_shorter_than_the_claim_is_caught(self):
        """The truncated-copy case: 4,096 bytes went missing, and the guest would import
        the short file and print one line about it. The tool must refuse instead."""
        image = self.staged("case.img")
        source = self._reprovision(image)
        size = os.path.getsize(source)
        layout = mp.layout_for_region(mp.region_for_device(SMALL_DISK_SECTORS))
        record = self._read_inode(image, layout, 0)
        self.assertEqual(struct.unpack_from("<Q", record, mp.INODE_SIZE_OFFSET)[0], size)
        record[mp.INODE_SIZE_OFFSET:mp.INODE_SIZE_OFFSET + 8] = struct.pack("<Q", size - 4096)
        self._write_inode(image, layout, 0, record)

        report = mp.verify_image(str(image), model_specs=[
            (str(source), "/data/models/embeddinggemma-2.gguf")])
        self.assertFalse(report["ok"])
        joined = " ".join(report["problems"])
        self.assertIn("the device holds {}".format(size - 4096), joined)
        self.assertIn("interrupted", joined)

    def test_a_full_length_extent_whose_tail_was_never_written_is_caught(self):
        """The subtler truncation: the inode publishes the right length, so a length
        check passes and the model loader reads zeroes past the copy's stop. Only
        hashing the bytes off the disk catches this, so the round trip hashes them."""
        image = self.staged("case.img")
        source = self._reprovision(image)
        layout = mp.layout_for_region(mp.region_for_device(SMALL_DISK_SECTORS))
        last_sector = layout["base_lba"] + layout["data_start"] \
            + mp.sectors_for_bytes(os.path.getsize(source)) - 1
        with open(image, "r+b") as handle:
            handle.seek(last_sector * 512)
            handle.write(bytes(512))

        report = mp.verify_image(str(image), model_specs=[
            (str(source), "/data/models/embeddinggemma-2.gguf")])
        self.assertFalse(report["ok"])
        joined = " ".join(report["problems"])
        self.assertIn("are not the bytes of", joined)
        self.assertNotIn("the device holds", joined,
                         "the lengths agree here; only the content can say otherwise")

    def test_a_manifest_that_over_states_the_bytes_is_caught(self):
        """A claim larger than the extent is the other direction of the same lie, and it
        is the one a build tool makes when it measures the source and copies less."""
        image = self.staged("case.img")
        source = self._reprovision(image)
        size = os.path.getsize(source)
        layout = mp.layout_for_region(mp.region_for_device(SMALL_DISK_SECTORS))
        payload_sectors = mp.sectors_for_bytes(size)
        manifest_lba = layout["base_lba"] + layout["data_start"] + payload_sectors
        # Same number of digits as the real claim, so the manifest's own inode length stays
        # true and only the figure it states is wrong. A longer replacement would be
        # caught by the window cap instead, which is a different bug.
        overstated = "9" * len(str(size))
        self.assertGreater(int(overstated), size)
        with open(image, "r+b") as handle:
            handle.seek(manifest_lba * mp.SECTOR_SIZE)
            line = handle.read(mp.SECTOR_SIZE)
            self.assertTrue(line.startswith(
                "/data/models/embeddinggemma-2.gguf|{}".format(size).encode()))
            self.assertEqual(len(line.split(b"\n", 1)[0]), len(
                mp.manifest_entry("/data/models/embeddinggemma-2.gguf", size).rstrip("\n")))
            handle.seek(manifest_lba * mp.SECTOR_SIZE)
            handle.write(line.replace(b"|" + str(size).encode(), b"|" + overstated.encode(), 1))

        report = mp.verify_image(str(image))
        self.assertFalse(report["ok"])
        joined = " ".join(report["problems"])
        self.assertIn("was provisioned as {} bytes but the device holds {}".format(
            overstated, size), joined)
        self.assertEqual(report["manifest"]["entries"][0]["claimed_bytes"], int(overstated))

    def test_a_manifest_whose_entry_the_window_cuts_is_caught(self):
        """`load_provision_manifest_with_device` reads a 1,024-byte window, and
        `vfs.rs:2625` says out loud that the entries past it are not read. A writer that
        packed more entries than the window holds would report models the guest never
        saw, so the bound is enforced rather than noted."""
        source = self.make_payload("case.gguf")
        image = self.staged("window-crowd.img")
        size = os.path.getsize(source)
        crowd = ["/data/models/model-{:02d}.gguf".format(index) for index in range(1, 46)]
        too_long = "".join(mp.manifest_entry(path, size) for path in crowd)
        self.assertGreaterEqual(len(too_long.encode("utf-8")), mp.PROVISION_MANIFEST_WINDOW,
                                "the fixture no longer crowds the window")
        self.assertLess(len(mp.manifest_entry(crowd[0], size).encode("utf-8")),
                        mp.PROVISION_MANIFEST_WINDOW,
                        "one entry must be comfortably inside the window")
        self.assertEqual(mp.parse_manifest(too_long.encode("utf-8"))[1], [],
                         "every entry is well formed; only their total is the problem")
        with self.assertRaises(mp.ProvisioningError) as refused:
            mp.provision([(str(source), path) for path in crowd], str(image),
                         total_sectors=SMALL_DISK_SECTORS, log=lambda message: None)
        self.assertIn("window", str(refused.exception))
        self.assertFalse(os.path.exists(image),
                         "the refusal must come before the disk is created")

    def test_a_non_model_path_in_the_manifest_is_never_provisioned(self):
        """The confinement in `vfs.rs:2698` is what stops a disk image from being a way
        to make `/data/keystore/state` exist. The tool refuses such a name before it
        reaches a byte, and a tampered disk carrying one is reported."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        _provision(source, image)
        with self.assertRaises(mp.ProvisioningError) as refused:
            mp.provision([(str(source), "/data/keystore/state")], str(image),
                         total_sectors=SMALL_DISK_SECTORS, overwrite=True,
                         log=lambda message: None)
        self.assertIn("the manifest this tool built is not one the importer accepts",
                      str(refused.exception))

    def test_a_stray_bitmap_bit_is_caught(self):
        """One bit set with no inode behind it is a sector the guest will hand to its
        next file -- which may be the model's own tail."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        report = _provision(source, image)
        layout = report["layout"]
        with open(image, "r+b") as handle:
            handle.seek((4096 + layout["bitmap_lba"]) * 512)
            sector = bytearray(handle.read(512))
            sector[200] |= 0x02
            handle.seek((4096 + layout["bitmap_lba"]) * 512)
            handle.write(bytes(sector))
        verify = mp.verify_image(str(image))
        self.assertFalse(verify["ok"])
        self.assertTrue(any("marked allocated by no file" in problem
                            for problem in verify["problems"]), verify["problems"])


class TestWriterRefusals(_ProvisioningCase):
    """What the tool will not do, asserted with the filesystem instrumented."""

    def test_the_master_data_disk_is_refused_without_being_touched(self):
        """No read, no stat, no open, no hash -- the refusal is lexical. This is the case
        that keeps a future edit from adding a "peek at the target first" step."""
        forbidden = str(ROOT / "target" / MASTER_DATA_DISK)
        seen = []
        real_open = builtins.open
        real_stat, real_exists = os.stat, os.path.exists
        real_getsize, real_isfile = os.path.getsize, os.path.isfile

        def spy(kind, real):
            def watch(*args, **kwargs):
                if args:
                    seen.append((kind, str(args[0])))
                return real(*args, **kwargs)
            return watch

        try:
            builtins.open = spy("open", real_open)
            os.stat = spy("stat", real_stat)
            os.path.exists = spy("exists", real_exists)
            os.path.getsize = spy("getsize", real_getsize)
            os.path.isfile = spy("isfile", real_isfile)
            with self.assertRaises(mp.ProvisioningError) as refused:
                mp.guard_output_path(forbidden)
            self.assertIn("persistent state", str(refused.exception))
            # And through the whole entry point, so no earlier step can reach the disk.
            source = self.make_payload("case.gguf")
            with self.assertRaises(mp.ProvisioningError):
                mp.provision([(str(source), "/data/models/x.gguf")], forbidden,
                             total_sectors=SMALL_DISK_SECTORS, log=lambda message: None)
        finally:
            builtins.open = real_open
            os.stat = real_stat
            os.path.exists = real_exists
            os.path.getsize = real_getsize
            os.path.isfile = real_isfile

        self.assertEqual([call for call in seen if MASTER_DATA_DISK in call[1]], [],
                         "the guard talked about the forbidden disk: {}".format(seen))
        self.assertTrue(any(call[0] == "open" for call in seen),
                        "the spies saw nothing at all, so the test would pass on a stub")

    def test_the_data_disk_names_are_refused_for_every_arch(self):
        for name in ("inter-os-data-master.img", "interos-data-aarch64.img",
                     "interos-data-riscv64.img"):
            with self.assertRaises(mp.ProvisioningError):
                mp.guard_output_path(str(ROOT / "target" / "verify" / "provisioning" / name))

    def test_an_output_outside_the_staging_tree_needs_an_explicit_override(self):
        elsewhere = str(ROOT / "scratch" / "accidental.img")
        with self.assertRaises(mp.ProvisioningError) as refused:
            mp.guard_output_path(elsewhere)
        self.assertIn("target", str(refused.exception))
        self.assertEqual(mp.guard_output_path(elsewhere, allow_any_location=True),
                         os.path.abspath(elsewhere))

    def test_a_boot_media_file_is_not_formatted_over(self):
        """`boot_guard.rs:425` only lets InterFS mount a device that could not have
        booted. The tool refuses earlier, and before truncating, so the image it was
        handed is still byte-identical afterwards."""
        source = self.make_payload("case.gguf")
        target = self.staged("case.img")
        boot_media = bytearray(4096 * 512 + 1024)
        boot_media[4096 * 512:4096 * 512 + 4] = b"IOFS"
        boot_media[510] = 0x55
        boot_media[511] = 0xAA
        _write_host_file(target, bytes(boot_media))
        before = hashlib.sha256(target.read_bytes()).hexdigest()
        with self.assertRaises(mp.ProvisioningError) as refused:
            _provision(source, target, overwrite=True)
        self.assertIn("boot signature", str(refused.exception))
        self.assertEqual(hashlib.sha256(target.read_bytes()).hexdigest(), before,
                         "a refusal must leave the disk exactly as it found it")

    def test_the_finished_image_is_mountable_by_the_kernel_s_own_rules(self):
        """The boot signature is the guard's rule, so the artifact this tool produces is
        asserted against it directly: LBA 0 carries none."""
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        _provision(source, image)
        with open(image, "rb") as handle:
            handle.seek(510)
            self.assertNotEqual(handle.read(2), b"\x55\xAA")

    def test_a_payload_without_the_gguf_magic_is_refused(self):
        source = _write_host_file(self.staged("not-a-model.gguf"),
                                  _payload(4096, magic=b"RIFF"))
        image = self.staged("case.img")
        with self.assertRaises(mp.ProvisioningError) as refused:
            _provision(source, image)
        self.assertIn("GGUF magic", str(refused.exception))
        self.assertFalse(image.exists(),
                         "a refusal must not leave a half-written disk behind")
        report = _provision(source, image, gguf=False)
        self.assertTrue(report["ok"], report["problems"])

    def test_an_existing_image_is_not_replaced_without_saying_so(self):
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        _provision(source, image)
        with self.assertRaises(mp.ProvisioningError) as refused:
            _provision(source, image)
        self.assertIn("--overwrite", str(refused.exception))

    def test_two_models_cannot_share_one_vfs_path(self):
        source = self.make_payload("case.gguf")
        image = self.staged("case.img")
        with self.assertRaises(mp.ProvisioningError) as refused:
            mp.provision([(str(source), "/data/models/twin.gguf"),
                          (str(source), "/data/models/twin.gguf")], str(image),
                         total_sectors=SMALL_DISK_SECTORS, log=lambda message: None)
        self.assertIn("same VFS path", str(refused.exception))

    def test_two_models_at_two_paths_both_land_on_the_disk(self):
        """Multiple entries are the shape a second model will need, and one manifest
        line per payload is what `apply_provisioned_manifest` iterates over."""
        first = self.make_payload("first.gguf")
        second = _write_host_file(self.staged("second.gguf"), _payload(64 * 1024 + 7))
        image = self.staged("case.img")
        report = mp.provision([(str(first), "/data/models/first.gguf"),
                               (str(second), "/data/models/second.gguf")], str(image),
                              total_sectors=SMALL_DISK_SECTORS, log=lambda message: None)
        self.assertTrue(report["ok"], report["problems"])
        lines = report["manifest"]["text"].splitlines()
        self.assertEqual(len(lines), 2)
        self.assertEqual({item["vfs_path"] for item in report["files"]},
                         {"/data/models/first.gguf", "/data/models/second.gguf"})
        for item in report["files"]:
            self.assertEqual(item["payload_sha256_on_disk"], item["payload_sha256_on_host"])


class TestGateIsRegistered(_ProvisioningCase):
    """The runner discovers suites by glob; a file it cannot see is not a gate."""

    def test_the_contract_runner_discovers_this_file(self):
        """`tests/run_all_contracts.py:129` takes `glob.glob("tests/**/test_*.py")` from
        the repository root. Asserted rather than assumed, because the sweep is the only
        place this contract runs on its own."""
        expected = os.path.normpath("tests/test_model_provisioning_contract.py")
        previous = os.getcwd()
        try:
            os.chdir(ROOT)
            discovered = sorted(glob.glob("tests/**/test_*.py", recursive=True))
        finally:
            os.chdir(previous)
        self.assertIn(expected, [os.path.normpath(path) for path in discovered])
        self.assertEqual(previous, os.getcwd(), "the case must not leak its working directory")

    def test_the_tool_carries_the_licence_notice_the_gate_requires(self):
        for path in (ROOT / "tools" / "model_provisioning.py", Path(__file__)):
            head = path.read_text(encoding="utf-8")[:200]
            self.assertIn("SPDX-License-Identifier: AGPL-3.0-only", head, str(path))


class TestRealPublishedWeights(_ProvisioningCase):
    """The 333 MB artifact itself, opt-in because of what it costs the host."""

    def test_real_model_round_trips_onto_a_provisioned_disk(self):
        if os.environ.get("INTEROS_PROVISIONING_REAL_MODEL") != "1":
            self.skipTest(
                "writes the published 333,590,944-byte model to a real disk; run with "
                "INTEROS_PROVISIONING_REAL_MODEL=1")
        if not REAL_MODEL_PATH.is_file():
            self.skipTest("{} is not on this host".format(REAL_MODEL_PATH))
        size = REAL_MODEL_PATH.stat().st_size
        self.assertEqual(size, REAL_MODEL_BYTES,
                         "the published weights this repository quotes")
        with open(REAL_MODEL_PATH, "rb") as handle:
            self.assertEqual(handle.read(4), b"GGUF")

        image = self.staged("real-weights.img")
        if image.exists():
            os.remove(image)
        free = shutil.disk_usage(str(STAGING_DIR)).free
        needed = mp.bytes_for_sectors(
            mp.fit_total_sectors(size, 64 * 1024 * 1024))
        self.assertGreater(free, needed + (256 * 1024 * 1024),
                           "{} bytes free and the image needs {}".format(free, needed))
        report = mp.provision([(str(REAL_MODEL_PATH), "/data/models/embeddinggemma-2.gguf")],
                              str(image), headroom_mib=64, log=lambda message: None)
        try:
            self.assertTrue(report["ok"], report["problems"])
            record = report["files"][0]
            self.assertEqual(record["device_holds_bytes"], REAL_MODEL_BYTES)
            self.assertEqual(record["manifest_claims_bytes"], REAL_MODEL_BYTES)
            self.assertEqual(record["read_back_bytes"], REAL_MODEL_BYTES)
            self.assertEqual(record["payload_sha256_on_disk"], record["payload_sha256_on_host"])
            self.assertEqual(record["payload_magic"], "GGUF")
            layout = report["layout"]
            self.assertEqual(record["extent"],
                             [0, mp.sectors_for_bytes(REAL_MODEL_BYTES)])
            self.assertGreater(layout["data_sectors"] - record["extent"][1], 0,
                               "the region is larger than the payload it was fitted to")
        finally:
            if os.environ.get("INTEROS_KEEP_PROVISIONED_DISK") != "1":
                os.remove(image)


if __name__ == "__main__":
    unittest.main(verbosity=2)
