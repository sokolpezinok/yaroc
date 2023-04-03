import logging
import queue
import threading
from typing import Callable

import pyudev
from sportident import SIReader, SIReaderControl, SIReaderReadout, SIReaderSRR

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


class SiWorker:
    def __init__(self, si: SIReader, worker_fn: Callable[[SIReader], None]):
        self.si = si
        self.thread = threading.Thread(target=worker_fn, args=(self.si,))
        self.thread.setDaemon(True)
        self.thread.start()

    def close(self, timeout: float | None = None):
        self.si.disconnect()
        self.thread.join(timeout)


class UdevSIManager:
    """
    Dynamically manages connecting and disconnecting SportIdent devices: SI readers or SRR dongles.

    Usage:
    si_manager = UdevSIManager()
    si_manager.loop()
    """

    def __init__(self, worker_fn: Callable[[SIReader], None]):
        context = pyudev.Context()
        monitor = pyudev.Monitor.from_netlink(context)
        monitor.filter_by("tty")
        observer = pyudev.MonitorObserver(monitor, self._handle_udev_event)
        observer.start()
        self.worker_fn = worker_fn
        self.queue = queue.Queue()

    def loop(self):
        si_workers = {}
        while True:
            (action, device_node) = self.queue.get()
            if action == "add":
                logging.info(f"Inserted SportIdent device {device_node}")
                try:
                    si = SIReaderReadout(device_node)
                    if si.get_type() == SIReader.M_SRR:
                        si.disconnect()
                        si = SIReaderSRR(device_node)
                    elif (
                        si.get_type() == SIReader.M_CONTROL
                        or si.get_type() == SIReader.M_BC_CONTROL
                    ):
                        si.disconnect()
                        si = SIReaderControl(device_node)
                    si_workers[device_node] = SiWorker(si, self.worker_fn)
                    logging.info(f"Connected to {si.port}")

                except Exception as err:
                    logging.error(f"Failed to connect to an SI station at {device_node}: {err}")
            elif action == "remove":
                if device_node in si_workers:
                    logging.info(f"Removed device {device_node}")
                    si_worker = si_workers[device_node]
                    si_worker.close()
                    del si_workers[device_node]

    def _handle_udev_event(self, action, device: pyudev.Device):
        try:
            is_sportident = (
                device.properties["ID_USB_VENDOR_ID"] == "10c4"
                and device.properties["ID_MODEL_ID"] == "800a"
            )
        except Exception:
            # pyudev sucks, it throws an exception when you're only doing a lookup
            is_sportident = False

        if is_sportident:
            device_node = device.device_node
            if action == "add":
                self.queue.put((action, device_node))
            if action == "remove":
                self.queue.put((action, device_node))
