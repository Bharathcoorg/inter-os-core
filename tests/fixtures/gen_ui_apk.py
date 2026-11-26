# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Deterministic generator for a tiny but VALID Android APK whose own DEX bytecode
builds its view tree.

The generated ``classes.dex`` contains a real ``MainActivity.onCreate`` method whose
Dalvik bytecode does exactly what an app does to publish UI:

    new-instance   v0, Landroid/widget/TextView;
    invoke-direct  {v0}, Landroid/widget/TextView;-><init>(Landroid/content/Context;)V
    const-string   v1, "Hello from app code"
    invoke-virtual {v0, v1}, Landroid/widget/TextView;->setText(Ljava/lang/CharSequence;)V
    invoke-virtual {p0, v0}, Landroid/app/Activity;->setContentView(Landroid/view/View;)V
    return-void

The VM view model (``crates/interos-acr`` ``DalvikVmState::dispatch_framework_method``)
records that call chain as a real ``ObjectPayload::View`` tree, so a host test can
parse this DEX, run ``onCreate`` and observe ``vm.content_view()`` carrying the app's
own text. This proves the view was built by app code, not by a placeholder.

The binary layout reuses the exact DEX/APK structures already emitted by the repo's
Rust writers (``crates/interos-android-format`` ``dex::create_test_dex`` and
``apk::create_test_apk``): a 035 DEX header, string/type/proto/method/class_def
tables, a code item, a class_data item, and a stored (uncompressed) PKZip container
holding ``AndroidManifest.xml`` + ``classes.dex``. No new container or bytecode format
is introduced.

Output is byte-for-byte deterministic (no timestamps, fixed ordering). Regenerate with:

    python tests/fixtures/gen_ui_apk.py
"""

import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = Path(__file__).resolve().parent

APK_OUT = FIXTURES / "gen_ui_app.apk"
DEX_OUT = FIXTURES / "gen_ui_classes.dex"

DEX_MAGIC = b"dex\n035\x00"
DEX_HEADER_SIZE = 0x70
ENDIAN_CONSTANT = 0x12345678

# PKZip signatures (stored entries only, matching apk::create_test_apk).
ZIP_LOCAL_FILE_HEADER_SIG = 0x04034B50
ZIP_CENTRAL_DIR_SIG = 0x02014B50
ZIP_END_OF_CENTRAL_DIR_SIG = 0x06054B50

# AXML chunk types.
RES_XML_TYPE = 0x0003
RES_STRING_POOL_TYPE = 0x0001
RES_XML_START_ELEMENT_TYPE = 0x0102

# The app's own text. The host test asserts this exact string is present in the
# rendered content view, proving the app's bytecode (not a placeholder) built it.
APP_TEXT = "Hello from app code"
PACKAGE_NAME = "org.interos.genui"
APP_CLASS = "Lorg/interos/MainActivity;"

# String pool. Order defines every string index referenced below.
STRINGS = [
    "V",                              # 0  shorty / void type
    "Ljava/lang/Object;",             # 1  MainActivity superclass
    "Lorg/interos/MainActivity;",     # 2  app class
    "onCreate",                       # 3
    "Landroid/widget/TextView;",      # 4
    "<init>",                         # 5
    "Landroid/content/Context;",      # 6
    "setText",                        # 7
    "Ljava/lang/CharSequence;",       # 8
    "Landroid/app/Activity;",         # 9
    "setContentView",                 # 10
    "Landroid/view/View;",            # 11
    "Landroid/os/Bundle;",            # 12
    APP_TEXT,                         # 13
]

# type_ids: each entry is an index into STRINGS holding a type descriptor.
TYPE_STRINGS = [0, 1, 2, 4, 6, 8, 9, 11, 12]
# type 0=V, 1=Object, 2=MainActivity, 3=TextView, 4=Context,
# type 5=CharSequence, 6=Activity, 7=View, 8=Bundle

# method_ids: (class_type_idx, proto_idx, name_string_idx).
# proto_idx is 0 ("V"); the repo DEX parser reads it as a string index (the shorty).
METHODS = [
    (2, 0, 3),   # 0: MainActivity.onCreate
    (3, 0, 5),   # 1: TextView.<init>
    (3, 0, 7),   # 2: TextView.setText
    (6, 0, 10),  # 3: Activity.setContentView
]

# Dalvik opcodes used.
OP_CONST_STRING = 0x1A
OP_NEW_INSTANCE = 0x22
OP_INVOKE_VIRTUAL = 0x6E
OP_INVOKE_DIRECT = 0x70
OP_RETURN_VOID = 0x0E


def uleb128(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value == 0:
            out.append(byte)
            break
        out.append(byte | 0x80)
    return bytes(out)


def adler32(data: bytes) -> int:
    s1 = 1
    s2 = 0
    for b in data:
        s1 = (s1 + b) % 65521
        s2 = (s2 + s1) % 65521
    return (s2 << 16) | s1


def build_oncreate_insns() -> bytes:
    """Dalvik bytecode for MainActivity.onCreate (registers_size=3, ins_size=1)."""
    insns = bytearray()
    # new-instance v0, type@3 (Landroid/widget/TextView;)  [21c]
    insns += bytes([OP_NEW_INSTANCE, 0x00, 0x03, 0x00])
    # invoke-direct {v0}, method@1 (TextView.<init>)  [35c]
    insns += bytes([OP_INVOKE_DIRECT, 0x10, 0x01, 0x00, 0x00, 0x00])
    # const-string v1, string@13 (APP_TEXT)  [21c]
    insns += bytes([OP_CONST_STRING, 0x01, 0x0D, 0x00])
    # invoke-virtual {v0, v1}, method@2 (TextView.setText)  [35c]
    insns += bytes([OP_INVOKE_VIRTUAL, 0x20, 0x02, 0x00, 0x10, 0x00])
    # invoke-virtual {p0, v0}, method@3 (Activity.setContentView)  [35c]
    insns += bytes([OP_INVOKE_VIRTUAL, 0x20, 0x03, 0x00, 0x02, 0x00])
    # return-void  [10x]
    insns += bytes([OP_RETURN_VOID, 0x00])
    return bytes(insns)


def code_item(registers_size: int, ins_size: int, outs_size: int, insns: bytes) -> bytes:
    assert len(insns) % 2 == 0
    out = bytearray()
    out += struct.pack("<HHHH", registers_size, ins_size, outs_size, 0)  # tries_size=0
    out += struct.pack("<I", 0)  # debug_info_off
    out += struct.pack("<I", len(insns) // 2)  # insns_size (code units)
    out += insns
    return bytes(out)


def build_dex() -> bytes:
    dex = bytearray()

    string_ids_off = DEX_HEADER_SIZE
    string_ids_size = len(STRINGS)
    type_ids_off = string_ids_off + string_ids_size * 4
    type_ids_size = len(TYPE_STRINGS)
    proto_ids_off = type_ids_off + type_ids_size * 4
    proto_ids_size = 1
    method_ids_off = proto_ids_off + proto_ids_size * 12
    method_ids_size = len(METHODS)
    class_defs_off = method_ids_off + method_ids_size * 8
    class_defs_size = 1
    data_off = class_defs_off + class_defs_size * 32

    dex += bytes(data_off)

    # 1. String data (uleb128 utf16 length + utf8 bytes + NUL), recording offsets.
    string_offsets = []
    for s in STRINGS:
        string_offsets.append(len(dex))
        raw = s.encode("utf-8")
        dex += uleb128(len(s))  # utf16 code-unit count (all ASCII here)
        dex += raw
        dex.append(0)

    while len(dex) % 4 != 0:
        dex.append(0)

    # 2. Code items.
    oncreate_code_off = len(dex)
    dex += code_item(3, 1, 2, build_oncreate_insns())
    stub_code_off = len(dex)
    dex += code_item(1, 0, 0, bytes([OP_RETURN_VOID, 0x00]))

    while len(dex) % 4 != 0:
        dex.append(0)

    # 3. Class data item: four direct methods (onCreate + three framework stubs so
    #    the interpreter's method_index lookup resolves; framework dispatch
    #    intercepts the stubs before any code runs).
    class_data_off = len(dex)
    dex += uleb128(0)  # static_fields_size
    dex += uleb128(0)  # instance_fields_size
    dex += uleb128(4)  # direct_methods_size
    dex += uleb128(0)  # virtual_methods_size
    for diff, code_off in [
        (0, oncreate_code_off),
        (1, stub_code_off),
        (1, stub_code_off),
        (1, stub_code_off),
    ]:
        dex += uleb128(diff)  # method_idx_diff (cumulative)
        dex += uleb128(1)     # access_flags (public)
        dex += uleb128(code_off)

    file_size = len(dex)
    data_size = file_size - data_off

    # 4. Header.
    header = bytearray(DEX_HEADER_SIZE)
    header[0:8] = DEX_MAGIC
    header[8:12] = struct.pack("<I", 0)  # checksum placeholder
    header[12:32] = bytes(20)            # signature (not verified by the parser)
    header[32:36] = struct.pack("<I", file_size)
    header[36:40] = struct.pack("<I", DEX_HEADER_SIZE)
    header[40:44] = struct.pack("<I", ENDIAN_CONSTANT)
    header[44:48] = struct.pack("<I", 0)  # link_size
    header[48:52] = struct.pack("<I", 0)  # link_off
    header[52:56] = struct.pack("<I", 0)  # map_off
    header[56:60] = struct.pack("<I", string_ids_size)
    header[60:64] = struct.pack("<I", string_ids_off)
    header[64:68] = struct.pack("<I", type_ids_size)
    header[68:72] = struct.pack("<I", type_ids_off)
    header[72:76] = struct.pack("<I", proto_ids_size)
    header[76:80] = struct.pack("<I", proto_ids_off)
    header[80:84] = struct.pack("<I", 0)  # field_ids_size
    header[84:88] = struct.pack("<I", 0)  # field_ids_off
    header[88:92] = struct.pack("<I", method_ids_size)
    header[92:96] = struct.pack("<I", method_ids_off)
    header[96:100] = struct.pack("<I", class_defs_size)
    header[100:104] = struct.pack("<I", class_defs_off)
    header[104:108] = struct.pack("<I", data_size)
    header[108:112] = struct.pack("<I", data_off)
    dex[0:DEX_HEADER_SIZE] = header

    # 5. String ids.
    for i, off in enumerate(string_offsets):
        pos = string_ids_off + i * 4
        dex[pos:pos + 4] = struct.pack("<I", off)

    # 6. Type ids.
    for i, s_idx in enumerate(TYPE_STRINGS):
        pos = type_ids_off + i * 4
        dex[pos:pos + 4] = struct.pack("<I", s_idx)

    # 7. Proto ids: shorty=0 ("V"), return_type=0 ("V"), parameters_off=0.
    pos = proto_ids_off
    dex[pos:pos + 12] = struct.pack("<III", 0, 0, 0)

    # 8. Method ids.
    for i, (class_idx, proto_idx, name_idx) in enumerate(METHODS):
        pos = method_ids_off + i * 8
        dex[pos:pos + 2] = struct.pack("<H", class_idx)
        dex[pos + 2:pos + 4] = struct.pack("<H", proto_idx)
        dex[pos + 4:pos + 8] = struct.pack("<I", name_idx)

    # 9. Class def: MainActivity extends Object, points at class_data.
    pos = class_defs_off
    dex[pos:pos + 4] = struct.pack("<I", 2)              # class_idx = MainActivity
    dex[pos + 4:pos + 8] = struct.pack("<I", 1)          # access_flags = public
    dex[pos + 8:pos + 12] = struct.pack("<I", 1)         # superclass_idx = Object
    dex[pos + 12:pos + 16] = struct.pack("<I", 0)        # interfaces_off
    dex[pos + 16:pos + 20] = struct.pack("<I", 0)        # source_file_idx
    dex[pos + 20:pos + 24] = struct.pack("<I", 0)        # annotations_off
    dex[pos + 24:pos + 28] = struct.pack("<I", class_data_off)
    dex[pos + 28:pos + 32] = struct.pack("<I", 0)        # static_values_off

    # 10. Adler-32 checksum over everything after the checksum field.
    checksum = adler32(bytes(dex[12:]))
    dex[8:12] = struct.pack("<I", checksum)

    return bytes(dex)


def build_axml_manifest(package: str) -> bytes:
    """Binary AndroidManifest.xml mirroring apk::create_test_apk (UTF-8 pool)."""
    strings = ["manifest", "package", package]
    axml = bytearray()
    axml += struct.pack("<H", RES_XML_TYPE)
    axml += struct.pack("<H", 8)
    axml += struct.pack("<I", 0)  # total_size placeholder

    str_pool_start = len(axml)
    axml += struct.pack("<H", RES_STRING_POOL_TYPE)
    axml += struct.pack("<H", 28)
    axml += struct.pack("<I", 0)  # chunk_size placeholder
    axml += struct.pack("<I", len(strings))
    axml += struct.pack("<I", 0)  # style count
    axml += struct.pack("<I", 1 << 8)  # flags = UTF8
    strings_data_offset = 28 + len(strings) * 4
    axml += struct.pack("<I", strings_data_offset)
    axml += struct.pack("<I", 0)  # styles start

    str_data = bytearray()
    for s in strings:
        raw = s.encode("utf-8")
        axml += struct.pack("<I", len(str_data))
        str_data.append(len(raw))
        str_data.append(len(raw))
        str_data += raw
        str_data.append(0)
    axml += str_data
    while len(axml) % 4 != 0:
        axml.append(0)
    str_pool_size = len(axml) - str_pool_start
    axml[str_pool_start + 4:str_pool_start + 8] = struct.pack("<I", str_pool_size)

    # START_ELEMENT <manifest package="...">
    axml += struct.pack("<H", RES_XML_START_ELEMENT_TYPE)
    axml += struct.pack("<H", 16)
    axml += struct.pack("<I", 56)  # 36 header + 20 attr
    axml += struct.pack("<I", 1)   # line
    axml += struct.pack("<I", 0xFFFFFFFF)  # comment
    axml += struct.pack("<I", 0xFFFFFFFF)  # ns
    axml += struct.pack("<I", 0)   # name_idx = "manifest"
    axml += struct.pack("<H", 20)  # attr_start
    axml += struct.pack("<H", 20)  # attr_size
    axml += struct.pack("<H", 1)   # attr_count
    axml += struct.pack("<H", 0)   # id_idx
    axml += struct.pack("<H", 0)   # class_idx
    axml += struct.pack("<H", 0)   # style_idx
    # attribute: package="org.interos.genui"
    axml += struct.pack("<I", 0xFFFFFFFF)  # ns
    axml += struct.pack("<I", 1)   # name_idx = "package"
    axml += struct.pack("<I", 2)   # raw_value_idx = package
    axml += struct.pack("<I", 0x030008)  # TYPE_STRING
    axml += struct.pack("<I", 2)   # data = package

    axml[4:8] = struct.pack("<I", len(axml))
    return bytes(axml)


def build_apk(dex_bytes: bytes) -> bytes:
    """Stored (uncompressed) PKZip with AndroidManifest.xml + classes.dex."""
    files = [
        ("AndroidManifest.xml", build_axml_manifest(PACKAGE_NAME)),
        ("classes.dex", dex_bytes),
    ]
    apk = bytearray()
    cd_entries = []
    for name, payload in files:
        local_offset = len(apk)
        name_bytes = name.encode("utf-8")
        length = len(payload)
        apk += struct.pack("<I", ZIP_LOCAL_FILE_HEADER_SIG)
        apk += struct.pack("<H", 20)  # version needed
        apk += struct.pack("<H", 0)   # flags
        apk += struct.pack("<H", 0)   # method = stored
        apk += struct.pack("<I", 0)   # mod time
        apk += struct.pack("<I", 0)   # crc32
        apk += struct.pack("<I", length)
        apk += struct.pack("<I", length)
        apk += struct.pack("<H", len(name_bytes))
        apk += struct.pack("<H", 0)  # extra len
        apk += name_bytes
        apk += payload
        cd_entries.append((name_bytes, length, local_offset))

    cd_offset = len(apk)
    for name_bytes, length, local_offset in cd_entries:
        apk += struct.pack("<I", ZIP_CENTRAL_DIR_SIG)
        apk += struct.pack("<H", 20)  # version made by
        apk += struct.pack("<H", 20)  # version needed
        apk += struct.pack("<H", 0)   # flags
        apk += struct.pack("<H", 0)   # method = stored
        apk += struct.pack("<I", 0)   # mod time
        apk += struct.pack("<I", 0)   # crc32
        apk += struct.pack("<I", length)
        apk += struct.pack("<I", length)
        apk += struct.pack("<H", len(name_bytes))
        apk += struct.pack("<H", 0)  # extra len
        apk += struct.pack("<H", 0)  # comment len
        apk += struct.pack("<H", 0)  # disk start
        apk += struct.pack("<H", 0)  # internal attr
        apk += struct.pack("<I", 0)  # external attr
        apk += struct.pack("<I", local_offset)
        apk += name_bytes

    cd_size = len(apk) - cd_offset
    apk += struct.pack("<I", ZIP_END_OF_CENTRAL_DIR_SIG)
    apk += struct.pack("<H", 0)  # disk num
    apk += struct.pack("<H", 0)  # cd disk
    apk += struct.pack("<H", len(files))
    apk += struct.pack("<H", len(files))
    apk += struct.pack("<I", cd_size)
    apk += struct.pack("<I", cd_offset)
    apk += struct.pack("<H", 0)  # comment len
    return bytes(apk)


def main() -> None:
    dex_bytes = build_dex()
    apk_bytes = build_apk(dex_bytes)
    DEX_OUT.write_bytes(dex_bytes)
    APK_OUT.write_bytes(apk_bytes)
    print(f"Wrote {DEX_OUT} ({len(dex_bytes)} bytes)")
    print(f"Wrote {APK_OUT} ({len(apk_bytes)} bytes)")


if __name__ == "__main__":
    main()