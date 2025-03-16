import asyncio
import datetime
import logging
import socket
import time
import tomllib

from dependency_injector.wiring import Provide, inject

from ..clients.client import ClientGroup
from ..pb.status_pb2 import DeviceEvent, EventType, MiniCallHome, Status
from ..rs import HostInfo, SiPunchLog, current_timestamp_millis
from ..sources.si import SiPunchManager
from ..utils.container import Container, create_clients
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr, is_windows


class PunchSender:
    @inject
    def __init__(
        self,
        client_group: ClientGroup,
        host_info: HostInfo,
        mch_interval: int | None = 30,
        si_manager: SiPunchManager = Provide[Container.si_manager],
    ):
        if client_group.len() == 0:
            logging.warning("No clients enabled, will listen to punches but nothing will be sent")
        self.client_group = client_group
        self.si_manager = si_manager
        self.host_info = host_info
        if mch_interval is None:
            mch_interval = 30
        self._mch_interval = mch_interval

    async def periodic_mini_call_home(self):
        # TODO: get rid of the following sleep
        await asyncio.sleep(5.0)
        while True:
            time_start = time.time()
            mini_call_home = create_sys_minicallhome()
            for code in self.si_manager.codes:
                mini_call_home.codes.append(code)
            status = Status()
            status.mini_call_home.CopyFrom(mini_call_home)
            await self.client_group.send_status(status, self.host_info.mac_address)
            await asyncio.sleep(self._mch_interval - (time.time() - time_start))

    async def send_punches(self):
        async for si_punch in self.si_manager.punches():
            asyncio.create_task(
                self.client_group.send_punch(
                    SiPunchLog.new(si_punch, self.host_info, datetime.datetime.now().astimezone())
                )
            )

    async def udev_events(self):
        # TODO: get rid of the following sleep
        await asyncio.sleep(3.0)  # sleep to allow for connecting
        async for dev_event in self.si_manager.device_events():
            mch = MiniCallHome()
            mch.time.millis_epoch = current_timestamp_millis()
            device_event = DeviceEvent()
            device_event.port = dev_event.device
            device_event.type = EventType.Added if dev_event.added else EventType.Removed
            status = Status()
            status.dev_event.CopyFrom(device_event)
            await self.client_group.send_status(status, self.host_info.mac_address)

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

    if "mac_addr" not in config:
        config["mac_addr"] = eth_mac_addr()
    assert config["mac_addr"] is not None
    hostname = socket.gethostname()
    config["hostname"] = hostname

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container", __name__])
    logging.info(f"Starting SendPunch for {hostname}/{config['mac_addr']}")

    client_group = await create_clients(container.client_factories)
    ps = PunchSender(
        client_group,
        HostInfo.new(hostname, config["mac_addr"]),
        config.get("call_home_interval", None),
    )
    await ps.loop()


if is_windows():
    from asyncio import WindowsSelectorEventLoopPolicy, set_event_loop_policy

    set_event_loop_policy(WindowsSelectorEventLoopPolicy())
asyncio.run(main())
