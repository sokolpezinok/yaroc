import asyncio
import logging
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime
from typing import AsyncIterator, List, Tuple

from aiomqtt import Client as MqttClient
from aiomqtt import MqttError
from aiomqtt.types import PayloadType

from ..clients.client import ClientGroup
from ..clients.mqtt import BROKER_PORT, BROKER_URL
from ..pb.status_pb2 import Status as StatusProto
from ..rs import MessageHandler, SiPunchLog
from ..utils.status import StatusDrawer
from .meshtastic import MeshtasticSerial


@dataclass
class PunchMessage:
    mac_addr: int
    raw: PayloadType
    now: datetime


@dataclass
class StatusMessage:
    mac_addr: int
    raw: PayloadType
    now: datetime


@dataclass
class MeshtasticStatusMessage:
    recv_mac_addr: int
    raw: PayloadType
    now: datetime


@dataclass
class MeshtasticSerialMessage:
    raw: PayloadType


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
    def extract_mac(topic: str) -> int:
        match = re.match("yar/([0-9a-f]{12})/.*", topic)
        if match is None or len(match.groups()) == 0:
            logging.error(f"Invalid topic: {topic}")
            raise Exception(f"Invalid topic {topic}")

        groups = match.groups()
        return int(groups[0], 16)

    def _on_message(self, raw: PayloadType, topic: str) -> Message | None:
        now = datetime.now().astimezone()

        try:
            if topic.endswith("/p"):
                mac_addr = self.extract_mac(topic)
                # await self._handle_punches(mac_addr, raw, now)
                return PunchMessage(mac_addr, raw, now)
            elif topic.endswith("/status"):
                mac_addr = self.extract_mac(topic)
                # await self._handle_status(mac_addr, raw, now)
                return StatusMessage(mac_addr, raw, now)
            elif self.meshtastic_channel is not None and topic.startswith(
                f"yar/2/e/{self.meshtastic_channel}/"
            ):
                recv_mac_addr = topic[10 + len(self.meshtastic_channel) :]
                recv_mac_addr_int = int(recv_mac_addr, 16)
                # self.handler.meshtastic_status_service_envelope(raw, now, recv_mac_addr_int)
                return MeshtasticStatusMessage(recv_mac_addr_int, raw, now)
            elif topic.startswith("yar/2/e/serial/"):
                # await self._handle_meshtastic_serial(raw)
                return MeshtasticSerialMessage(raw)
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


class YarocDaemon:
    def __init__(
        self,
        dns: List[Tuple[str, str]],
        client_group: ClientGroup,
        mqtt_forwarder: MqttForwader,
        display_model: str | None = None,
    ):
        self.client_group = client_group
        self.handler = MessageHandler(dns)
        self.drawer = StatusDrawer(self.handler, display_model)
        self.executor = ThreadPoolExecutor(max_workers=1)

        self.mqtt_forwarder = mqtt_forwarder
        self.msh_serial = MeshtasticSerial(
            self.on_msh_status, self._handle_meshtastic_serial_mesh_packet
        )

    @staticmethod
    def _payload_to_bytes(payload: PayloadType) -> bytes:
        if isinstance(payload, bytes):
            return payload
        elif isinstance(payload, str):
            return payload.encode("utf-8")
        else:
            raise TypeError("Unexpected type of a message payload")

    async def _process_punch(self, punch: SiPunchLog):
        logging.info(punch)
        await self.client_group.send_punch(punch)

    async def _handle_punches(self, msg: PunchMessage):
        try:
            punches = self.handler.punches(self._payload_to_bytes(msg.raw), msg.mac_addr)
        except Exception as err:
            logging.error(f"Error while constructing SI punches: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_meshtastic_serial_mesh_packet(self, msg: MeshtasticSerialMessage):
        try:
            punches = self.handler.meshtastic_serial_mesh_packet(self._payload_to_bytes(msg.raw))
        except Exception as err:
            logging.error(f"Error while constructing SI punch: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_meshtastic_serial(self, msg: MeshtasticSerialMessage):
        try:
            punches = self.handler.meshtastic_serial_service_envelope(
                self._payload_to_bytes(msg.raw)
            )
        except Exception as err:
            logging.error(f"Error while constructing SI punch: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_status(self, msg: StatusMessage):
        try:
            # We cannot return union types from Rust, so we have to parse the proto to detect the
            # type
            status = StatusProto.FromString(self._payload_to_bytes(msg.raw))
        except Exception as err:
            logging.error(err)
            return

        try:
            oneof = status.WhichOneof("msg")
            self.handler.status_update(self._payload_to_bytes(msg.raw), msg.mac_addr)
            if oneof != "disconnected":
                await self.client_group.send_status(status, f"{msg.mac_addr:0x}")
        except Exception as err:
            logging.error(f"Failed to construct proto: {err}")

    def on_msh_status(self, msg: PayloadType, recv_mac_addr: int):
        now = datetime.now().astimezone()
        self.handler.meshtastic_status_mesh_packet(self._payload_to_bytes(msg), now, recv_mac_addr)

    def _handle_msh_status_service_envelope(self, msg: MeshtasticStatusMessage):
        self.handler.meshtastic_status_service_envelope(
            self._payload_to_bytes(msg.raw), msg.now, msg.recv_mac_addr
        )

    async def draw_table(self):
        await asyncio.sleep(20.0)
        while True:
            time_start = time.time()
            self.executor.submit(self.drawer.draw_status)
            await asyncio.sleep(60 - (time.time() - time_start))

    async def loop(self):
        asyncio.create_task(self.client_group.loop())
        asyncio.create_task(self.draw_table())
        asyncio.create_task(self.msh_serial.loop())

        try:
            async for msg in self.mqtt_forwarder.messages():
                if isinstance(msg, PunchMessage):
                    await self._handle_punches(msg)
                elif isinstance(msg, StatusMessage):
                    await self._handle_status(msg)
                elif isinstance(msg, MeshtasticStatusMessage):
                    self._handle_meshtastic_serial(msg)
                else:
                    await self._handle_meshtastic_serial(msg)
        except asyncio.exceptions.CancelledError:
            logging.error("Interrupted, exiting")
            sys.exit(0)
