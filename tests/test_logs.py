import unittest
from datetime import datetime, timedelta

from yaroc.rs import DbmSnr, MshLogMessage


class TestMshLogMessage(unittest.TestCase):
    def test_volt_batt(self):
        timestamp = datetime.fromisoformat("2024-01-28 13:15:25.755721 +01:00")
        log_message = MshLogMessage("spr01", timestamp, timestamp + timedelta(milliseconds=1230))
        log_message.voltage_battery = (4.012, 82)
        self.assertEqual("spr01 13:15:25: batt 4.012V 82%, latency 1.23s", f"{log_message}")

    def test_position(self):
        timestamp = datetime.fromisoformat("2024-01-28 13:15:25.755721 +01:00")
        log_message = MshLogMessage("spr01", timestamp, timestamp + timedelta(milliseconds=1230))

        log_message.set_position(48.29633, 17.26675, 170, timestamp)
        self.assertEqual(
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s", f"{log_message}"
        )

    def test_position_dbm(self):
        timestamp = datetime.fromisoformat("2024-01-28 13:15:25.755721 +01:00")
        log_message = MshLogMessage("spr01", timestamp, timestamp + timedelta(milliseconds=1230))

        log_message.set_position(48.29633, 17.26675, 170, timestamp)
        log_message.dbm_snr = DbmSnr(-80, 4.25, (813, "spr02"))
        self.assertEqual(
            "spr01 13:15:25: coords 48.29633 17.26675 170m, latency 1.23s, -80dBm 4.25SNR 0.81km"
            " from spr02",
            f"{log_message}",
        )
