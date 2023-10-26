import asyncio
import logging
from concurrent.futures import Future
from datetime import datetime, timedelta
from typing import Tuple

from aiomqtt import Client as AioMqttClient
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
        name_prefix: str = "PahoMQTT",
        broker_url: str = BROKER_URL,
        broker_port: int = BROKER_PORT,
    ):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)
        self.name = f"{name_prefix}-{mac_address}"
        self.broker_url = broker_url
        self.broker_port = broker_port

    def __del__(self):
        self.client.loop_stop()

    async def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ):
        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        punches.sending_timestamp.GetCurrentTime()
        return await self._send(self.topic_punches, punches.SerializeToString())

    async def send_coords(self, lat: float, lon: float, alt: float, timestamp: datetime):
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return await self._send(self.topic_coords, coords.SerializeToString())

    async def send_mini_call_home(self, mch: MiniCallHome):
        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return await self._send(self.topic_status, status.SerializeToString(), qos=0)

    async def _send(self, topic: str, message: bytes, qos: int = 1):
        disconnected = Disconnected()
        disconnected.client_name = self.name

        status = Status()
        status.disconnected.CopyFrom(disconnected)
        will = Will(topic=self.topic_status, payload=status.SerializeToString(), qos=1)

        # TODO: as a first hack this is fine, but the client should be persisted
        async with AioMqttClient(
            self.broker_url,
            self.broker_port,
            timeout=20,
            will=will,
            client_id=self.name,
            clean_session=False,
            max_inflight_messages=100,
            logger=logging.getLogger(),
        ) as client:
            # TODO: Add connection/disconnected notifications

            try:
                await client.publish(topic, payload=message, qos=qos)
                logging.info("Message sent")  # TODO: message ID
            except MqttCodeError as e:
                logging.error(f"Message not sent: {e}")


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(
        self,
        mac_address: str,
        async_at: AsyncATCom,
        retry_loop: asyncio.AbstractEventLoop,
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
            self._handle_registration,
            broker_url,
            broker_port,
        )
        self._include_sending_timestamp = False
        self._retries = BackoffBatchedRetries(
            self._send_punches, 3.0, 2.0, timedelta(hours=3), retry_loop, batch_count=4
        )

    def _handle_registration(self, line: str):
        return self._retries.execute(self._sim7020.mqtt_connect)

    def _send_punches(self, punches: list[Punch]) -> list[bool | None]:
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        res = self._sim7020.mqtt_send(self.topic_punches, punches_proto.SerializeToString(), qos=1)
        if res:
            return [res] * len(punches)
        else:
            return [None] * len(punches)

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> Future:
        return self._retries.send(
            create_punch_proto(card_number, si_time, code, mode, process_time)
        )

    def send_coords(self, lat: float, lon: float, alt: float, timestamp: datetime) -> Future:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString(), "GPS coordinates")

    def send_mini_call_home(self, mch: MiniCallHome) -> Future:
        fut = self._retries.execute(self._sim7020.get_signal_info)
        res = fut.result()
        if res is not None:
            (dbm, cellid) = res
            mch.signal_dbm = dbm
            mch.cellid = cellid

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), "MiniCallHome")

    def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0) -> Future:
        return self._retries.execute(self._sim7020.mqtt_send, topic, message, qos)
