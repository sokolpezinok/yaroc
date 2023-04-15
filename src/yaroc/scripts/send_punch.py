import logging
import time
from datetime import datetime
from threading import Event, Thread

from sportident import SIReader

from ..clients.client import Client
from ..clients.meos import MeosClient
from ..clients.mqtt import SimpleMqttClient
from ..clients.roc import RocClient
from ..utils.sys_info import create_minicallhome
from ..utils.udev_si import UdevSIManager

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

START = 3
FINISH = 4
BEACON_CONTROL = 18
TOPIC = "yaroc/47"


clients: list[Client] = []
# clients.append(MeosClient("192.168.88.165", 10000))
clients.append(SimpleMqttClient(TOPIC, "SendPunch"))
clients.append(RocClient("b827eb1d3c4f"))


def si_worker(si: SIReader, finished: Event):
    while True:
        if finished.is_set():
            return

        if si.poll_sicard():
            card_data = si.read_sicard()
        else:
            time.sleep(1.0)
            continue

        now = datetime.now()
        card_number = card_data["card_number"]
        messages = []
        for punch in card_data["punches"]:
            (code, tim) = punch
            messages.append((code, tim, BEACON_CONTROL))
        if isinstance(card_data["start"], datetime):
            messages.append((8, card_data["start"], START))
        if isinstance(card_data["finish"], datetime):
            messages.append((10, card_data["finish"], FINISH))

        for code, tim, mode in messages:
            logging.info(f"{card_number} punched {code} at {tim}, received after {now-tim}")
            for client in clients:
                client.send_punch(card_number, tim, now, code, mode)


def periodic_mini_call_home():
    while True:
        mch = create_minicallhome()
        for client in clients:
            client.send_mini_call_home(mch)
        time.sleep(20.0)  # TODO: make the timeout configurable


thread = Thread(target=periodic_mini_call_home)
thread.daemon = True
thread.start()


si_manager = UdevSIManager(si_worker)
si_manager.loop()
