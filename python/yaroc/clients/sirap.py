import asyncio
import logging
from datetime import time
from typing import Literal

from ..pb.status_pb2 import Status
from ..rs import SiPunchLog
from .client import Client

ENDIAN: Literal["little", "big"] = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
CARD = int(64).to_bytes(1, ENDIAN)
PUNCH_START = 1
PUNCH_FINISH = 2

CODE_DAY = int(0).to_bytes(4, ENDIAN)


class SirapClient(Client):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self.connected = False

    def __del__(self):
        if self._socket is not None:
            self._socket.close()

    async def _connect(self, host: str, port: int):
        if self.connected:
            return
        try:
            reader, writer = await asyncio.open_connection(host, port)
            self._reader = reader
            self._writer = writer
            self.connected = True
        except Exception as err:
            logging.error(f"Error connecting to SIRAP endpoint: {err}")
            self.connected = False
            return

    async def loop(self):
        while True:
            await self._connect(self.host, self.port)
            await asyncio.sleep(20)  # TODO: configure timeout

    @staticmethod
    def _time_to_bytes(daytime: time) -> bytes:
        total_seconds = ((daytime.hour * 60) + daytime.minute) * 60 + daytime.second
        return (total_seconds * 10).to_bytes(4, ENDIAN)

    @staticmethod
    def _serialize_punch(card_number: int, si_daytime: time, code: int) -> bytes:
        return (
            PUNCH
            + code.to_bytes(2, ENDIAN)
            + card_number.to_bytes(4, ENDIAN)
            + CODE_DAY
            + SirapClient._time_to_bytes(si_daytime)
        )

    async def send_punch(self, punch_log: SiPunchLog) -> bool:
        punch = punch_log.punch
        message = SirapClient._serialize_punch(punch.card, punch.time.time(), punch.code)
        return await self._send(message)

    async def send_status(self, status: Status, mac_addr: str) -> bool:
        return True

    @staticmethod
    def _serialize_card(
        card_number: int,
        start: time | None,
        finish: time | None,
        punches: list[tuple[int, time]],
    ) -> bytes:
        def serialize_card_punch(code: int, si_daytime: time) -> bytes:
            return code.to_bytes(4, ENDIAN) + SirapClient._time_to_bytes(si_daytime)

        punch_count: int = len(punches) + int(start is not None) + int(finish is not None)
        result = (
            CARD
            + punch_count.to_bytes(2, ENDIAN)
            + card_number.to_bytes(4, ENDIAN)
            + CODE_DAY
            + SirapClient._time_to_bytes(time())
        )
        if start is not None:
            result += serialize_card_punch(PUNCH_START, start)
        for code, tim in punches:
            result += serialize_card_punch(code, tim)
        if finish is not None:
            result += serialize_card_punch(PUNCH_FINISH, finish)
        return result

    async def send_card(
        self,
        card_number: int,
        start: time | None,
        finish: time | None,
        punches: list[tuple[int, time]],
    ) -> bool:
        message = SirapClient._serialize_card(card_number, start, finish, punches)
        return await self._send(message)

    def close(self, timeout=10):
        pass

    # TODO: consider using https://pypi.org/project/backoff/
    async def _send(self, message: bytes) -> bool:
        if not self.connected:
            raise Exception("Not connected")
        try:
            self._writer.write(message)
            await self._writer.drain()
            return True
        except (ConnectionResetError, BrokenPipeError) as err:
            self.connected = False
            raise err
        except Exception as err:
            raise err
        return False
