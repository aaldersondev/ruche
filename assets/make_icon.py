# -*- coding: utf-8 -*-
"""Genere l'icone de Ruche : une alveole sur fond sombre.

Produit trois fichiers, tous versionnes :
  ruche.ico        icone multi-tailles embarquee dans l'exe (build.rs)
  icon_64.rgba     64x64 en RGBA brut, charge par la fenetre egui sans decodeur
  icon.png         rendu 256 px pour le README

    python assets/make_icon.py
"""
import math
import os
import struct

from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
SS = 8  # suréchantillonnage
SIZES = [16, 24, 32, 48, 64, 128, 256]

BG_TOP = (38, 40, 45, 255)
BG_BOTTOM = (24, 25, 28, 255)
AMBER = (242, 179, 61, 255)
AMBER_LIGHT = (255, 209, 102, 255)


def hexagon(cx, cy, radius, rotation=math.pi / 2):
    """Hexagone pointe en haut, centre sur (cx, cy)."""
    return [
        (
            cx + radius * math.cos(rotation + i * math.pi / 3),
            cy + radius * math.sin(rotation + i * math.pi / 3),
        )
        for i in range(6)
    ]


def render(size):
    big = size * SS
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    # fond : carre arrondi avec un degrade vertical discret
    radius = int(big * 0.22)
    gradient = Image.new("RGBA", (1, big))
    for y in range(big):
        t = y / max(1, big - 1)
        gradient.putpixel(
            (0, y),
            tuple(int(a + (b - a) * t) for a, b in zip(BG_TOP, BG_BOTTOM)),
        )
    gradient = gradient.resize((big, big))
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, big - 1, big - 1], radius, fill=255)
    img.paste(gradient, (0, 0), mask)

    # alveole : anneau epais + point central
    cx = cy = big / 2
    outer = big * 0.34
    draw.polygon(hexagon(cx, cy, outer), fill=AMBER)
    draw.polygon(hexagon(cx, cy, outer * 0.62), fill=(0, 0, 0, 0))

    # l'interieur du trou reprend le fond, pas la transparence
    hole = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    hole_mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(hole_mask).polygon(hexagon(cx, cy, outer * 0.62), fill=255)
    hole.paste(gradient, (0, 0), hole_mask)
    img.alpha_composite(hole)

    # petite alveole pleine au centre : lisible meme en 16 px
    draw.polygon(hexagon(cx, cy, outer * 0.3), fill=AMBER_LIGHT)

    return img.resize((size, size), Image.LANCZOS)


def main():
    images = [render(s) for s in SIZES]
    ico = os.path.join(HERE, "ruche.ico")
    images[-1].save(ico, format="ICO", sizes=[(s, s) for s in SIZES])
    print("ecrit", ico)

    png = os.path.join(HERE, "icon.png")
    images[-1].save(png)
    print("ecrit", png)

    rgba_path = os.path.join(HERE, "icon_64.rgba")
    rgba = render(64).convert("RGBA").tobytes()
    assert len(rgba) == 64 * 64 * 4, len(rgba)
    with open(rgba_path, "wb") as fh:
        fh.write(rgba)
    print("ecrit", rgba_path, len(rgba), "octets")

    # planche de controle : toutes les tailles cote a cote
    strip = Image.new("RGBA", (sum(s for s in SIZES) + 10 * len(SIZES), 256),
                      (60, 60, 60, 255))
    x = 0
    for image, s in zip(images, SIZES):
        strip.paste(image, (x, (256 - s) // 2), image)
        x += s + 10
    preview = os.path.join(HERE, "preview.png")
    strip.save(preview)
    print("ecrit", preview)


if __name__ == "__main__":
    main()
