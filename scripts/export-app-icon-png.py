"""Regenerate the checked-in application icon PNGs from the master artwork.

The master is already hand-cropped, so the whole square canvas is scaled straight
to every target size: nothing is trimmed, recentred, or padded again.

Requires Pillow and numpy:

    python scripts/export-app-icon-png.py

The resulting PNGs are what scripts/export-app-icon.py packs into the ICO.
"""
from pathlib import Path
import sys

import numpy as np
from PIL import Image

FOLDER = Path(__file__).resolve().parents[1] / 'resources/app-icon'
MASTER = FOLDER / 'rust-explorer-master.png'
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256]


def resize_premultiplied(image: Image.Image, size: tuple[int, int]) -> Image.Image:
    """Resize RGBA in premultiplied alpha so transparent pixels cannot bleed."""
    pixels = np.asarray(image, dtype=np.float32) / 255.0
    alpha = pixels[..., 3]
    premultiplied = pixels[..., :3] * alpha[..., None]

    channels = []
    for channel in range(3):
        band = Image.fromarray(np.ascontiguousarray(premultiplied[..., channel]), mode='F')
        channels.append(np.asarray(band.resize(size, Image.LANCZOS), dtype=np.float32))
    band = Image.fromarray(np.ascontiguousarray(alpha), mode='F')
    alpha = np.clip(np.asarray(band.resize(size, Image.LANCZOS), dtype=np.float32), 0.0, 1.0)

    rgb = np.clip(np.stack(channels, axis=-1) / np.maximum(alpha[..., None], 1e-6), 0.0, 1.0)
    rgb[alpha <= 1e-6] = 0.0

    out = np.empty((size[1], size[0], 4), dtype=np.float32)
    out[..., :3] = rgb
    out[..., 3] = alpha
    return Image.fromarray(np.rint(out * 255.0).astype(np.uint8), 'RGBA')


def main() -> int:
    if not MASTER.is_file():
        print('Missing master artwork:', MASTER)
        return 1

    master = Image.open(MASTER).convert('RGBA')
    if master.width != master.height:
        print(f'Master is not square ({master.width}x{master.height}); export assumes a square canvas.')
        return 1

    for size in SIZES:
        icon = resize_premultiplied(master, (size, size))
        target = FOLDER / f'rust-explorer-{size}.png'
        icon.save(target, 'PNG', optimize=True, compress_level=9)
        print(f'{target.name}: {size}x{size} ({target.stat().st_size} bytes)')

    print('Run scripts/export-app-icon.py to repack rust-explorer.ico.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
