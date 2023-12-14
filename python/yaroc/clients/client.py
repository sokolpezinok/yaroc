import asyncio
import logging
from abc import ABC, abstractmethod

import serial
from serial_asyncio import open_serial_connection

from ..pb.status_pb2 import MiniCallHome
from ..rs import SiPunch


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
    """Serial client emulating an SRR dongle."""
    def __init__(self, port: str):
        self.port = port
        self.writer = None

    async def loop(self):
        try:
            async with asyncio.timeout(10):
                reader, self.writer = await open_serial_connection(
                    url=self.port,
                    baudrate=38400,
                    timeout=5,
                )
            logging.info(f"Connected to SRR sink at {self.port}")
        except Exception as err:
            logging.error(f"Error connecting to {self.port}: {err}")
            return

        while True:
            data = await reader.readuntil(b"\x03")
            if data == b"\xff\x02\x02\xf0\x01Mm\n\x03":
                logging.info("Responding to orienteering software")
                self.writer.write(b"\xff\x02\xf0\x03\x12\x8cMb?\x03")
                data = await reader.readuntil(b"\x03")
                if data == b"\x02\x83\x02\x00\x80\xbf\x17\x03":
                    # MeOS
                    msg = (
                        b"\xff\x02\x83\x83\x12\x8c\x00\r\x00\x12\x8c\x04450\x16\x0b\x0fo!\xff\xff"
                        b"\xff\x02\x06\x00\x1b\x17?\x18\x18\x06)\x08\x05>\xfe\n\xeb\n\xeb\xff\xff"
                        b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x92\xba\x1aB\x01\xff\xff"
                        b"\xe1\xff\xff\xff\xff\xff\x01\x01\x01\x0b\x07\x0c\x00\r]\x0eD\x0f\xec\x10-"
                        b"\x11;\x12s\x13#\x14;\x15\x01\x19\x1d\x1a\x1c\x1b\xc7\x1c\x00\x1d\xb0!\xb6"
                        b'"\x10#\xea$\n%\x00&\x11,\x88-1.\x0b\xff\xff\xff\xff\xff\xff\xff\xff\xff'
                        b"\xff\xff\xff\xff\xff\xf9\xc3\x03"
                    )
                    self.writer.write(msg)

    async def send_punch(self, punch: SiPunch) -> bool:
        if self.writer is None:
            logging.error("Serial client not connected")
            return False
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
