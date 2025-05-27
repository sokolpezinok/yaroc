import asyncio
import logging
import re
from dataclasses import dataclass
from datetime import datetime
from typing import AsyncIterator, List, Tuple

from aiomqtt import Client as MqttClient
from aiomqtt import MqttError
from aiomqtt.types import PayloadType

from ..clients.mqtt import BROKER_PORT, BROKER_URL


@dataclass
class PunchMessage:
    mac_addr: int
    raw: bytes
    now: datetime


@dataclass
class StatusMessage:
    mac_addr: int
    raw: bytes
    now: datetime


@dataclass
class MeshtasticStatusMessage:
    recv_mac_addr: int
    raw: bytes
    now: datetime


@dataclass
class MeshtasticSerialMessage:
    raw: bytes
    now: datetime


Message = PunchMessage | StatusMessage | MeshtasticStatusMessage | MeshtasticSerialMessage


class MqttForwader:
    def __init__(
        self,
        dns: List[Tuple[str, str]],
        broker_url: str | None,
        broker_port: int | None,
        meshtastic_channel: str | None,
    ):
        self.dns = dns
        self.broker_url = BROKER_URL if broker_url is None else broker_url
        self.broker_port = BROKER_PORT if broker_port is None else broker_port
        self.meshtastic_channel = meshtastic_channel

    @staticmethod
    def _payload_to_bytes(payload: PayloadType) -> bytes:
        if isinstance(payload, bytes):
            return payload
        elif isinstance(payload, str):
            return payload.encode("utf-8")
        else:
            raise TypeError("Unexpected type of a message payload")

    @staticmethod
    def extract_mac(topic: str) -> int:
        match = re.match("yar/([0-9a-f]{12})/.*", topic)
        if match is None or len(match.groups()) == 0:
            logging.error(f"Invalid topic: {topic}")
            raise Exception(f"Invalid topic {topic}")

        groups = match.groups()
        return int(groups[0], 16)

    def _on_message(self, raw: PayloadType, topic: str) -> Message | None:
        now = datetime.now().astimezone()
        raw = self._payload_to_bytes(raw)

        try:
            if topic.endswith("/p"):
                mac_addr = self.extract_mac(topic)
                return PunchMessage(mac_addr, raw, now)
            elif topic.endswith("/status"):
                mac_addr = self.extract_mac(topic)
                return StatusMessage(mac_addr, raw, now)
            elif self.meshtastic_channel is not None and topic.startswith(
                f"yar/2/e/{self.meshtastic_channel}/"
            ):
                recv_mac_addr = topic[10 + len(self.meshtastic_channel) :]
                recv_mac_addr_int = int(recv_mac_addr, 16)
                return MeshtasticStatusMessage(recv_mac_addr_int, raw, now)
            elif topic.startswith("yar/2/e/serial/"):
                return MeshtasticSerialMessage(raw, now)
        except Exception as err:
            logging.error(f"Failed processing message: {err}")

        return None

    async def messages(self) -> AsyncIterator[Message]:
        online_macs, radio_macs = [], []
        for mac, _ in self.dns:
            if len(mac) == 12:
                online_macs.append(mac)
            elif len(mac) == 8:
                radio_macs.append(mac)

        while True:
            try:
                async with MqttClient(
                    self.broker_url,
                    self.broker_port,
                    timeout=15,
                    logger=logging.getLogger(),
                ) as client:
                    logging.info(f"Connected to mqtt://{self.broker_url}")
                    for mac_addr in online_macs:
                        await client.subscribe(f"yar/{mac_addr}/#", qos=1)
                    for mac_addr in radio_macs:
                        await client.subscribe(f"yar/2/e/serial/!{mac_addr}", qos=1)
                        await client.subscribe(
                            f"yar/2/e/{self.meshtastic_channel}/!{mac_addr}", qos=1
                        )

                    async for mqtt_msg in client.messages:
                        message = self._on_message(mqtt_msg.payload, mqtt_msg.topic.value)
                        if message is not None:
                            yield message
            except MqttError:
                logging.error(f"Connection lost to mqtt://{self.broker_url}")
                await asyncio.sleep(10.0)
