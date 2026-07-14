import unittest
from datetime import timedelta
from unittest.mock import patch

from yaroc.utils.container import Container


class TestContainer(unittest.TestCase):
    @patch("yaroc.utils.container.MessageHandlerBuilder")
    def test_container_meshtastic_disabled(self, mock_builder_cls):
        mock_builder = mock_builder_cls.return_value
        mock_builder.with_dns.return_value = mock_builder
        mock_builder.with_meshtastic.return_value = mock_builder
        mock_builder.with_sportident.return_value = mock_builder

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
        }
        container = Container()
        container.config.from_dict(config)

        handler = container.message_handler()
        self.assertEqual(handler, mock_builder.build.return_value)
        mock_builder.with_meshtastic.assert_called_with(False)
        mock_builder.with_sportident.assert_called_with(True)

    @patch("yaroc.utils.container.MessageHandlerBuilder")
    def test_container_meshtastic_enabled(self, mock_builder_cls):
        mock_builder = mock_builder_cls.return_value
        mock_builder.with_dns.return_value = mock_builder
        mock_builder.with_meshtastic.return_value = mock_builder
        mock_builder.with_sportident.return_value = mock_builder

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
            "meshtastic": {
                "watch_usb": True,
            },
        }
        container = Container()
        container.config.from_dict(config)

        handler = container.message_handler()
        self.assertEqual(handler, mock_builder.build.return_value)
        mock_builder.with_meshtastic.assert_called_with(True)
        mock_builder.with_sportident.assert_called_with(True)

    @patch("yaroc.utils.container.MessageHandlerBuilder")
    def test_container_meshtastic_dns(self, mock_builder_cls):
        mock_builder = mock_builder_cls.return_value
        mock_builder.with_dns.return_value = mock_builder
        mock_builder.with_meshtastic.return_value = mock_builder
        mock_builder.with_sportident.return_value = mock_builder

        config = {
            "punch_source": {
                "usb": {"enable": True},
            },
            "meshtastic": {
                "watch_usb": True,
                "mac-addresses": {
                    "node1": "001122334455",
                },
            },
        }
        container = Container()
        container.config.from_dict(config)

        handler = container.message_handler()
        self.assertEqual(handler, mock_builder.build.return_value)
        mock_builder.with_dns.assert_called_with([("001122334455", "node1")])
        mock_builder.with_meshtastic.assert_called_with(True)
        mock_builder.with_sportident.assert_called_with(True)

    @patch("yaroc.utils.container.MessageHandlerBuilder")
    def test_container_fake_punch(self, mock_builder_cls):
        mock_builder = mock_builder_cls.return_value
        mock_builder.with_dns.return_value = mock_builder
        mock_builder.with_meshtastic.return_value = mock_builder
        mock_builder.with_sportident.return_value = mock_builder
        mock_builder.with_fake_punch.return_value = mock_builder

        config = {
            "punch_source": {
                "fake": {
                    "enable": True,
                    "interval": 10,
                    "card": 12345,
                    "code": 99,
                },
            },
        }
        container = Container()
        container.config.from_dict(config)

        handler = container.message_handler()
        self.assertEqual(handler, mock_builder.build.return_value)
        mock_builder.with_fake_punch.assert_called_with(timedelta(seconds=10), 12345, 99)

    @patch("yaroc.utils.container.MessageHandlerBuilder")
    def test_container_fake_punch_default(self, mock_builder_cls):
        mock_builder = mock_builder_cls.return_value
        mock_builder.with_dns.return_value = mock_builder
        mock_builder.with_meshtastic.return_value = mock_builder
        mock_builder.with_sportident.return_value = mock_builder
        mock_builder.with_fake_punch.return_value = mock_builder

        config = {
            "punch_source": {
                "fake": {
                    "enable": True,
                    "interval": 10,
                },
            },
        }
        container = Container()
        container.config.from_dict(config)

        handler = container.message_handler()
        self.assertEqual(handler, mock_builder.build.return_value)
        mock_builder.with_fake_punch.assert_called_with(timedelta(seconds=10), 46283, 47)
