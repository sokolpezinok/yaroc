import unittest

from yaroc.utils.modem_manager import SignalInfo
from yaroc.utils.sys_info import NetworkType


class TestSignalInfo(unittest.TestCase):
    def test_repr(self):
        ns = SignalInfo(NetworkType.Lte, -86, 11)
        self.assertEqual(f"{ns}", "LTE RSSI -86dBm, SNR 11dB")
