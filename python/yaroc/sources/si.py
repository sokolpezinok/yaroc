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
from pyudev import Context, Device
from serial_asyncio import open_serial_connection
from usbmonitor import USBMonitor
from usbmonitor.attributes import DEVNAME, ID_MODEL_ID, ID_VENDOR_ID

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
        logging.info("Starting USB SportIdent device manager")
        self.monitor = USBMonitor(({ID_VENDOR_ID: "10c4"}, {ID_VENDOR_ID: "1a86"}))
        self.monitor.start_monitoring(
            on_connect=self._add_usb_device, on_disconnect=self._remove_usb_device
        )

        for device_id, parent_device_info in self.monitor.get_available_devices().items():
            self._add_usb_device(device_id, parent_device_info)
        context = Context()

        while True:
            action, parent_device_info = await self._device_queue.get()
            parent_device_node = parent_device_info[DEVNAME]

            if action == "add":
                await asyncio.sleep(3.0)  # Give the TTY subystem more time
                parent_device = Device.from_device_file(context, parent_device_node)
                lst = list(context.list_devices(subsystem="tty").match_parent(parent_device))
                if len(lst) == 0:
                    continue
                device_node = lst[0].device_node
                if device_node in self._udev_workers:
                    return
                logging.info(f"Inserted SportIdent device {device_node}")

                try:
                    worker = SerialSiWorker(device_node, self.mac_addr)
                    task = asyncio.create_task(worker.loop(queue))
                    self._udev_workers[parent_device_node] = (worker, task, device_node)
                    await status_queue.put(DeviceEvent(True, device_node))
                except Exception as e:
                    logging.error(e)
            elif action == "remove":
                if parent_device_node in self._udev_workers:
                    si_worker, _, device_node = self._udev_workers[parent_device_node]
                    logging.info(f"Removed device {device_node}")
                    si_worker.close()
                    del self._udev_workers[parent_device_node]
                    await status_queue.put(DeviceEvent(False, device_node))

    @staticmethod
    def _is_silabs(device_info: dict[str, Any]):
        return device_info[ID_VENDOR_ID] == "10c4"

    @staticmethod
    def _is_sandberg(device_info: dict[str, Any]):
        return device_info[ID_VENDOR_ID] == "1a86" and device_info[ID_MODEL_ID] == "55d4"

    def stop(self):
        self._observer.stop()
        self.monitor.stop_monitoring()

    def _add_usb_device(self, device_id: str, device_info: dict[str, Any]):
        try:
            if not self._is_silabs(device_info) and not self._is_sandberg(device_info):
                return
            asyncio.run_coroutine_threadsafe(
                self._device_queue.put(("add", device_info)), self._loop
            )
        except Exception as err:
            logging.error(err)

    def _remove_usb_device(self, device_id, device_info: dict[str, Any]):
        asyncio.run_coroutine_threadsafe(
            self._device_queue.put(("remove", device_info)), self._loop
        )

    def __str__(self):
        res = []
        for worker, _, _ in self._udev_workers.values():
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
