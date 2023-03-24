import unittest
from datetime import time

from yaroc.clients.meos import MeosClient


class TestMeos(unittest.TestCase):
    def test_punch_serialization(self):
        message = MeosClient._serialize_punch(46283, time(hour=7, minute=3, second=20), code=31)
        self.assertEqual(message, b"\x00\x1f\x00\xcb\xb4\x00\x00\x00\x00\x00\x00\x30\xe0\x03\x00")

    def test_card_serialization(self):
        start = time(hour=8, minute=0, second=2)
        finish = time(hour=8, minute=9, second=13)
        message = MeosClient._serialize_card(46283, start, finish, [])
        self.assertEqual(
            message,
            b"\x40\x02\x00\xcb\xb4\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00\x00\x00\x14"
            b"\x65\x04\x00\x02\x00\x00\x00\x9a\x7a\x04\x00",
        )


if __name__ == "__main__":
    unittest.main()
