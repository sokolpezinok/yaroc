import timestamp_pb2 as _timestamp_pb2
from google.protobuf.internal import containers as _containers
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Iterable as _Iterable, Mapping as _Mapping, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class Punches(_message.Message):
    __slots__ = ["punches", "sending_timestamp"]
    PUNCHES_FIELD_NUMBER: _ClassVar[int]
    SENDING_TIMESTAMP_FIELD_NUMBER: _ClassVar[int]
    punches: _containers.RepeatedScalarFieldContainer[bytes]
    sending_timestamp: _timestamp_pb2.Timestamp
    def __init__(self, punches: _Optional[_Iterable[bytes]] = ..., sending_timestamp: _Optional[_Union[_timestamp_pb2.Timestamp, _Mapping]] = ...) -> None: ...
