import asyncio
import logging
from concurrent.futures import Future
from datetime import datetime, time, timedelta
from typing import Literal

from ..pb.status_pb2 import MiniCallHome
# TODO: consider using https://pypi.org/project/backoff/
from ..utils.retries import BackoffRetries
from .client import Client

ENDIAN: Literal["little", "big"] = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
CARD = int(64).to_bytes(1, ENDIAN)
PUNCH_START = 1
PUNCH_FINISH = 2

CODE_DAY = int(0).to_bytes(4, ENDIAN)


class SirapClient(Client):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int, loop: asyncio.AbstractEventLoop):
        self.host = host
        self.port = port
        self.connected = False

        self._backoff_sender = BackoffRetries(self._send, 0.2, 2.0, timedelta(minutes=10), loop)
        asyncio.run_coroutine_threadsafe(self.keep_connected(), loop)

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

    async def keep_connected(self):
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

    def send_punch(
        self,
        card_number: int,
        sitime: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> Future:
        del mode
        message = SirapClient._serialize_punch(card_number, sitime.time(), code)
        return self._backoff_sender.send(message)

    def send_mini_call_home(self, mch: MiniCallHome):
        pass

    @staticmethod
    def _serialize_card(
        card_number: int, start: time | None, finish: time | None, punches: list[tuple[int, time]]
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

    def send_card(
        self,
        card_number: int,
        start: time | None,
        finish: time | None,
        punches: list[tuple[int, time]],
    ) -> Future:
        message = SirapClient._serialize_card(card_number, start, finish, punches)
        return self._backoff_sender.send(message)

    def close(self, timeout=10):
        self._backoff_sender.close(timeout)

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
