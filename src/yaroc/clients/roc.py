import logging
import math
from datetime import datetime

from requests.adapters import PoolManager, Retry

from .client import Client

ROC_SEND_PUNCH = "https://roc.olresultat.se/ver7.1/sendpunches_v2.php"
ROC_RECEIVEDATA = "https://roc.olresultat.se/ver7.1/receivedata.php"


class RocClient(Client):
    """Class for sending punches to ROC"""

    def __init__(self, macaddr: str):
        self.macaddr = macaddr
        retries = Retry(backoff_factor=1.0)
        self.http = PoolManager(retries=retries)

    def send_punch(
        self,
        card_number: int,
        sitime: datetime,
        now: datetime,
        code: int,
        mode: int,
    ):
        def length(x: int):
            return int(math.log10(x)) + 1

        data = {
            "control1": str(code),
            "sinumber1": str(card_number),
            "stationmode1": str(mode),
            "date1": sitime.strftime("%Y-%m-%d"),
            "sitime1": sitime.strftime("%H:%M:%S"),
            "ms1": sitime.strftime("%f")[:3],
            "roctime1": str(now)[:19],
            "macaddr": self.macaddr,
            "1": "f",
            "length": str(118 + sum(map(length, [code, card_number, mode]))),
        }

        try:
            response = self.http.request(
                "POST",
                ROC_SEND_PUNCH,
                encode_multipart=False,
                fields=data,
            )
            logging.debug(f"Got response {response.status}: {response.data}")
        except Exception as e:
            logging.error(e)

    def minicallhome(
        self,
    ):
        data = {
            "function": "callhome",
            "command": "setmini",
            "macaddr": self.macaddr,
            "failedcallhomes": "0",
            "localipaddress": "192.168.1.14",
            "codes": "",
            "totaldatatx": "0",
            "totaldatarx": "0",
            "signaldBm": "0",
            "temperature": "45",
            "networktype": "101",  # LTE is 101, figure out the rest
            "volts": "1.2",
            "minFreq": "600",
            "maxFreq": "1200",
            "freq": "800",
        }

        try:
            response = self.http.request(
                "GET",
                ROC_RECEIVEDATA,
                fields=data,
            )
            logging.debug(f"Got response {response.status}: {response.data.decode('utf-8')}")
        except Exception as e:
            logging.error(e)
