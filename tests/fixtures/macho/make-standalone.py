"""Wrap the fixture's flat x86-64 text in a fixed-address Mach-O header."""

from pathlib import Path
import struct
import sys


code = Path(sys.argv[1]).read_bytes()
output = Path(sys.argv[2])
base = 0x1_0000_0000
entry_offset = 0x200
segment = struct.pack(
    "<II16sQQQQIIII",
    0x19, 72, b"__TEXT", base, 4096, 0, entry_offset + len(code), 5, 5, 0, 0,
)
state = [0] * 21
state[16] = base + entry_offset
thread = struct.pack("<IIII21Q", 5, 184, 4, 42, *state)
header = struct.pack("<8I", 0xFEEDFACF, 0x01000007, 3, 2, 2, len(segment + thread), 1, 0)
output.write_bytes((header + segment + thread).ljust(entry_offset, b"\0") + code)
