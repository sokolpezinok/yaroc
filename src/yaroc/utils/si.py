import asyncio
import logging
import time
from abc import ABC, abstractmethod
from dataclasses import dataclass
from datetime import datetime
from threading import Event, Lock, Thread
from typing import AsyncIterator, Dict

import pyudev
from pyudev import Device

from sportident import SIReader, SIReaderControl, SIReaderReadout, SIReaderSRR

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18


@dataclass
class SiPunch:
    card: int
    code: int
    time: datetime
    mode: int


class SiWorker:
    def __init__(self, si: SIReader, queue: asyncio.Queue, loop: asyncio.AbstractEventLoop):
        self.si = si
        self.finished = Event()
        self._queue = queue
        self._loop = loop
        self.thread = Thread(target=self._worker_fn, daemon=True)
        self.thread.start()
        self.codes: set[int] = set()

    def _worker_fn(self):
        while True:
            if self.finished.is_set():
                return

            if self.si.poll_sicard():
                card_data = self.si.read_sicard()
            else:
                time.sleep(1.0)
                continue

            now = datetime.now()
            card_number = card_data["card_number"]
            series = card_number // 2**16
            if series >= 1 and series <= 4:
                card_number += series * 34464

            messages = []
            for punch in card_data["punches"]:
                (code, tim) = punch
                messages.append((code, tim, BEACON_CONTROL))
            if isinstance(card_data["start"], datetime):
                messages.append((1, card_data["start"], START_MODE))
            if isinstance(card_data["finish"], datetime):
                messages.append((2, card_data["finish"], FINISH_MODE))

            for code, tim, mode in messages:
                logging.info(f"{card_number} punched {code} at {tim}, received after {now-tim}")
                asyncio.run_coroutine_threadsafe(
                    self._queue.put(SiPunch(card_number, code, tim, mode)), self._loop
                )
                self.codes.add(code)

    def __str__(self):
        codes_str = ",".join(map(str, self.codes)) if len(self.codes) >= 1 else "0"
        if isinstance(self.si, SIReaderSRR):
            return f"{codes_str}-srr"
        if isinstance(self.si, SIReaderControl):
            return f"{codes_str}-control"

    def close(self, timeout: float = DEFAULT_TIMEOUT_MS):
        self.finished.set()
        self.si.disconnect()
        self.thread.join(timeout)


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
        self._si_workers_lock = Lock()
        self._si_workers: Dict[str, SiWorker] = {}
        self._queue: asyncio.Queue[SiPunch] = asyncio.Queue()
        self._device_queue: asyncio.Queue[str] = asyncio.Queue()
        self._loop = asyncio.get_event_loop()

        for device in context.list_devices():
            self._handle_udev_event("add", device)
        self._observer = pyudev.MonitorObserver(self.monitor, self._handle_udev_event)
        self._observer.start()
        logging.info("Starting udev-based SportIdent device manager")

    def __str__(self) -> str:
        with self._si_workers_lock:
            return ",".join(str(worker) for worker in self._si_workers.values())

    async def punches(self) -> AsyncIterator[SiPunch]:
        while True:
            yield await self._queue.get()

    async def udev_events(self) -> AsyncIterator[Device]:
        while True:
            yield await self._device_queue.get()

    def _is_sportident(self, device: Device):
        try:
            is_sportident = (
                device.subsystem == "tty"
                and device.properties["ID_VENDOR_ID"] == "10c4"
                and device.properties["ID_MODEL_ID"] == "800a"
            )
            return is_sportident
        except Exception:
            # pyudev sucks, it throws an exception when you're only doing a lookup
            return False

    def stop(self):
        self._observer.stop()

    def _handle_udev_event(self, action, device: Device):
        if not self._is_sportident(device):
            return
        device_node = device.device_node
        asyncio.run_coroutine_threadsafe(self._device_queue.put(device), self._loop)
        if action == "add":
            with self._si_workers_lock:
                if device_node in self._si_workers:
                    return
            logging.info(f"Inserted SportIdent device {device_node}")
            try:
                si = SIReaderReadout(device_node)
                is_control = False
                if si.get_type() == SIReader.M_SRR:
                    is_control = True
                    si.disconnect()
                    si = SIReaderSRR(device_node)
                elif si.get_type() == SIReader.M_CONTROL or si.get_type() == SIReader.M_BC_CONTROL:
                    is_control = True
                    si.disconnect()
                    si = SIReaderControl(device_node)

                if is_control:
                    with self._si_workers_lock:
                        self._si_workers[device_node] = SiWorker(si, self._queue, self._loop)
                        logging.info(f"Connected to {si.port}")
                else:
                    logging.warn(f"Station {si.port} not an SRR dongle or not set in autosend mode")

            except Exception as err:
                logging.error(f"Failed to connect to an SI station at {device_node}: {err}")
        elif device.action == "remove":
            with self._si_workers_lock:
                if device_node in self._si_workers:
                    logging.info(f"Removed device {device_node}")
                    si_worker = self._si_workers[device_node]
                    si_worker.close()
                    del self._si_workers[device_node]


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

    async def punches(self) -> AsyncIterator[SiPunch]:
        for i in range(1000):
            time_start = time.time()
            yield SiPunch(46283, (i + 1) % 1000, datetime.now(), 18)
            await asyncio.sleep(self._punch_interval - (time.time() - time_start))

    async def udev_events(self) -> AsyncIterator[Device]:
        await asyncio.sleep(10000000)
        yield None
