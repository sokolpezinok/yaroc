from datetime import datetime
from enum import IntEnum
from typing import ClassVar as _ClassVar

class SiPunch(object):
    CARD_FIELD_NUMBER: _ClassVar[int]
    CODE_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    card: int
    code: int
    time: datetime
    mode: int
    mac_addr: str
    raw: bytes

    @staticmethod
    def new(card: int, code: int, time: datetime, mode: int, mac_addr: str) -> "SiPunch": ...
    @staticmethod
    def from_raw(payload: bytes, mac_addr: str) -> "SiPunch": ...
    @staticmethod
    def from_msh_serial(payload: bytes) -> "SiPunch": ...

class RaspberryModel(IntEnum):
    Unknown = 0
    V1A = 1
    V1B = 2
    V1Ap = 3
    V1Bp = 4
    V2A = 5
    V2B = 6
    V3A = 7
    V3B = 8
    V3Ap = 9
    V3Bp = 10
    V4A = 11
    V4B = 12
    V5A = 13
    V5B = 14
    VZero = 15
    VZeroW = 16

    @staticmethod
    def from_string(model_info: str) -> "RaspberryModel": ...

class CellularRocStatus(object):
    @staticmethod
    def new() -> "CellularRocStatus": ...
    def punch(self, punch: SiPunch): ...
    def mqtt_connect_update(self, dbm: int, cellid: int): ...
    def disconnect(self): ...

class MeshtasticRocStatus(object):
    @staticmethod
    def new() -> "MeshtasticRocStatus": ...
    def punch(self, punch: SiPunch): ...
    def update_dbm(self, dbm: int): ...
    def update_voltage(self, voltage: float): ...
    def update_position(self, lat: float, lon: float, timestamp: datetime): ...
    def distance_m(self, other: MeshtasticRocStatus) -> float: ...

class NodeInfo(object):
    name: str
    dbm: int | None
    codes: list[int]
    last_update: datetime
    last_punch: datetime

class DbmSnr(object):
    def __init__(self, dbm: int, snr: float, distance: tuple[float, str] | None): ...

class MshLogMessage(object):
    voltage_battery: tuple[float, int]
    dbm_snr: DbmSnr
    def __init__(self, name: str, timestamp: datetime, now: datetime): ...
    def set_position(self, lat: float, lon: float, elevation: int, timestamp: datetime): ...

class CellularLogMessage(object):
    dbm: float | None
    cellid: int | None
    temperature: float | None
    def __init__(self, name: str, timestamp: datetime, now: datetime, voltage: float): ...
    def set_position(self, lat: float, lon: float, elevation: int, timestamp: datetime): ...
