"""Makes the Windows installer's pictures from the Sweepwise logo: the header (150 x 57, top of every page) and the
side picture (164 x 314, welcome and finish pages). Both are 24-bit BMP files, as the installer needs."""
import pathlib

from PIL import Image, ImageDraw, ImageFont

root = pathlib.Path(__file__).resolve().parent.parent
icons = root / 'src-tauri' / 'icons'
dest = root / 'src-tauri' / 'installer'
dest.mkdir(parents=True, exist_ok=True)
logo = Image.open(icons / '128x128@2x.png').convert('RGBA')
bold = r'C:\Windows\Fonts\segoeuib.ttf'
regular = r'C:\Windows\Fonts\segoeui.ttf'
NAVY, NAVY2, TEAL, WHITE, MUTED = (10, 16, 32), (18, 28, 51), (45, 212, 191), (255, 255, 255), (140, 155, 184)


def paste(img, size, xy):
    mark = logo.resize((size, size), Image.LANCZOS)
    img.paste(mark, xy, mark)


# Header: white like the installer's top band, the shield on the left and the name beside it.
head = Image.new('RGB', (150, 57), WHITE)
paste(head, 40, (8, 8))
d = ImageDraw.Draw(head)
d.text((54, 15), 'Sweepwise', font=ImageFont.truetype(bold, 17), fill=NAVY)
head.save(dest / 'header.bmp')

# Side picture: Sweepwise's night navy, fading down, with the shield, the name and one line under it.
side = Image.new('RGB', (164, 314), NAVY)
d = ImageDraw.Draw(side)
for y in range(314):
    t = y / 313
    d.line([(0, y), (163, y)], fill=tuple(round(a + (b - a) * t) for a, b in zip(NAVY, NAVY2)))
paste(side, 96, (34, 46))
name_font = ImageFont.truetype(bold, 22)
w = d.textlength('Sweepwise', font=name_font)
d.text(((164 - w) / 2, 154), 'Sweepwise', font=name_font, fill=WHITE)
small = ImageFont.truetype(regular, 12)
for i, line in enumerate(('Free PC care', 'Clean. Check. Undo.')):
    w = d.textlength(line, font=small)
    d.text(((164 - w) / 2, 188 + i * 17), line, font=small, fill=TEAL if i == 0 else MUTED)
side.save(dest / 'sidebar.bmp')
print('wrote header.bmp and sidebar.bmp')
