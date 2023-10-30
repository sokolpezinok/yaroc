import asyncio
import logging
from datetime import datetime, timedelta
from typing import Tuple

from aiomqtt import Client as AioMqttClient
from aiomqtt import MqttError
from aiomqtt.client import Will
from aiomqtt.error import MqttCodeError

from ..pb.punches_pb2 import Punch, Punches
from ..pb.status_pb2 import Disconnected, MiniCallHome, Status
from ..pb.utils import create_coords_proto, create_punch_proto
from ..utils.async_serial import AsyncATCom
from ..utils.retries import BackoffBatchedRetries
from ..utils.sim7020 import SIM7020Interface
from .client import Client

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883
CONNECT_TIMEOUT = 45


def topics_from_mac(mac_address: str) -> Tuple[str, str, str]:
    return (
        f"yaroc/{mac_address}/p",
        f"yaroc/{mac_address}/coords",
        f"yaroc/{mac_address}/status",
    )


class MqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(
        self,
        mac_address: str,
        name_prefix: str = "aiomqtt",
        broker_url: str = BROKER_URL,
        broker_port: int = BROKER_PORT,
    ):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)
        self.name = f"{name_prefix}-{mac_address}"
        self.broker_url = broker_url
        self.broker_port = broker_port

        disconnected = Disconnected()
        disconnected.client_name = self.name

        status = Status()
        status.disconnected.CopyFrom(disconnected)
        will = Will(topic=self.topic_status, payload=status.SerializeToString(), qos=1)

        self.client = AioMqttClient(
            self.broker_url,
            self.broker_port,
            timeout=20,
            will=will,
            client_id=self.name,
            clean_session=False,
            max_inflight_messages=100,
            logger=logging.getLogger(),
        )

    def __del__(self):
        self.client.loop_stop()

    async def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> bool:
        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        punches.sending_timestamp.GetCurrentTime()
        return await self._send(self.topic_punches, punches.SerializeToString(), qos=1)

    async def send_coords(self, lat: float, lon: float, alt: float, timestamp: datetime) -> bool:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return await self._send(self.topic_coords, coords.SerializeToString(), qos=0)

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return await self._send(self.topic_status, status.SerializeToString(), qos=0)

    async def _send(self, topic: str, msg: bytes, qos: int) -> bool:
        try:
            await self.client.publish(topic, payload=msg, qos=qos)
            logging.info("Message sent")
            return True
        except MqttCodeError as e:
            logging.error(f"Message not sent: {e}")
            return False

    async def loop(self):
        while True:
            try:
                async with self.client:
                    logging.info(f"Connected to mqtt://{BROKER_URL}")
                    await asyncio.sleep(10000000.0)
            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await asyncio.sleep(5.0)


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(
        self,
        mac_address: str,
        async_at: AsyncATCom,
        name_prefix: str = "SIM7020",
        connect_timeout: float = CONNECT_TIMEOUT,
        broker_url: str = BROKER_URL,
        broker_port: int = 1883,
    ):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)
        name = f"{name_prefix}-{mac_address}"
        self._sim7020 = SIM7020Interface(
            async_at,
            self.topic_status,
            name,
            connect_timeout,
            # self._handle_registration,
            (lambda x: None),
            broker_url,
            broker_port,
        )
        self._include_sending_timestamp = False
        self._retries = BackoffBatchedRetries(
            self._send_punches, False, 3.0, 2.0, timedelta(hours=3), batch_count=4
        )

    async def loop(self):
        await asyncio.sleep(10000000.0)

    async def _handle_registration(self, line: str):
        await self._retries.execute(self._sim7020.mqtt_connect)

    async def _send_punches(self, punches: list[Punch]) -> list[bool]:
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        res = self._sim7020.mqtt_send(self.topic_punches, punches_proto.SerializeToString(), qos=1)
        return [res] * len(punches)

    async def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> bool:
        res = await self._retries.send(
            create_punch_proto(card_number, si_time, code, mode, process_time)
        )
        return res if res is not None else False

    async def send_coords(self, lat: float, lon: float, alt: float, timestamp: datetime) -> bool:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return await self._send(self.topic_coords, coords.SerializeToString(), "GPS coordinates")

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        res = await self._retries.execute(self._sim7020.get_signal_info)
        if res is not None:
            (dbm, cellid) = res
            mch.signal_dbm = dbm
            mch.cellid = cellid

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return await self._send(self.topic_status, status.SerializeToString(), "MiniCallHome")

    async def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0) -> bool:
        return await self._retries.execute(self._sim7020.mqtt_send, topic, message, qos)
