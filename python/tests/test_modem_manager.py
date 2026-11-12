import sys
import unittest

if sys.platform == "linux":
    from yaroc.utils.modem_manager import SignalInfo
    from yaroc.utils.sys_info import NetworkType


@unittest.skipUnless(sys.platform == "linux", "ModemManager is only supported on Linux")
class TestSignalInfo(unittest.TestCase):
    def test_repr(self):
        ns = SignalInfo(NetworkType.Lte, -86, 11)
        self.assertEqual(f"{ns}", "LTE RSRP -86dBm, SNR 11dB")
