import unittest
from datetime import timedelta
from unittest.mock import patch

from yaroc.utils.container import Container, create_clients


class TestContainer(unittest.IsolatedAsyncioTestCase):
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

    @patch("yaroc.utils.container.logging.error")
    async def test_create_clients_roc_unknown_device(self, mock_logging_error):
        container = Container()
        config = {
            "roc": {
                "enable": True,
                "override": {
                    "spr01": "b827eba22867",
                    "spr05": "b827eba22868",
                },
            },
        }
        mac_addresses = {
            "spr01": "112233445566",
        }
        client_group = await create_clients(
            container.client_factories, mac_addresses, config=config
        )
        self.assertEqual(client_group.len(), 1)
        roc_client = client_group.clients[0]
        self.assertEqual(roc_client.mac_override_map, {"112233445566": "b827eba22867"})
        mock_logging_error.assert_called_once_with(
            "Cannot override MAC for spr05: device not found in mac-addresses"
        )
