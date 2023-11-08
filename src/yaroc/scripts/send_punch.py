import asyncio
import logging
import time
import tomllib

from dependency_injector.wiring import Provide, inject

from ..clients.client import Client, ClientGroup
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
        self.client_group = ClientGroup(clients)
        self.si_manager = si_manager
        self._mch_interval = 20

    async def periodic_mini_call_home(self):
        await asyncio.sleep(5.0)
        while True:
            time_start = time.time()
            mini_call_home = create_sys_minicallhome()
            mini_call_home.codes = str(self.si_manager)
            await self.client_group.send_mini_call_home(mini_call_home)
            await asyncio.sleep(self._mch_interval - (time.time() - time_start))

    async def send_punches(self):
        async for si_punch in self.si_manager.punches():
            asyncio.run_coroutine_threadsafe(
                self.client_group.send_punch(si_punch),
                asyncio.get_event_loop(),
            )

    async def udev_events(self):
        async for device in self.si_manager.udev_events():
            mch = MiniCallHome()
            mch.time.GetCurrentTime()
            device_name = device.device_node.removeprefix("/dev/").lower()
            if device.action == "add" or device.action is None:
                mch.codes = f"siadded-{device_name}"
            else:
                mch.codes = f"siremoved-{device_name}"
            await self.client_group.send_mini_call_home(mch)

    async def loop(self):
        await asyncio.gather(
            self.periodic_mini_call_home(),
            self.send_punches(),
            self.udev_events(),
            self.client_group.loop(),
        )


async def main():
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

    clients = [
        await c if isinstance(c, asyncio.Future) else c
        for c in create_clients(container.client_factories)
    ]
    ps = PunchSender(clients)
    await ps.loop()


asyncio.run(main())
