import unittest
from datetime import datetime

from yaroc.rs import SiPunch


class TestSportident(unittest.TestCase):
    def test_new(self):
        t = datetime(2023, 11, 23, 10, 0, 3, 792969).astimezone()
        punch = SiPunch.new(1715004, 47, t, 2)
        self.assertEqual(
            bytes(punch.raw),
            b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x08\x8c\xa3\xcb\x02\x00\x00\xe3\x51\x03",
        )

    def test_decode(self):
        message = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        punch = SiPunch.from_raw(message)
        self.assertEqual(punch.card, 1715004)
        self.assertEqual(punch.code, 47)
        self.assertEqual(punch.mode, 2)

        self.assertEqual(punch.time.weekday(), 3)
        self.assertEqual(punch.time.hour, 10)
        self.assertEqual(punch.time.minute, 0)
        self.assertEqual(punch.time.second, 3)
        self.assertEqual(punch.time.microsecond, 792968)
