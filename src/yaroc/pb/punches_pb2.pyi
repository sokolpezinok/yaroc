from google.protobuf import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Punch(_message.Message):
    __slots__ = ["card", "code", "mode", "process_time", "si_time"]
    CARD_FIELD_NUMBER: _ClassVar[int]
    CODE_FIELD_NUMBER: _ClassVar[int]
    MODE_FIELD_NUMBER: _ClassVar[int]
    PROCESS_TIME_FIELD_NUMBER: _ClassVar[int]
    SI_TIME_FIELD_NUMBER: _ClassVar[int]
    card: int
    code: int
    mode: int
    process_time: _timestamp_pb2.Timestamp
    si_time: _timestamp_pb2.Timestamp
    def __init__(self, code: _Optional[int] = ..., card: _Optional[int] = ..., si_time: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ..., process_time: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ..., mode: _Optional[int] = ...) -> None: ...

class Punches(_message.Message):
    __slots__ = ["punches"]
    PUNCHES_FIELD_NUMBER: _ClassVar[int]
    punches: _containers.RepeatedCompositeFieldContainer[Punch]
    def __init__(self, punches: _Optional[_Iterable[_Union[Punch, _Mapping]]] = ...) -> None: ...
