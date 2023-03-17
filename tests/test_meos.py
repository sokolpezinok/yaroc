import unittest
from datetime import time

from yaroc.clients.meos import MeosClient


class TestMeos(unittest.TestCase):
    def test_serialization(self):
        message = MeosClient._serialize(46283, time(hour=7, minute=3, second=20), code=31)
        self.assertEqual(message, b"\x00\x1f\x00\xcb\xb4\x00\x00\x00\x00\x00\x00\x30\xe0\x03\x00")


if __name__ == "__main__":
    unittest.main()
