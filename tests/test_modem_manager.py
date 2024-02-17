import unittest

from yaroc.utils.modem_manager import NetworkState, NetworkType


class TestNetworkState(unittest.TestCase):
    def test_repr(self):
        ns = NetworkState(NetworkType.Lte, -86, 11)
        self.assertEqual(f"{ns}", "LTE RSSI -86dBm, SNR 11dB")
