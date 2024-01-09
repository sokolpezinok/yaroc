# -*- coding: utf-8 -*-
import itertools

from PIL import Image, ImageDraw, ImageFont


def draw_table(table: list[list[str]], width: int, height: int, horiz_pad: int = 1) -> Image:
    """Draws a table as an image of size width x height from the given text in `table`."""
    image = Image.new("1", (width, height), 0xFF)
    draw = ImageDraw.Draw(image)
    char_height = 12
    font = ImageFont.truetype("DejaVuSans.ttf", char_height)

    row_count = len(table)
    row_len = len(table[0])
    if any([len(row) != row_len for row in table]):
        raise Exception("Wrong number of columns")

    cols = []
    for z in range(row_len):
        cols.append(max(font.getlength(row[z]) for row in table))

    total_horiz_pad = 1 + horiz_pad * 2
    real_width = sum(cols) + len(cols) * total_horiz_pad
    real_height = char_height * row_count + row_count

    for i, column in enumerate(itertools.accumulate(cols[:-1])):
        pos = total_horiz_pad * (i + 1) + column
        draw.line((pos, 0, pos, real_height), fill=0)

    for y in range(1, row_count):
        pos = y + y * char_height
        draw.line((0, pos, real_width, pos), fill=0)

    for i, row in enumerate(table):
        y = i + i * char_height
        for j, column in enumerate(itertools.accumulate([0] + cols)):
            xx = total_horiz_pad * j + column
            if j < len(row):
                draw.text((xx + 1, y), row[j], font=font, fill=0)

    return image


im = draw_table(
    [
        ["name", "dBm", "code", "last info", "last punch"],
        ["spe01", "-75", "55", "2min ago", "1min ago" + "\u2713"],
        ["spe02", "-82", "64", "20s ago", "15 min ago"],
    ],
    296,
    152,
)
im.show()
