import logging
import math
import random
from asyncio import Lock, sleep
from dataclasses import dataclass
from datetime import timedelta
from typing import Dict

from aiomqtt import Client as AioMqttClient
from aiomqtt import MqttError
from aiomqtt.client import Will
from aiomqtt.error import MqttCodeError

from ..pb.punches_pb2 import Punch, Punches
from ..pb.status_pb2 import Disconnected, Status
from ..pb.utils import create_punch_proto
from ..rs import SiPunch
from ..utils.async_serial import AsyncATCom
from ..utils.modem_manager import ModemManager, NetworkType
from ..utils.retries import BackoffBatchedRetries
from ..utils.sim7020 import SIM7020Interface
from .client import Client

BROKER_URL = "broker.emqx.io"
BROKER_PORT = 1883
CONNECT_TIMEOUT = 45


@dataclass
class Topics:
    punch: str
    status: str
    command: str

    @staticmethod
    def from_mac(mac_address: str):
        return Topics(
            f"yar/{mac_address}/p",
            f"yar/{mac_address}/status",
            f"yar/{mac_address}/cmd",
        )


class MqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(
        self,
        hostname: str,
        mac_addr: str,
        broker_url: str | None,
        broker_port: int | None,
    ):
        self.topics: Dict[str, Topics] = {}
        self.name = f"aiomqtt-{hostname}"
        self.mac_addr = mac_addr
        self.broker_url = BROKER_URL if broker_url is None else broker_url
        self.broker_port = BROKER_PORT if broker_port is None else broker_port

        disconnected = Disconnected()
        disconnected.client_name = self.name

        status = Status()
        status.disconnected.CopyFrom(disconnected)
        topics = self.get_topics(mac_addr)
        will = Will(topic=topics.status, payload=status.SerializeToString(), qos=1)

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

    def get_topics(self, mac_addr: str) -> Topics:
        if mac_addr in self.topics:
            return self.topics[mac_addr]
        self.topics[mac_addr] = Topics.from_mac(mac_addr)
        return self.topics[mac_addr]

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
        topics = self.get_topics(punch.host_info.mac_address)
        return await self._send(topics.punch, punches.SerializeToString(), 1, "Punch")

    async def send_status(self, status: Status, mac_addr: str) -> bool:
        try:
            if status.WhichOneof("msg") == "mini_call_home":
                modems = await self.mm.get_modems()
                if len(modems) > 0:
                    network_state = await self.mm.get_signal(modems[0])
                    logging.debug(f"Network state: {network_state}")
                    if network_state.rssi is not None:
                        status.mini_call_home.signal_dbm = round(network_state.rssi)
                    if network_state.snr is not None:
                        status.mini_call_home.signal_snr = round(network_state.snr)
                    status.mini_call_home.network_type = network_state.type.value
                    if network_state.type == NetworkType.Unknown and random.randint(0, 4) == 2:
                        await self.mm.signal_setup(modems[0], 20)
        except Exception as e:
            logging.error(f"Error while getting signal strength: {e}")

        topics = self.get_topics(mac_addr)
        return await self._send(topics.status, status.SerializeToString(), 0, "MiniCallHome")

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
                        topics = self.get_topics(self.mac_addr)
                        await self.client.subscribe(topics.command)
                        async for message in messages:
                            logging.info("Got a command message, processing is not implemented")

            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await sleep(5.0)


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(
        self,
        hostname: str,
        mac_address: str,
        async_at: AsyncATCom,
        broker_url: str | None,
        broker_port: int | None,
        connect_timeout: float = CONNECT_TIMEOUT,
    ):
        self.topics = Topics.from_mac(mac_address)
        name = f"SIM7020-{hostname}"
        self._sim7020 = SIM7020Interface(
            async_at,
            self.topics.status,
            name,
            connect_timeout,
            BROKER_URL if broker_url is None else broker_url,
            BROKER_PORT if broker_port is None else broker_port
        )
        self._include_sending_timestamp = False
        self._retries = BackoffBatchedRetries(
            self._send_punches, False, 2.0, math.sqrt(2.0), timedelta(hours=3), batch_count=4
        )
        self._lock = Lock()

    async def loop(self):
        async with self._lock:
            await self._sim7020.setup()
        await sleep(10000000.0)

    async def _send_punches(self, punches: list[Punch]) -> list[bool]:
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.GetCurrentTime()
        async with self._lock:
            res = await self._sim7020.mqtt_send(
                self.topics.punch, punches_proto.SerializeToString(), qos=1
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

    async def send_status(self, status: Status, mac_addr: str) -> bool:
        if status.WhichOneof("msg") == "mini_call_home":
            async with self._lock:
                res = await self._sim7020.get_signal_info()
                if res is not None:
                    (dbm, cellid, snr) = res
                    mch = status.mini_call_home
                    mch.signal_dbm = dbm
                    mch.signal_snr = snr
                    mch.cellid = cellid
                    mch.network_type = NetworkType.NbIot.value

        return await self._send(self.topics.status, status.SerializeToString(), "MiniCallHome")

    async def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0) -> bool:
        async with self._lock:
            res = await self._sim7020.mqtt_send(topic, message, qos)
            if isinstance(res, str):
                logging.error(f"MQTT sending of {message_type} failed: {res}")
                return False
            logging.info(f"{message_type} sent via MQTT")
            return res
