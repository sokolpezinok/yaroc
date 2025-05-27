import unittest
from datetime import datetime

from yaroc.rs import SiPunch
from yaroc.utils.sys_info import extract_com


class TestSportident(unittest.TestCase):
    def test_new(self):
        t = datetime(2023, 11, 23, 10, 0, 3, 792969).astimezone()
        punch = SiPunch.new(1715004, 47, t, 2)
        self.assertEqual(
            bytes(punch.raw),
            b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x01P\xe3\x03",
        )

    def test_decode(self):
        bsf8_msg = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        punch = SiPunch.from_raw(bsf8_msg)
        self.assertEqual(punch.card, 1715004)
        self.assertEqual(punch.code, 47)
        self.assertEqual(punch.mode, 2)

        self.assertEqual(punch.time.weekday(), 3)
        self.assertEqual(punch.time.hour, 10)
        self.assertEqual(punch.time.minute, 0)
        self.assertEqual(punch.time.second, 3)
        self.assertEqual(punch.time.microsecond, 792968)

        SIAC_msg = b"\xff\x02\xd3\r\x80\x02\x0f{\xc0\xd9\x011\n\xb9t\x00\x01\x8e\xcb\x03"
        punch = SiPunch.from_raw(SIAC_msg)
        self.assertEqual(punch.card, 8110297)
        self.assertEqual(punch.code, 2)
        self.assertEqual(punch.mode, 4)  # Finish

        self.assertEqual(punch.time.weekday(), 6)
        self.assertEqual(punch.time.hour, 15)
        self.assertEqual(punch.time.minute, 29)
        self.assertEqual(punch.time.second, 14)
        self.assertEqual(punch.time.microsecond, 722656)


class TestUsbDetection(unittest.TestCase):
    def test_com_extraction(self):
        com_port = extract_com("SportIdent UART to USB (COM12)")
        self.assertEqual(com_port, "COM12")
