import asyncio
import logging
from abc import ABC, abstractmethod
from asyncio import Task
from typing import Sequence

from ..pb.status_pb2 import Status
from ..rs import SiPunchLog


class Client(ABC):
    """A client implementation

    If the client fails to connect or access a device, it should not crash, but try later in the
    'loop' function.
    """

    @abstractmethod
    async def loop(self):
        pass

    @abstractmethod
    async def send_punch(self, punch_log: SiPunchLog):
        pass

    @abstractmethod
    async def send_status(self, status: Status, mac_addr: str):
        pass

    @abstractmethod
    def name(self) -> str:
        pass


class ClientGroup:
    def __init__(self, clients: list[Client], tasks: list[Task]):
        self.clients = clients
        self.tasks = tasks

    def len(self) -> int:
        return len(self.clients)

    def handle_results(self, results: Sequence[BaseException | None]):
        for result, client in zip(results, self.clients):
            if isinstance(result, Exception):
                logging.error(f"{client.name()} failed: {result}")

    async def loop(self):
        loops = [client.loop() for client in self.clients]
        await asyncio.gather(*loops, return_exceptions=True)

    async def send_status(self, status: Status, mac_address: str) -> Sequence[None | BaseException]:
        handles = [client.send_status(status, mac_address) for client in self.clients]
        results = await asyncio.gather(*handles, return_exceptions=True)
        self.handle_results(results)
        return results

    async def send_punch(self, punch: SiPunchLog) -> Sequence[None | BaseException]:
        handles = [client.send_punch(punch) for client in self.clients]
        results = await asyncio.gather(*handles, return_exceptions=True)
        self.handle_results(results)
        return results
