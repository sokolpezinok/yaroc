import asyncio
import logging
from asyncio import Queue
from typing import Any, Callable, Coroutine

from usbmonitor import USBMonitor
from usbmonitor.attributes import DEVNAME, ID_VENDOR_ID

from ..rs import MshDevHandler
from ..sources.si import SI_LABS
from ..utils.sys_info import tty_device_from_usb


class UsbSerialManager:
    def __init__(
        self,
        msh_dev_handler: MshDevHandler | None = None,
        si_device_notifier: Queue[str] | None = None,
    ):
        self._loop = asyncio.get_event_loop()
        self._msh_dev_queue: Queue[tuple[bool, str, str]] = Queue()
        self._msh_dev_handler = msh_dev_handler
        self._si_device_notifier = si_device_notifier

    @staticmethod
    def _tty_acm(device_info: dict[str, Any]) -> str | None:
        return tty_device_from_usb(device_info)

    @staticmethod
    def _is_silabs(device_info: dict[str, Any]):
        return device_info.get(ID_VENDOR_ID, "").lower() == SI_LABS

    async def loop(self):
        monitor = USBMonitor()
        for device_id, parent_device_info in monitor.get_available_devices().items():
            self._add_usb_device(device_id, parent_device_info)
        monitor.start_monitoring(self._add_usb_device, self._remove_usb_device)

        while self._msh_dev_handler is not None:
            added, tty_acm, device_node = await self._msh_dev_queue.get()
            if added:
                await asyncio.sleep(3.0)  # Give the TTY subystem more time
                await self._msh_dev_handler.add_device(tty_acm, device_node)
            else:
                self._msh_dev_handler.remove_device(device_node)

        # Sleep forever
        await asyncio.get_running_loop().create_future()

    def _add_usb_device(self, device_id: str, device_info: dict[str, Any]):
        try:
            tty_acm = self._tty_acm(device_info)
            if tty_acm is None:
                return
            device_node = device_info.get(DEVNAME, device_id)
            if not device_node.endswith("001") and any(x in tty_acm for x in ["ACM", "COM"]):
                asyncio.run_coroutine_threadsafe(
                    self._msh_dev_queue.put((True, tty_acm, device_node)), self._loop
                )
            elif self._si_device_notifier is not None and self._is_silabs(device_info):
                asyncio.run_coroutine_threadsafe(self._si_device_notifier.put(tty_acm), self._loop)
        except Exception as err:
            logging.error(err)

    def _remove_usb_device(self, device_id: str, device_info: dict[str, Any]):
        device_node = device_info.get(DEVNAME, device_id)
        asyncio.run_coroutine_threadsafe(
            self._msh_dev_queue.put((False, "Unknown", device_node)), self._loop
        )


async def forward_queue(coroutine: Callable[[str], Coroutine], si_device_notifier: Queue[str]):
    while True:
        new_device = await si_device_notifier.get()
        await asyncio.sleep(2.0)  # Give the TTY system more time
        await coroutine(new_device)
