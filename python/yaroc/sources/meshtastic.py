import asyncio
import logging
from asyncio import Queue
from typing import Any

from usbmonitor import USBMonitor
from usbmonitor.attributes import DEVNAME

from ..rs import MshDevHandler
from ..utils.sys_info import tty_device_from_usb


class MeshtasticSerial:
    def __init__(self, msh_dev_handler: MshDevHandler):
        self._loop = asyncio.get_event_loop()
        self._device_queue: Queue[tuple[bool, str, str]] = Queue()
        self._handler = msh_dev_handler

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
                await self._handler.add_device(tty_acm, device_node)
            else:
                self._handler.remove_device(device_node)

        await asyncio.sleep(10000000)

    def _add_usb_device(self, _device_id: str, device_info: dict[str, Any]):
        try:
            tty_acm, device_node = self._tty_acm(device_info)
            if not device_node.endswith("001") and tty_acm is not None and "ACM" in tty_acm:
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
