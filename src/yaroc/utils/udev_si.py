import logging
import threading
from typing import Callable, Dict

import pyudev
from sportident import SIReader, SIReaderControl, SIReaderReadout, SIReaderSRR

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


DEFAULT_TIMEOUT_MS = 3.0


class SiWorker:
    def __init__(self, si: SIReader, worker_fn: Callable[[SIReader, threading.Event], None]):
        self.si = si
        self.finished = threading.Event()
        self.thread = threading.Thread(target=worker_fn, args=(self.si, self.finished))
        self.thread.setDaemon(True)
        self.thread.start()

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

    def __init__(self, worker_fn: Callable[[SIReader, threading.Event], None]):
        context = pyudev.Context()
        self.monitor = pyudev.Monitor.from_netlink(context)
        self.monitor.filter_by("tty")
        self.si_workers: Dict[str, SiWorker] = {}
        self.worker_fn = worker_fn
        for device in context.list_devices():
            self._handle_udev_event("add", device)

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
                    self.si_workers[device_node] = SiWorker(si, self.worker_fn)
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
