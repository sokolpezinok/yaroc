import pyudev
from sportident import SIReaderReadout, SIReaderSRR, SIReader, SIReaderControl
import logging
import queue

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


class UdevSIManager:
    """
    Dynamically manages connecting and disconnecting SportIdent devices: SI readers or SRR dongles.

    Usage:
    si_manager = UdevSIManager()
    si_manager.loop()
    """

    def __init__(self):
        context = pyudev.Context()
        monitor = pyudev.Monitor.from_netlink(context)
        monitor.filter_by("tty")
        observer = pyudev.MonitorObserver(monitor, self._handle_udev_event)
        observer.start()
        self.queue = queue.Queue()

    def loop(self):
        si_devices = {}
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
                    logging.info(f"Connected to {si.port}")
                    si_devices[device_node] = si

                except Exception:
                    logging.error(f"Failed to connect to an SI station at {device_node}")
            elif action == "remove":
                if device_node in si_devices:
                    logging.info(f"Removed device {device_node}")
                    si_devices[device_node].disconnect()
                    del si_devices[device_node]

    def close(self, timeout=None):
        self.thread.join(timeout)

    def _handle_udev_event(self, action, device):
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
