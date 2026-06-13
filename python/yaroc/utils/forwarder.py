import asyncio
import datetime
import logging
import signal
import socket
from concurrent.futures import ThreadPoolExecutor

from ..clients.client import ClientGroup
from ..pb.status_pb2 import Status
from ..rs import (
    CellularLog,
    Event,
    HostInfo,
    MessageHandlerBuilder,
    NodeInfo,
    SiPunch,
    SiPunchLog,
)
from .status import StatusDrawer
from .sys_info import eth_mac_addr, is_windows


class Forwarder:
    def __init__(
        self, client_group: ClientGroup, builder: MessageHandlerBuilder, drawer: StatusDrawer
    ):
        hostname = socket.gethostname()
        mac_addr = eth_mac_addr() or "000000000000"
        self.host_info = HostInfo.new(hostname, mac_addr)
        self.client_group = client_group
        self.executor = ThreadPoolExecutor(max_workers=1)
        self.drawer = drawer
        self.handler, self.usb_serial_manager = builder.build()

    async def _handle_punches(self, punches: list[SiPunchLog]):
        tasks = []
        for punch in punches:
            logging.info(punch)
            tasks.append(self.client_group.send_punch(punch))
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_punch(self, punch: SiPunch):
        logging.info(f"Local punch: {punch.card} punched {punch.code}")
        await self.client_group.send_punch(
            SiPunchLog.new(punch, self.host_info, datetime.datetime.now().astimezone())
        )

    async def _handle_cellular_log(self, log: CellularLog):
        logging.info(log)
        proto_bytes = log.to_proto()
        if proto_bytes is not None:
            try:
                status = Status.FromString(proto_bytes)
                await self.client_group.send_status(status, log.mac_address())
            except Exception as err:
                logging.error(f"Failed to forward status: {err}")

    async def _draw_table(self, node_infos: list[NodeInfo]):
        self.executor.submit(self.drawer.draw_status, node_infos)

    def handle_event(self, ev: Event) -> asyncio.Task | None:
        match ev:
            case Event.SiPunchLogs(logs):
                return asyncio.create_task(self._handle_punches(logs))
            case Event.SiPunch(punch):
                return asyncio.create_task(self._handle_punch(punch))
            case Event.CellularLog(log):
                return asyncio.create_task(self._handle_cellular_log(log))
            case Event.MeshtasticLog(log):
                logging.info(log)
            case Event.DeviceEvnt(added, device):
                logging.info(f"Device event: added={added}, device={device}")
            case Event.NodeInfos(node_infos):
                return asyncio.create_task(self._draw_table(node_infos))
        return None

    async def handle_messages(self):
        while True:
            try:
                ev = await self.handler.next_event()
                self.handle_event(ev)
            except Exception as e:
                logging.error(f"Error while getting next message: {e}")

    def shutdown(self):
        self.executor.shutdown(wait=True)

    async def loop(self):
        def handle_exception(loop, context):
            msg = context.get("exception", context["message"])
            logging.error(f"Caught exception: {msg}")

        def shutdown(signum=None, frame=None):
            if signum is not None:
                signal_name = signal.Signals(signum).name
                logging.info(f"Received signal {signal_name} ({signum}). Initiating shutdown...")
            shutdown_event.set()

        asyncio.get_event_loop().set_exception_handler(handle_exception)

        shutdown_event = asyncio.Event()

        if is_windows():
            signal.signal(signal.SIGTERM, shutdown)
            signal.signal(signal.SIGINT, shutdown)
        else:
            loop = asyncio.get_running_loop()
            for sig in (signal.SIGTERM, signal.SIGINT):
                loop.add_signal_handler(sig, shutdown)

        tasks = [
            asyncio.create_task(self.client_group.loop()),
            asyncio.create_task(self.handle_messages()),
            asyncio.ensure_future(self.usb_serial_manager.loop()),
        ]

        try:
            await shutdown_event.wait()
        except asyncio.exceptions.CancelledError:
            logging.info("Interrupted, exiting ...")

        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)

        self.executor.shutdown(wait=True)
        self.drawer.clear()
        logging.info("Main loop shutting down")
