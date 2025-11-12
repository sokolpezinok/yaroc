from datetime import datetime, timedelta
from enum import IntEnum
from typing import ClassVar as _ClassVar
from typing import List, Tuple

from yaroc.clients.client import Client
from yaroc.pb.status_pb2 import Status

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
    def from_raw(payload: bytes, now: datetime) -> "SiPunch" | None: ...

class SiPunchLog(object):
    punch: SiPunch
    latency: timedelta
    host_info: HostInfo

    @staticmethod
    def new(punch: SiPunch, host_info: HostInfo, now: datetime) -> "SiPunchLog": ...
    def is_meshtastic(self) -> bool: ...

class SiUartHandler(object):
    @staticmethod
    def new() -> "SiUartHandler": ...
    async def add_device(self, port: str, device_node: str): ...
    def remove_device(self, device_node: str): ...
    async def next_punch(self) -> bytes: ...

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

class MqttConfig(object):
    url: str
    port: int
    keep_alive: timedelta
    meshtastic_channel: str | None

class CellularLog(object):
    def __repr__(self) -> str: ...
    def to_proto(self) -> bytes | None: ...
    def mac_address(self) -> str: ...

class MeshtasticLog(object):
    def __repr__(self) -> str: ...

class Event(object):
    pass

class MshDevHandler(object):
    def add_device(self, port: str, device_node: str): ...
    def remove_device(self, device_node: str): ...

class MessageHandler(object):
    def __init__(self, dns: List[Tuple[str, str]], config: MqttConfig | None = None): ...
    def msh_dev_handler(self) -> MshDevHandler: ...
    async def next_event(self) -> Event: ...

def current_timestamp_millis() -> int: ...

class SerialClient(Client):
    @staticmethod
    async def create(port: str) -> "SerialClient": ...
    async def loop(self): ...
    async def add_mini_reader(self, port: str): ...
    async def send_punch(self, punch_log: SiPunchLog) -> bool: ...
    async def send_status(self, status: Status, mac_addr: str) -> bool: ...
