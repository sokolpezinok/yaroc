import asyncio
import logging
import socket
import time
from datetime import datetime
from threading import Event
from typing import AsyncIterator, Dict

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

    async def process_punch(self, punch: SiPunch, queue: asyncio.Queue):
        now = datetime.now().astimezone()
        logging.info(
            f"{punch.card} punched {punch.code} at {punch.time}, received after {now-punch.time}"
        )
        await queue.put(punch)
        self.codes.add(punch.code)

    def __str__(self):
        codes_str = ",".join(map(str, self.codes)) if len(self.codes) >= 1 else "0"
        return f"{codes_str}-{self.name}"

    def close(self):
        self._finished.set()


class SerialSiWorker(SiWorker):
    """Serial port worker"""

    def __init__(self, port: str):
        super().__init__()
        self.name = "srr"
        self.port = port

    async def loop(self, queue: asyncio.Queue):
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
                punch = SiPunch.from_raw(data)
                await self.process_punch(punch, queue)

            except Exception as err:
                logging.error(f"Loop crashing: {err}")


class BtSerialSiWorker(SiWorker):
    """Bluetooth serial worker"""

    def __init__(self, mac_addr: str):
        super().__init__()
        self.name = "lora"
        self.mac_addr = mac_addr

    async def loop(self, queue: asyncio.Queue):
        sock = socket.socket(socket.AF_BLUETOOTH, socket.SOCK_STREAM, socket.BTPROTO_RFCOMM)
        sock.setblocking(False)
        loop = asyncio.get_event_loop()
        try:
            await loop.sock_connect(sock, (self.mac_addr, 1))
        except Exception as err:
            logging.error(f"Error connecting to {self.mac_addr}: {err}")

        while not self._finished.is_set():
            try:
                data = await loop.sock_recv(sock, 20)
                if len(data) == 0:
                    await asyncio.sleep(1.0)
                    continue
                punch = SiPunch.from_raw(data)
                await self.process_punch(punch, queue)

            except Exception as err:
                logging.error(f"Loop crashing: {err}")
                return


class FakeSiWorker(SiWorker):
    """Creates fake SportIdent events, useful for benchmarks and tests."""

    def __init__(self, punch_interval_secs: int = 12):
        super().__init__()
        self._punch_interval = punch_interval_secs
        self.name = "fake"
        logging.info(
            "Starting a fake SportIdent device manager, sending a punch every "
            f"{self._punch_interval} seconds"
        )

    async def loop(self, queue: asyncio.Queue):
        for i in range(31, 1000):
            time_start = time.time()
            punch = SiPunch.new(46283, i, datetime.now().astimezone(), 18)
            await self.process_punch(punch, queue)
            await asyncio.sleep(self._punch_interval - (time.time() - time_start))


class SiManager:
    """
    Dynamically manages connecting and disconnecting SportIdent devices: SRR dongles or bluetooth
    serial sources.
    """

    def __init__(self, bt_mac_address: str | None = None, add_fake: bool = True) -> None:
        context = pyudev.Context()
        self.monitor = pyudev.Monitor.from_netlink(context)
        self.monitor.filter_by("tty")
        self._si_workers: Dict[str, SiWorker] = {}
        self._queue: asyncio.Queue[SiPunch] = asyncio.Queue()
        self._device_queue: asyncio.Queue[tuple[str, Device]] = asyncio.Queue()
        self._loop = asyncio.get_event_loop()
        self.bt_mac_address = bt_mac_address
        self.add_fake = add_fake

        for device in context.list_devices():
            self._handle_udev_event("add", device)
        self._observer = pyudev.MonitorObserver(self.monitor, self._handle_udev_event)
        self._observer.start()
        logging.info("Starting udev-based SportIdent device manager")

    def __str__(self) -> str:
        return ",".join(str(worker) for worker in self._si_workers.values())

    async def loop(self):
        loops = []
        try:
            if self.bt_mac_address is not None:
                worker = BtSerialSiWorker(self.bt_mac_address)
                self._si_workers[self.bt_mac_address] = worker
                loops.append(worker.loop(self._queue))
        except Exception as err:
            logging.error(f"Bluetooth serial init failed: {err}")

        if self.add_fake:
            worker = FakeSiWorker()
            self._si_workers["fake"] = worker
            loops.append(worker.loop(self._queue))
        await asyncio.gather(*loops)

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
                worker = SerialSiWorker(device_node)
                asyncio.run_coroutine_threadsafe(worker.loop(self._queue), asyncio.get_event_loop())
                self._si_workers[device_node] = worker
            except Exception as e:
                logging.error(e)
        elif action == "remove":
            if device_node in self._si_workers:
                logging.info(f"Removed device {device_node}")
                si_worker = self._si_workers[device_node]
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
