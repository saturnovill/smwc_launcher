# Convierte el atlas PNG de vvye/smw-font a TTF de pixeles.
import json, sys
from PIL import Image
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen

meta = json.load(open(sys.argv[1])); im = Image.open(sys.argv[2]).convert("RGB")
W, H, BASE, PX = meta["glyph-width"], meta["glyph-height"], meta["glyph-baseline"], 128
# El JSON original lista "ë→↑←↓…" pero el atlas dibuja "ë←→↑↓…".
meta["in-glyphs"] = [l.replace("ë→↑←↓", "ë←→↑↓") for l in meta["in-glyphs"]]
names, glyphs, widths, cmap = [".notdef"], {}, {".notdef": 5 * PX}, {}
glyphs[".notdef"] = TTGlyphPen(None).glyph()
for row, line in enumerate(meta["in-glyphs"]):
    for col, ch in enumerate(line):
        lit = lambda x, y: im.getpixel((col * W + x, row * H + y)) != (255, 255, 255)
        cols = [x for x in range(W) if any(lit(x, y) for y in range(H))]
        minx, maxx = (cols[0], cols[-1] + 1) if cols else (0, 0)
        pen = TTGlyphPen(None)
        # El espacio solo trae un punto que marca su ancho.
        for y in (range(H) if ch != " " else []):
            x = 0
            while x < W:
                if im.getpixel((col * W + x, row * H + y)) != (255, 255, 255):
                    x0 = x
                    while x < W and im.getpixel((col * W + x, row * H + y)) != (255, 255, 255): x += 1
                    top, bot = (BASE - y) * PX, (BASE - y - 1) * PX
                    a, b = (x0 - minx) * PX, (x - minx) * PX
                    pen.moveTo((a, bot)); pen.lineTo((a, top)); pen.lineTo((b, top)); pen.lineTo((b, bot)); pen.closePath()
                else: x += 1
        name = f"u{ord(ch):04X}"; names.append(name); cmap[ord(ch)] = name
        glyphs[name] = pen.glyph()
        widths[name] = (maxx if ch == " " else maxx - minx + meta["glyph-spacing"]) * PX
fb = FontBuilder(1024, isTTF=True)
fb.setupGlyphOrder(names); fb.setupCharacterMap(cmap); fb.setupGlyf(glyphs)
fb.setupHorizontalMetrics({n: (widths[n], 0) for n in names})
fb.setupHorizontalHeader(ascent=meta["font-ascend"], descent=meta["font-descend"])
fb.setupNameTable({"familyName": "SMW", "styleName": "Regular"})
fb.setupOS2(sTypoAscender=meta["font-ascend"], sTypoDescender=meta["font-descend"], usWinAscent=meta["font-ascend"], usWinDescent=-meta["font-descend"])
fb.setupPost(); fb.save(sys.argv[3])
