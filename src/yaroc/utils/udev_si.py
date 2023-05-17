import logging
import time
from datetime import datetime
from threading import Event, Thread
from typing import Callable, Dict

import pyudev
from sportident import SIReader, SIReaderControl, SIReaderReadout, SIReaderSRR

from ..clients.client import Client

DEFAULT_TIMEOUT_MS = 3.0
START_MODE = 3
FINISH_MODE = 4
BEACON_CONTROL = 18


class SiWorker:
    def __init__(self, si: SIReader, clients: list[Client]):
        self.si = si
        self.finished = Event()
        self.clients = clients
        self.thread = Thread(target=self._worker_fn, daemon=True)
        self.thread.start()

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
                for client in self.clients:
                    # TODO: some of the clients are blocking, they shouldn't do that
                    client.send_punch(card_number, tim, code, mode)

    def __str__(self):
        if isinstance(self.si, SIReaderSRR):
            return "0-srr"
        if isinstance(self.si, SIReaderControl):
            return "0-control"

    def close(self, timeout: float = DEFAULT_TIMEOUT_MS):
        self.finished.set()
        self.si.disconnect()
        self.thread.join(timeout)


class UdevSIManager:
    """
    Dynamically manages connecting and disconnecting SportIdent devices: SI readers or SRR dongles.

    Usage:
    si_manager = UdevSIManager()
    si_manager.loop()
    """

    def __init__(self, udev_handler: Callable[[pyudev.Device], None], clients: list[Client]):
        context = pyudev.Context()
        self.monitor = pyudev.Monitor.from_netlink(context)
        self.monitor.filter_by("tty")
        self.si_workers: Dict[str, SiWorker] = {}
        self._clients = clients
        self._udev_handler = udev_handler
        for device in context.list_devices():
            self._handle_udev_event("add", device)

    def __str__(self) -> str:
        return ",".join(str(worker) for worker in self.si_workers.values())

    def _is_sportident(self, device: pyudev.Device):
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

    def loop(self):
        for device in iter(self.monitor.poll, None):
            self._handle_udev_event(device.action, device)

    def _handle_udev_event(self, action, device: pyudev.Device):
        if not self._is_sportident(device):
            return
        device_node = device.device_node
        self._udev_handler(device)
        if action == "add":
            if device_node in self.si_workers:
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
                    self.si_workers[device_node] = SiWorker(si, self._clients)
                    logging.info(f"Connected to {si.port}")
                else:
                    logging.warn(f"Station {si.port} not an SRR dongle or not set in autosend mode")

            except Exception as err:
                logging.error(f"Failed to connect to an SI station at {device_node}: {err}")
        elif device.action == "remove":
            if device_node in self.si_workers:
                logging.info(f"Removed device {device_node}")
                si_worker = self.si_workers[device_node]
                si_worker.close()
                del self.si_workers[device_node]
