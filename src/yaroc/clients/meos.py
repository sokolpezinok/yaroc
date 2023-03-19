import logging
import socket
from datetime import datetime, time, timedelta

from ..utils.backoff import BackoffSender
from .client import Client

ENDIAN = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
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
    def _serialize(card_number: int, si_daytime: time, code: int) -> bytes:
        total_seconds = ((si_daytime.hour * 60) + si_daytime.minute) * 60 + si_daytime.second
        result = (
            PUNCH
            + code.to_bytes(2, ENDIAN)
            + card_number.to_bytes(4, ENDIAN)
            + CODE_DAY
            + (total_seconds * 10).to_bytes(4, ENDIAN)
        )
        return result

    def send_punch(
        self,
        card_number: int,
        sitime: datetime,
        now: datetime,
        code: int,
        mode: int,
    ):
        del mode, now
        message = MeosClient._serialize(card_number, sitime.time(), code)
        self._backoff_sender.send(message)

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
