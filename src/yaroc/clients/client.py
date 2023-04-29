from abc import ABC, abstractmethod
from datetime import datetime

from ..pb.status_pb2 import MiniCallHome


class Client(ABC):
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
