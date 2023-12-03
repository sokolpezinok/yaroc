from datetime import datetime
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
    raw: bytes

    @staticmethod
    def new(card: int, code: int, time: datetime, mode: int) -> "SiPunch": ...
    @staticmethod
    def from_raw(payload: bytes) -> "SiPunch": ...
