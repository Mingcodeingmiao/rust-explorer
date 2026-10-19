"""Pack the checked-in Microsoft Fluent PNG rasterizations into a Windows ICO.

Artwork provenance: resources/fluent-icons/SOURCES.json (app-folder).
This script does not accept replacement artwork or copy Windows resources.
"""
from pathlib import Path
import struct

folder = Path(__file__).resolve().parents[1] / 'resources/app-icon'
sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256]
offset = 6 + 16 * len(sizes)
entries = []
payload = bytearray()
for size in sizes:
    png = (folder / f'rust-explorer-{size}.png').read_bytes()
    assert png[:8] == b'\x89PNG\r\n\x1a\n'
    assert struct.unpack('>II', png[16:24]) == (size, size)
    entries.append(struct.pack('<BBBBHHII', size % 256, size % 256, 0, 0, 1, 32, len(png), offset))
    offset += len(png)
    payload.extend(png)
(folder / 'rust-explorer.ico').write_bytes(struct.pack('<HHH', 0, 1, len(sizes)) + b''.join(entries) + payload)
print('Packed official Fluent fallback ICO:', ', '.join(map(str, sizes)))
