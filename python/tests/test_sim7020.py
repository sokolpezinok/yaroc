import unittest
from unittest.mock import AsyncMock, patch

from yaroc.clients.mqtt import SIM7020MqttClient
from yaroc.utils.async_serial import AsyncATCom, ATResponse
from yaroc.utils.sim7020 import SIM7020Interface


class TestSIM7020(unittest.IsolatedAsyncioTestCase):
    async def test_sim7020_interface_default_bands(self):
        async_at = AsyncMock(spec=AsyncATCom)
        async_at.call.return_value = ATResponse(full_response=[], success=True)

        sim = SIM7020Interface(
            async_at=async_at,
            will_topic="test/will",
            client_name="test_client",
            connect_timeout=10,
            broker_url="broker.example.com",
            broker_port=1883,
            apn="lpwa.vodafone.com",
        )

        await sim.setup()
        async_at.call.assert_any_await("AT+CBAND=3,8,20")
        async_at.call.assert_any_await('AT*MCGDEFCONT="IP","lpwa.vodafone.com"', timeout=10)

    @patch("yaroc.clients.mqtt.SIM7020Interface")
    def test_sim7020_mqtt_client_config(self, mock_interface):
        async_at = AsyncMock(spec=AsyncATCom)

        # Default APN and bands when not specified in config
        SIM7020MqttClient(
            hostname="host1",
            mac_address="001122334455",
            async_at=async_at,
            config={"broker_url": "broker.test", "broker_port": 1883},
        )
        mock_interface.assert_called_with(
            async_at,
            "yar/001122334455/status",
            "SIM7020-host1",
            35,
            "broker.test",
            1883,
            "lpwa.vodafone.com",
            [3, 8, 20],
        )

        # Custom APN specified in config
        SIM7020MqttClient(
            hostname="host2",
            mac_address="001122334455",
            async_at=async_at,
            config={"broker_url": "broker.test", "broker_port": 1883, "apn": "custom.apn"},
        )
        mock_interface.assert_called_with(
            async_at,
            "yar/001122334455/status",
            "SIM7020-host2",
            35,
            "broker.test",
            1883,
            "custom.apn",
            [3, 8, 20],
        )

        # Custom bands list specified in config
        SIM7020MqttClient(
            hostname="host3",
            mac_address="001122334455",
            async_at=async_at,
            config={"broker_url": "broker.test", "broker_port": 1883, "bands": [8, 20]},
        )
        mock_interface.assert_called_with(
            async_at,
            "yar/001122334455/status",
            "SIM7020-host3",
            35,
            "broker.test",
            1883,
            "lpwa.vodafone.com",
            [8, 20],
        )
