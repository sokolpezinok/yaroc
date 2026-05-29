import asyncio
import unittest
from asyncio import Queue
from datetime import datetime
from unittest.mock import AsyncMock, patch

from yaroc.rs import Event, HostInfo, SiPunch, SiPunchLog
from yaroc.sources.si import UdevSiFactory


class TestSportident(unittest.TestCase):
    def test_new(self):
        t = datetime(2023, 11, 23, 10, 0, 3, 792969).astimezone()
        punch = SiPunch.new(1715004, 47, t, 2)
        self.assertEqual(
            bytes(punch.raw),
            b"\xff\x02\xd3\r\x80\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\xef \x03",
        )

    def test_decode(self):
        bsf8_msg = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        now = datetime.now().astimezone()
        punch = SiPunch.from_raw(bsf8_msg, now)
        self.assertEqual(punch.card, 1715004)
        self.assertEqual(punch.code, 47)
        self.assertEqual(punch.mode, 2)

        self.assertEqual(punch.time.weekday(), 3)
        self.assertEqual(punch.time.hour, 10)
        self.assertEqual(punch.time.minute, 0)
        self.assertEqual(punch.time.second, 3)
        self.assertEqual(punch.time.microsecond, 792968)

        SIAC_msg = b"\xff\x02\xd3\r\x80\x02\x0f{\xc0\xd9\x011\n\xb9t\x00\x01\x8e\xcb\x03"
        punch = SiPunch.from_raw(SIAC_msg, now)
        self.assertEqual(punch.card, 8110297)
        self.assertEqual(punch.code, 2)
        self.assertEqual(punch.mode, 4)  # Finish

        self.assertEqual(punch.time.weekday(), 6)
        self.assertEqual(punch.time.hour, 15)
        self.assertEqual(punch.time.minute, 29)
        self.assertEqual(punch.time.second, 14)
        self.assertEqual(punch.time.microsecond, 722656)


class TestSiWorker(unittest.IsolatedAsyncioTestCase):
    async def test_udev_si_factory_punches(self):
        # Create UdevSiFactory
        worker = UdevSiFactory(enable_meshtastic=True)
        self.assertTrue(worker.enable_meshtastic)

        # Mock the handler and usb_serial_manager returned by MessageHandler
        mock_handler = AsyncMock()
        mock_usb_manager = AsyncMock()

        # Let's mock MessageHandler constructor by patching it
        with patch("yaroc.sources.si.MessageHandler") as mock_mh:
            mock_mh.return_value = (mock_handler, mock_usb_manager)

            queue = Queue()
            status_queue = Queue()

            t = datetime.now().astimezone()
            punch = SiPunch.new(1715004, 47, t, 2)

            host_info = HostInfo.new("test_host", "001122334455")
            punch_log = SiPunchLog.new(punch, host_info, t)

            # In our mock next_event, we return a sequence of events, then raise a CancelledError to break the infinite loop
            mock_handler.next_event.side_effect = [
                Event.SiPunch(punch),
                Event.SiPunchLogs([punch_log]),
                Event.DeviceEvnt(True, "test_device"),
                asyncio.CancelledError(),
            ]

            # Since loop gathers both next_event loop and usb_serial_manager.loop(),
            # usb_serial_manager.loop() is also AsyncMock, so it will return immediately.
            # We catch CancelledError to end the worker.loop cleanly.
            try:
                await worker.loop(queue, status_queue)
            except asyncio.CancelledError:
                pass

            self.assertEqual(queue.qsize(), 2)
            p1 = await queue.get()
            p2 = await queue.get()
            self.assertEqual(p1.card, 1715004)
            self.assertEqual(p2.card, 1715004)

            self.assertEqual(status_queue.qsize(), 1)
            dev_ev = await status_queue.get()
            self.assertEqual(dev_ev.device, "test_device")
            self.assertTrue(dev_ev.added)


class TestContainer(unittest.TestCase):
    def test_container_meshtastic_disabled(self):
        from yaroc.utils.container import Container

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
        }
        container = Container()
        container.config.from_dict(config)

        workers = container.workers()
        self.assertEqual(len(workers), 1)
        self.assertFalse(workers[0].enable_meshtastic)

    def test_container_meshtastic_enabled(self):
        from yaroc.utils.container import Container

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
            "meshtastic": {
                "watch_usb": True,
            },
        }
        container = Container()
        container.config.from_dict(config)

        workers = container.workers()
        self.assertEqual(len(workers), 1)
        self.assertTrue(workers[0].enable_meshtastic)

    def test_container_meshtastic_dns(self):
        from yaroc.utils.container import Container

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
            "meshtastic": {
                "watch_usb": True,
                "mac-addresses": {
                    "node1": "001122334455",
                },
            },
        }
        container = Container()
        container.config.from_dict(config)

        workers = container.workers()
        self.assertEqual(len(workers), 1)
        self.assertTrue(workers[0].enable_meshtastic)
        self.assertEqual(workers[0].dns, [("001122334455", "node1")])
