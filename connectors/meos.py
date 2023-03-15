from datetime import datetime, time
import socket
import logging
from .connector import Connector


PUNCH = int(0).to_bytes(1, "big")


class MeosConnector(Connector):
    """Class for sending punches to MeOS"""

    def __init__(self, host: str, port: int):
        self._socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._socket.connect((host, port))

    def __del__(self):
        self._socket.close()

    def _serialize(self, card_number: int, si_daytime: time, code: int) -> bytes:
        # Test case:
        # card_number = 46283
        # si_daytime = time(hour=7, minute=3, second=20)
        # code = 31
        # b'\x00\x1f\x00\xcb\xb4\x00\x00\x00\x00\x00\x000\xe0\x03\x00'

        total_seconds = (
            (si_daytime.hour * 60) + si_daytime.minute
        ) * 60 + si_daytime.second
        result = (
            PUNCH
            + code.to_bytes(2, "little")
            + card_number.to_bytes(4, "little")
            + int(0).to_bytes(4, "little")
            + (total_seconds * 10).to_bytes(4, "little")
        )
        return result

    def send_punch(
        self, card_number: int, sitime: datetime, now: datetime, code: int, mode: int
    ):
        del mode, now
        return self._send(self._serialize(card_number, sitime.time(), code))

    def _send(self, message: bytes):
        try:
            return self._socket.sendall(message)
        except Exception as e:
            logging.error(e)
            return e
