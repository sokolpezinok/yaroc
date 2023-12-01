import asyncio
import logging
import time
from abc import ABC, abstractmethod
from concurrent.futures import Future
from datetime import datetime
from threading import Event
from typing import Any, AsyncIterator, Dict

import pyudev
from pyudev import Device
from serial_asyncio import open_serial_connection
from yaroc.rs import SiPunch

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18


class SiWorker:
    def __init__(self):
        self._finished = Event()
        self.codes: set[int] = set()

    async def loop(self, queue: asyncio.Queue, port: str):
        try:
            async with asyncio.timeout(10):
                reader, writer = await open_serial_connection(
                    url=port, baudrate=38400, rtscts=False
                )
        except Exception as err:
            logging.error(f"Error connecting to {port}: {err}")

        while not self._finished.is_set():
            try:
                data = await reader.read(20)  # TODO: readuntil?
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                await self.process_punch(data, queue)

            except Exception as err:
                logging.error(f"Loop crashing: {err}")
                return

    async def process_punch(self, data: bytes, queue: asyncio.Queue):
        punch = SiPunch.from_raw(data)
        now = datetime.now().astimezone()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time}, received after {now-punch.time}"
        )
        await queue.put(punch)
        self.codes.add(punch.code)

    def __str__(self):
        codes_str = ",".join(map(str, self.codes)) if len(self.codes) >= 1 else "0"
        return f"{codes_str}-srr"

    def close(self):
        self._finished.set()


class SiManager(ABC):
    @abstractmethod
    async def punches(self):
        pass

    @abstractmethod
    async def udev_events(self):
        pass


class UdevSiManager(SiManager):
    """
    Dynamically manages connecting and disconnecting SportIdent devices: SI readers or SRR dongles.

    Usage:
    si_manager = UdevSiManager()
    ...
    si_manager.stop()
    """

    def __init__(self) -> None:
        context = pyudev.Context()
        self.monitor = pyudev.Monitor.from_netlink(context)
        self.monitor.filter_by("tty")
        self._si_workers: Dict[str, tuple[SiWorker, Future[Any]]] = {}
        self._queue: asyncio.Queue[SiPunch] = asyncio.Queue()
        self._device_queue: asyncio.Queue[tuple[str, Device]] = asyncio.Queue()
        self._loop = asyncio.get_event_loop()

        for device in context.list_devices():
            self._handle_udev_event("add", device)
        self._observer = pyudev.MonitorObserver(self.monitor, self._handle_udev_event)
        self._observer.start()
        logging.info("Starting udev-based SportIdent device manager")

    def __str__(self) -> str:
        return ",".join(str(worker) for worker in self._si_workers.values())

    async def punches(self) -> AsyncIterator[SiPunch]:
        while True:
            yield await self._queue.get()

    def _handle_device_internal(self, action: str, device: Device):
        device_node = device.device_node
        if action == "add":
            if device_node in self._si_workers:
                return
            logging.info(f"Inserted SportIdent device {device_node}")

            try:
                worker = SiWorker()
                fut = asyncio.run_coroutine_threadsafe(
                    worker.loop(self._queue, device_node), asyncio.get_event_loop()
                )
                self._si_workers[device_node] = (worker, fut)
                logging.info(f"Asynchronously connected to {device_node}")
            except Exception as e:
                logging.error(e)
        elif action == "remove":
            if device_node in self._si_workers:
                logging.info(f"Removed device {device_node}")
                (si_worker, _) = self._si_workers[device_node]
                si_worker.close()
                del self._si_workers[device_node]

    async def udev_events(self) -> AsyncIterator[Device]:
        while True:
            action, device = await self._device_queue.get()
            self._handle_device_internal(action, device)
            yield device

    @staticmethod
    def _is_sl(device: Device):
        try:
            return device.subsystem == "tty" and device.properties["ID_VENDOR_ID"] == "10c4"
        except Exception:
            # pyudev sucks, it throws an exception when you're only doing a lookup
            return False

    @staticmethod
    def _is_sandberg(device: Device):
        try:
            return (
                device.subsystem == "tty"
                and device.properties["ID_VENDOR_ID"] == "1a86"
                and device.properties["ID_MODEL_ID"] == "55d4"
            )
        except Exception:
            # pyudev sucks, it throws an exception when you're only doing a lookup
            return False

    def stop(self):
        self._observer.stop()

    def _handle_udev_event(self, action, device: Device):
        if not self._is_sl(device) and not self._is_sandberg(device):
            return
        asyncio.run_coroutine_threadsafe(self._device_queue.put((action, device)), self._loop)


class FakeSiManager(SiManager):
    """
    Creates fake SportIdent events, useful for benchmarks and tests.

    Usage:
    si_manager = FakeSiManager()
    ...
    si_manager.stop()
    """

    def __init__(self):
        self._punch_interval = 12
        logging.info(
            "Starting a fake SportIdent device manager, sending a punch every "
            f"{self._punch_interval} seconds"
        )

    def __str__(self) -> str:
        return ""

    async def punches(self) -> AsyncIterator[SiPunch]:
        for i in range(30, 1000):
            time_start = time.time()
            yield SiPunch.new(46283, (i + 1) % 1000, datetime.now().astimezone(), 18)
            await asyncio.sleep(self._punch_interval - (time.time() - time_start))

    async def udev_events(self) -> AsyncIterator[Device]:
        await asyncio.sleep(10000000)
        yield None
