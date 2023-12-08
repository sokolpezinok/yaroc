import asyncio
import logging
import socket
import time
import tomllib

from dependency_injector.wiring import Provide, inject

from ..clients.client import ClientGroup
from ..pb.status_pb2 import MiniCallHome
from ..sources.si import SiPunchManager
from ..utils.container import Container, create_clients
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr


class PunchSender:
    @inject
    def __init__(
        self,
        client_group: ClientGroup,
        mac_addr: str,
        si_manager: SiPunchManager = Provide[Container.si_manager],
    ):
        if client_group.len() == 0:
            logging.warning("No clients enabled, will listen to punches but nothing will be sent")
        self.client_group = client_group
        self.si_manager = si_manager
        self.mac_addr = mac_addr
        self._mch_interval = 20

    async def periodic_mini_call_home(self):
        # TODO: get rid of the following sleep
        await asyncio.sleep(5.0)
        while True:
            time_start = time.time()
            mini_call_home = create_sys_minicallhome(self.mac_addr)
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
        # TODO: get rid of the following sleep
        await asyncio.sleep(3.0)  # sleep to allow for connecting
        async for dev_event in self.si_manager.device_events():
            mch = MiniCallHome()
            mch.time.GetCurrentTime()
            mch.mac_address = self.mac_addr
            device_name = dev_event.device.removeprefix("/dev/").lower()
            if dev_event.added:
                mch.codes = f"siadded-{device_name}"
            else:
                mch.codes = f"siremoved-{device_name}"
            await self.client_group.send_mini_call_home(mch)

    async def loop(self):
        try:
            await asyncio.gather(
                self.si_manager.loop(),
                self.periodic_mini_call_home(),
                self.send_punches(),
                self.udev_events(),
                self.client_group.loop(),
            )
        except asyncio.exceptions.CancelledError:
            logging.error("Interrupted, exiting")
            import sys

            sys.exit(0)


async def main():
    with open("send-punch.toml", "rb") as f:
        config = tomllib.load(f)
    if "si_punches" not in config:
        config["si_punches"] = "udev"

    if "mac_addr" not in config:
        config["mac_addr"] = eth_mac_addr()
    assert config["mac_addr"] is not None
    config["hostname"] = socket.gethostname()

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container", __name__])
    logging.info(f"Starting SendPunch for MAC {config['hostname']}/{config['mac_addr']}")

    client_group = await create_clients(container.client_factories)
    ps = PunchSender(client_group, config["mac_addr"])
    await ps.loop()


asyncio.run(main())
