import asyncio
import logging
import math
from datetime import datetime

import aiohttp

from ..pb.status_pb2 import MiniCallHome
from .client import Client

ROC_SEND_PUNCH = "https://roc.olresultat.se/ver7.1/sendpunches_v2.php"
ROC_RECEIVEDATA = "https://roc.olresultat.se/ver7.1/receivedata.php"


class RocClient(Client):
    """Class for sending punches to ROC"""

    def __init__(self, macaddr: str):
        self.macaddr = macaddr

    async def loop(self):
        timeout = aiohttp.ClientTimeout(total=20)
        self.session = aiohttp.ClientSession(timeout=timeout)
        async with self.session:
            await asyncio.sleep(1000000)

    async def send_punch(
        self,
        card_number: int,
        sitime: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> bool:
        def length(x: int):
            return int(math.log10(x)) + 1

        if process_time is None:
            process_time = datetime.now()
        data = {
            "control1": str(code),
            "sinumber1": str(card_number),
            "stationmode1": str(mode),
            "date1": sitime.strftime("%Y-%m-%d"),
            "sitime1": sitime.strftime("%H:%M:%S"),
            "ms1": sitime.strftime("%f")[:3],
            "roctime1": str(process_time)[:19],
            "macaddr": self.macaddr,
            "1": "f",
            "length": str(118 + sum(map(length, [code, card_number, mode]))),
        }

        try:
            async with self.session.post(
                ROC_SEND_PUNCH, encode_multipart=False, data=data
            ) as response:
                if response.status < 300:
                    logging.info("Punch sent to ROC")
                    return True
                else:
                    logging.error("ROC error {}: {}", response.status, await response.text())
                    return False
        except Exception as e:
            logging.error(f"ROC error: {e}")
            return False

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        params = {
            "function": "callhome",
            "command": "setmini",
            "macaddr": self.macaddr,
            "failedcallhomes": "0",
            "localipaddress": mch.local_ip,
            "codes": mch.codes,
            "totaldatatx": str(mch.totaldatarx),
            "totaldatarx": str(mch.totaldatatx),
            "signaldBm": str(-mch.signal_dbm),
            "temperature": str(mch.cpu_temperature),
            "networktype": str(mch.network_type),
            "volts": str(mch.volts),
            "freq": str(mch.freq),
            "minFreq": str(mch.min_freq),
            "maxFreq": str(mch.max_freq),
        }
        try:
            async with self.session.get(ROC_RECEIVEDATA, params=params) as response:
                if response.status < 300:
                    logging.info("MiniCallHome sent to ROC")
                    return True
                else:
                    logging.error("ROC error {}: {}", response.status, await response.text())
                    return False
        except Exception as e:
            logging.error(f"ROC error: {e}")
            return False
