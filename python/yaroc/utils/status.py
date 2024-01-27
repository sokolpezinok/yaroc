import itertools
from dataclasses import dataclass, field
from datetime import datetime, timedelta
from enum import Enum
from typing import Callable, Dict, Set

from PIL import Image, ImageDraw, ImageFont

from ..rs import Position


class CellularConnectionState(Enum):
    Unknown = 0
    Unregistered = 1
    Registered = 2
    MqttConnected = 3


def human_time(delta: timedelta) -> str:
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


@dataclass
class CellularRocStatus:
    voltage: float = 0.0
    state: CellularConnectionState = CellularConnectionState.Unknown
    dbm: int = 0  # These are only relevant when registered, it could be tied to the state enum
    cell: int = 0
    codes: Set[int] = field(default_factory=set)
    last_update: datetime | None = None
    last_punch: datetime | None = None

    def disconnect(self):
        self.dbm = 0
        self.cell = 0
        self.state = CellularConnectionState.Unknown

    def punch(self, timestamp: datetime, code: int):
        self.last_punch = timestamp
        self.codes.add(code)

    def connection_state(self, dbm: int, cell: int):
        self.dbm = dbm
        self.cell = cell
        self.state = CellularConnectionState.MqttConnected
        self.last_update = datetime.now().astimezone()

    def to_dict(self) -> Dict[str, str]:
        res = {}
        if self.state == CellularConnectionState.MqttConnected:
            res["dbm"] = f"{self.dbm}"
            res["cell"] = f"{self.cell:X}"
        if len(self.codes) > 0:
            res["code"] = ",".join(map(str, self.codes))
        if self.last_update is not None:
            res["last_update"] = human_time(datetime.now().astimezone() - self.last_update)
        if self.last_punch is not None:
            res["last_punch"] = human_time(datetime.now().astimezone() - self.last_punch)
        return res


@dataclass
class MeshtasticRocStatus:
    voltage: float = 0.0
    position: Position | None = None
    dbm: int | None = None
    codes: Set[int] = field(default_factory=set)
    last_update: datetime | None = None
    last_punch: datetime | None = None

    def to_dict(self) -> Dict[str, str]:
        res = {}
        if len(self.codes) > 0:
            res["code"] = ",".join(map(str, self.codes))
        if self.dbm is not None:
            res["dbm"] = f"{self.dbm}"
        if self.last_update is not None:
            res["last_update"] = human_time(datetime.now().astimezone() - self.last_update)
        if self.last_punch is not None:
            res["last_punch"] = human_time(datetime.now().astimezone() - self.last_punch)
        return res

    def punch(self, timestamp: datetime, code: int):
        self.last_punch = timestamp
        self.codes.add(code)

    def update_voltage(self, voltage: float):
        self.voltage = voltage
        self.last_update = datetime.now().astimezone()

    def update_dbm(self, dbm: int):
        self.dbm = dbm
        self.last_update = datetime.now().astimezone()

    def update_position(self, lat: float, lon: float, timestamp: datetime):
        self.position = Position.new(lat, lon, timestamp)
        self.last_update = datetime.now().astimezone()


class StatusTracker:
    """Class for tracking the status of all nodes"""

    def __init__(self, dns_resolver: Callable[[str], str], display_model: str | None = None):
        self.cellular_status: Dict[str, CellularRocStatus] = {}
        self.meshtastic_status: Dict[str, MeshtasticRocStatus] = {}
        self.dns_resolver = dns_resolver
        if display_model is not None:
            import epaper

            self.epd = epaper.epaper(display_model).EPD()
            self.epd.init(0)
            self.epd.Clear()
        else:
            self.epd = None

    def get_cellular_status(self, mac_addr: str) -> CellularRocStatus:
        return self.cellular_status.setdefault(mac_addr, CellularRocStatus())

    def get_meshtastic_status(self, mac_addr: str) -> MeshtasticRocStatus:
        return self.meshtastic_status.setdefault(mac_addr, MeshtasticRocStatus())

    def generate_info_table(self) -> list[list[str]]:
        table = []
        for mac_addr, status in self.cellular_status.items():
            row = [self.dns_resolver(mac_addr)]
            map = status.to_dict()
            row.append(map.get("dbm", ""))
            row.append(map.get("code", ""))
            row.append(map.get("last_update", ""))
            row.append(map.get("last_punch", ""))
            table.append(row)

        for mac_addr, msh_status in self.meshtastic_status.items():
            row = [self.dns_resolver(mac_addr)]
            map = msh_status.to_dict()
            row.append(map.get("dbm", ""))
            row.append(map.get("code", ""))
            row.append(map.get("last_update", ""))
            row.append(map.get("last_punch", ""))
            table.append(row)
        return table

    def distance_km(self, mac_addr1: str, mac_addr2: str) -> float | None:
        msh_status1 = self.get_meshtastic_status(mac_addr1)
        msh_status2 = self.get_meshtastic_status(mac_addr2)
        if msh_status1 is None or msh_status2 is None:
            return None
        pos1 = msh_status1.position
        pos2 = msh_status2.position
        if pos1 is None or pos2 is None:
            return None
        return pos1.distance_m(pos2) / 1000

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

        for i, partial_sum in enumerate(itertools.accumulate(cols[:-1])):
            x = calc_col_start(i + 1, partial_sum)
            draw.line((x, 0, x, real_height), fill=0)

        for row_idx in range(1, row_count):
            y = calc_row_start(row_idx)
            draw.line((0, y, real_width, y), fill=0)

        for row_idx, row in enumerate(table):
            y = calc_row_start(row_idx)
            for col_idx, partial_sum in enumerate(itertools.accumulate([0] + cols[:-1])):
                x = calc_col_start(col_idx, partial_sum)
                draw.text((x + horiz_pad, y), row[col_idx], font=font, fill=0)

        return image

    def draw_status(self):
        if self.epd is None:
            return
        image = StatusTracker.draw_table(
            [
                ["name", "dBm", "code", "last info", "last punch"],
            ]
            + self.generate_info_table(),
            self.epd.height,
            self.epd.width,
        )
        self.epd.display(self.epd.getbuffer(image))
