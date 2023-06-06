import asyncio
import logging
import tomllib

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
        self.si_manager = UdevSIManager(self.udev_handler)

    async def periodic_mini_call_home(self):
        while True:
            mini_call_home = create_sys_minicallhome()
            mini_call_home.codes = str(self.si_manager)
            self.send_mini_call_home(mini_call_home)
            await asyncio.sleep(20)

    async def send_punches(self):
        async for card_number, code, tim, mode in self.si_manager.punches():
            for client in self.clients:
                # TODO: some of the clients are blocking, they shouldn't do that
                client.send_punch(card_number, tim, code, mode)

    def send_mini_call_home(self, mch: MiniCallHome):
        for client in self.clients:
            handle = client.send_mini_call_home(mch)
            if isinstance(client, SIM7020MqttClient):  # TODO: convert all clients to Future

                def handle_mini_call_home(fut):
                    try:
                        if fut.result():
                            logging.info("MiniCallHome sent")
                        else:
                            logging.error("MiniCallHome not sent")
                    except Exception as err:
                        logging.error(f"MiniCallHome not sent: {err}")

                handle.add_done_callback(handle_mini_call_home)

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
        async_loop = asyncio.get_event_loop()
        asyncio.run_coroutine_threadsafe(self.periodic_mini_call_home(), async_loop)
        asyncio.run_coroutine_threadsafe(self.send_punches(), async_loop)
        async_loop.run_forever()


def main():
    mac_addr = eth_mac_addr()
    assert mac_addr is not None

    with open("send-punch.toml", "rb") as f:
        config = tomllib.load(f)

    container = Container()
    container.config.from_dict(config)
    container.config.mac_addr.from_value(mac_addr)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])
    logging.info(f"Starting SendPunch for MAC {mac_addr}")

    clients = create_clients(mac_addr, container.client_factories)
    ps = PunchSender(clients)
    ps.loop()
