//! Host-side test harness for the shipped AArch64 FDT parser.
//!
//! The production parser is included by path so these tests observe the exact
//! code that runs on the kernel, including its bounds handling. A device tree is
//! bootloader-supplied and therefore untrusted: every case below is a tree a
//! hostile or buggy bootloader could hand over.

#[path = "../../../../kernel-aarch64/src/fdt.rs"]
pub mod fdt;

// The GIC driver only needs the parser, so it can be exercised here too.
#[path = "../../../../kernel-aarch64/src/gic.rs"]
pub mod gic;

extern crate alloc;

use fdt::{ArmFdtParser, FdtError};
use gic::IntcDescription;

const FDT_MAGIC: u32 = 0xD00D_FEED;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const END: u32 = 9;

/// Accumulates a device tree the way a generator would emit one.
#[derive(Default)]
struct Builder {
    tokens: Vec<u8>,
    strings: Vec<u8>,
}

impl Builder {
    fn new() -> Self {
        Self::default()
    }

    fn intern(&mut self, name: &str) -> u32 {
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        offset
    }

    fn push_u32(&mut self, value: u32) {
        self.tokens.extend_from_slice(&value.to_be_bytes());
    }

    fn begin_node(&mut self, name: &str) {
        self.push_u32(BEGIN_NODE);
        self.tokens.extend_from_slice(name.as_bytes());
        self.tokens.push(0);
        while self.tokens.len() % 4 != 0 {
            self.tokens.push(0);
        }
    }

    fn end_node(&mut self) {
        self.push_u32(END_NODE);
    }

    fn prop(&mut self, name: &str, value: &[u8]) {
        let name_off = self.intern(name);
        self.push_u32(PROP);
        self.push_u32(value.len() as u32);
        self.push_u32(name_off);
        self.tokens.extend_from_slice(value);
        while self.tokens.len() % 4 != 0 {
            self.tokens.push(0);
        }
    }

    fn finish(self) -> Vec<u8> {
        let mut blob = Vec::new();
        let off_mem_rsvmap = 40usize;
        let off_dt_struct = off_mem_rsvmap + 16; // one terminating entry
        let mut tokens = self.tokens.clone();
        tokens.extend_from_slice(&END.to_be_bytes());
        let off_dt_strings = off_dt_struct + tokens.len();

        let totalsize = off_dt_strings + self.strings.len();

        blob.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        blob.extend_from_slice(&(totalsize as u32).to_be_bytes());
        blob.extend_from_slice(&(off_dt_struct as u32).to_be_bytes());
        blob.extend_from_slice(&(off_dt_strings as u32).to_be_bytes());
        blob.extend_from_slice(&(off_mem_rsvmap as u32).to_be_bytes());
        blob.extend_from_slice(&17u32.to_be_bytes()); // version
        blob.extend_from_slice(&16u32.to_be_bytes()); // last_comp_version
        blob.extend_from_slice(&0u32.to_be_bytes()); // boot_cpuid_phys
        blob.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());
        blob.extend_from_slice(&(tokens.len() as u32).to_be_bytes());

        blob.extend_from_slice(&0u64.to_be_bytes()); // reserved map terminator
        blob.extend_from_slice(&0u64.to_be_bytes());

        blob.extend_from_slice(&tokens);
        blob.extend_from_slice(&self.strings);
        blob
    }
}

fn be_u64(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

fn be_u32(value: u32) -> [u8; 4] {
    value.to_be_bytes()
}

/// A tree with root `#address-cells`/`#size-cells` of 2 and one 512 MiB bank.
fn single_bank_tree() -> Vec<u8> {
    let mut b = Builder::new();
    b.begin_node("");
    b.prop("#address-cells", &be_u32(2));
    b.prop("#size-cells", &be_u32(2));
    b.begin_node("memory@40000000");
    b.prop("device_type", b"memory\0");
    let mut reg = Vec::new();
    reg.extend_from_slice(&be_u64(0x4000_0000));
    reg.extend_from_slice(&be_u64(512 * 1024 * 1024));
    b.prop("reg", &reg);
    b.end_node();
    b.end_node();
    b.finish()
}

#[test]
fn parses_memory_from_a_valid_tree() {
    let blob = single_bank_tree();
    let parser = ArmFdtParser::new(&blob).expect("valid tree must parse");
    let (base, size) = parser.parse_memory_range().expect("memory must be found");
    assert_eq!(base, 0x4000_0000);
    assert_eq!(size, 512 * 1024 * 1024);
}

#[test]
fn sums_every_memory_bank() {
    let mut b = Builder::new();
    b.begin_node("");
    b.prop("#address-cells", &be_u32(2));
    b.prop("#size-cells", &be_u32(2));
    for (addr, len) in [
        (0x4000_0000u64, 0x2000_0000u64),
        (0x8000_0000u64, 0x1000_0000u64),
    ] {
        b.begin_node("memory");
        b.prop("device_type", b"memory\0");
        let mut reg = Vec::new();
        reg.extend_from_slice(&be_u64(addr));
        reg.extend_from_slice(&be_u64(len));
        b.prop("reg", &reg);
        b.end_node();
    }
    b.end_node();
    let blob = b.finish();
    let parser = ArmFdtParser::new(&blob).unwrap();
    let (base, size) = parser.parse_memory_range().unwrap();
    assert_eq!(base, 0x4000_0000, "must report the lowest bank");
    assert_eq!(
        size, 0x3000_0000,
        "must total every bank, not just the first"
    );
}

#[test]
fn rejects_bad_magic() {
    let mut blob = single_bank_tree();
    blob[0] = 0x00;
    assert_eq!(ArmFdtParser::new(&blob).err(), Some(FdtError::InvalidMagic));
}

#[test]
fn rejects_blob_shorter_than_header() {
    assert_eq!(
        ArmFdtParser::new(&[0u8; 8]).err(),
        Some(FdtError::BufferTooShort)
    );
}

#[test]
fn rejects_totalsize_larger_than_blob() {
    let mut blob = single_bank_tree();
    blob[4..8].copy_from_slice(&1_000_000u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::BufferTooShort)
    );
}

#[test]
fn rejects_structure_block_past_totalsize() {
    // Without this check the walk limit points beyond the blob and every token
    // read becomes an out-of-bounds access.
    let mut blob = single_bank_tree();
    blob[36..40].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::CorruptedStructure)
    );
}

#[test]
fn rejects_structure_offset_past_totalsize() {
    let mut blob = single_bank_tree();
    blob[8..12].copy_from_slice(&0xFFFF_FFFCu32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::CorruptedStructure)
    );
}

#[test]
fn rejects_strings_block_past_totalsize() {
    let mut blob = single_bank_tree();
    blob[32..36].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::CorruptedStructure)
    );
}

#[test]
fn rejects_reserve_map_past_totalsize() {
    let mut blob = single_bank_tree();
    let near_end = (blob.len() - 4) as u32;
    blob[16..20].copy_from_slice(&near_end.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::CorruptedStructure)
    );
}

#[test]
fn rejects_future_version() {
    let mut blob = single_bank_tree();
    blob[20..24].copy_from_slice(&18u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::UnsupportedVersion(18))
    );
}

#[test]
fn rejects_ancient_version() {
    let mut blob = single_bank_tree();
    blob[20..24].copy_from_slice(&15u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::UnsupportedVersion(15))
    );
}

#[test]
fn rejects_last_comp_version_newer_than_version() {
    let mut blob = single_bank_tree();
    // version is 17, so a last_comp_version above it is incoherent.
    blob[24..28].copy_from_slice(&18u32.to_be_bytes());
    assert_eq!(
        ArmFdtParser::new(&blob).err(),
        Some(FdtError::CorruptedStructure)
    );
}

/// The property length is the classic FDT attack: a huge value must be refused
/// rather than sliced out of the blob.
#[test]
fn rejects_property_length_past_structure_block() {
    let mut b = Builder::new();
    b.begin_node("");
    b.prop("reg", &be_u64(0x4000_0000));
    b.end_node();
    let mut blob = b.finish();

    // Locate the property token and overwrite its length field.
    let off_dt_struct = u32::from_be_bytes(blob[8..12].try_into().unwrap()) as usize;
    let mut cursor = off_dt_struct;
    loop {
        let token = u32::from_be_bytes(blob[cursor..cursor + 4].try_into().unwrap());
        if token == PROP {
            break;
        }
        cursor += 4;
        assert!(cursor < off_dt_struct + 4096, "prop token not found");
    }
    blob[cursor + 4..cursor + 8].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());

    let parser = ArmFdtParser::new(&blob).expect("header is still self-consistent");
    let node = parser.find_node_by_name("").expect("root node is findable");
    assert_eq!(
        parser.get_node_property(&node, "reg").err(),
        Some(FdtError::CorruptedStructure),
        "an oversized property length must not produce a slice"
    );
}

#[test]
fn rejects_property_length_just_past_structure_block() {
    let mut b = Builder::new();
    b.begin_node("");
    b.prop("reg", &be_u64(0x4000_0000));
    b.end_node();
    let mut blob = b.finish();

    let off_dt_struct = u32::from_be_bytes(blob[8..12].try_into().unwrap()) as usize;
    let size_dt_struct = u32::from_be_bytes(blob[36..40].try_into().unwrap()) as usize;
    let mut cursor = off_dt_struct;
    while u32::from_be_bytes(blob[cursor..cursor + 4].try_into().unwrap()) != PROP {
        cursor += 4;
    }
    // Claim more than the whole structure block holds.
    let oversized = size_dt_struct as u32;
    blob[cursor + 4..cursor + 8].copy_from_slice(&oversized.to_be_bytes());

    let parser = ArmFdtParser::new(&blob).unwrap();
    let node = parser.find_node_by_name("").unwrap();
    assert!(parser.get_node_property(&node, "reg").is_err());
}

#[test]
fn rejects_property_name_outside_strings_block() {
    let mut b = Builder::new();
    b.begin_node("");
    b.prop("reg", &be_u64(0x4000_0000));
    b.end_node();
    let mut blob = b.finish();

    let off_dt_struct = u32::from_be_bytes(blob[8..12].try_into().unwrap()) as usize;
    let mut cursor = off_dt_struct;
    while u32::from_be_bytes(blob[cursor..cursor + 4].try_into().unwrap()) != PROP {
        cursor += 4;
    }
    // Point the name offset past the strings block entirely.
    blob[cursor + 8..cursor + 12].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());

    let parser = ArmFdtParser::new(&blob).unwrap();
    let node = parser.find_node_by_name("").unwrap();
    assert_eq!(
        parser.get_node_property(&node, "reg").err(),
        Some(FdtError::CorruptedStructure)
    );
}

#[test]
fn rejects_unterminated_node_name() {
    let mut b = Builder::new();
    b.begin_node("memory@1");
    b.prop("reg", &be_u64(0x4000_0000));
    b.end_node();
    let mut blob = b.finish();

    // Fill everything from the node-name region to the end of the structure
    // block with non-zero bytes so no name terminator remains.
    let off_dt_struct = u32::from_be_bytes(blob[8..12].try_into().unwrap()) as usize;
    let size_dt_struct = u32::from_be_bytes(blob[36..40].try_into().unwrap()) as usize;
    let struct_end = off_dt_struct + size_dt_struct;

    // The last node in the tree is "memory@1"; its name starts after its token.
    let mut cursor = off_dt_struct;
    let mut last_name_start = None;
    while cursor + 4 <= struct_end {
        let token = u32::from_be_bytes(blob[cursor..cursor + 4].try_into().unwrap());
        if token == BEGIN_NODE {
            last_name_start = Some(cursor + 4);
        }
        if token == END_NODE || token == END {
            break;
        }
        cursor += 4;
    }
    let name_start = last_name_start.expect("tree must contain a node");
    assert!(name_start < struct_end);
    for byte in &mut blob[name_start..struct_end] {
        *byte = b'x';
    }

    assert_eq!(
        ArmFdtParser::new(&blob)
            .unwrap()
            .find_node_by_name("memory")
            .err(),
        Some(FdtError::CorruptedStructure)
    );
}

/// `intc` must not select `intcmux`: binding MMIO to the wrong node is a
/// security-relevant failure, not a cosmetic one.
#[test]
fn node_prefix_does_not_match_a_different_node() {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("intcmux@8000000");
    b.prop("compatible", b"vendor,intcmux\0");
    b.prop("reg", &be_u64(0x8000_0000));
    b.end_node();
    b.begin_node("intc@8001000");
    b.prop("compatible", b"arm,gic-400\0");
    b.prop("reg", &be_u64(0x8001_0000));
    b.end_node();
    b.end_node();
    let blob = b.finish();

    let parser = ArmFdtParser::new(&blob).unwrap();
    let node = parser
        .find_node_by_name("intc")
        .expect("intc must be found");
    assert!(
        node.name.starts_with("intc@"),
        "prefix match selected {:?}",
        node.name
    );

    let by_compatible = parser.find_node_by_compatible("arm,gic-400").unwrap();
    assert!(by_compatible.name.starts_with("intc@"));
}

#[test]
fn compatible_requires_an_exact_entry() {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("uart");
    b.prop("compatible", b"arm,pl011\0vendor,other-uart\0");
    b.prop("reg", &be_u64(0x0900_0000));
    b.end_node();
    b.end_node();
    let blob = b.finish();

    let parser = ArmFdtParser::new(&blob).unwrap();
    assert!(parser.find_node_by_compatible("arm,pl011").is_ok());
    // A substring of a listed string is not a match.
    assert!(parser.find_node_by_compatible("pl011").is_err());
    assert!(parser.find_node_by_compatible("vendor,other-uart").is_ok());
}

#[test]
fn bootargs_drops_the_terminator() {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("chosen");
    b.prop("bootargs", b"console=ttyAMA0 quiet\0");
    b.end_node();
    b.end_node();
    let blob = b.finish();

    let parser = ArmFdtParser::new(&blob).unwrap();
    assert_eq!(parser.get_bootargs().unwrap(), "console=ttyAMA0 quiet");
}

#[test]
fn missing_node_and_property_report_not_found() {
    let blob = single_bank_tree();
    let parser = ArmFdtParser::new(&blob).unwrap();
    assert_eq!(
        parser.find_node_by_name("chosen").err(),
        Some(FdtError::NodeNotFound)
    );
    let root = parser.find_node_by_name("").unwrap();
    assert_eq!(
        parser.get_node_property(&root, "nope").err(),
        Some(FdtError::PropertyNotFound)
    );
}

#[test]
fn nested_node_properties_are_not_confused_with_the_parent() {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("child");
    b.prop("reg", &be_u64(0xDEAD_BEEF));
    b.end_node();
    b.end_node();
    let blob = b.finish();

    let parser = ArmFdtParser::new(&blob).unwrap();
    let root = parser.find_node_by_name("").unwrap();
    assert_eq!(
        parser.get_node_property(&root, "reg").err(),
        Some(FdtError::PropertyNotFound),
        "a child's property must not resolve against its parent"
    );
    let child = parser.find_node_by_name("child").unwrap();
    assert_eq!(
        parser.get_node_property(&child, "reg").unwrap(),
        &be_u64(0xDEAD_BEEF)[..]
    );
}

/// Mirrors the QEMU `virt` layout: an `intc` node carrying the marker property
/// and a nested v2m distributor frame.
fn qemu_virt_gicv2m_tree() -> Vec<u8> {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("intc@8000000");
    b.prop("compatible", b"arm,cortex-a15-gic\0");
    b.prop("interrupt-controller", b"");
    b.prop("#interrupt-cells", &be_u32(3));
    b.prop("#address-cells", &be_u32(2));
    b.prop("#size-cells", &be_u32(2));
    let mut reg = Vec::new();
    reg.extend_from_slice(&be_u64(0x0800_0000));
    reg.extend_from_slice(&be_u64(0x0003_0000));
    b.prop("reg", &reg);
    b.begin_node("v2m@8020000");
    b.prop("compatible", b"arm,gic-v2m-frame\0");
    let mut reg2 = Vec::new();
    reg2.extend_from_slice(&be_u64(0x0802_0000));
    reg2.extend_from_slice(&be_u64(0x0000_2000));
    b.prop("reg", &reg2);
    b.end_node();
    b.end_node();
    b.end_node();
    b.finish()
}

/// A GICv2 declares the distributor and CPU interface as two windows.
fn gicv2_tree() -> Vec<u8> {
    let mut b = Builder::new();
    b.begin_node("");
    b.begin_node("intc@8000000");
    b.prop("compatible", b"arm,gic-400\0");
    b.prop("interrupt-controller", b"");
    b.prop("#address-cells", &be_u32(2));
    b.prop("#size-cells", &be_u32(2));
    let mut reg = Vec::new();
    reg.extend_from_slice(&be_u64(0x0800_0000));
    reg.extend_from_slice(&be_u64(0x0000_1000));
    reg.extend_from_slice(&be_u64(0x0801_0000));
    reg.extend_from_slice(&be_u64(0x0000_1000));
    b.prop("reg", &reg);
    b.end_node();
    b.end_node();
    b.finish()
}

#[test]
fn gicv2_windows_are_taken_from_the_device_tree() {
    let blob = gicv2_tree();
    let parser = ArmFdtParser::new(&blob).unwrap();
    assert_eq!(
        gic::describe_interrupt_controller(&parser),
        IntcDescription::Gicv2 {
            distributor: 0x0800_0000,
            cpu_interface: 0x0801_0000,
        }
    );
}

/// GICv2m is recognised but not register-compatible with GICv2, so the driver
/// must decline rather than write GICv2 offsets into a v2m frame.
#[test]
fn gicv2m_is_reported_unsupported_not_as_gicv2() {
    let blob = qemu_virt_gicv2m_tree();
    let parser = ArmFdtParser::new(&blob).unwrap();
    match gic::describe_interrupt_controller(&parser) {
        IntcDescription::Unsupported(model) => {
            assert!(
                !model.is_empty(),
                "the recognised model must be named, not blank"
            );
        }
        other => panic!("expected Unsupported, got {:?}", other),
    }
}

#[test]
fn a_platform_without_an_interrupt_controller_reports_absent() {
    let blob = single_bank_tree();
    let parser = ArmFdtParser::new(&blob).unwrap();
    assert_eq!(
        gic::describe_interrupt_controller(&parser),
        IntcDescription::Absent
    );
}
