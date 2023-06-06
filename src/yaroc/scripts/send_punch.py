import logging
import time
import tomllib
from threading import Thread

from pyudev import Device
from sportident import SIReader

from ..clients.client import Client
from ..clients.mqtt import SIM7020MqttClient
from ..pb.status_pb2 import MiniCallHome
from ..utils.container import Container, create_clients
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr
from ..utils.udev_si import UdevSIManager


class PunchSender:
    def __init__(self, clients: list[Client]):
        if len(clients) == 0:
            logging.warning("No clients enabled, will listen to punches but nothing will be sent")
        self.clients = clients
        self.si_manager = UdevSIManager(self.udev_handler, clients)

        thread = Thread(target=self.periodic_mini_call_home, daemon=True)
        thread.start()

    @staticmethod
    def handle_mini_call_home(fut):
        try:
            if fut.result():
                logging.info("MiniCallHome sent")
            else:
                logging.error("MiniCallHome not sent")
        except Exception as err:
            logging.error(f"MiniCallHome not sent: {err}")

    def send_mini_call_home(self, mch: MiniCallHome):
        for client in self.clients:
            handle = client.send_mini_call_home(mch)
            if isinstance(client, SIM7020MqttClient):
                # TODO: convert all clients to Future
                handle.add_done_callback(PunchSender.handle_mini_call_home)

    def periodic_mini_call_home(self):
        while True:
            mch = create_sys_minicallhome()
            mch.codes = str(self.si_manager)
            self.send_mini_call_home(mch)
            time.sleep(20.0)  # TODO: make the timeout configurable

    def udev_handler(self, device: Device):
        mch = MiniCallHome()
        mch.time.GetCurrentTime()
        device_name = device.device_node.removeprefix("/dev/").lower()
        if device.action == "add" or device.action is None:
            mch.codes = f"siadded-{device_name}"
        else:
            mch.codes = f"siremoved-{device_name}"
        self.send_mini_call_home(mch)

    def loop(self):
        self.si_manager.loop()


def main():
    mac_addr = eth_mac_addr()
    assert mac_addr is not None

    with open("send-punch.toml", "rb") as f:
        config = tomllib.load(f)

    container = Container()
    container.config.from_dict(config)
    container.config.mac_addr.from_value(mac_addr)
    container.init_resources()
    container.wire(modules=[__name__])
    logging.info(f"Starting SendPunch for MAC {mac_addr}")

    clients = create_clients(config["client"], mac_addr, container.client_factories, container.loop)
    ps = PunchSender(clients)
    ps.loop()
