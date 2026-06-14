import asyncio
import datetime
import logging
import signal
import time
from concurrent.futures import ThreadPoolExecutor

from ..clients.client import ClientGroup
from ..pb.status_pb2 import DeviceEvent, EventType, Status
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
from .sys_info import create_sys_minicallhome, is_windows


class Forwarder:
    def __init__(
        self,
        host_info: HostInfo,
        client_group: ClientGroup,
        builder: MessageHandlerBuilder,
        drawer: StatusDrawer = StatusDrawer(None),
        mch_interval: int | float | None = None,
    ):
        self.host_info = host_info
        self.client_group = client_group
        self.executor = ThreadPoolExecutor(max_workers=1)
        self.drawer = drawer
        self.handler = builder.build()
        self._codes: set[int] = set()
        self._tasks: set[asyncio.Task] = set()
        self._mch_interval = mch_interval

    @property
    def codes(self) -> set[int]:
        return self._codes

    async def _handle_punches(self, punches: list[SiPunchLog]):
        tasks = []
        for punch_log in punches:
            logging.info(punch_log)
            self._codes.add(punch_log.punch.code)
            tasks.append(self.client_group.send_punch(punch_log))
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_punch(self, punch: SiPunch):
        now = datetime.datetime.now().astimezone()
        latency = (now - punch.time).total_seconds()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time:%H:%M:%S.%f}, latency "
            f"{latency:3.2f}s"
        )
        self._codes.add(punch.code)
        await self.client_group.send_punch(SiPunchLog.new(punch, self.host_info, now))

    async def _handle_cellular_log(self, log: CellularLog):
        logging.info(log)
        proto_bytes = log.to_proto()
        if proto_bytes is not None:
            try:
                status = Status.FromString(proto_bytes)
                await self.client_group.send_status(status, log.mac_address())
            except Exception as err:
                logging.error(f"Failed to forward status: {err}")

    async def _handle_device_event(self, added: bool, device: str):
        device_event = DeviceEvent()
        device_event.port = device
        device_event.type = EventType.Added if added else EventType.Removed  # type: ignore
        status = Status()
        status.dev_event.CopyFrom(device_event)
        await self.client_group.send_status(status, self.host_info.mac_address)

    async def periodic_mini_call_home(self):
        if self._mch_interval is not None:
            await asyncio.sleep(self._mch_interval)
            while True:
                time_start = time.time()
                mini_call_home = create_sys_minicallhome()
                for code in self.codes:
                    mini_call_home.codes.append(code)
                status = Status()
                status.mini_call_home.CopyFrom(mini_call_home)
                await self.client_group.send_status(status, self.host_info.mac_address)
                await asyncio.sleep(max(0.0, self._mch_interval - (time.time() - time_start)))

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
                # TODO: branch on added being true and false
                logging.info(f"Device event: added={added}, device={device}")
                return asyncio.create_task(self._handle_device_event(added, device))
            case Event.NodeInfos(node_infos):
                return asyncio.create_task(self._draw_table(node_infos))
        return None

    async def handle_messages(self):
        while True:
            try:
                ev = await self.handler.next_event()
                task = self.handle_event(ev)
                if task is not None:
                    self._tasks.add(task)
                    task.add_done_callback(self._tasks.discard)
            except Exception as e:
                logging.error(f"Error while getting next message: {e}")

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
            asyncio.create_task(self.periodic_mini_call_home()),
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
