import logging
import socket
from concurrent.futures import Future
from datetime import datetime, time, timedelta
from typing import Literal

from ..pb.status_pb2 import MiniCallHome
# TODO: consider using https://pypi.org/project/backoff/
from ..utils.backoff import BackoffSender
from .client import Client

ENDIAN: Literal["little", "big"] = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
CARD = int(64).to_bytes(1, ENDIAN)
PUNCH_START = 1
PUNCH_FINISH = 2

CODE_DAY = int(0).to_bytes(4, ENDIAN)


class MeosClient(Client):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int):
        self.address = (host, port)
        self._connect()

        self._backoff_sender = BackoffSender(
            self._send, self._on_publish, 0.2, 2.0, timedelta(minutes=10)
        )

    def __del__(self):
        if self._socket is not None:
            self._socket.close()

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
            + MeosClient._time_to_bytes(si_daytime)
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
        message = MeosClient._serialize_punch(card_number, sitime.time(), code)
        return self._backoff_sender.send(message)

    def send_mini_call_home(self, mch: MiniCallHome):
        pass

    @staticmethod
    def _serialize_card(
        card_number: int, start: time | None, finish: time | None, punches: list[tuple[int, time]]
    ) -> bytes:
        def serialize_card_punch(code: int, si_daytime: time) -> bytes:
            return code.to_bytes(4, ENDIAN) + MeosClient._time_to_bytes(si_daytime)

        punch_count: int = len(punches) + int(start is not None) + int(finish is not None)
        result = (
            CARD
            + punch_count.to_bytes(2, ENDIAN)
            + card_number.to_bytes(4, ENDIAN)
            + CODE_DAY
            + MeosClient._time_to_bytes(time())
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
        message = MeosClient._serialize_card(card_number, start, finish, punches)
        return self._backoff_sender.send(message)

    def close(self, timeout=10):
        self._backoff_sender.close(timeout)

    def _connect(self):
        try:
            self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        except OSError:
            self._socket = None
            return
        self._socket.connect(self.address)

    def _send(self, message: bytes):
        try:
            if self._socket is None:
                raise Exception("Not connected")

            ret = self._socket.sendall(message)
            if ret is None:
                return
            raise Exception("Failed sending")
        except socket.error as err:
            if self._socket is not None:
                self._socket.close()
            self._connect()
            raise err

    def _on_publish(self, message: bytes):
        del message
        logging.info("Published!")
