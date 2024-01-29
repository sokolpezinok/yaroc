import unittest
from datetime import datetime, timedelta

from yaroc.rs import CellularLogMessage


class TestCellularLogMessage(unittest.TestCase):
    def test_dbm_cellid(self):
        timestamp = datetime.fromisoformat("2024-01-28 17:40:43.674831 +01:00")
        log_message = CellularLogMessage(
            "spe01",
            timestamp,
            timestamp + timedelta(milliseconds=1390),
            1.26,
        )
        log_message.dbm = -87
        log_message.cellid = 2580590
        "spe01 17:40:43: 51.54°C, -87dBm, cell 27606E, 1.26V, latency 1.39s"
