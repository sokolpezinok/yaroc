import logging
import socket
import threading
from datetime import datetime, time, timedelta

from ..utils.scheduler import BackoffSender
from .client import Client

ENDIAN = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
CODE_DAY = int(0).to_bytes(4, ENDIAN)


class MeosClient(Client):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int):
        self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._socket.connect((host, port))
        self._backoff_sender = BackoffSender(
            self._send, self._on_publish, 0.2, 2.0, timedelta(minutes=10)
        )

    def __del__(self):
        self._socket.close()

    def loop_start(self):
        self.thread = threading.Thread(target=self._backoff_sender.loop)
        self.thread.daemon = True
        self.thread.start()

    @staticmethod
    def _serialize(card_number: int, si_daytime: time, code: int) -> bytes:
        total_seconds = (
            (si_daytime.hour * 60) + si_daytime.minute
        ) * 60 + si_daytime.second
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
        self._backoff_sender.send((message,))

    def _send(self, message: bytes):
        return self._socket.sendall(message)

    def _on_publish(self, message: bytes):
        del message
        logging.info("Published!")
