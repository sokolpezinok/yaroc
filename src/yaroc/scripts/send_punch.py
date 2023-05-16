import logging
import time
import tomllib
from datetime import datetime
from threading import Event, Thread

from pyudev import Device
from sportident import SIReader

from ..clients.client import Client
from ..clients.meos import MeosClient
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..clients.roc import RocClient
from ..pb.status_pb2 import MiniCallHome
from ..utils.script import setup_logging
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr
from ..utils.udev_si import UdevSIManager

START = 3
FINISH = 4
BEACON_CONTROL = 18


with open("send-punch.toml", "rb") as f:
    config = tomllib.load(f)

setup_logging(config)

mac_addr = eth_mac_addr()
assert mac_addr is not None
logging.info(f"Starting SendPunch for MAC {mac_addr}")

sim7020_conf = config["client"]["sim7020"]
meos_conf = config["client"]["meos"]
mqtt_conf = config["client"]["mqtt"]
roc_conf = config["client"]["roc"]

clients: list[Client] = []
if sim7020_conf.get("enable", True):
    logging.info(f"Enabled SIM7020 MQTT client at {sim7020_conf['device']}")
    clients.append(SIM7020MqttClient(mac_addr, sim7020_conf["device"], "SendPunch"))
if meos_conf.get("enable", True):
    logging.info("Enabled SIRAP client")
    clients.append(MeosClient(meos_conf["ip"], meos_conf["port"]))
if mqtt_conf.get("enable", True):
    logging.info("Enabled MQTT client")
    clients.append(MqttClient(mac_addr))
if roc_conf.get("enable", True):
    logging.info("Enabled ROC client")
    clients.append(RocClient(mac_addr))

if len(clients) == 0:
    logging.warning("No clients enabled, will listen to punches but nothing will be sent")


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
            messages.append((1, card_data["start"], START))
        if isinstance(card_data["finish"], datetime):
            messages.append((2, card_data["finish"], FINISH))

        for code, tim, mode in messages:
            logging.info(f"{card_number} punched {code} at {tim}, received after {now-tim}")
            for client in clients:
                # TODO: some of the clients are blocking, they shouldn't do that
                client.send_punch(card_number, tim, code, mode)


def send_mini_call_home(mch: MiniCallHome):
    for client in clients:
        client.send_mini_call_home(mch)


def udev_handler(device: Device):
    mch = MiniCallHome()
    mch.time.GetCurrentTime()
    device_name = device.device_node.removeprefix("/dev/").lower()
    if device.action == "add" or device.action is None:
        mch.codes = f"siadded-{device_name}"
    else:
        mch.codes = f"siremoved-{device_name}"
    send_mini_call_home(mch)


si_manager = UdevSIManager(si_worker, udev_handler)


def periodic_mini_call_home():
    while True:
        mch = create_sys_minicallhome()
        mch.codes = str(si_manager)
        send_mini_call_home(mch)
        time.sleep(20.0)  # TODO: make the timeout configurable


thread = Thread(target=periodic_mini_call_home)
thread.daemon = True
thread.start()


si_manager.loop()
