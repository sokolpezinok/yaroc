import asyncio
import logging
import math
from datetime import datetime
from typing import Dict

import aiohttp
from aiohttp_retry import ExponentialRetry, RetryClient

from ..pb.status_pb2 import CellNetworkType, EventType, Status
from ..rs import SiPunchLog
from ..utils.sys_info import FREQ_MULTIPLIER
from .client import Client

ROC_SEND_PUNCH = "https://roc.olresultat.se/ver7.1/sendpunches_v2.php"
ROC_RECEIVEDATA = "https://roc.olresultat.se/ver7.1/receivedata.php"


class RocClient(Client):
    """Class for sending punches to ROC"""

    def __init__(self, mac_override_map: Dict[str, str] = {}):
        self.mac_override_map = mac_override_map

    async def loop(self):
        session = aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=50))
        retry_options = ExponentialRetry(attempts=5, start_timeout=3)
        self.client = RetryClient(
            client_session=session, raise_for_status=True, retry_options=retry_options
        )
        async with self.client:
            await asyncio.sleep(10000000)  # We need to sleep, otherwise the client will be GC-ed

    async def send_punch(
        self,
        punch_log: SiPunchLog,
    ) -> bool:
        def length(x: int):
            if x == 0:
                return 1
            if x < 0:
                return length(-x) + 1

            return int(math.log10(x)) + 1

        punch = punch_log.punch
        now = datetime.now()

        mac_address = punch_log.host_info.mac_address
        mac_address = self.mac_override_map.get(mac_address, mac_address)
        data = {
            "control1": str(punch.code),
            "sinumber1": str(punch.card),
            "stationmode1": str(punch.mode),
            "date1": punch.time.strftime("%Y-%m-%d"),
            "sitime1": punch.time.strftime("%H:%M:%S"),
            "ms1": punch.time.strftime("%f")[:3],
            "roctime1": str(now)[:19],
            "macaddr": mac_address,
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

    async def send_status(self, status: Status, mac_address: str) -> bool:
        mac_address = self.mac_override_map.get(mac_address, mac_address)
        oneof = status.WhichOneof("msg")
        if oneof == "mini_call_home":
            mch = status.mini_call_home
            if mch.network_type == CellNetworkType.Lte:  # type: ignore
                network_type = "101"
            elif mch.network_type == CellNetworkType.Umts:  # type: ignore
                network_type = "41"
            else:
                network_type = "0"
            params = {
                "function": "callhome",
                "command": "setmini",
                "macaddr": mac_address,
                "failedcallhomes": "0",
                "localipaddress": ".".join(map(lambda x: str(int(x)), mch.local_ip.to_bytes(4))),
                "codes": ",".join(str(code) for code in mch.codes),
                "totaldatatx": str(mch.totaldatarx),
                "totaldatarx": str(mch.totaldatatx),
                "signaldBm": str(-mch.signal_dbm),
                "temperature": str(mch.cpu_temperature),
                "networktype": network_type,
                "volts": str(mch.millivolts / 1000.0),
                "freq": str(mch.freq * FREQ_MULTIPLIER),
                "minFreq": str(mch.min_freq * FREQ_MULTIPLIER),
                "maxFreq": str(mch.max_freq * FREQ_MULTIPLIER),
            }
        elif oneof == "dev_event":
            dev_event = status.dev_event
            if dev_event.type == EventType.Added:  # type: ignore
                codes = f"siadded-{dev_event.port}"
            else:
                codes = f"siremoved-{dev_event.port}"
            params = {
                "function": "callhome",
                "command": "setmini",
                "macaddr": mac_address,
                "failedcallhomes": "0",
                "codes": codes,
            }

        try:
            async with self.client.get(ROC_RECEIVEDATA, params=params) as response:
                if response.status < 300:
                    logging.info("MiniCallHome sent to ROC")
                    return True
                else:
                    logging.error(f"ROC error {response.status}: {await response.text()}")
                    return False
        except Exception as e:
            logging.error(f"ROC error: {e}")
            return False
