import asyncio
import logging
import math
from datetime import datetime

import aiohttp
from aiohttp_retry import ExponentialRetry, RetryClient

from yaroc.rs import SiPunch

from ..pb.status_pb2 import MiniCallHome
from ..utils.modem_manager import NetworkType
from .client import Client

ROC_SEND_PUNCH = "https://roc.olresultat.se/ver7.1/sendpunches_v2.php"
ROC_RECEIVEDATA = "https://roc.olresultat.se/ver7.1/receivedata.php"


class RocClient(Client):
    """Class for sending punches to ROC"""

    def __init__(self, macaddr: str):
        self.macaddr = macaddr

    async def loop(self):
        session = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=20))
        retry_options = ExponentialRetry(attempts=5, start_timeout=3)
        self.client = RetryClient(
            client_session=session, raise_for_status=True, retry_options=retry_options
        )
        async with self.client:
            await asyncio.sleep(1000000)

    async def send_punch(
        self,
        punch: SiPunch,
    ) -> bool:
        def length(x: int):
            if x == 0:
                return 1
            if x < 0:
                return length(-x) + 1

            return int(math.log10(x)) + 1

        now = datetime.now()
        data = {
            "control1": str(punch.code),
            "sinumber1": str(punch.card),
            "stationmode1": str(punch.mode),
            "date1": punch.time.strftime("%Y-%m-%d"),
            "sitime1": punch.time.strftime("%H:%M:%S"),
            "ms1": punch.time.strftime("%f")[:3],
            "roctime1": str(now)[:19],
            "macaddr": self.macaddr,
            "1": "f",
            "length": str(118 + sum(map(length, [punch.code, punch.card, punch.mode]))),
        }

        try:
            async with self.client.post(ROC_SEND_PUNCH, data=data) as response:
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
        if mch.network_type == NetworkType.Lte:
            network_type = "101"
        elif mch.network_type == NetworkType.Umts:
            network_type = "41"
        else:
            network_type = "0"
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
            "networktype": network_type,
            "volts": str(mch.volts),
            "freq": str(mch.freq),
            "minFreq": str(mch.min_freq),
            "maxFreq": str(mch.max_freq),
        }
        try:
            async with self.client.get(ROC_RECEIVEDATA, params=params) as response:
                if response.status < 300:
                    logging.info("MiniCallHome sent to ROC")
                    return True
                else:
                    logging.error("ROC error {}: {}", response.status, await response.text())
                    return False
        except Exception as e:
            logging.error(f"ROC error: {e}")
            return False
