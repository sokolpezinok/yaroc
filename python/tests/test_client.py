import unittest
from unittest.mock import AsyncMock, MagicMock

from yaroc.clients.client import Client, ClientGroup
from yaroc.pb.status_pb2 import Status
from yaroc.rs import SiPunchLog


class MockClient(Client):
    def __init__(self, name="MockClient"):
        self._name = name
        self.send_punch = AsyncMock()
        self.send_status = AsyncMock()
        self.loop = AsyncMock()

    def name(self) -> str:
        return self._name

    async def loop(self):
        await self.loop()

    async def send_punch(self, punch_log: SiPunchLog):
        await self.send_punch(punch_log)

    async def send_status(self, status: Status, mac_addr: str):
        await self.send_status(status, mac_addr)


class TestClient(unittest.IsolatedAsyncioTestCase):
    async def test_send_punch_noexcept_awaits(self):
        client = MockClient()
        punch_log = MagicMock(spec=SiPunchLog)
        assert await client.send_punch_noexcept(punch_log)
        client.send_punch.assert_awaited_once_with(punch_log)

    async def test_send_status_noexcept_awaits(self):
        client = MockClient()
        status = Status()
        mac_addr = "00:11:22:33:44:55"

        assert await client.send_status_noexcept(status, mac_addr)
        client.send_status.assert_awaited_once_with(status, mac_addr)

    async def test_send_punch_noexcept_exception(self):
        client = MockClient()
        client.send_punch.side_effect = Exception("Failed")
        punch_log = MagicMock(spec=SiPunchLog)
        with self.assertLogs(level="ERROR") as cm:
            assert not await client.send_punch_noexcept(punch_log)
        assert any("MockClient failed: Failed" in log for log in cm.output)
        client.send_punch.assert_awaited_once()

    async def test_client_group_send_punch(self):
        client1 = MockClient("Client1")
        client2 = MockClient("Client2")
        group = ClientGroup([client1, client2], [])
        punch_log = MagicMock(spec=SiPunchLog)
        results = await group.send_punch(punch_log)
        assert len(results) == 2
        client1.send_punch.assert_awaited_once_with(punch_log)
        client2.send_punch.assert_awaited_once_with(punch_log)
