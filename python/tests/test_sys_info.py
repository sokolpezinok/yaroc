import os
import socket
import unittest
from unittest.mock import MagicMock, mock_open, patch

from yaroc.rs import RaspberryModel
from yaroc.utils.sys_info import (
    eth_mac_addr,
    find_config_file,
    is_windows,
    local_ip,
    raspberrypi_model,
)


class TestSysInfo(unittest.TestCase):
    def test_match(self):
        model = RaspberryModel.from_string("Raspberry Pi 2 Model B Rev 1.1")
        self.assertEqual(model, RaspberryModel.V2B)

    @patch("psutil.net_if_addrs")
    def test_eth_mac_addr(self, mock_net_if_addrs):
        mock_addr = MagicMock()
        mock_addr.family = 17  # AF_LINK usually
        mock_addr.address = "00:11:22:33:44:55"

        # We need to mock psutil.AF_LINK as well since it's used in the function
        with patch("yaroc.utils.sys_info.psutil.AF_LINK", 17):
            mock_net_if_addrs.return_value = {"eth0": [mock_addr]}
            self.assertEqual(eth_mac_addr(), "001122334455")

            mock_net_if_addrs.return_value = {
                "wlan0": [mock_addr]  # doesn't start with e
            }
            self.assertEqual(eth_mac_addr(), None)

    @patch("psutil.net_if_addrs")
    def test_local_ip(self, mock_net_if_addrs):
        mock_addr = MagicMock()
        mock_addr.family = socket.AF_INET
        mock_addr.address = "192.168.1.100"

        mock_net_if_addrs.return_value = {"eth0": [mock_addr]}

        ip_int = int.from_bytes(map(int, "192.168.1.100".split(".")))
        self.assertEqual(local_ip(), ip_int)

    @patch("yaroc.utils.sys_info.sys.platform", "win32")
    def test_is_windows_win32(self):
        self.assertTrue(is_windows())

    @patch("yaroc.utils.sys_info.sys.platform", "linux")
    @patch("yaroc.utils.sys_info.os.name", "posix")
    def test_is_windows_linux(self):
        self.assertFalse(is_windows())

    @patch("io.open", new_callable=mock_open, read_data="Raspberry Pi 3 Model B Plus Rev 1.3")
    def test_raspberrypi_model(self, mock_file):
        model = raspberrypi_model()
        self.assertEqual(model, RaspberryModel.V3B)

    @patch("io.open", side_effect=FileNotFoundError)
    def test_raspberrypi_model_not_found(self, mock_file):
        model = raspberrypi_model()
        self.assertEqual(model, RaspberryModel.Unknown)

    @patch("yaroc.utils.sys_info.os.path.exists")
    def test_find_config_file_local(self, mock_exists):
        mock_exists.return_value = True
        self.assertEqual(find_config_file("config.toml"), "config.toml")

    @patch("yaroc.utils.sys_info.is_windows")
    @patch("yaroc.utils.sys_info.os.path.exists")
    @patch.dict("os.environ", {"XDG_CONFIG_HOME": "/custom/config"})
    def test_find_config_file_linux_xdg(self, mock_exists, mock_is_windows):
        mock_is_windows.return_value = False
        # First check (local) returns False, second check (XDG) returns True
        mock_exists.side_effect = [False, True]
        self.assertEqual(
            find_config_file("config.toml"),
            os.path.join("/custom/config", "yaroc", "config.toml"),
        )

    @patch("yaroc.utils.sys_info.is_windows")
    @patch("yaroc.utils.sys_info.os.path.exists")
    @patch("yaroc.utils.sys_info.os.path.expanduser")
    @patch.dict("os.environ", {}, clear=True)
    def test_find_config_file_linux_default(self, mock_expanduser, mock_exists, mock_is_windows):
        mock_is_windows.return_value = False
        mock_expanduser.return_value = "/home/user"
        mock_exists.side_effect = [False, True]
        self.assertEqual(
            find_config_file("config.toml"),
            os.path.join("/home/user", ".config", "yaroc", "config.toml"),
        )

    @patch("yaroc.utils.sys_info.is_windows")
    @patch("yaroc.utils.sys_info.os.path.exists")
    @patch.dict("os.environ", {"APPDATA": "C:\\Users\\user\\AppData\\Roaming"})
    def test_find_config_file_windows_appdata(self, mock_exists, mock_is_windows):
        mock_is_windows.return_value = True
        mock_exists.side_effect = [False, True]
        expected = os.path.join("C:\\Users\\user\\AppData\\Roaming", "yaroc", "config.toml")
        self.assertEqual(find_config_file("config.toml"), expected)
