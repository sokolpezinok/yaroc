from abc import ABC, abstractmethod
from datetime import datetime

from ..pb.status_pb2 import MiniCallHome


class Client(ABC):
    @abstractmethod
    def send_punch(self, card_number: int, si_time: datetime, now: datetime, code: int, mode: int):
        pass

    @abstractmethod
    def send_mini_call_home(self, mch: MiniCallHome):
        pass
