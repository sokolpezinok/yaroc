from abc import ABC, abstractmethod
from datetime import datetime


class Client(ABC):
    @abstractmethod
    def send_punch(self, card_number: int, sitime: datetime, now: datetime, code: int):
        pass
