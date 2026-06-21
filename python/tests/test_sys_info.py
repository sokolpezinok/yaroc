import os
import socket
import sys
import tempfile
import unittest
from pathlib import Path
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

    def test_find_config_file_local(self):
        import tempfile

        with tempfile.NamedTemporaryFile(suffix="-test-config.toml", delete=False) as f:
            temp_file_path = f.name
        try:
            self.assertEqual(find_config_file(temp_file_path), temp_file_path)
        finally:
            os.remove(temp_file_path)

    def test_find_config_file_fallback(self):
        # Test non-existent file
        self.assertEqual(
            find_config_file("non-existent-config-file-12345.toml"),
            "non-existent-config-file-12345.toml",
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            if sys.platform.lower() == "win32" or os.name.lower() == "nt":
                # Windows tests
                orig_appdata = os.environ.get("APPDATA")
                os.environ["APPDATA"] = tmpdir
                try:
                    yaroc_dir = Path(tmpdir) / "yaroc"
                    yaroc_dir.mkdir(parents=True, exist_ok=True)
                    mock_config = yaroc_dir / "test_config_appdata.toml"
                    mock_config.touch()

                    resolved = find_config_file("test_config_appdata.toml")
                    self.assertEqual(resolved, str(mock_config))
                finally:
                    if orig_appdata is not None:
                        os.environ["APPDATA"] = orig_appdata
                    else:
                        os.environ.pop("APPDATA", None)
            else:
                # Linux/Unix tests
                # 1. Test XDG_CONFIG_HOME fallback
                orig_xdg = os.environ.get("XDG_CONFIG_HOME")
                os.environ["XDG_CONFIG_HOME"] = tmpdir
                try:
                    yaroc_dir = Path(tmpdir) / "yaroc"
                    yaroc_dir.mkdir(parents=True, exist_ok=True)
                    mock_config = yaroc_dir / "test_config_xdg.toml"
                    mock_config.touch()

                    resolved = find_config_file("test_config_xdg.toml")
                    self.assertEqual(resolved, str(mock_config))
                finally:
                    if orig_xdg is not None:
                        os.environ["XDG_CONFIG_HOME"] = orig_xdg
                    else:
                        os.environ.pop("XDG_CONFIG_HOME", None)

                # 2. Test HOME fallback
                orig_home = os.environ.get("HOME")
                orig_xdg = os.environ.get("XDG_CONFIG_HOME")
                if orig_xdg is not None:
                    os.environ.pop("XDG_CONFIG_HOME")
                os.environ["HOME"] = tmpdir
                try:
                    yaroc_dir = Path(tmpdir) / ".config" / "yaroc"
                    yaroc_dir.mkdir(parents=True, exist_ok=True)
                    mock_config = yaroc_dir / "test_config_home.toml"
                    mock_config.touch()

                    resolved = find_config_file("test_config_home.toml")
                    self.assertEqual(resolved, str(mock_config))
                finally:
                    if orig_home is not None:
                        os.environ["HOME"] = orig_home
                    else:
                        os.environ.pop("HOME", None)
                    if orig_xdg is not None:
                        os.environ["XDG_CONFIG_HOME"] = orig_xdg
