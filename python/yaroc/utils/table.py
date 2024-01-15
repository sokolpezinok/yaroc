# -*- coding: utf-8 -*-
import itertools

from PIL import Image, ImageDraw, ImageFont


def draw_table(table: list[list[str]], width: int, height: int, horiz_pad: int = 1) -> Image.Image:
    """Draws a table as an image of size width x height from the given text in `table`."""
    image = Image.new("1", (width, height), 0xFF)
    draw = ImageDraw.Draw(image)
    char_height = 12
    font = ImageFont.truetype("DejaVuSans.ttf", char_height)

    total_horiz_pad = 1 + horiz_pad * 2
    row_count, col_count = len(table), len(table[0])
    if any([len(row) != col_count for row in table]):
        raise Exception("Wrong number of columns")

    cols = [max(font.getlength(row[z]) for row in table) for z in range(col_count)]

    def calc_row_start(row: int):
        return row * char_height + row - 1

    def calc_col_start(col: int, partial_sum: int):
        return col * total_horiz_pad + partial_sum

    real_height = calc_row_start(row_count)
    real_width = calc_col_start(len(cols), sum(cols))

    for i, partial_sum in enumerate(itertools.accumulate(cols[:-1])):
        x = calc_col_start(i + 1, partial_sum)
        draw.line((x, 0, x, real_height), fill=0)

    for row in range(1, row_count):
        y = calc_row_start(row)
        draw.line((0, y, real_width, y), fill=0)

    for row_idx, row in enumerate(table):
        y = calc_row_start(row_idx)
        for col_idx, partial_sum in enumerate(itertools.accumulate([0] + cols[:-1])):
            x = calc_col_start(col_idx, partial_sum)
            draw.text((x + horiz_pad, y), row[col_idx], font=font, fill=0)

    return image
