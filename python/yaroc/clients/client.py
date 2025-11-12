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
    async def send_punch(self, punch_log: SiPunchLog) -> bool:
        pass

    @abstractmethod
    async def send_status(self, status: Status, mac_addr: str) -> bool:
        return True


class ClientGroup:
    def __init__(self, clients: list[Client], tasks: list[Task]):
        self.clients = clients
        self.tasks = tasks

    def len(self) -> int:
        return len(self.clients)

    @staticmethod
    def handle_results(results: Sequence[bool | BaseException]):
        for result in results:
            if isinstance(result, Exception):
                # TODO: write client name too
                logging.error(f"{result}")

    async def loop(self):
        loops = [client.loop() for client in self.clients]
        await asyncio.gather(*loops, return_exceptions=True)

    async def send_status(self, status: Status, mac_address: str) -> Sequence[bool | BaseException]:
        handles = [client.send_status(status, mac_address) for client in self.clients]
        results = await asyncio.gather(*handles, return_exceptions=True)
        ClientGroup.handle_results(results)
        return results

    async def send_punch(self, punch: SiPunchLog) -> Sequence[bool | BaseException]:
        handles = [client.send_punch(punch) for client in self.clients]
        results = await asyncio.gather(*handles)
        ClientGroup.handle_results(results)
        return results
