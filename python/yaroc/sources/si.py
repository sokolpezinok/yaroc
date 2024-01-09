import asyncio
import logging
import socket
import time
from asyncio import Queue
from asyncio.tasks import Task
from dataclasses import dataclass
from datetime import datetime
from threading import Event
from typing import AsyncIterator, Dict

import pyudev
import serial
from pyudev import Device
from serial_asyncio import open_serial_connection

from ..rs import SiPunch

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18


@dataclass
class DeviceEvent:
    added: bool
    device: str


class SiWorker:
    def __init__(self):
        self._codes: set[int] = set()

    async def process_punch(self, punch: SiPunch, queue: Queue):
        now = datetime.now().astimezone()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time:%H:%M:%S.%f}, received after "
            f"{(now-punch.time).total_seconds():3.2f}s"
        )
        await queue.put(punch)
        self._codes.add(punch.code)

    def __str__(self):
        codes_str = ",".join(map(str, self._codes)) if len(self._codes) >= 1 else "0"
        return f"{codes_str}-{self.name}"


class SerialSiWorker(SiWorker):
    """Serial port worker"""

    def __init__(self, port: str, mac_addr: str):
        super().__init__()
        self.port = port
        self.name = "srr"
        self.mac_addr = mac_addr
        self._finished = Event()

    async def loop(self, queue: Queue[SiPunch]):
        try:
            async with asyncio.timeout(10):
                reader, writer = await open_serial_connection(
                    url=self.port, baudrate=38400, rtscts=False
                )
            logging.info(f"Connected to SRR source at {self.port}")
        except Exception as err:
            logging.error(f"Error connecting to {self.port}: {err}")

        while not self._finished.is_set():
            try:
                data = await reader.read(20)
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                punch = SiPunch.from_raw(data, self.mac_addr)
                await self.process_punch(punch, queue)

            except serial.serialutil.SerialException as err:
                logging.error(f"Fatal serial exception: {err}")
                return
            except Exception as err:
                logging.error(f"Loop crashing: {err}")
                await asyncio.sleep(5.0)
                return

    def close(self):
        self._finished.set()


class BtSerialSiWorker(SiWorker):
    """Bluetooth serial worker"""

    def __init__(self, mac_addr: str):
        super().__init__()
        self.name = "lora"
        self.mac_addr = mac_addr
        logging.info(f"Starting a bluetooth serial worker, connecting to {mac_addr}")

    def __hash__(self):
        return self.mac_addr.__hash__()

    async def loop(self, queue: Queue, _status_queue):
        sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
        sock.setblocking(False)
        loop = asyncio.get_event_loop()
        try:
            await loop.sock_connect(sock, (self.mac_addr, 1))
        except Exception as err:
            logging.error(f"Error connecting to {self.mac_addr}: {err}")
        logging.info(f"Connected to {self.mac_addr}")

        while True:
            try:
                data = await loop.sock_recv(sock, 20)
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                punch = SiPunch.from_raw(data, self.mac_addr)
                await self.process_punch(punch, queue)

            except Exception as err:
                logging.error(f"Loop crashing: {err}")
                return


class UdevSiFactory(SiWorker):
    def __init__(self, mac_addr: str):
        self._udev_workers: Dict[str, tuple[SerialSiWorker, Task]] = {}
        self._device_queue: Queue[tuple[str, Device]] = Queue()
        self.mac_addr = mac_addr

    async def loop(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        self._loop = asyncio.get_event_loop()
        context = pyudev.Context()
        monitor = pyudev.Monitor.from_netlink(context)
        monitor.filter_by("tty")
        observer = pyudev.MonitorObserver(monitor, self._handle_udev_event)
        observer.start()
        logging.info("Starting udev-based SportIdent device manager")

        try:
            for device in context.list_devices():
                self._handle_udev_event("add", device)
        except Exception as e:
            logging.error(e)
        while True:
            action, device = await self._device_queue.get()

            device_node = device.device_node
            if action == "add":
                if device_node in self._udev_workers:
                    return
                logging.info(f"Inserted SportIdent device {device_node}")

                try:
                    worker = SerialSiWorker(device_node, self.mac_addr)
                    task = asyncio.create_task(worker.loop(queue))
                    self._udev_workers[device_node] = (worker, task)
                    await status_queue.put(DeviceEvent(True, device_node))
                except Exception as e:
                    logging.error(e)
            elif action == "remove":
                if device_node in self._udev_workers:
                    logging.info(f"Removed device {device_node}")
                    si_worker, _ = self._udev_workers[device_node]
                    si_worker.close()
                    del self._udev_workers[device_node]
                    await status_queue.put(DeviceEvent(False, device_node))

    @staticmethod
    def _is_silabs(device: Device):
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
        if not self._is_silabs(device) and not self._is_sandberg(device):
            return
        asyncio.run_coroutine_threadsafe(self._device_queue.put((action, device)), self._loop)

    def __str__(self):
        res = []
        for worker, _ in self._udev_workers.values():
            res.append(str(worker))
        return ",".join(res)


class FakeSiWorker(SiWorker):
    """Creates fake SportIdent events, useful for benchmarks and tests."""

    def __init__(self, mac_addr: str, punch_interval_secs: float = 12):
        super().__init__()
        self._punch_interval = punch_interval_secs
        self.mac_addr = mac_addr
        self.name = "fake"
        logging.info(
            "Starting a fake SportIdent worker, sending a punch every "
            f"{self._punch_interval} seconds"
        )

    def __hash__(self):
        return "fake".__hash__()

    async def loop(self, queue: Queue, _status_queue):
        del _status_queue
        for i in range(31, 1000):
            time_start = time.time()
            punch = SiPunch.new(46283, i, datetime.now().astimezone(), 18, self.mac_addr)
            await self.process_punch(punch, queue)
            await asyncio.sleep(self._punch_interval - (time.time() - time_start))


class SiPunchManager:
    """
    Manages devices delivering SportIdent punch data, typically SRR dongles but also punches
    delivered by radio (LoRa) or Bluetooth.

    Also issues an event whenever a devices has been connected or removed.
    """

    def __init__(self, workers: list[SiWorker]) -> None:
        self._si_workers: set[SiWorker] = set(workers)
        self._queue: Queue[SiPunch] = Queue()
        self._status_queue: Queue[DeviceEvent] = Queue()

    def __str__(self) -> str:
        return ",".join(str(worker) for worker in self._si_workers)

    async def loop(self):
        loops = []
        for worker in self._si_workers:
            self._si_workers.add(worker)
            loops.append(worker.loop(self._queue, self._status_queue))
        await asyncio.gather(*loops, return_exceptions=True)

    async def punches(self) -> AsyncIterator[SiPunch]:
        while True:
            yield await self._queue.get()

    async def device_events(self) -> AsyncIterator[DeviceEvent]:
        while True:
            yield await self._status_queue.get()
