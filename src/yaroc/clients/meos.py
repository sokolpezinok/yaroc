import logging
import socket
from datetime import datetime, time

from .client import Client

ENDIAN = "little"
PUNCH = int(0).to_bytes(1, ENDIAN)
CODE_DAY = int(0).to_bytes(4, ENDIAN)


class MeosClient(Client):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int):
        self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._socket.connect((host, port))

    def __del__(self):
        self._socket.close()

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
        self, card_number: int, sitime: datetime, now: datetime, code: int, mode: int
    ):
        del mode, now
        return self._send(MeosClient._serialize(card_number, sitime.time(), code))

    def _send(self, message: bytes):
        try:
            return self._socket.sendall(message)
        except Exception as e:
            logging.error(e)
            return e
