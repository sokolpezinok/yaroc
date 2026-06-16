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
    signal_strength: str
    battery_percentage: int | None
    codes: list[int]
    last_update: datetime
    last_punch: datetime

class MqttConfig(object):
    url: str
    port: int
    credentials: tuple[str, str] | None
    keep_alive: timedelta
    meshtastic_channel: str | None

class CellularLog(object):
    def __repr__(self) -> str: ...
    def to_proto(self) -> bytes | None: ...
    def mac_address(self) -> str: ...

class MeshtasticLog(object):
    service_envelope: bytes
    channel: str
    gateway_id: str
    def __repr__(self) -> str: ...

class MeshtasticPunches(object):
    punch_logs: List[SiPunchLog]
    service_envelope: bytes
    channel: str
    gateway_id: str
    def __repr__(self) -> str: ...

class Event(object):
    class CellularLog(Event):
        __match_args__ = ("log",)
        log: CellularLog

    class SiPunchLogs(Event):
        __match_args__ = ("logs",)
        logs: List[SiPunchLog]

    class SiPunch(Event):
        __match_args__ = ("punch",)
        punch: SiPunch

    class MeshtasticLog(Event):
        __match_args__ = ("log",)
        log: MeshtasticLog
        service_envelope: bytes

    class MeshtasticPunches(Event):
        __match_args__ = ("log",)
        punches: MeshtasticPunches

    class NodeInfos(Event):
        __match_args__ = ("node_infos",)
        node_infos: List[NodeInfo]

    class DeviceEvnt(Event):
        __match_args__ = ("added", "device")
        added: bool
        device: str

class PyUsbSerialFactory(object):
    pass

class MessageHandlerBuilder(object):
    def __init__(self) -> None: ...
    def with_dns(self, dns: List[Tuple[str, str]]) -> "MessageHandlerBuilder": ...
    def with_mqtt_configs(self, mqtt_configs: List[MqttConfig]) -> "MessageHandlerBuilder": ...
    def with_node_info_interval(self, interval: timedelta) -> "MessageHandlerBuilder": ...
    def with_meshtastic_timeout(self, timeout: timedelta) -> "MessageHandlerBuilder": ...
    def with_meshtastic(self, enable: bool) -> "MessageHandlerBuilder": ...
    def with_sportident(self, enable: bool) -> "MessageHandlerBuilder": ...
    def with_sportident_factory(
        self, factory: PyUsbSerialFactory | None
    ) -> "MessageHandlerBuilder": ...
    def with_tcp(self, host: str) -> "MessageHandlerBuilder": ...
    def with_fake_punch(self, interval: timedelta) -> "MessageHandlerBuilder": ...
    def build(self) -> "MessageHandler": ...

class MessageHandler(object):
    async def next_event(self) -> Event: ...

def current_timestamp_millis() -> int: ...

class SerialClient(Client):
    @staticmethod
    async def create(port: str, retry: bool) -> "SerialClient": ...
    async def loop(self): ...
    async def send_punch(self, punch_log: SiPunchLog): ...
    async def send_punch_noexcept(self, punch_log: SiPunchLog): ...
    async def send_status(self, status: Status, mac_addr: str): ...
    async def send_status_noexcept(self, status: Status, mac_addr: str): ...
    def name(self) -> str: ...
    def usb_serial_factory(self) -> PyUsbSerialFactory: ...
