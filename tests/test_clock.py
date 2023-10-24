import unittest
from datetime import datetime, timezone

from yaroc.utils.sys_info import is_time_off


class TestClock(unittest.TestCase):
    def test_competitor_parsing(self):
        modem_clock = "23/06/09,12:06:31+08"
        now = datetime(2023, 6, 9, 12, 6, 25, tzinfo=timezone.utc)
        self.assertEqual(
            is_time_off(modem_clock, now), datetime(2023, 6, 9, 12, 6, 31, tzinfo=timezone.utc)
        )
        now = datetime(2023, 6, 9, 12, 6, 27, tzinfo=timezone.utc)
        self.assertEqual(is_time_off(modem_clock, now), None)
