"""Generate the original OCR Translate scan/language app mark.

Run with Python + Pillow from any directory. Geometry is shared by the SVG
master and supersampled PNG/ICO exports; no font is needed for the logo.
Windows ICO entries use individually rendered, optically adjusted small sizes.
"""
from pathlib import Path
import struct

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent
ASSETS = ROOT / "assets"
SIZES = (16, 20, 24, 30, 32, 40, 48, 60, 64, 72, 96, 128, 256)
TOP = (20, 48, 89)
BOTTOM = (7, 115, 132)
WHITE = (247, 253, 255, 255)
MINT = (110, 242, 220, 255)


def mix(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def geometry(size=256):
    """Paths in a 256-unit coordinate system, with optical size variants."""
    small = size <= 24
    frame_width = 14 if small else 10
    corners = [
        [(46, 79), (46, 46), (79, 46)],
        [(177, 210), (210, 210), (210, 177)],
    ]
    if not small:
        corners += [
            [(177, 46), (210, 46), (210, 79)],
            [(46, 177), (46, 210), (79, 210)],
        ]
    paths = [(points, MINT, frame_width) for points in corners]
    if small:
        # A single bold language glyph avoids unreadable miniature lettering.
        paths += [
            ([(82, 180), (125, 76), (131, 76), (174, 180)], WHITE, 17),
            ([(97, 145), (159, 145)], WHITE, 15),
        ]
    else:
        # Hand-drawn 文; all strokes stay editable and font-independent.
        paths += [
            ([(94, 73), (99, 81)], WHITE, 10),
            ([(67, 92), (126, 92)], WHITE, 10),
            ([(81, 98), (88, 110), (98, 121), (111, 131), (125, 139)], WHITE, 10),
            ([(115, 98), (110, 110), (100, 122), (86, 132), (69, 140)], WHITE, 10),
            ([(133, 183), (154, 125), (160, 125), (181, 183)], WHITE, 11),
            ([(142, 160), (172, 160)], WHITE, 10),
        ]
    return paths


def make(size, optical_size=None):
    supersample = 4 if size >= 128 else 8
    n = size * supersample
    scale = n / 256
    image = Image.new("RGBA", (n, n))
    draw = ImageDraw.Draw(image)
    for y in range(n):
        # Restrained navy-to-petrol diagonal gradient, not a glossy badge.
        for x in range(n):
            t = max(0, min(1, (0.30 * x + 0.70 * y) / max(1, n - 1)))
            image.putpixel((x, y), (*mix(TOP, BOTTOM, t), 255))
    mask = Image.new("L", (n, n))
    ImageDraw.Draw(mask).rounded_rectangle(
        (10 * scale, 10 * scale, 246 * scale, 246 * scale),
        radius=53 * scale, fill=255,
    )
    image.putalpha(mask)
    draw = ImageDraw.Draw(image)
    for points, color, width in geometry(optical_size or size):
        pts = [(round(x * scale), round(y * scale)) for x, y in points]
        w = round(width * scale)
        draw.line(pts, fill=color, width=w, joint="curve")
        radius = w / 2
        for x, y in pts:
            draw.ellipse((x - radius, y - radius, x + radius, y + radius), fill=color)
    result = image.resize((size, size), Image.Resampling.LANCZOS)
    # Lanczos can produce an alpha=1 fringe outside the rounded tile.
    alpha = result.getchannel("A").point(lambda value: 0 if value <= 2 else value)
    result.putalpha(alpha)
    return result


def svg_master():
    strokes = []
    for points, color, width in geometry():
        xy = " ".join(f"{x},{y}" for x, y in points)
        rgb = "#" + "".join(f"{v:02x}" for v in color[:3])
        strokes.append(f'  <polyline points="{xy}" stroke="{rgb}" stroke-width="{width}"/>')
    return '''<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256" role="img" aria-labelledby="title desc">
  <title id="title">OCR Translate</title>
  <desc id="desc">Mint capture corners framing the Chinese character 文 and a Latin A on a navy-to-petrol rounded tile.</desc>
  <defs>
    <linearGradient id="tile" x1="0" y1="0" x2="0.5172413793" y2="1.2068965517" gradientUnits="objectBoundingBox">
      <stop stop-color="#143059"/>
      <stop offset="1" stop-color="#077384"/>
    </linearGradient>
  </defs>
  <rect x="10" y="10" width="236" height="236" rx="53" fill="url(#tile)"/>
  <g fill="none" stroke-linecap="round" stroke-linejoin="round">
''' + "\n".join(strokes) + '''
  </g>
</svg>
'''


def font(size, chinese=False):
    candidates = (["C:/Windows/Fonts/msyh.ttc"] if chinese else []) + [
        "C:/Windows/Fonts/segoeui.ttf", "C:/Windows/Fonts/arial.ttf",
    ]
    for path in candidates:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    return ImageFont.load_default(size=size)


def composite(canvas, icon, position):
    canvas.alpha_composite(icon, position)


def preview(images):
    """Actual-size Windows samples, not artificially enlarged small assets."""
    sheet = Image.new("RGBA", (1120, 660), "#f3f6fa")
    draw = ImageDraw.Draw(sheet)
    draw.text((40, 28), "OCR Translate", fill="#132941", font=font(30))
    draw.text((40, 73), "应用图标 · 扫描识别 / 语言转换", fill="#5b6c80", font=font(18, True))
    draw.rounded_rectangle((32, 121, 1088, 457), radius=24, fill="white")
    old = ASSETS / "icon_previous.png"
    if old.exists():
        composite(sheet, Image.open(old).convert("RGBA").resize((256, 256)), (48, 139))
        draw.text((146, 405), "原图标", fill="#65748a", font=font(17, True))
    composite(sheet, images[256], (365, 139))
    draw.text((455, 405), "新图标", fill="#132941", font=font(17, True))
    draw.line((690, 157, 690, 419), fill="#e4eaf2", width=2)
    draw.text((729, 164), "WINDOWS SIZES", fill="#6d7d90", font=font(15))
    x = 733
    for size in (16, 24, 32, 48, 64):
        composite(sheet, images[size], (x, 260 - size // 2))
        draw.text((x, 310), str(size), fill="#5b6c80", font=font(14))
        x += size + 19
    draw.text((729, 362), "16–24 px 使用简化图形", fill="#5b6c80", font=font(15, True))
    draw.text((729, 391), "小尺寸保留轮廓和对比度", fill="#5b6c80", font=font(15, True))
    for left, bg, fg, label in [(32, "#ffffff", "#334155", "浅色任务栏"), (572, "#172433", "#dce9f5", "深色任务栏")]:
        draw.rounded_rectangle((left, 487, left + 516, 618), radius=20, fill=bg)
        draw.text((left + 24, 505), label, fill=fg, font=font(15, True))
        composite(sheet, images[32], (left + 30, 555))
        draw.text((left + 82, 557), "OCR Translate", fill=fg, font=font(20))
        composite(sheet, images[16], (left + 457, 563))
    sheet.convert("RGB").save(ASSETS / "icon_preview.png")


def verify(images):
    data = (ASSETS / "icon.ico").read_bytes()
    reserved, kind, count = struct.unpack_from("<HHH", data)
    assert (reserved, kind, count) == (0, 1, len(SIZES))
    actual = []
    for i in range(count):
        w, h, _, _, _, _, length, offset = struct.unpack_from("<BBBBHHII", data, 6 + 16 * i)
        actual.append((w or 256, h or 256))
        assert length > 0 and offset + length <= len(data)
    assert set(actual) == {(s, s) for s in SIZES}
    with Image.open(ASSETS / "icon.ico") as ico:
        for size in SIZES:
            frame = ico.ico.getimage((size, size)).convert("RGBA")
            assert frame.tobytes() == images[size].tobytes(), f"ICO frame {size} was not the optical variant"
    for size, image in images.items():
        assert image.mode == "RGBA" and image.size == (size, size)
        assert image.getpixel((0, 0))[3] == 0
    print("Verified all 13 ICO sizes, exact optical variants, RGBA and transparent corners.")


def main():
    ASSETS.mkdir(exist_ok=True)
    images = {size: make(size) for size in SIZES}
    (ASSETS / "icon.svg").write_text(svg_master(), encoding="utf-8")
    for size in (16, 32, 256):
        images[size].save(ASSETS / f"icon_{size}.png")
    # The existing tray loader uses this 64px source. Keep the simplified mark
    # at high resolution so Windows can scale it cleanly for every monitor DPI.
    make(64, optical_size=16).save(ASSETS / "icon_64.png")
    images[256].save(ASSETS / "icon.ico", format="ICO", sizes=[(s, s) for s in SIZES], append_images=[images[s] for s in SIZES if s != 256])
    preview(images)
    verify(images)
    print("Generated assets/icon.ico, icon.svg, PNGs and icon_preview.png.")


if __name__ == "__main__":
    main()
