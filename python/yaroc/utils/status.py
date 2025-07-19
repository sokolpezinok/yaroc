import logging
from datetime import datetime
from itertools import accumulate

from PIL import Image, ImageDraw, ImageFont

from ..rs import NodeInfo


class StatusDrawer:
    """Class for tracking the status of all nodes"""

    def __init__(
        self,
        display_model: str | None = None,
    ):
        if display_model is not None:
            import epaper

            self.epd = epaper.epaper(display_model).EPD()
            self.epd.init(0)
            self.epd.Clear()
        else:
            self.epd = None

    def generate_info_table(self, node_infos: list[NodeInfo]) -> list[list[str]]:
        def human_time(timestamp: datetime | None) -> str:
            if timestamp is None:
                return ""
            delta = datetime.now().astimezone() - timestamp
            if delta.total_seconds() < 10:
                return f"{delta.total_seconds():.1f}s ago"
            if delta.total_seconds() < 60:
                return f"{delta.total_seconds():.0f}s ago"
            minutes = delta.total_seconds() / 60
            if minutes < 10:
                return f"{minutes:.1f}m ago"
            if minutes < 60:
                return f"{minutes:.0f}m ago"
            return f"{minutes / 60:.1f}h ago"

        table = []
        for node_info in node_infos:
            node_info.codes.sort()
            table.append(
                [
                    node_info.name,
                    str(node_info.rssi_dbm) if node_info.rssi_dbm is not None else "",
                    f"{node_info.snr_db:.0f}" if node_info.snr_db is not None else "",
                    ",".join(str(code) for code in node_info.codes),
                    human_time(node_info.last_update),
                    human_time(node_info.last_punch),
                ]
            )
        return table

    @staticmethod
    def draw_table(
        table: list[list[str]], width: int, height: int, horiz_pad: int = 1
    ) -> Image.Image:
        """Draws a table as an image of size width x height from the given text in `table`."""

        image = Image.new("1", (width, height), 0xFF)
        draw = ImageDraw.Draw(image)
        char_height = 12
        font = ImageFont.truetype("DejaVuSans.ttf", char_height)

        total_horiz_pad = 1 + horiz_pad * 2
        row_count, col_count = len(table), len(table[0])
        if any([len(row) != col_count for row in table]):
            raise Exception("Wrong number of columns")

        cols = [int(max(font.getlength(row[z]) for row in table)) for z in range(col_count)]

        def calc_row_start(row: int) -> int:
            return row * char_height + row - 1

        def calc_col_start(col: int, partial_sum: int) -> int:
            return col * total_horiz_pad + partial_sum

        real_height = calc_row_start(row_count)
        real_width = calc_col_start(len(cols), sum(cols))

        for i, partial_sum in enumerate(accumulate(cols[:-1])):
            x = calc_col_start(i + 1, partial_sum)
            draw.line((x, 0, x, real_height), fill=0)

        for row_idx in range(1, row_count):
            y = calc_row_start(row_idx)
            draw.line((0, y, real_width, y), fill=0)

        for row_idx, row in enumerate(table):
            y = calc_row_start(row_idx)
            for col_idx, partial_sum in enumerate(accumulate([0] + cols[:-1])):
                x = calc_col_start(col_idx, partial_sum)
                draw.text((x + horiz_pad, y), row[col_idx], font=font, fill=0)

        return image

    def draw_status(self, node_infos: list[NodeInfo]):
        if self.epd is None:
            return
        logging.info("Drawing new status table")
        image = StatusDrawer.draw_table(
            [
                ["name", "rssi", "SNR", "code", "last info", "last punch"],
            ]
            + self.generate_info_table(node_infos),
            self.epd.height,
            self.epd.width,
        )
        self.epd.display(self.epd.getbuffer(image))

    def clear(self):
        if self.epd is None:
            return
        self.epd.Clear()
