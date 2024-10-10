from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from typing import ClassVar as _ClassVar, Optional as _Optional

DESCRIPTOR: _descriptor.FileDescriptor

class Timestamp(_message.Message):
    __slots__ = ["millis_epoch"]
    MILLIS_EPOCH_FIELD_NUMBER: _ClassVar[int]
    millis_epoch: int
    def __init__(self, millis_epoch: _Optional[int] = ...) -> None: ...
