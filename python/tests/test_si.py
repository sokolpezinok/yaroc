import unittest
from datetime import datetime

from yaroc.rs import HostInfo, SiPunch, SiPunchLog


class TestSportident(unittest.TestCase):
    def test_new(self):
        t = datetime(2023, 11, 23, 10, 0, 3, 792969)
        punch = SiPunch.new(1715004, 47, t, 2)
        self.assertEqual(
            bytes(punch.raw),
            b"\xff\x02\xd3\r\x80\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01\xef \x03",
        )

    def test_decode(self):
        bsf8_msg = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        now = datetime(2023, 11, 26, 18, 0)
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

    def test_punch_log(self):
        now = datetime(2023, 11, 26, 18, 0)
        punch = SiPunch.from_raw(
            b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03", now
        )
        self.assertIsNotNone(punch)
        host_info = HostInfo.new("ROC1", "123456789012")
        log = SiPunchLog.new(punch, host_info, punch.time)
        self.assertIn("ROC1 1715004 punched 47 at", str(log))
        self.assertIn("latency 0.00s", str(log))
        with self.assertRaises(AttributeError):
            punch.card = 123
        with self.assertRaises(AttributeError):
            host_info.mac_address = "foo"

    def test_equality(self):
        now = datetime(2023, 11, 26, 18, 0)
        raw = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        punch1 = SiPunch.from_raw(raw, now)
        punch2 = SiPunch.from_raw(raw, now)
        self.assertEqual(punch1, punch2)

        h1 = HostInfo.new("ROC1", "123456789012")
        h2 = HostInfo.new("ROC1", "123456789012")
        h3 = HostInfo.new("ROC2", "123456789012")
        self.assertEqual(h1, h2)
        self.assertNotEqual(h1, h3)
