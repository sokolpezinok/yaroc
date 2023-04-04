import logging
import sys
import time
from datetime import datetime
from threading import Event

from sportident import SIReader

from ..clients.meos import MeosClient
from ..clients.mqtt import SimpleMqttClient
from ..utils.udev_si import UdevSIManager

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

START = 3
FINISH = 4
BEACON_CONTROL = 18
TOPIC = "spe/47"


# TODO: allow multiple clients
client = "mqtt"
if client == "meos":
    client = MeosClient("192.168.88.165", 10000)
elif client == "mqtt":
    client = SimpleMqttClient(TOPIC, "SendPunch")
else:
    logging.error("")
    sys.exit(1)


def si_worker(si: SIReader, evnt: Event):
    while True:
        if evnt.is_set():
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
            client.send_punch(card_number, tim, now, code, mode)


si_manager = UdevSIManager(si_worker)
si_manager.loop()
