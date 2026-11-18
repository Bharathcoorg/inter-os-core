
#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

import struct
import sys

def read_value(f, val_type):
    if val_type == 0: return struct.unpack("<B", f.read(1))[0] # Uint8
    elif val_type == 1: return struct.unpack("<b", f.read(1))[0] # Int8
    elif val_type == 2: return struct.unpack("<H", f.read(2))[0] # Uint16
    elif val_type == 3: return struct.unpack("<h", f.read(2))[0] # Int16
    elif val_type == 4: return struct.unpack("<I", f.read(4))[0] # Uint32
    elif val_type == 5: return struct.unpack("<i", f.read(4))[0] # Int32
    elif val_type == 6: return struct.unpack("<f", f.read(4))[0] # Float32
    elif val_type == 7: return struct.unpack("<?", f.read(1))[0] # Bool
    elif val_type == 8: # String
        str_len = struct.unpack("<Q", f.read(8))[0]
        return f.read(str_len).decode("utf-8")
    elif val_type == 10: return struct.unpack("<Q", f.read(8))[0] # Uint64
    elif val_type == 11: return struct.unpack("<q", f.read(8))[0] # Int64
    elif val_type == 12: return struct.unpack("<d", f.read(8))[0] # Float64
    elif val_type == 9: # Array
        elem_type = struct.unpack("<I", f.read(4))[0]
        elem_count = struct.unpack("<Q", f.read(8))[0]
        # Just skip for printing simplicity to avoid recursion depth/too much output
        for _ in range(elem_count):
            # Recurse carefully or just skip
            # Skipping is safer:
            skip_value_simple(f, elem_type)
        return f"Array(count={elem_count}, type={elem_type})"
    else:
        raise ValueError(f"Unknown type: {val_type}")

def skip_value_simple(f, val_type):
    if val_type in (0, 1, 7): f.read(1)
    elif val_type in (2, 3): f.read(2)
    elif val_type in (4, 5, 6): f.read(4)
    elif val_type in (10, 11, 12): f.read(8)
    elif val_type == 8:
        str_len = struct.unpack("<Q", f.read(8))[0]
        f.read(str_len)
    elif val_type == 9:
        elem_type = struct.unpack("<I", f.read(4))[0]
        elem_count = struct.unpack("<Q", f.read(8))[0]
        for _ in range(elem_count):
            skip_value_simple(f, elem_type)

def dump_gguf(path):
    with open(path, "rb") as f:
        magic = f.read(4)
        if magic != b"GGUF": return
        version = struct.unpack("<I", f.read(4))[0]
        tensor_count = struct.unpack("<Q", f.read(8))[0]
        kv_count = struct.unpack("<Q", f.read(8))[0]
        
        print(f"Version: {version}, Tensors: {tensor_count}, KV: {kv_count}")

        for _ in range(kv_count):
            key_len = struct.unpack("<Q", f.read(8))[0]
            key = f.read(key_len).decode("utf-8")
            val_type = struct.unpack("<I", f.read(4))[0]
            val = read_value(f, val_type)
            print(f"Key: {key}, Val: {val}")
            
if __name__ == "__main__":
    dump_gguf(sys.argv[1])
