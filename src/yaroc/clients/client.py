from abc import ABC, abstractmethod
import asyncio
from datetime import datetime

from ..pb.status_pb2 import MiniCallHome
from ..utils.si import SiPunch


class Client(ABC):
    """A client implementation

    All 'send*' functions must be non-blocking. Sending should be deferred to another thread and the
    functions should return a future-like object that can be awaited on. The 'send*' functions must
    not throw.

    If the client fails to connect or access a device, it should not crash, but maybe try later.
    """

    @abstractmethod
    async def loop(self):
        pass

    @abstractmethod
    async def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> bool:
        pass

    @abstractmethod
    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        pass


class ClientGroup:
    def __init__(self, clients: list[Client]):
        self.clients = clients

    async def loop(self):
        loops = [client.loop() for client in self.clients]
        await asyncio.gather(*loops)

    async def send_mini_call_home(self, mch: MiniCallHome) -> list[bool]:
        handles = [client.send_mini_call_home(mch) for client in self.clients]
        return await asyncio.gather(*handles)

    async def send_punch(self, punch: SiPunch) -> list[bool]:
        handles = [
            client.send_punch(punch.card, punch.time, punch.code, punch.mode)
            for client in self.clients
        ]
        return await asyncio.gather(*handles)
