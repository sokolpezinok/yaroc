from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Disconnected(_message.Message):
    __slots__ = ["client_name"]
    CLIENT_NAME_FIELD_NUMBER: _ClassVar[int]
    client_name: str
    def __init__(self, client_name: _Optional[str] = ...) -> None: ...

class MiniCallHome(_message.Message):
    __slots__ = ["cellid", "codes", "cpu_temperature", "freq", "local_ip", "mac_address", "max_freq", "min_freq", "network_type", "signal_dbm", "time", "totaldatarx", "totaldatatx", "volts"]
    CELLID_FIELD_NUMBER: _ClassVar[int]
    CODES_FIELD_NUMBER: _ClassVar[int]
    CPU_TEMPERATURE_FIELD_NUMBER: _ClassVar[int]
    FREQ_FIELD_NUMBER: _ClassVar[int]
    LOCAL_IP_FIELD_NUMBER: _ClassVar[int]
    MAC_ADDRESS_FIELD_NUMBER: _ClassVar[int]
    MAX_FREQ_FIELD_NUMBER: _ClassVar[int]
    MIN_FREQ_FIELD_NUMBER: _ClassVar[int]
    NETWORK_TYPE_FIELD_NUMBER: _ClassVar[int]
    SIGNAL_DBM_FIELD_NUMBER: _ClassVar[int]
    TIME_FIELD_NUMBER: _ClassVar[int]
    TOTALDATARX_FIELD_NUMBER: _ClassVar[int]
    TOTALDATATX_FIELD_NUMBER: _ClassVar[int]
    VOLTS_FIELD_NUMBER: _ClassVar[int]
    cellid: int
    codes: str
    cpu_temperature: float
    freq: int
    local_ip: str
    mac_address: str
    max_freq: int
    min_freq: int
    network_type: int
    signal_dbm: int
    time: _timestamp_pb2.Timestamp
    totaldatarx: int
    totaldatatx: int
    volts: float
    def __init__(self, mac_address: _Optional[str] = ..., local_ip: _Optional[str] = ..., cpu_temperature: _Optional[float] = ..., freq: _Optional[int] = ..., min_freq: _Optional[int] = ..., max_freq: _Optional[int] = ..., volts: _Optional[float] = ..., signal_dbm: _Optional[int] = ..., cellid: _Optional[int] = ..., network_type: _Optional[int] = ..., totaldatarx: _Optional[int] = ..., totaldatatx: _Optional[int] = ..., time: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ..., codes: _Optional[str] = ...) -> None: ...

class Status(_message.Message):
    __slots__ = ["disconnected", "mini_call_home"]
    DISCONNECTED_FIELD_NUMBER: _ClassVar[int]
    MINI_CALL_HOME_FIELD_NUMBER: _ClassVar[int]
    disconnected: Disconnected
    mini_call_home: MiniCallHome
    def __init__(self, disconnected: _Optional[_Union[Disconnected, _Mapping]] = ..., mini_call_home: _Optional[_Union[MiniCallHome, _Mapping]] = ...) -> None: ...
