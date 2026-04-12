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
        """Infinite loop to process background tasks or maintain connection."""
        pass

    async def send_punch_noexcept(self, punch_log: SiPunchLog) -> bool:
        """Send a punch log entry, catching any exceptions."""
        try:
            await self.send_punch(punch_log)
            return True
        except Exception as e:
            logging.error(f"{self.name()} failed: {e}")
            return False

    @abstractmethod
    async def send_punch(self, punch_log: SiPunchLog):
        """Send a punch log entry."""
        pass

    async def send_status_noexcept(self, status: Status, mac_addr: str) -> bool:
        """Send a status update, catching any exceptions."""
        try:
            await self.send_status(status, mac_addr)
            return True
        except Exception as e:
            logging.error(f"{self.name()} failed: {e}")
            return False

    @abstractmethod
    async def send_status(self, status: Status, mac_addr: str):
        """Send a status update."""
        pass

    @abstractmethod
    def name(self) -> str:
        """Get the name of the client."""
        pass


class ClientGroup:
    """A group of clients managed as a single entity."""

    def __init__(self, clients: list[Client], tasks: list[Task]):
        self.clients = clients
        self.tasks = tasks

    def len(self) -> int:
        """Number of clients in the group."""
        return len(self.clients)

    async def loop(self):
        """Run the infinite loops of all clients in the group."""
        loops = [client.loop() for client in self.clients]
        await asyncio.gather(*loops, return_exceptions=True)

    async def send_status(self, status: Status, mac_address: str) -> Sequence[bool]:
        """Send a status update to all clients in the group."""
        handles = [client.send_status_noexcept(status, mac_address) for client in self.clients]
        return await asyncio.gather(*handles)

    async def send_punch(self, punch: SiPunchLog) -> Sequence[bool]:
        """Send a punch log entry to all clients in the group."""
        handles = [client.send_punch_noexcept(punch) for client in self.clients]
        return await asyncio.gather(*handles)
