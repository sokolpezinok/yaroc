from datetime import datetime

import LiveObjects
from .connector import Connector


class LiveObjectsConnector(Connector):
    """Class for LiveObjects communication"""

    def __init__(self):
        self.lo = LiveObjects.Connection()
        self.lo.connect()

    def __del__(self):
        self.lo.disconnect()

    def send(
        self, card_number: int, sitime: datetime, now: datetime, code: int, mode: int
    ):
        def length_of_number(x: int) -> int:
            return len(str(x))

        self.lo.add_to_payload("sinumber", card_number)
        self.lo.add_to_payload("control", code)
        self.lo.add_to_payload("date", sitime.strftime("%H:%M:%S"))
        self.lo.add_to_payload("sitime", sitime.strftime("%Y-%m-%d"))
        self.lo.add_to_payload("ms", sitime.strftime("%f")[:3])
        self.lo.add_to_payload("roctime", now.strftime("%Y-%m-%d %H:%M:%S"))
        self.lo.add_to_payload("macaddr", "b827eb1d3c4f")
        self.lo.add_to_payload(
            "length", length_of_number(card_number) + length_of_number(code)
        )
        self.lo.add_to_payload("mode", mode)
        self.lo.send_data()
