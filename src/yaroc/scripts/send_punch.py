import asyncio
import logging
import tomllib

from dependency_injector.wiring import Provide, inject

from ..clients.client import Client
from ..clients.mqtt import SIM7020MqttClient
from ..pb.status_pb2 import MiniCallHome
from ..utils.container import Container, create_clients
from ..utils.si import SiManager
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr


class PunchSender:
    @inject
    def __init__(
        self, clients: list[Client], si_manager: SiManager = Provide[Container.si_manager]
    ):
        if len(clients) == 0:
            logging.warning("No clients enabled, will listen to punches but nothing will be sent")
        self.clients = clients
        self.si_manager = si_manager

    async def periodic_mini_call_home(self):
        while True:
            mini_call_home = create_sys_minicallhome()
            mini_call_home.codes = str(self.si_manager)
            self.send_mini_call_home(mini_call_home)
            await asyncio.sleep(20)

    async def send_punches(self):
        async for si_punch in self.si_manager.punches():
            for client in self.clients:
                # TODO: some of the clients are blocking, they shouldn't do that
                try:
                    client.send_punch(si_punch.card, si_punch.time, si_punch.code, si_punch.mode)
                except Exception as err:
                    logging.error(err)

    async def udev_events(self):
        async for device in self.si_manager.udev_events():
            mch = MiniCallHome()
            mch.time.GetCurrentTime()
            device_name = device.device_node.removeprefix("/dev/").lower()
            if device.action == "add" or device.action is None:
                mch.codes = f"siadded-{device_name}"
            else:
                mch.codes = f"siremoved-{device_name}"
            self.send_mini_call_home(mch)

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

    def loop(self):
        async_loop = asyncio.get_event_loop()
        asyncio.run_coroutine_threadsafe(self.periodic_mini_call_home(), async_loop)
        asyncio.run_coroutine_threadsafe(self.send_punches(), async_loop)
        asyncio.run_coroutine_threadsafe(self.udev_events(), async_loop)
        async_loop.run_forever()


def main():
    mac_addr = eth_mac_addr()
    assert mac_addr is not None

    with open("send-punch.toml", "rb") as f:
        config = tomllib.load(f)
    if "si_punches" not in config:
        config["si_punches"] = "udev"

    container = Container()
    container.config.from_dict(config)
    container.config.mac_addr.from_value(mac_addr)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container", __name__])
    logging.info(f"Starting SendPunch for MAC {mac_addr}")

    clients = create_clients(container.client_factories)
    ps = PunchSender(clients)
    ps.loop()
