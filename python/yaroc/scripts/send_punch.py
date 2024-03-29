import asyncio
import logging
import socket
import time
import tomllib

from dependency_injector.wiring import Provide, inject

from ..clients.client import ClientGroup
from ..pb.status_pb2 import DeviceEvent, EventType, MiniCallHome, Status
from ..sources.si import SiPunchManager
from ..utils.container import Container, create_clients
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr


class PunchSender:
    @inject
    def __init__(
        self,
        client_group: ClientGroup,
        mac_addr: str,
        mch_interval: int | None = 30,
        si_manager: SiPunchManager = Provide[Container.si_manager],
    ):
        if client_group.len() == 0:
            logging.warning("No clients enabled, will listen to punches but nothing will be sent")
        self.client_group = client_group
        self.si_manager = si_manager
        self.mac_addr = mac_addr
        if mch_interval is None:
            mch_interval = 30
        self._mch_interval = mch_interval

    async def periodic_mini_call_home(self):
        # TODO: get rid of the following sleep
        await asyncio.sleep(5.0)
        while True:
            time_start = time.time()
            mini_call_home = create_sys_minicallhome()
            mini_call_home.codes = str(self.si_manager)
            status = Status()
            status.mini_call_home.CopyFrom(mini_call_home)
            await self.client_group.send_status(status, self.mac_addr)
            await asyncio.sleep(self._mch_interval - (time.time() - time_start))

    async def send_punches(self):
        async for si_punch in self.si_manager.punches():
            asyncio.create_task(self.client_group.send_punch(si_punch))

    async def udev_events(self):
        # TODO: get rid of the following sleep
        await asyncio.sleep(3.0)  # sleep to allow for connecting
        async for dev_event in self.si_manager.device_events():
            mch = MiniCallHome()
            mch.time.GetCurrentTime()
            device_event = DeviceEvent()
            device_event.port = dev_event.device
            device_event.type = EventType.Added if dev_event.added else EventType.Removed
            status = Status()
            status.dev_event.CopyFrom(device_event)
            await self.client_group.send_status(status, self.mac_addr)

    async def loop(self):
        def handle_exception(loop, context):
            msg = context.get("exception", context["message"])
            logging.error(f"Caught exception: {msg}")

        asyncio.get_event_loop().set_exception_handler(handle_exception)

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
    logging.info(f"Starting SendPunch for {config['hostname']}/{config['mac_addr']}")

    client_group = await create_clients(container.client_factories)
    ps = PunchSender(client_group, config["mac_addr"], config.get("call_home_interval", None))
    await ps.loop()


asyncio.run(main())
