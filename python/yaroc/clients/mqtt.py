import asyncio
import logging
import random
from asyncio import Lock
from datetime import datetime, timedelta
from typing import Tuple

from aiomqtt import Client as AioMqttClient
from aiomqtt import MqttError
from aiomqtt.client import Will
from aiomqtt.error import MqttCodeError

from yaroc.rs import SiPunch

from ..pb.punches_pb2 import Punch, Punches
from ..pb.status_pb2 import Disconnected, MiniCallHome, Status
from ..pb.utils import create_punch_proto
from ..utils.async_serial import AsyncATCom
from ..utils.modem_manager import ModemManager
from ..utils.retries import BackoffBatchedRetries
from ..utils.sim7020 import SIM7020Interface
from .client import Client

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883
CONNECT_TIMEOUT = 45


def topics_from_mac(mac_address: str) -> Tuple[str, str, str]:
    return (
        f"yaroc/{mac_address}/p",
        f"yaroc/{mac_address}/status",
        f"yaroc/{mac_address}/cmd",
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
        self.topic_punches, self.topic_status, self.topic_cmd = topics_from_mac(mac_address)
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

    async def send_punch(
        self,
        punch: SiPunch,
    ) -> bool:
        punches = Punches()
        try:
            punches.punches.append(create_punch_proto(punch))
        except Exception as err:
            logging.error(f"Creation of Punch proto failed: {err}")
        punches.sending_timestamp.GetCurrentTime()
        return await self._send(self.topic_punches, punches.SerializeToString(), 1, "Punch")

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        try:
            modems = await self.mm.get_modems()
            if len(modems) > 0:
                (signal, network_type) = await self.mm.get_signal(modems[0])
                mch.signal_dbm = round(signal)
                mch.network_type = network_type
                if abs(signal) < 1 and random.randint(0, 10) == 7:
                    await self.mm.signal_setup(modems[0], 20)
        except Exception as e:
            logging.error(f"Error while getting signal strength: {e}")

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return await self._send(self.topic_status, status.SerializeToString(), 0, "MiniCallHome")

    async def _send(self, topic: str, msg: bytes, qos: int, message_type: str):
        try:
            await self.client.publish(topic, payload=msg, qos=qos)
            logging.info(f"{message_type} sent via MQTT")
            return True
        except MqttCodeError as e:
            logging.error(f"{message_type} not sent: {e}")
            return False

    async def loop(self):
        self.mm = await ModemManager.new()
        while True:
            try:
                async with self.client:
                    logging.info(f"Connected to mqtt://{BROKER_URL}")
                    async with self.client.messages() as messages:
                        await self.client.subscribe(self.topic_cmd)
                        async for message in messages:
                            logging.info("Got a command message, processing is not implemented")

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
        self.topic_punches, self.topic_status, self.topic_cmd = topics_from_mac(mac_address)
        name = f"{name_prefix}-{mac_address}"
        self._sim7020 = SIM7020Interface(
            async_at,
            self.topic_status,
            name,
            connect_timeout,
            broker_url,
            broker_port,
        )
        self._include_sending_timestamp = False
        self._retries = BackoffBatchedRetries(
            self._send_punches, False, 3.0, 2.0, timedelta(hours=3), batch_count=4
        )
        self._lock = Lock()

    async def loop(self):
        async with self._lock:
            await self._sim7020.setup()
        await asyncio.sleep(10000000.0)

    async def _send_punches(self, punches: list[Punch]) -> list[bool]:
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        async with self._lock:
            res = await self._sim7020.mqtt_send(
                self.topic_punches, punches_proto.SerializeToString(), qos=1
            )
            if isinstance(res, str):
                logging.error(f"Sending of punches failed: {res}")
                return [False] * len(punches)
            return [True] * len(punches)

    async def send_punch(
        self,
        punch: SiPunch,
    ) -> bool:
        res = await self._retries.send(create_punch_proto(punch))
        return res if res is not None else False

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        async with self._lock:
            res = await self._sim7020.get_signal_info()
            if res is not None:
                (dbm, cellid) = res
                mch.signal_dbm = dbm
                mch.cellid = cellid

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return await self._send(self.topic_status, status.SerializeToString(), "MiniCallHome")

    async def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0) -> bool:
        async with self._lock:
            res = await self._sim7020.mqtt_send(topic, message, qos)
            if isinstance(res, str):
                logging.error(f"MQTT sending of {message_type} failed: {res}")
                return False
            return res
