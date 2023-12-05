import asyncio
import logging
import re
import tomllib
from datetime import datetime, timezone
from typing import Dict

from aiomqtt import Client as MqttClient
from aiomqtt import Message, MqttError
from aiomqtt.types import PayloadType
from google.protobuf.timestamp_pb2 import Timestamp
from meshtastic.mqtt_pb2 import ServiceEnvelope
from meshtastic.portnums_pb2 import SERIAL_APP, TELEMETRY_APP
from meshtastic.telemetry_pb2 import Telemetry

from yaroc.rs import SiPunch

from ..clients.client import ClientGroup
from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Status
from ..utils.container import Container, create_clients

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


class MqttForwader:
    def __init__(self, client_groups: Dict[str, ClientGroup], dns: Dict[str, str]):
        self.client_groups = client_groups
        self.dns = dns

    @staticmethod
    def _prototime_to_datetime(prototime: Timestamp) -> datetime:
        return prototime.ToDatetime().replace(tzinfo=timezone.utc).astimezone()

    @staticmethod
    def _payload_to_bytes(payload: PayloadType) -> bytes:
        if isinstance(payload, bytes):
            return payload
        elif isinstance(payload, str):
            return payload.encode("utf-8")
        else:
            raise TypeError("Unexpected type of a message payload")

    async def _process_punch(
        self,
        punch: SiPunch,
        mac_addr: str,
        now: datetime,
        send_time: datetime | None,
    ):
        log_message = (
            f"{self.dns[mac_addr]} {punch.card:7} punched {punch.code:03} "
            f"at {punch.time:%H:%M:%S.%f}, "
        )
        if send_time is None:
            log_message += f"latency {(now - punch.time).total_seconds():6.2f}s"
        else:
            log_message += (
                f"sent {send_time:%H:%M:%S.%f}, network latency "
                f"{(now - send_time).total_seconds():6.2f}s"
            )

        logging.info(log_message)
        await self.client_groups[mac_addr].send_punch(punch)

    async def _handle_punches(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            punches = Punches.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        for punch in punches.punches:
            try:
                si_punch = SiPunch.from_raw(punch.raw)
            except Exception as err:
                logging.error(f"Error while constructing SiPunch: {err}")

            if punches.HasField("sending_timestamp"):
                send_time = MqttForwader._prototime_to_datetime(punches.sending_timestamp)
            else:
                send_time = None
            await self._process_punch(si_punch, mac_addr, now, send_time)

    async def _handle_meshtastic_serial(self, payload: PayloadType, mac_addr: str, now: datetime):
        try:
            se = ServiceEnvelope.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        if not se.packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return
        if se.packet.decoded.portnum != SERIAL_APP:
            logging.debug(f"Ignoring message with portnum {se.packet.decoded.pornum}")
            return

        try:
            punch = SiPunch.from_raw(se.packet.decoded.payload)
            # TODO: change MAC address
            await self._process_punch(punch, "8c8caa504e8a", now, None)
        except Exception as err:
            logging.error(f"Error while constructing SiPunch: {err}")

    async def _handle_status(self, mac_addr: str, payload: PayloadType, now: datetime):
        try:
            status = Status.FromString(MqttForwader._payload_to_bytes(payload))
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        oneof = status.WhichOneof("msg")
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
        elif oneof == "mini_call_home":
            mch = status.mini_call_home
            orig_time = MqttForwader._prototime_to_datetime(mch.time)
            total_latency = now - orig_time
            if mch.freq > 0.0:
                log_message = (
                    f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: {mch.cpu_temperature:5.2f}°C, "
                    f"{mch.signal_dbm:4}dBm, "
                )
                if mch.cellid > 0:
                    log_message += f"cell {mch.cellid:X}, "
                log_message += f"{mch.volts:3.2f}V, {mch.freq:4}MHz, "
            else:
                log_message = f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: {mch.codes}, "
            log_message += f"latency {total_latency.total_seconds():6.2f}s"
            logging.info(log_message)
            await self.client_groups[mac_addr].send_mini_call_home(mch)

    async def _handle_meshtastic_status(self, payload: PayloadType, mac_addr: str, now: datetime):
        try:
            se = ServiceEnvelope.FromString(payload)
        except Exception as err:
            logging.error(f"Error while parsing protobuf: {err}")
            return
        if not se.packet.HasField("decoded"):
            logging.error("Encrypted message! Disable encryption for meshtastic MQTT")
            return
        if se.packet.decoded.portnum != TELEMETRY_APP:
            return

        try:
            telemetry = Telemetry.FromString(se.packet.decoded.payload)
            orig_time = datetime.fromtimestamp(telemetry.time).astimezone()
            total_latency = now - orig_time
            metrics = telemetry.device_metrics

            log_message = (
                f"{self.dns[mac_addr]} {orig_time:%H:%M:%S}: battery {metrics.battery_level}%, "
                f"{metrics.voltage:4.3f}V, latency {total_latency.total_seconds():6.2f}s"
            )
            logging.info(log_message)
        except Exception as err:
            logging.error(f"Error while constructing Telemetry: {err}")

    @staticmethod
    def extract_mac(topic: str) -> str:
        match = re.match("yar/([0-9a-f]{12})/.*", topic)
        if match is None or len(match.groups()) == 0:
            logging.error(f"Invalid topic: {topic}")
            raise Exception(f"Invalid topic {topic}")

        groups = match.groups()
        return groups[0]

    @staticmethod
    def extract_mac_meshtastic(topic: str) -> str:
        match = re.match("yar/2/c/.*/!([0-9a-f]{8})*", topic)
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
                await self._handle_punches(mac_addr, msg.payload, now)
            elif topic.endswith("/status"):
                mac_addr = MqttForwader.extract_mac(topic)
                await self._handle_status(mac_addr, msg.payload, now)
            elif topic.startswith("yar/2/c/LongFast/"):
                mac_addr = MqttForwader.extract_mac_meshtastic(topic)
                await self._handle_meshtastic_status(msg.payload, mac_addr, now)
            elif topic.startswith("yar/2/c/serial/"):
                mac_addr = MqttForwader.extract_mac_meshtastic(topic)
                await self._handle_meshtastic_serial(msg.payload, mac_addr, now)
        except Exception as err:
            logging.error(f"Failed processing message: {err}")

    async def loop(self):
        async_loop = asyncio.get_event_loop()
        for client_group in self.client_groups.values():
            asyncio.run_coroutine_threadsafe(client_group.loop(), async_loop)

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
                        for mac_addr in self.client_groups.keys():
                            await client.subscribe(f"yar/{mac_addr}/#", qos=1)
                        await client.subscribe("yar/2/c/serial/#", qos=1)
                        await client.subscribe("yar/2/c/LongFast/#", qos=1)
                        async for message in messages:
                            await self._on_message(message)
            except MqttError:
                logging.error(f"Connection lost to mqtt://{BROKER_URL}")
                await asyncio.sleep(5.0)
            except asyncio.exceptions.CancelledError:
                logging.error("Interrupted, exiting")
                import sys

                sys.exit(0)


def main():
    with open("mqtt-forwarder.toml", "rb") as f:
        config = tomllib.load(f)
    config.pop("mqtt", None)  # Disallow MQTT forwarding to break infinite loops
    config.pop("sim7020", None)  # Disallow MQTT forwarding to break infinite loops

    container = Container()
    container.config.from_dict(config)
    container.init_resources()
    container.wire(modules=["yaroc.utils.container"])

    client_groups = {}
    dns = {}
    for name, mac_address in config["mac-addresses"].items():
        clients = create_clients(container.client_factories, mac_address=mac_address)
        if len(clients) == 0:
            logging.info(f"Listening to {name}/{mac_address} without forwarding")
        client_groups[str(mac_address)] = ClientGroup(clients)
        dns[str(mac_address)] = name

    forwarder = MqttForwader(client_groups, dns)
    asyncio.run(forwarder.loop())
