import asyncio
import logging
import re
import time
from datetime import datetime
from typing import Dict

from aiomqtt import Client as MqttClient
from aiomqtt import Message, MqttError
from aiomqtt.types import PayloadType

from ..clients.client import ClientGroup
from ..clients.mqtt import BROKER_PORT, BROKER_URL
from ..pb.status_pb2 import Status as StatusProto
from ..rs import MessageHandler, SiPunch
from ..utils.status import StatusDrawer


class MqttForwader:
    def __init__(
        self,
        client_group: ClientGroup,
        dns: Dict[str, str],
        meshtastic_channel: str | None,
        meshtastic_mac_addr: str | None = None,
        display_model: str | None = None,
    ):
        self.client_group = client_group
        self.dns = dns
        self.meshtastic_channel = meshtastic_channel
        self.handler = MessageHandler.new(dns, meshtastic_mac_addr)
        self.drawer = StatusDrawer(self.handler, display_model)

    @staticmethod
    def _payload_to_bytes(payload: PayloadType) -> bytes:
        if isinstance(payload, bytes):
            return payload
        elif isinstance(payload, str):
            return payload.encode("utf-8")
        else:
            raise TypeError("Unexpected type of a message payload")

    async def _process_punch(self, punch: SiPunch):
        logging.info(punch)
        await self.client_group.send_punch(punch)

    async def _handle_punches(self, mac_addr: str, payload: PayloadType):
        try:
            punches = self.handler.punches(MqttForwader._payload_to_bytes(payload), mac_addr)
        except Exception as err:
            logging.error(f"Error while constructing SI punches: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_meshtastic_serial(self, payload: PayloadType):
        try:
            punches = self.handler.meshtastic_serial_msg(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while constructing SI punch: {err}")
            return

        tasks = [self._process_punch(punch) for punch in punches]
        await asyncio.gather(*tasks, return_exceptions=True)

    async def _handle_status(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            # We cannot return union types from Rust, so we have to parse the proto to detect the
            # type
            status = StatusProto.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(err)
            return

        oneof = status.WhichOneof("msg")
        try:
            self.handler.status_update(MqttForwader._payload_to_bytes(payload), mac_addr)
            if oneof != "disconnected":
                await self.client_group.send_status(status, mac_addr)
        except Exception as err:
            logging.error(f"Failed to construct proto: {err}")

    @staticmethod
    def extract_mac(topic: str) -> str:
        match = re.match("yar/([0-9a-f]{12})/.*", topic)
        if match is None or len(match.groups()) == 0:
            logging.error(f"Invalid topic: {topic}")
            raise Exception(f"Invalid topic {topic}")

        groups = match.groups()
        return groups[0]

    async def _on_message(self, msg: Message):
        now = datetime.now().astimezone()
        topic = msg.topic.value

        try:
            if topic.endswith("/p"):
                mac_addr = MqttForwader.extract_mac(topic)
                await self._handle_punches(mac_addr, msg.payload)
            elif topic.endswith("/status"):
                mac_addr = MqttForwader.extract_mac(topic)
                await self._handle_status(mac_addr, msg.payload, now)
            elif self.meshtastic_channel is not None and topic.startswith(
                f"yar/2/e/{self.meshtastic_channel}/"
            ):
                recv_mac_addr = topic[10 + len(self.meshtastic_channel) :]
                self.handler.msh_status_update(
                    MqttForwader._payload_to_bytes(msg.payload), now, recv_mac_addr
                )

            elif topic.startswith("yar/2/e/serial/"):
                await self._handle_meshtastic_serial(msg.payload)
        except Exception as err:
            logging.error(f"Failed processing message: {err}")

    async def draw_table(self):
        await asyncio.sleep(20.0)
        while True:
            time_start = time.time()
            self.drawer.draw_status()  # Move to another thread
            await asyncio.sleep(60 - (time.time() - time_start))

    async def loop(self):
        asyncio.create_task(self.client_group.loop())
        asyncio.create_task(self.draw_table())

        online_macs, radio_macs = [], []
        for mac in self.dns.keys():
            if len(mac) == 12:
                online_macs.append(mac)
            elif len(mac) == 8:
                radio_macs.append(mac)

        while True:
            try:
                async with MqttClient(
                    BROKER_URL,
                    BROKER_PORT,
                    timeout=15,
                    logger=logging.getLogger(),
                ) as client:
                    logging.info(f"Connected to mqtt://{BROKER_URL}")
                    async with client.messages() as messages:
                        for mac_addr in online_macs:
                            await client.subscribe(f"yar/{mac_addr}/#", qos=1)
                        for mac_addr in radio_macs:
                            await client.subscribe(f"yar/2/e/serial/!{mac_addr}", qos=1)
                            await client.subscribe(
                                f"yar/2/e/{self.meshtastic_channel}/!{mac_addr}", qos=1
                            )

                        async for message in messages:
                            asyncio.create_task(self._on_message(message))
            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await asyncio.sleep(5.0)
            except asyncio.exceptions.CancelledError:
                logging.error("Interrupted, exiting")
                import sys

                sys.exit(0)
