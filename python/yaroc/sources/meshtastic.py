import asyncio
import logging
from asyncio import Queue
from typing import Any

from meshtastic.serial_interface import SerialInterface
from pubsub import pub
from usbmonitor import USBMonitor
from usbmonitor.attributes import DEVNAME

from ..utils.sys_info import tty_device_from_usb


class MeshtasticSerial:
    def __init__(self, status_callback, punch_callback):
        self.status_callback = status_callback
        self.punch_callback = punch_callback
        self._loop = asyncio.get_event_loop()
        self.recv_mac_addr_int = 0
        self._device_queue: Queue[tuple[bool, str, str]] = Queue()
        self._serial = None
        self._device_node = None

    def on_receive(self, packet, interface):
        portnum = packet.get("decoded", {}).get("portnum", "")
        raw = packet["raw"].SerializeToString()
        if portnum == "SERIAL_APP":
            asyncio.run_coroutine_threadsafe(self.punch_callback(raw), self._loop)
        elif portnum == "TELEMETRY_APP":
            self.status_callback(raw, self.recv_mac_addr_int)

    @staticmethod
    def _tty_acm(device_info: dict[str, Any]) -> tuple[str | None, str]:
        device_node = device_info[DEVNAME]
        return (tty_device_from_usb(device_node), device_node)

    async def loop(self):
        monitor = USBMonitor()
        for device_id, parent_device_info in monitor.get_available_devices().items():
            self._add_usb_device(device_id, parent_device_info)
        monitor.start_monitoring(self._add_usb_device, self._remove_usb_device)

        while True:
            added, tty_acm, device_node = await self._device_queue.get()
            if added:
                await asyncio.sleep(3.0)  # Give the TTY subystem more time
                try:
                    self._serial = SerialInterface(tty_acm)
                    self._device_node = device_node
                    self.recv_mac_addr_int = self._serial.myInfo.my_node_num
                    logging.info(f"Connected to Meshtastic serial at {tty_acm}")
                    pub.subscribe(self.on_receive, "meshtastic.receive")
                except Exception as err:
                    logging.error(f"Error while connecting to Meshtastic serial at {err}")
            else:
                if self._device_node == device_node:
                    # TODO: We should also close when this object is destroyed
                    self._serial.close()
                    self._serial = None
                    self._device_node = None

        await asyncio.sleep(1000000)

    def _add_usb_device(self, _device_id: str, device_info: dict[str, Any]):
        try:
            tty_acm, device_node = self._tty_acm(device_info)
            if tty_acm is not None and "ACM" in tty_acm:
                asyncio.run_coroutine_threadsafe(
                    self._device_queue.put((True, tty_acm, device_node)), self._loop
                )
        except Exception as err:
            logging.error(err)

    def _remove_usb_device(self, _device_id: str, device_info: dict[str, Any]):
        device_node = device_info[DEVNAME]
        asyncio.run_coroutine_threadsafe(
            self._device_queue.put((False, "Unknown", device_node)), self._loop
        )
