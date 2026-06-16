import logging
import math
from asyncio import Lock, get_running_loop, sleep
from dataclasses import dataclass
from datetime import timedelta
from typing import Any, Dict

from aiomqtt import Client as AioMqttClient
from aiomqtt import MqttCodeError, MqttError
from aiomqtt.client import Will

from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import CellNetworkType, Disconnected, Status
from ..rs import MeshtasticLog, MeshtasticPunches, SiPunchLog, current_timestamp_millis
from ..utils.async_serial import AsyncATCom
from ..utils.retries import BackoffBatchedRetries
from ..utils.sim7020 import SIM7020Interface
from .client import Client

BROKER_URL = "broker.emqx.io"
BROKER_PORT = 1883
CONNECT_TIMEOUT = 35


@dataclass
class Topics:
    punch: str
    status: str

    @staticmethod
    def from_mac(mac_address: str):
        return Topics(
            f"yar/{mac_address}/p",
            f"yar/{mac_address}/status",
        )


class MqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(self, hostname: str, mac_addr: str, config: Dict[str, Any]):
        self.topics: Dict[str, Topics] = {}
        self._name = f"aiomqtt-{hostname}"
        self.mac_addr = mac_addr
        self.broker_url = config.get("broker_url", BROKER_URL)
        self.broker_port = config.get("broker_port", BROKER_PORT)

        disconnected = Disconnected()
        disconnected.client_name = self._name

        status = Status()
        status.disconnected.CopyFrom(disconnected)
        topics = self.get_topics(mac_addr)
        will = Will(topic=topics.status, payload=status.SerializeToString(), qos=1)

        self.client = AioMqttClient(
            self.broker_url,
            self.broker_port,
            timeout=20,
            will=will,
            identifier=self._name,
            clean_session=False,
            max_inflight_messages=100,
            logger=logging.getLogger(),
        )

    def get_topics(self, mac_addr: str) -> Topics:
        if mac_addr in self.topics:
            return self.topics[mac_addr]
        self.topics[mac_addr] = Topics.from_mac(mac_addr)
        return self.topics[mac_addr]

    def name(self) -> str:
        return self._name

    async def send_punch(
        self,
        punch_log: SiPunchLog,
    ):
        punches = Punches()
        try:
            punches.punches.append(punch_log.punch.raw)
        except Exception as err:
            logging.error(f"Creation of Punch proto failed: {err}")
        punches.sending_timestamp.millis_epoch = current_timestamp_millis()
        topics = self.get_topics(punch_log.host_info.mac_address)
        await self._send(topics.punch, punches.SerializeToString(), 1, "Punch")

    async def send_status(self, status: Status, mac_addr: str):
        topics = self.get_topics(mac_addr)
        await self._send(topics.status, status.SerializeToString(), 0, "MiniCallHome")

    async def send_meshtastic(self, log: MeshtasticLog | MeshtasticPunches):
        topic = f"yar/2/e/{log.channel}/{log.gateway_id}"
        typ = type(log).__name__
        await self._send(topic, log.service_envelope, 1, typ)

    async def _send(self, topic: str, msg: bytes, qos: int, message_type: str):
        try:
            await self.client.publish(topic, payload=msg, qos=qos)
            logging.info(f"{message_type} sent via MQTT")
        except MqttCodeError as e:
            raise ConnectionError(f"{message_type} not sent: {e}")

    async def loop(self):
        while True:
            try:
                async with self.client:
                    logging.info(f"Connected to mqtt://{self.broker_url}")
                    # Sleep forever
                    await get_running_loop().create_future()

            except MqttError:
                logging.error(f"Connection lost to mqtt://{self.broker_url}")
                await sleep(5.0)


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(
        self,
        hostname: str,
        mac_address: str,
        async_at: AsyncATCom,
        config: Dict[str, Any],
        connect_timeout: float = CONNECT_TIMEOUT,
    ):
        self.topics = Topics.from_mac(mac_address)
        self._name = f"SIM7020-{hostname}"
        self._sim7020 = SIM7020Interface(
            async_at,
            self.topics.status,
            self._name,
            connect_timeout,
            config.get("broker_url", BROKER_URL),
            config.get("broker_port", BROKER_PORT),
        )
        self._include_sending_timestamp = False
        self._retries = BackoffBatchedRetries(
            self._send_punches, False, 2.0, math.sqrt(2.0), timedelta(hours=3), batch_count=4
        )
        self._lock = Lock()

    def name(self) -> str:
        return self._name

    async def loop(self):
        async with self._lock:
            await self._sim7020.setup()
        # Sleep forever
        await get_running_loop().create_future()

    async def _send_punches(self, punches: list[bytes]) -> list[bool]:
        punches_proto = Punches()
        for punch in punches:
            punches_proto.punches.append(punch)
        if self._include_sending_timestamp:
            punches_proto.sending_timestamp.millis_epoch = current_timestamp_millis()
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
        punch_log: SiPunchLog,
    ):
        await self._retries.send(punch_log.punch.raw)

    async def send_status(self, status: Status, mac_addr: str):
        if status.WhichOneof("msg") == "mini_call_home":
            async with self._lock:
                res = await self._sim7020.get_signal_info()
                if res is not None:
                    (rsrp_dbm, cellid, snr, ecl) = res
                    mch = status.mini_call_home
                    mch.rsrp_dbm = rsrp_dbm
                    mch.signal_snr_cb = snr * 10
                    mch.cellid = cellid
                    if ecl == 0:
                        mch.network_type = CellNetworkType.NbIotEcl0  # type: ignore
                    elif ecl == 1:
                        mch.network_type = CellNetworkType.NbIotEcl1  # type: ignore
                    elif ecl == 2:
                        mch.network_type = CellNetworkType.NbIotEcl2  # type: ignore

        await self._send(self.topics.status, status.SerializeToString(), "MiniCallHome")

    async def send_meshtastic(self, log: MeshtasticLog | MeshtasticPunches):
        topic = f"yar/2/e/{log.channel}/{log.gateway_id}"
        typ = type(log).__name__
        logging.info(f"{topic} + {typ}")
        await self._send(topic, log.service_envelope, typ)

    async def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0) -> bool:
        async with self._lock:
            res = await self._sim7020.mqtt_send(topic, message, qos)
            if isinstance(res, str):
                logging.error(f"MQTT sending of {message_type} failed: {res}")
                return False
            logging.info(f"{message_type} sent via MQTT")
            return res
