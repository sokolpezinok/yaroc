import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

Added: EventType
DESCRIPTOR: _descriptor.FileDescriptor
Lte: CellNetworkType
LteM: CellNetworkType
NbIotEcl0: CellNetworkType
NbIotEcl1: CellNetworkType
NbIotEcl2: CellNetworkType
Removed: EventType
Umts: CellNetworkType
Unknown: EventType
UnknownNetworkType: CellNetworkType

class Coordinates(_message.Message):
    __slots__ = ["altitude", "latitude", "longitude", "time"]
    ALTITUDE_FIELD_NUMBER: _ClassVar[int]
    LATITUDE_FIELD_NUMBER: _ClassVar[int]
    LONGITUDE_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    altitude: float
    latitude: float
    longitude: float
    time: _timestamp_pb2.Timestamp
    def __init__(self, latitude: _Optional[float] = ..., longitude: _Optional[float] = ..., altitude: _Optional[float] = ..., time: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class DeviceEvent(_message.Message):
    __slots__ = ["port", "type"]
    PORT_FIELD_NUMBER: _ClassVar[int]
    TYPE_FIELD_NUMBER: _ClassVar[int]
    port: str
    type: EventType
    def __init__(self, port: _Optional[str] = ..., type: _Optional[_Union[EventType, str]] = ...) -> None: ...

class Disconnected(_message.Message):
    __slots__ = ["client_name"]
    CLIENT_NAME_FIELD_NUMBER: _ClassVar[int]
    client_name: str
    def __init__(self, client_name: _Optional[str] = ...) -> None: ...

class MiniCallHome(_message.Message):
    __slots__ = ["cellid", "codes", "cpu_temperature", "freq", "local_ip", "max_freq", "millivolts", "min_freq", "network_type", "signal_dbm", "signal_snr_cb", "time", "totaldatarx", "totaldatatx"]
    CELLID_FIELD_NUMBER: _ClassVar[int]
    CODES_FIELD_NUMBER: _ClassVar[int]
    CPU_TEMPERATURE_FIELD_NUMBER: _ClassVar[int]
    FREQ_FIELD_NUMBER: _ClassVar[int]
    LOCAL_IP_FIELD_NUMBER: _ClassVar[int]
    MAX_FREQ_FIELD_NUMBER: _ClassVar[int]
    MILLIVOLTS_FIELD_NUMBER: _ClassVar[int]
    MIN_FREQ_FIELD_NUMBER: _ClassVar[int]
    NETWORK_TYPE_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_DBM_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_SNR_CB_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    TOTALDATARX_FIELD_NUMBER: _ClassVar[int]
    TOTALDATATX_FIELD_NUMBER: _ClassVar[int]
    cellid: int
    codes: _containers.RepeatedScalarFieldContainer[int]
    cpu_temperature: float
    freq: int
    local_ip: int
    max_freq: int
    millivolts: int
    min_freq: int
    network_type: CellNetworkType
    signal_dbm: int
    signal_snr_cb: int
    time: _timestamp_pb2.Timestamp
    totaldatarx: int
    totaldatatx: int
    def __init__(self, local_ip: _Optional[int] = ..., cpu_temperature: _Optional[float] = ..., freq: _Optional[int] = ..., min_freq: _Optional[int] = ..., max_freq: _Optional[int] = ..., millivolts: _Optional[int] = ..., signal_dbm: _Optional[int] = ..., signal_snr_cb: _Optional[int] = ..., cellid: _Optional[int] = ..., network_type: _Optional[_Union[CellNetworkType, str]] = ..., codes: _Optional[_Iterable[int]] = ..., totaldatarx: _Optional[int] = ..., totaldatatx: _Optional[int] = ..., time: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...

class Status(_message.Message):
    __slots__ = ["dev_event", "disconnected", "mini_call_home"]
    DEV_EVENT_FIELD_NUMBER: _ClassVar[int]
    DISCONNECTED_FIELD_NUMBER: _ClassVar[int]
    MINI_CALL_HOME_FIELD_NUMBER: _ClassVar[int]
    dev_event: DeviceEvent
    disconnected: Disconnected
    mini_call_home: MiniCallHome
    def __init__(self, disconnected: _Optional[_Union[Disconnected, _Mapping]] = ..., mini_call_home: _Optional[_Union[MiniCallHome, _Mapping]] = ..., dev_event: _Optional[_Union[DeviceEvent, _Mapping]] = ...) -> None: ...

class EventType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = []

class CellNetworkType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = []
