from datetime import datetime, timedelta
from enum import IntEnum
from typing import ClassVar as _ClassVar
from typing import List, Tuple

class HostInfo(object):
    mac_address: str
    @staticmethod
    def new(name: str, mac_address: str) -> "HostInfo": ...

class SiPunch(object):
    CARD_FIELD_NUMBER: _ClassVar[int]
    CODE_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    card: int
    code: int
    time: datetime
    mode: int
    raw: bytes

    @staticmethod
    def new(
        card: int,
        code: int,
        time: datetime,
        mode: int,
    ) -> "SiPunch": ...
    @staticmethod
    def from_raw(payload: bytes) -> "SiPunch": ...

class SiPunchLog(object):
    punch: SiPunch
    latency: timedelta
    host_info: HostInfo

    @staticmethod
    def new(punch: SiPunch, host_info: HostInfo, now: datetime) -> "SiPunchLog": ...
    @staticmethod
    def from_msh_serial(payload: bytes) -> "SiPunchLog": ...
    def is_meshtastic(self) -> bool: ...

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

class NodeInfo(object):
    name: str
    rssi_dbm: int | None
    snr_db: float | None
    codes: list[int]
    last_update: datetime
    last_punch: datetime

class MessageHandler(object):
    @staticmethod
    def new(dns: List[Tuple[str, str]], meshtastic_override_mac: str | None): ...
    def meshtastic_serial_msg(self, payload: bytes) -> list[SiPunch]: ...
    def meshtastic_status_update(self, payload: bytes, now: datetime, recv_mac_address: int): ...
    def punches(self, payload: bytes, mac_address: int) -> list[SiPunch]: ...
    def status_update(self, payload: bytes, mac_address: int): ...
    def node_infos(self) -> list[NodeInfo]: ...

def current_timestamp_millis() -> int: ...
