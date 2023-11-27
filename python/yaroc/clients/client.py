import asyncio
import logging
from abc import ABC, abstractmethod

import serial
from serial_asyncio import open_serial_connection

from yaroc.rs import SiPunch

from ..pb.status_pb2 import MiniCallHome


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
    async def send_punch(self, punch: SiPunch) -> bool:
        pass

    @abstractmethod
    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        return True


class SerialClient(Client):
    def __init__(self, port: str):
        self.port = port
        self.writer = None

    async def loop(self):
        try:
            async with asyncio.timeout(10):
                _, self.writer = await open_serial_connection(
                    url=self.port, baudrate=38400, rtscts=False
                )
            logging.info(f"Connected to SRR sink at {self.port}")
        except Exception as err:
            logging.error(f"Error connecting to {self.port}: {err}")
            return

    async def send_punch(self, punch: SiPunch) -> bool:
        try:
            self.writer.write(bytes(punch.raw))
            return True
        except serial.serialutil.SerialException as err:
            logging.error(f"Fatal serial exception: {err}")
            return False

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        return True


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
        handles = [client.send_punch(punch) for client in self.clients]
        return await asyncio.gather(*handles)
