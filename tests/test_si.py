import unittest

from yaroc.utils.si import RawSiWorker


class TestSportident(unittest.TestCase):
    def test_decode(self):
        message = b"\xff\x02\xd3\r\x00\x2f\x00\x1a\x2b\x3c\x18\x8c\xa3\xcb\x02\tPZ\x86\x03"
        punch = RawSiWorker.decode_srr_msg(message)
        self.assertEqual(punch.card, 1715004)
        self.assertEqual(punch.code, 47)
        self.assertEqual(punch.mode, 2)

        # TODO: implement (and test) day of week
        self.assertEqual(punch.time.second, 3)
        self.assertEqual(punch.time.minute, 0)
        self.assertEqual(punch.time.hour, 10)
