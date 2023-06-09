import unittest
from datetime import datetime

from yaroc.utils.sys_info import is_time_off


class TestMeos(unittest.TestCase):
    def test_competitor_parsing(self):
        modem_clock = "23/06/09,12:06:31+08"
        now = datetime(2023, 6, 9, 14, 6, 25)
        self.assertEqual(
            is_time_off(modem_clock, now), datetime(2023, 6, 9, 14, 6, 31).astimezone()
        )
        now = datetime(2023, 6, 9, 14, 6, 27)
        self.assertEqual(is_time_off(modem_clock, now), None)
