import asyncio
import unittest
from unittest.mock import AsyncMock, MagicMock

from yaroc.clients.client import Client, ClientGroup
from yaroc.pb.status_pb2 import Status
from yaroc.rs import MeshtasticLog, MeshtasticPunches, SiPunchLog


class MockClient(Client):
    def __init__(self, name="MockClient"):
        self._name = name
        self.send_punch = AsyncMock()
        self.send_status = AsyncMock()
        self.send_meshtastic = AsyncMock()
        self.loop = AsyncMock()

    def name(self) -> str:
        return self._name

    async def loop(self):
        await self.loop()

    async def send_punch(self, punch_log: SiPunchLog):
        await self.send_punch(punch_log)

    async def send_status(self, status: Status, mac_addr: str):
        await self.send_status(status, mac_addr)

    async def send_meshtastic(self, msg: MeshtasticLog | MeshtasticPunches):
        await self.send_meshtastic(msg)


class MockClientMinimal(Client):
    def __init__(self, name="MockClientMinimal"):
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

    async def test_send_meshtastic_noexcept_awaits(self):
        client = MockClient()
        meshtastic_log = MagicMock(spec=MeshtasticLog)

        assert await client.send_meshtastic_noexcept(meshtastic_log)
        client.send_meshtastic.assert_awaited_once_with(meshtastic_log)

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

    async def test_client_group_send_meshtastic(self):
        client1 = MockClient("Client1")
        client2 = MockClient("Client2")
        group = ClientGroup([client1, client2], [])
        meshtastic_log = MagicMock(spec=MeshtasticLog)
        results = await group.send_meshtastic(meshtastic_log)
        assert len(results) == 2
        client1.send_meshtastic.assert_awaited_once_with(meshtastic_log)
        client2.send_meshtastic.assert_awaited_once_with(meshtastic_log)

    async def test_send_meshtastic_punches_noexcept(self):
        client = MockClientMinimal()
        punch_log1 = MagicMock(spec=SiPunchLog)
        punch_log2 = MagicMock(spec=SiPunchLog)
        punches = MagicMock(spec=MeshtasticPunches)
        punches.punch_logs = [punch_log1, punch_log2]

        assert await client.send_meshtastic_noexcept(punches)
        assert client.send_punch.call_count == 2
        client.send_punch.assert_any_await(punch_log1)
        client.send_punch.assert_any_await(punch_log2)

    async def test_client_group_loop_cancellation(self):
        client1 = MockClient("Client1")
        client2 = MockClient("Client2")

        async def endless_loop():
            await asyncio.sleep(100)

        client1.loop = endless_loop
        client2.loop = endless_loop

        task_cancelled = False

        async def external_task():
            nonlocal task_cancelled
            try:
                await asyncio.sleep(100)
            except asyncio.CancelledError:
                task_cancelled = True
                raise

        ext_task = asyncio.create_task(external_task())
        group = ClientGroup([client1, client2], [ext_task])

        group_task = asyncio.create_task(group.loop())
        await asyncio.sleep(0.01)
        group_task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await group_task
        assert task_cancelled

    async def test_client_group_loop_isolation(self):
        client1 = MockClient("Client1")
        client2 = MockClient("Client2")

        client2_cancelled = False

        async def failing_loop():
            raise RuntimeError("Client 1 failed")

        async def working_loop():
            nonlocal client2_cancelled
            try:
                await asyncio.sleep(0.1)
            except asyncio.CancelledError:
                client2_cancelled = True
                raise

        client1.loop = failing_loop
        client2.loop = working_loop

        group = ClientGroup([client1, client2], [])
        group_task = asyncio.create_task(group.loop())
        await asyncio.sleep(0.05)
        # Client 2 should still be running despite Client 1 failing
        assert not client2_cancelled
        assert not group_task.done()
        group_task.cancel()
