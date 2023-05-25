from abc import ABC, abstractmethod
from datetime import datetime

from ..pb.status_pb2 import MiniCallHome


class Client(ABC):
    """A client implementation

    All 'send*' functions must be non-blocking. Sending should be deferred to another thread and the
    functions should return a future-like object that can be awaited on.

    If the client fails to connect or access a device, it should not crash, but maybe try later.
    """

    @abstractmethod
    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ):
        pass

    @abstractmethod
    def send_mini_call_home(self, mch: MiniCallHome):
        pass
