import asyncio
import logging
import socket
import time
from asyncio import Queue
from asyncio.tasks import Task
from dataclasses import dataclass
from datetime import datetime
from threading import Event
from typing import Any, AsyncIterator, Dict

import serial
from serial_asyncio import open_serial_connection
from usbmonitor import USBMonitor
from usbmonitor.attributes import DEVNAME, ID_MODEL_ID, ID_VENDOR_ID

from ..rs import SiPunch
from ..utils.sys_info import tty_device_from_usb

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

    async def process_punch(self, punch: SiPunch, queue: Queue[SiPunch]):
        now = datetime.now().astimezone()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time:%H:%M:%S.%f}, received after "
            f"{(now - punch.time).total_seconds():3.2f}s"
        )
        await queue.put(punch)
        self._codes.add(punch.code)

    @property
    def codes(self) -> set[int]:
        return self._codes


class SerialSiWorker(SiWorker):
    """Serial port worker"""

    def __init__(self, port: str):
        super().__init__()
        self.port = port
        self.name = "srr"
        self._finished = Event()

    async def loop(self, queue: Queue[SiPunch]):
        successful = False
        for i in range(3):
            try:
                async with asyncio.timeout(10):
                    reader, writer = await open_serial_connection(
                        url=self.port, baudrate=38400, rtscts=False
                    )
                logging.info(f"Connected to SRR source at {self.port}")
                successful = True
                break
            except Exception as err:
                logging.error(f"Error connecting to {self.port}: {err}")
                await asyncio.sleep(5.0)
        if not successful:
            return

        while not self._finished.is_set():
            try:
                data = await reader.read(20)
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                now = datetime.now().astimezone()
                punch = SiPunch.from_raw(data, now)
                if punch is not None:
                    await self.process_punch(punch, queue)

            except serial.serialutil.SerialException as err:
                logging.error(f"Fatal serial exception: {err}")
                return
            except Exception as err:
                logging.error(f"Serial worker loop error: {err}")
                await asyncio.sleep(5.0)

    def close(self):
        self._finished.set()


class BtSerialSiWorker(SiWorker):
    """Bluetooth serial worker"""

    def __init__(self, mac_addr: str):
        super().__init__()
        self.name = "lora"
        self.mac_address = mac_addr
        logging.info(f"Starting a bluetooth serial worker, connecting to {mac_addr}")

    def __hash__(self):
        return self.mac_address.__hash__()

    async def loop(self, queue: Queue, _status_queue):
        sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
        sock.setblocking(False)
        loop = asyncio.get_event_loop()
        try:
            await loop.sock_connect(sock, (self.mac_address, 1))
        except Exception as err:
            logging.error(f"Error connecting to {self.mac_address}: {err}")
        logging.info(f"Connected to {self.mac_address}")

        while True:
            try:
                data = await loop.sock_recv(sock, 20)
                now = datetime.now().astimezone()
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                punch = SiPunch.from_raw(data, now)
                if punch is not None:
                    await self.process_punch(punch, queue)

            except Exception as err:
                logging.error(f"Loop crashing: {err}")
                return


class UdevSiFactory(SiWorker):
    def __init__(self):
        self._udev_workers: Dict[str, tuple[SerialSiWorker, Task, str]] = {}
        self._device_queue: Queue[tuple[str, dict[str, Any]]] = Queue()

    async def loop(self, queue: Queue[SiPunch], status_queue: Queue[DeviceEvent]):
        self._loop = asyncio.get_event_loop()
        logging.info("Starting USB SportIdent device manager")
        self.monitor = USBMonitor(({ID_VENDOR_ID: "10c4"}, {ID_VENDOR_ID: "1a86"}))
        self.monitor.start_monitoring(
            on_connect=self._add_usb_device, on_disconnect=self._remove_usb_device
        )

        for device_id, parent_device_info in self.monitor.get_available_devices().items():
            self._add_usb_device(device_id, parent_device_info)

        while True:
            action, parent_device_info = await self._device_queue.get()
            parent_device_node = parent_device_info[DEVNAME]

            try:
                if action == "add":
                    await asyncio.sleep(3.0)  # Give the TTY subystem more time

                    device_node = tty_device_from_usb(parent_device_node)
                    if device_node is None or device_node in self._udev_workers:
                        continue
                    logging.info(f"Inserted SportIdent device {device_node}")

                    worker = SerialSiWorker(device_node)
                    task = asyncio.create_task(worker.loop(queue))
                    self._udev_workers[parent_device_node] = (worker, task, device_node)
                    await status_queue.put(DeviceEvent(True, device_node))
                elif action == "remove":
                    if parent_device_node in self._udev_workers:
                        si_worker, _, device_node = self._udev_workers[parent_device_node]
                        logging.info(f"Removed device {device_node}")
                        si_worker.close()
                        del self._udev_workers[parent_device_node]
                        await status_queue.put(DeviceEvent(False, device_node))
            except Exception as e:
                logging.error(e)

    @staticmethod
    def _is_silabs(device_info: dict[str, Any]):
        return device_info[ID_VENDOR_ID] == "10c4"

    @staticmethod
    def _is_sandberg(device_info: dict[str, Any]):
        return device_info[ID_VENDOR_ID] == "1a86" and device_info[ID_MODEL_ID] == "55d4"

    def stop(self):
        self._observer.stop()
        self.monitor.stop_monitoring()

    def _add_usb_device(self, _device_id: str, device_info: dict[str, Any]):
        try:
            if not self._is_silabs(device_info) and not self._is_sandberg(device_info):
                return
            asyncio.run_coroutine_threadsafe(
                self._device_queue.put(("add", device_info)), self._loop
            )
        except Exception as err:
            logging.error(err)

    def _remove_usb_device(self, _device_id: str, device_info: dict[str, Any]):
        asyncio.run_coroutine_threadsafe(
            self._device_queue.put(("remove", device_info)), self._loop
        )

    @property
    def codes(self) -> set[int]:
        worker_codes = [worker.codes for worker, _, _ in self._udev_workers.values()]
        return set().union(*worker_codes)


class FakeSiWorker(SiWorker):
    """Creates fake SportIdent events, useful for benchmarks and tests."""

    def __init__(self, punch_interval_secs: float | None = None):
        super().__init__()
        self.name = "fake"
        self._punch_interval = punch_interval_secs if punch_interval_secs is not None else 12.0
        logging.info(
            "Starting a fake SportIdent worker, sending a punch every "
            f"{self._punch_interval} seconds"
        )

    def __hash__(self):
        return "fake".__hash__()

    async def loop(self, queue: Queue, _status_queue):
        del _status_queue
        while True:
            time_start = time.time()
            now = datetime.now().astimezone()
            punch = SiPunch.new(46283, 47, now, 18)
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

    async def loop(self):
        loops = []
        for worker in self._si_workers:
            self._si_workers.add(worker)
            loops.append(worker.loop(self._queue, self._status_queue))
        await asyncio.sleep(3)  # Allow some time for an MQTT connection
        await asyncio.gather(*loops, return_exceptions=True)

    async def punches(self) -> AsyncIterator[SiPunch]:
        while True:
            yield await self._queue.get()

    async def device_events(self) -> AsyncIterator[DeviceEvent]:
        while True:
            yield await self._status_queue.get()

    @property
    def codes(self) -> set[int]:
        worker_codes = [worker.codes for worker in self._si_workers]
        return set().union(*worker_codes)
