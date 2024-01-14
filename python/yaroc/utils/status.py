from dataclasses import dataclass, field
from datetime import datetime, timedelta
from enum import Enum
from typing import Dict, Set, TypeAlias


class CellularConnectionState(Enum):
    Unknown = 0
    Unregistered = 1
    Registered = 2
    MqttConnected = 3


@dataclass
class Position:
    lat: float
    lon: float
    elevation: float
    timestamp: datetime


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
    codes: Set[int] = field(default_factory=set)
    last_update: datetime | None = None
    last_punch: datetime | None = None

    def to_dict(self) -> Dict[str, str]:
        res = {}
        if len(self.codes) > 0:
            res["code"] = ",".join(map(str, self.codes))
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

    def update_position(self, lat: float, lon: float, timestamp: datetime):
        self.position = Position(lat, lon, 0, timestamp)
        self.last_update = datetime.now().astimezone()


RocStatus: TypeAlias = CellularRocStatus | MeshtasticRocStatus
