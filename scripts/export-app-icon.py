"""Export the original generated artwork at Windows icon resolutions."""
from pathlib import Path
import shutil
import sys
from PIL import Image

root = Path(__file__).resolve().parents[1]
folder = root / 'resources' / 'app-icon'
folder.mkdir(parents=True, exist_ok=True)
master = folder / 'rust-explorer-master.png'
if len(sys.argv) > 1:
    if master.exists():
        raise SystemExit('Master already exists; preserve the original artwork.')
    shutil.copy2(sys.argv[1], master)
image = Image.open(master).convert('RGBA')
assert image.width == image.height
assert image.getextrema()[3][0] == 0, 'Artwork must contain transparent pixels'
sizes = [16, 20, 24, 32, 40, 48, 64, 96, 128, 192, 256]
for size in sizes:
    image.resize((size, size), Image.Resampling.LANCZOS).save(folder / f'rust-explorer-{size}.png')
image.save(folder / 'rust-explorer.ico', format='ICO', sizes=[(n, n) for n in sizes])
with Image.open(folder / 'rust-explorer.ico') as icon:
    assert icon.ico.sizes() == {(n, n) for n in sizes}
    for size in sizes:
        frame = icon.ico.getimage((size, size))
        assert frame.mode == 'RGBA' and frame.size == (size, size)
print('Exported transparent PNGs and ICO:', ', '.join(map(str, sizes)))
