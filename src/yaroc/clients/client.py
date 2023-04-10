from abc import ABC, abstractmethod
from datetime import datetime


class Client(ABC):
    @abstractmethod
    def send_punch(self, card_number: int, si_time: datetime, now: datetime, code: int, mode: int):
        pass
